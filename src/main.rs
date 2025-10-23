mod config;
mod dataset;
mod http;
mod model;
mod service;
mod store;
mod tokenizer;

use std::sync::Arc;

use anyhow::Result;
use config::Settings;
use dataset::ConversationScripts;
use http::build_router;
use service::ChatService;
use tokenizer::load;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();
    let settings = Settings::load()?;
    dataset::ensure_sample_dataset(&settings.dataset_path)?;
    let tokenizer = load(&settings.tokenizer)?;
    let scripts = ConversationScripts::load(&settings.dataset_path, &tokenizer)?;
    let service = Arc::new(ChatService::new(
        scripts,
        tokenizer.clone(),
        settings.tokens_per_second,
    ));
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
