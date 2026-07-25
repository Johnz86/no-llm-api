use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use no_llm_api::config::{Cli, DatasetSettings, IdentityModeArg, Settings};
use no_llm_api::dataset::{self, ConversationScripts};
use no_llm_api::http::{RouterOptions, build_router_with_options};
use no_llm_api::models::ModelCatalogue;
use no_llm_api::service::ChatService;
use no_llm_api::sim::identity::IdentityMode;
use no_llm_api::sim::scenario::Scenario;
use no_llm_api::tokenizer::load;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();
    let print_config = cli.print_config;
    let settings = Settings::resolve(cli)?;

    if print_config {
        println!("{}", settings.to_json());
        return Ok(());
    }

    init_tracing();
    let tokenizer = load(&settings.tokenizer)?;
    let identity_mode = match settings.identity_mode {
        IdentityModeArg::Derived => IdentityMode::Derived,
        IdentityModeArg::Clock => IdentityMode::Clock,
    };
    let service = match &settings.dataset {
        DatasetSettings::Parquet { path } => {
            dataset::ensure_sample_dataset(path)?;
            let scripts = ConversationScripts::load(path, &tokenizer)?;
            Arc::new(
                ChatService::new(scripts, tokenizer.clone(), settings.tokens_per_second)
                    .with_identity_mode(identity_mode)
                    .with_tokenizer_name(settings.tokenizer.preset.clone()),
            )
        }
        DatasetSettings::Live(live_settings) => build_live_service(
            live_settings,
            tokenizer.clone(),
            settings.tokens_per_second,
            identity_mode,
        )?,
    };

    let catalogue = ModelCatalogue::resolve(
        settings.models.path.as_deref(),
        settings.models.ids.as_deref(),
    );
    let scenario = Scenario::resolve(&settings.scenario)?;
    tracing::info!(
        target: "no_llm_api",
        models = catalogue.ids().count(),
        scenario = %scenario.name,
        control_plane = settings.control_plane.enabled,
        "configuration resolved"
    );

    let options = RouterOptions {
        models: catalogue,
        cors: settings.cors.clone(),
        scenario,
        seed: settings.seed,
        auth: settings.auth.clone(),
        control_plane: settings.control_plane.clone(),
    };
    let (app, _state) = build_router_with_options(service.clone(), options);
    let listener = TcpListener::bind(settings.bind_address).await?;
    tracing::info!(target: "no_llm_api", "listening on {}", settings.bind_address);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Builds the live-proxy service, or explains that this binary was built without it.
#[cfg(feature = "live")]
fn build_live_service(
    live_settings: &no_llm_api::config::LiveSettings,
    tokenizer: std::sync::Arc<tiktoken_rs::CoreBPE>,
    rate: std::num::NonZeroU32,
    identity_mode: IdentityMode,
) -> Result<Arc<ChatService>> {
    let backend = no_llm_api::live::LiveBackend::new(live_settings)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(Arc::new(
        ChatService::with_live(backend, tokenizer, rate).with_identity_mode(identity_mode),
    ))
}

#[cfg(not(feature = "live"))]
fn build_live_service(
    _live_settings: &no_llm_api::config::LiveSettings,
    _tokenizer: std::sync::Arc<tiktoken_rs::CoreBPE>,
    _rate: std::num::NonZeroU32,
    _identity_mode: IdentityMode,
) -> Result<Arc<ChatService>> {
    anyhow::bail!(
        "DATASET_SOURCE=live requires a build with the `live` feature: \
         cargo run --features live. The default build is offline on purpose."
    )
}

/// Drains in-flight streams on Ctrl-C or SIGTERM instead of letting `docker stop`
/// truncate them after its ten second grace period.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(error) => tracing::warn!(target: "no_llm_api", ?error, "SIGTERM handler failed"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!(target: "no_llm_api", "shutdown signal received; draining");
}

fn init_tracing() {
    static INIT: once_cell::sync::OnceCell<()> = once_cell::sync::OnceCell::new();
    INIT.get_or_init(|| {
        let env_filter =
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_target(false)
            .compact()
            .init();
    });
}
