mod config;
mod dataset;
mod http;
mod live;
mod model;
mod service;
mod store;
mod tokenizer;

use std::sync::Arc;

use anyhow::Result;
use config::{DatasetSettings, Settings};
use dataset::ConversationScripts;
use http::build_router;
use live::LiveBackend;
use service::ChatService;
use tokenizer::load;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();
    let settings = Settings::load()?;
    let tokenizer = load(&settings.tokenizer)?;
    let service = match &settings.dataset {
        DatasetSettings::Parquet { path } => {
            dataset::ensure_sample_dataset(path)?;
            let scripts = ConversationScripts::load(path, &tokenizer)?;
            Arc::new(ChatService::new(
                scripts,
                tokenizer.clone(),
                settings.tokens_per_second,
            ))
        }
        DatasetSettings::Live(live_settings) => {
            let backend = LiveBackend::new(live_settings)
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            Arc::new(ChatService::with_live(
                backend,
                tokenizer.clone(),
                settings.tokens_per_second,
            ))
        }
    };
    let app = build_router(service.clone());
    let listener = TcpListener::bind(settings.bind_address).await?;
    tracing::info!(target: "no_llm_api", "listening on {}", settings.bind_address);
    axum::serve(listener, app).await?;
    Ok(())
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
