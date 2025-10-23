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
    pub dataset_path: PathBuf,
}

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("invalid bind address: {0}")]
    InvalidBindAddress(String),
    #[error("invalid tokens per second: {0}")]
    InvalidTokenRate(String),
}

impl Settings {
    pub fn load() -> Result<Self, SettingsError> {
        let bind_address =
            env::var("BIND_ADDRESS").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
        let tokens_per_second = env::var("TOKENS_PER_SECOND").unwrap_or_else(|_| "30".to_string());
        let dataset_path =
            env::var("DATASET_PATH").unwrap_or_else(|_| "data/conversations.parquet".to_string());

        let bind_address = bind_address
            .parse()
            .map_err(|_| SettingsError::InvalidBindAddress(bind_address.clone()))?;
        let rate_value: u32 = tokens_per_second
            .parse()
            .map_err(|_| SettingsError::InvalidTokenRate(tokens_per_second.clone()))?;
        let tokens_per_second = NonZeroU32::new(rate_value)
            .ok_or_else(|| SettingsError::InvalidTokenRate(tokens_per_second.clone()))?;

        Ok(Self {
            bind_address,
            tokens_per_second,
            dataset_path: PathBuf::from(dataset_path),
        })
    }
}
