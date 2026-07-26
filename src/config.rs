//! Runtime configuration.
//!
//! Every setting is a `clap` flag with an environment fallback, so `--help` is
//! the documentation and `--print-config` is the resolved truth. Values are typed
//! and validated once, here, rather than re-parsed at each use site.

use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use thiserror::Error;

/// The command line, which doubles as the environment contract.
#[derive(Debug, Clone, Parser)]
#[command(
    name = "no-llm-api",
    about = "A deterministic, offline mock of the OpenAI Chat Completions API",
    version
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Address to bind the HTTP listener to.
    #[arg(long, env = "BIND_ADDRESS", default_value = "127.0.0.1:8080")]
    pub bind_address: SocketAddr,

    /// Streaming token budget; controls SSE pacing.
    #[arg(long, env = "TOKENS_PER_SECOND", default_value = "30")]
    pub tokens_per_second: NonZeroU32,

    /// Where replies come from.
    #[arg(long, env = "DATASET_SOURCE", default_value = "parquet")]
    pub dataset_source: DatasetSource,

    /// Parquet fixture to load (and optionally append to).
    #[arg(
        long,
        env = "DATASET_PATH",
        default_value = "data/conversations.parquet"
    )]
    pub dataset_path: PathBuf,

    /// Tokenizer preset used for pacing and usage counts.
    #[arg(
        long = "tokenizer",
        env = "TOKENIZER_MODEL",
        default_value = "cl100k-base"
    )]
    pub tokenizer: TokenizerPreset,

    /// JSON model catalogue backing GET /models.
    #[arg(long, env = "MODELS_PATH")]
    pub models_path: Option<PathBuf>,

    /// Comma-separated model ids, used when no catalogue file is available.
    #[arg(long = "models", env = "MODELS", value_delimiter = ',')]
    pub models: Option<Vec<String>>,

    /// `*` mirrors the request Origin; otherwise a comma-separated allow-list.
    #[arg(long, env = "NO_LLM_CORS_ORIGINS", default_value = "*")]
    pub cors_origins: String,

    /// Behaviour profile: a built-in name or a path to a scenario file.
    #[arg(long, env = "NO_LLM_SCENARIO", default_value = "default")]
    pub scenario: String,

    /// Seed for every simulated random decision.
    #[arg(long, env = "NO_LLM_SEED", default_value = "0")]
    pub seed: u64,

    /// Where `created` comes from.
    #[arg(long, env = "NO_LLM_IDENTITY_MODE", default_value = "derived")]
    pub identity_mode: IdentityModeArg,

    /// Authorization enforcement.
    #[arg(long, env = "NO_LLM_AUTH_MODE", default_value = "off")]
    pub auth_mode: AuthMode,

    /// Accepted bearer keys when `--auth-mode keys` is set.
    #[arg(long, env = "NO_LLM_AUTH_KEYS", value_delimiter = ',')]
    pub auth_keys: Vec<String>,

    /// Keys that answer 403 instead of 401, to exercise key-rotation paths.
    #[arg(long, env = "NO_LLM_AUTH_FORBIDDEN_KEYS", value_delimiter = ',')]
    pub auth_forbidden_keys: Vec<String>,

    /// Expose the /_mock control plane. Defaults to on for loopback binds only.
    #[arg(long, env = "NO_LLM_CONTROL_PLANE")]
    pub control_plane: Option<bool>,

    /// Token required by the control plane when the bind address is not loopback.
    #[arg(long, env = "NO_LLM_CONTROL_TOKEN")]
    pub control_token: Option<String>,

    /// Persist live completions to parquet.
    #[arg(
        long,
        env = "LIVE_RECORD",
        default_value = "false",
        num_args = 0..=1,
        default_missing_value = "true"
    )]
    pub live_record: bool,

    /// Output parquet for recorded live sessions.
    #[arg(long, env = "LIVE_RECORD_PATH")]
    pub live_record_path: Option<PathBuf>,

    /// Expose six Prometheus counters at GET /metrics.
    #[arg(long, env = "NO_LLM_METRICS", default_value = "false", num_args = 0..=1, default_missing_value = "true")]
    pub metrics: bool,

    /// Print the resolved configuration as JSON and exit.
    #[arg(long)]
    pub print_config: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Probe an HTTP readiness endpoint without requiring curl.
    Health(HealthArgs),
}

