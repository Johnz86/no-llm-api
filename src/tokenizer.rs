use std::sync::Arc;

use anyhow::Result;
use thiserror::Error;
use tiktoken_rs::{CoreBPE, cl100k_base, o200k_base, p50k_base, p50k_edit, r50k_base};

use crate::config::TokenizerSettings;

#[derive(Debug, Error)]
pub enum TokenizerError {
    #[error("unsupported tokenizer preset `{0}`")]
    UnsupportedPreset(String),
    #[error("failed to initialize tokenizer: {0}")]
    Initialization(#[from] anyhow::Error),
}

pub fn load(settings: &TokenizerSettings) -> Result<Arc<CoreBPE>, TokenizerError> {
    let preset = settings.preset.trim().to_ascii_lowercase();
    let encoder: Result<CoreBPE> = match preset.as_str() {
        "cl100k_base" => cl100k_base(),
        "p50k_base" => p50k_base(),
        "p50k_edit" => p50k_edit(),
        "r50k_base" => r50k_base(),
        "o200k_base" => o200k_base(),
        other => return Err(TokenizerError::UnsupportedPreset(other.to_string())),
    };

    Ok(Arc::new(encoder.map_err(TokenizerError::Initialization)?))
}
