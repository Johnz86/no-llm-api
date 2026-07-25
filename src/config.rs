use std::env;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::path::PathBuf;

use thiserror::Error;

/// Represents runtime configuration for the mock service.
#[derive(Debug, Clone)]
pub struct Settings {
    pub bind_address: SocketAddr,
    pub tokens_per_second: NonZeroU32,
    pub dataset: DatasetSettings,
    pub tokenizer: TokenizerSettings,
}

/// Describes how the service should source conversation data.
#[derive(Debug, Clone)]
pub enum DatasetSettings {
    Parquet { path: PathBuf },
    Live(LiveSettings),
}

/// Captures configuration required to proxy or record live completions.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct LiveSettings {
    pub record: bool,
    pub record_path: Option<PathBuf>,
}

/// Collects tokenizer-related configuration knobs.
#[derive(Debug, Clone)]
pub struct TokenizerSettings {
    pub preset: String,
}

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("invalid bind address: {0}")]
    InvalidBindAddress(String),
    #[error("invalid tokens per second: {0}")]
    InvalidTokenRate(String),
    #[error("invalid dataset source: {0}")]
    InvalidDatasetSource(String),
}

impl Settings {
    pub fn load() -> Result<Self, SettingsError> {
        let bind_address_raw =
            env::var("BIND_ADDRESS").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
        let tokens_per_second_raw =
            env::var("TOKENS_PER_SECOND").unwrap_or_else(|_| "30".to_string());
        let tokenizer_preset =
            env::var("TOKENIZER_MODEL").unwrap_or_else(|_| "cl100k_base".to_string());
        let dataset_source = env::var("DATASET_SOURCE").unwrap_or_else(|_| "parquet".to_string());
        let dataset_path_value =
            env::var("DATASET_PATH").unwrap_or_else(|_| "data/conversations.parquet".to_string());

        let dataset = match dataset_source.to_ascii_lowercase().as_str() {
            "parquet" => DatasetSettings::Parquet {
                path: PathBuf::from(dataset_path_value.clone()),
            },
            "live" => {
                let record_flag = env::var("LIVE_RECORD").unwrap_or_else(|_| "false".to_string());
                let record = matches!(
                    record_flag.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes"
                );
                let record_path = env::var("LIVE_RECORD_PATH")
                    .ok()
                    .map(PathBuf::from)
                    .or_else(|| record.then(|| PathBuf::from(dataset_path_value.clone())));
                DatasetSettings::Live(LiveSettings {
                    record,
                    record_path,
                })
            }
            other => return Err(SettingsError::InvalidDatasetSource(other.to_string())),
        };

        let bind_address = bind_address_raw
            .parse()
            .map_err(|_| SettingsError::InvalidBindAddress(bind_address_raw.clone()))?;
        let rate_value: u32 = tokens_per_second_raw
            .parse()
            .map_err(|_| SettingsError::InvalidTokenRate(tokens_per_second_raw.clone()))?;
        let tokens_per_second = NonZeroU32::new(rate_value)
            .ok_or_else(|| SettingsError::InvalidTokenRate(tokens_per_second_raw.clone()))?;

        Ok(Self {
            bind_address,
            tokens_per_second,
            dataset,
            tokenizer: TokenizerSettings {
                preset: tokenizer_preset,
            },
        })
    }
}