#[derive(Debug, Clone, Args)]
pub struct HealthArgs {
    /// Readiness URL to probe.
    #[arg(long, default_value = "http://127.0.0.1:8080/ready")]
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DatasetSource {
    Parquet,
    Live,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenizerPreset {
    #[value(name = "cl100k-base", alias = "cl100k_base")]
    Cl100kBase,
    #[value(name = "o200k-base", alias = "o200k_base")]
    O200kBase,
    #[value(name = "p50k-base", alias = "p50k_base")]
    P50kBase,
    #[value(name = "p50k-edit", alias = "p50k_edit")]
    P50kEdit,
    #[value(name = "r50k-base", alias = "r50k_base")]
    R50kBase,
}

impl TokenizerPreset {
    pub fn as_str(self) -> &'static str {
        match self {
            TokenizerPreset::Cl100kBase => "cl100k_base",
            TokenizerPreset::O200kBase => "o200k_base",
            TokenizerPreset::P50kBase => "p50k_base",
            TokenizerPreset::P50kEdit => "p50k_edit",
            TokenizerPreset::R50kBase => "r50k_base",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum IdentityModeArg {
    /// Ids and timestamps derive from the request digest.
    #[default]
    Derived,
    /// `created` follows the wall clock.
    Clock,
}

/// Decision D4: prefixed names, `off` by default so nothing regresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMode {
    #[default]
    Off,
    /// Any non-empty bearer token is accepted; GUIs insist on a key field.
    AnyBearer,
    /// Only keys in the accept-list are allowed.
    Keys,
}

/// Represents runtime configuration for the mock service.
#[derive(Debug, Clone, Serialize)]
pub struct Settings {
    pub bind_address: SocketAddr,
    pub tokens_per_second: NonZeroU32,
    pub dataset: DatasetSettings,
    pub tokenizer: TokenizerSettings,
    pub models: ModelSettings,
    pub cors: CorsSettings,
    pub scenario: String,
    pub seed: u64,
    pub identity_mode: IdentityModeArg,
    pub auth: AuthSettings,
    pub control_plane: ControlPlaneSettings,
    pub metrics: bool,
}

/// Describes how the service should source conversation data.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "source", rename_all = "lowercase")]
pub enum DatasetSettings {
    Parquet { path: PathBuf },
    Live(LiveSettings),
}

/// Captures configuration required to proxy or record live completions.
#[derive(Debug, Clone, Serialize)]
pub struct LiveSettings {
    pub record: bool,
    pub record_path: Option<PathBuf>,
}

/// Collects tokenizer-related configuration knobs.
#[derive(Debug, Clone, Serialize)]
pub struct TokenizerSettings {
    pub preset: String,
}

/// Where the model catalogue comes from, in precedence order.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ModelSettings {
    pub path: Option<PathBuf>,
    pub ids: Option<Vec<String>>,
}

/// Which origins the browser-facing CORS layer accepts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CorsSettings {
    /// Mirror whatever `Origin` the browser sent.
    MirrorAny,
    List(Vec<String>),
}

impl Default for CorsSettings {
    fn default() -> Self {
        Self::MirrorAny
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AuthSettings {
    pub mode: AuthMode,
    pub keys: Vec<String>,
    pub forbidden_keys: Vec<String>,
}

/// Decision D1: one `/_mock` namespace on the API listener, enabled by default
/// only for loopback binds; a public bind additionally requires a token.
#[derive(Debug, Clone, Serialize)]
pub struct ControlPlaneSettings {
    pub enabled: bool,
    pub token: Option<String>,
}

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("invalid bind address: {0}")]
    InvalidBindAddress(String),
    #[error("invalid tokens per second: {0}")]
    InvalidTokenRate(String),
    #[error("invalid dataset source: {0}")]
    InvalidDatasetSource(String),
    #[error("--auth-mode keys requires at least one --auth-keys value")]
    MissingAuthKeys,
    #[error("command line: {0}")]
    Cli(String),
}

impl Settings {
    /// Resolves settings from the environment only, for library and test use.
    pub fn load() -> Result<Self, SettingsError> {
        let cli = Cli::try_parse_from(["no-llm-api"])
            .map_err(|error| SettingsError::Cli(error.to_string()))?;
        Self::resolve(cli)
    }

