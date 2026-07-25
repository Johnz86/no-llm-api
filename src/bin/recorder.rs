use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use no_llm_api::config::LiveSettings;
use no_llm_api::config::TokenizerSettings;
use no_llm_api::dataset::{read_dataset_rows, rows_from_interaction, write_dataset};
use no_llm_api::live::LiveBackend;
use no_llm_api::model::ChatCompletionRequest;
use no_llm_api::tokenizer::load;
use serde::Deserialize;

#[derive(Parser)]
#[command(
    name = "no-llm-recorder",
    about = "Record live completions into a parquet dataset"
)]
struct Cli {
    #[arg(long)]
    input: PathBuf,

    #[arg(long, default_value = "data/recordings.parquet")]
    output: PathBuf,

    #[arg(long, default_value = "cl100k_base")]
    tokenizer: String,
}

#[derive(Debug, Deserialize)]
struct RecordingDefinition {
    conversation_id: String,
    request: ChatCompletionRequest,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let args = Cli::parse();

    let contents = fs::read_to_string(&args.input)
        .with_context(|| format!("failed to read {}", args.input.display()))?;
    let definitions: Vec<RecordingDefinition> = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse {}", args.input.display()))?;

    if definitions.is_empty() {
        println!("no conversations provided; nothing to record");
        return Ok(());
    }

    let tokenizer = load(&TokenizerSettings {
        preset: args.tokenizer.clone(),
    })?;

    let live_settings = LiveSettings {
        record: false,
        record_path: None,
    };
    let backend =
        LiveBackend::new(&live_settings).map_err(|error| anyhow::anyhow!(error.to_string()))?;

    let mut rows = read_dataset_rows(&args.output)?;
    for definition in definitions {
        println!("recording {}", definition.conversation_id);
        let result = backend
            .create_completion(&definition.request, &tokenizer)
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        rows.extend(rows_from_interaction(
            &definition.conversation_id,
            &definition.request.messages,
            &result.response,
        ));
    }

    write_dataset(&args.output, &rows)?;
    println!("wrote {} rows to {}", rows.len(), args.output.display());
    Ok(())
}