    /// Validates a parsed command line into settings.
    pub fn resolve(cli: Cli) -> Result<Self, SettingsError> {
        let dataset = match cli.dataset_source {
            DatasetSource::Parquet => DatasetSettings::Parquet {
                path: cli.dataset_path.clone(),
            },
            DatasetSource::Live => DatasetSettings::Live(LiveSettings {
                record: cli.live_record,
                record_path: cli
                    .live_record_path
                    .clone()
                    .or_else(|| cli.live_record.then(|| cli.dataset_path.clone())),
            }),
        };

        if cli.auth_mode == AuthMode::Keys && cli.auth_keys.is_empty() {
            return Err(SettingsError::MissingAuthKeys);
        }

        let models_path = cli
            .models_path
            .clone()
            .or_else(|| {
                let default = PathBuf::from("data/models.json");
                default.exists().then_some(default)
            })
            .filter(|path| path.exists());

        let cors = match cli.cors_origins.trim() {
            "" | "*" => CorsSettings::MirrorAny,
            list => CorsSettings::List(
                list.split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .collect(),
            ),
        };

        let loopback = cli.bind_address.ip().is_loopback();
        let control_plane = ControlPlaneSettings {
            enabled: cli.control_plane.unwrap_or(loopback),
            token: cli.control_token.clone(),
        };
        if control_plane.enabled && !loopback && control_plane.token.is_none() {
            tracing::warn!(
                target: "no_llm_api",
                "control plane is enabled on a non-loopback bind without a token; \
                 anyone who can reach the port can reshape the system under test"
            );
        }

        Ok(Self {
            bind_address: cli.bind_address,
            tokens_per_second: cli.tokens_per_second,
            dataset,
            tokenizer: TokenizerSettings {
                preset: cli.tokenizer.as_str().to_string(),
            },
            models: ModelSettings {
                path: models_path,
                ids: cli.models.clone().filter(|ids| !ids.is_empty()),
            },
            cors,
            scenario: cli.scenario.clone(),
            seed: cli.seed,
            identity_mode: cli.identity_mode,
            auth: AuthSettings {
                mode: cli.auth_mode,
                keys: cli.auth_keys.clone(),
                forbidden_keys: cli.auth_forbidden_keys.clone(),
            },
            control_plane,
            metrics: cli.metrics,
        })
    }

    /// The `--print-config` payload.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self)
            .unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli(args: &[&str]) -> Cli {
        let mut full = vec!["no-llm-api"];
        full.extend_from_slice(args);
        Cli::try_parse_from(full).expect("parse")
    }

    #[test]
    fn defaults_are_the_documented_ones() {
        let settings = Settings::resolve(cli(&[])).unwrap();
        assert_eq!(settings.bind_address.to_string(), "127.0.0.1:8080");
        assert_eq!(settings.tokens_per_second.get(), 30);
        assert_eq!(settings.tokenizer.preset, "cl100k_base");
        assert_eq!(settings.cors, CorsSettings::MirrorAny);
        assert_eq!(settings.auth.mode, AuthMode::Off);
        assert_eq!(settings.scenario, "default");
        assert!(matches!(settings.dataset, DatasetSettings::Parquet { .. }));
    }

    #[test]
    fn control_plane_defaults_to_loopback_only() {
        assert!(Settings::resolve(cli(&[])).unwrap().control_plane.enabled);
        assert!(
            !Settings::resolve(cli(&["--bind-address", "0.0.0.0:8080"]))
                .unwrap()
                .control_plane
                .enabled
        );
        assert!(
            Settings::resolve(cli(&[
                "--bind-address",
                "0.0.0.0:8080",
                "--control-plane",
                "true"
            ]))
            .unwrap()
            .control_plane
            .enabled
        );
    }

    #[test]
    fn keys_mode_requires_keys() {
        assert!(Settings::resolve(cli(&["--auth-mode", "keys"])).is_err());
        let settings =
            Settings::resolve(cli(&["--auth-mode", "keys", "--auth-keys", "a,b"])).unwrap();
        assert_eq!(settings.auth.keys, vec!["a", "b"]);
    }

    #[test]
    fn tokenizer_accepts_both_spellings() {
        for value in ["cl100k-base", "cl100k_base"] {
            let settings = Settings::resolve(cli(&["--tokenizer", value])).unwrap();
            assert_eq!(settings.tokenizer.preset, "cl100k_base");
        }
    }

    #[test]
    fn cors_list_is_parsed() {
        let settings =
            Settings::resolve(cli(&["--cors-origins", "http://a.test, http://b.test"])).unwrap();
        assert_eq!(
            settings.cors,
            CorsSettings::List(vec!["http://a.test".into(), "http://b.test".into()])
        );
    }

    #[test]
    fn print_config_is_valid_json_covering_every_group() {
        let settings = Settings::resolve(cli(&[])).unwrap();
        let value: serde_json::Value = serde_json::from_str(&settings.to_json()).unwrap();
        for key in [
            "bind_address",
            "tokens_per_second",
            "dataset",
            "tokenizer",
            "models",
            "cors",
            "scenario",
            "seed",
            "identity_mode",
            "auth",
            "control_plane",
        ] {
            assert!(
                value.get(key).is_some(),
                "missing '{key}' in --print-config"
            );
        }
    }

    #[test]
    fn live_record_path_falls_back_to_the_dataset_path() {
        let settings = Settings::resolve(cli(&[
            "--dataset-source",
            "live",
            "--live-record",
            "true",
            "--dataset-path",
            "data/x.parquet",
        ]))
        .unwrap();
        match settings.dataset {
            DatasetSettings::Live(live) => {
                assert_eq!(live.record_path, Some(PathBuf::from("data/x.parquet")));
            }
            other => panic!("unexpected dataset settings: {other:?}"),
        }
    }
}
