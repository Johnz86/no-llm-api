use std::path::PathBuf;

use async_openai::Client;
use async_openai::config::{AzureConfig, OpenAIConfig};
use async_openai::error::OpenAIError;
use async_openai::types::chat::{CreateChatCompletionRequest, CreateChatCompletionResponse};
use serde_json::{self, Value};
use thiserror::Error;
use tiktoken_rs::CoreBPE;

use crate::config::LiveSettings;
use crate::dataset::{DatasetError, append_dataset_rows, rows_from_interaction};
use crate::live_safety::{LiveCaps, redact_text};
use crate::model::{ChatCompletionRequest, ChatCompletionResponse};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::Semaphore;

#[derive(Debug)]
pub struct LiveBackend {
    client: LiveClient,
    record: bool,
    record_path: Option<PathBuf>,
    caps: LiveCaps,
    /// Bounds how many upstream calls can be in flight at once.
    permits: Semaphore,
    /// Bounds how many upstream calls one process can make at all.
    spent: AtomicU64,
}

#[derive(Debug)]
enum LiveClient {
    OpenAI(Client<OpenAIConfig>),
    Azure(Client<AzureConfig>),
}

#[derive(Debug)]
pub struct LiveResult {
    pub response: ChatCompletionResponse,
    pub tokens: Vec<u32>,
}

#[derive(Debug, Error)]
pub enum LiveBackendError {
    #[error("missing environment variable `{0}`")]
    MissingEnv(&'static str),
    #[error("failed to serialize request: {0}")]
    RequestSerialization(#[from] serde_json::Error),
    #[error("openai error: {0}")]
    Api(#[from] OpenAIError),
    #[error("dataset error: {0}")]
    Dataset(#[from] DatasetError),
    #[error("live request budget of {0} requests for this process is exhausted")]
    BudgetExhausted(u64),
    #[error("live request timed out after {0}s")]
    Timeout(u64),
}

impl LiveBackend {
    pub fn new(settings: &LiveSettings) -> Result<Self, LiveBackendError> {
        let client = build_client()?;
        let caps = LiveCaps::default();
        Ok(Self {
            client,
            record: settings.record,
            record_path: settings.record_path.clone(),
            permits: Semaphore::new(caps.max_concurrent_requests),
            spent: AtomicU64::new(0),
            caps,
        })
    }

    pub async fn create_completion(
        &self,
        request: &ChatCompletionRequest,
        tokenizer: &CoreBPE,
    ) -> Result<LiveResult, LiveBackendError> {
        let spent = self.spent.fetch_add(1, Ordering::Relaxed);
        if spent >= self.caps.max_requests_per_session {
            return Err(LiveBackendError::BudgetExhausted(
                self.caps.max_requests_per_session,
            ));
        }
        let _permit = self
            .permits
            .acquire()
            .await
            .expect("live permit semaphore closed");

        let mut openai_request = convert_request(request)?;
        openai_request.stream = Some(false);

        let call = async {
            match &self.client {
                LiveClient::OpenAI(client) => client.chat().create(openai_request).await,
                LiveClient::Azure(client) => client.chat().create(openai_request).await,
            }
        };
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(self.caps.request_timeout_secs),
            call,
        )
        .await
        .map_err(|_| LiveBackendError::Timeout(self.caps.request_timeout_secs))??;

        let response = convert_response(response)?;
        let tokens = tokens_for_response(&response, tokenizer);

        if self.record {
            self.record_interaction(request, &response)?;
        }

        Ok(LiveResult { response, tokens })
    }

    fn record_interaction(
        &self,
        request: &ChatCompletionRequest,
        response: &ChatCompletionResponse,
    ) -> Result<(), LiveBackendError> {
        let Some(path) = self.record_path.as_ref() else {
            return Ok(());
        };

        if response.choices.is_empty() {
            return Ok(());
        }

        // One redaction pass before anything reaches disk: recorded output can
        // quote a key that a user pasted into a prompt.
        let mut rows = rows_from_interaction(&response.id, &request.messages, response);
        for row in &mut rows {
            if let Some(content) = row.content.as_ref() {
                row.content = Some(std::borrow::Cow::Owned(redact_text(content)));
            }
            if let Some(parts) = row.content_parts.as_ref() {
                row.content_parts = Some(std::borrow::Cow::Owned(redact_text(parts)));
            }
            if let Some(refusal) = row.refusal.as_ref() {
                row.refusal = Some(std::borrow::Cow::Owned(redact_text(refusal)));
            }
            if let Some(tool_calls) = row.tool_calls.as_ref() {
                row.tool_calls = Some(std::borrow::Cow::Owned(redact_text(tool_calls)));
            }
        }
        append_dataset_rows(path, &rows)?;
        Ok(())
    }
}

fn build_client() -> Result<LiveClient, LiveBackendError> {
    if let Ok(endpoint) = std::env::var("AZURE_OPENAI_ENDPOINT") {
        let api_key = std::env::var("AZURE_OPENAI_API_KEY")
            .map_err(|_| LiveBackendError::MissingEnv("AZURE_OPENAI_API_KEY"))?;
        let deployment = std::env::var("AZURE_OPENAI_DEPLOYMENT_NAME")
            .map_err(|_| LiveBackendError::MissingEnv("AZURE_OPENAI_DEPLOYMENT_NAME"))?;
        let version =
            std::env::var("AZURE_OPENAI_API_VERSION").unwrap_or_else(|_| "2024-06-01".to_string());

        let config = AzureConfig::new()
            .with_api_base(endpoint)
            .with_api_key(api_key)
            .with_deployment_id(deployment)
            .with_api_version(version);
        Ok(LiveClient::Azure(Client::with_config(config)))
    } else {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| LiveBackendError::MissingEnv("OPENAI_API_KEY"))?;
        let mut config = OpenAIConfig::new().with_api_key(api_key);
        if let Ok(api_base) = std::env::var("OPENAI_API_BASE") {
            config = config.with_api_base(api_base);
        }
        if let Ok(org) = std::env::var("OPENAI_ORG_ID") {
            config = config.with_org_id(org);
        }
        if let Ok(project) = std::env::var("OPENAI_PROJECT_ID") {
            config = config.with_project_id(project);
        }
        Ok(LiveClient::OpenAI(Client::with_config(config)))
    }
}

fn convert_request(
    request: &ChatCompletionRequest,
) -> Result<CreateChatCompletionRequest, serde_json::Error> {
    let mut value = serde_json::to_value(request)?;
    if let Value::Object(ref mut map) = value {
        map.insert("stream".to_string(), Value::Bool(false));
    }
    serde_json::from_value(value)
}

fn convert_response(
    response: CreateChatCompletionResponse,
) -> Result<ChatCompletionResponse, serde_json::Error> {
    let value = serde_json::to_value(response)?;
    serde_json::from_value(value)
}

fn tokens_for_response(response: &ChatCompletionResponse, tokenizer: &CoreBPE) -> Vec<u32> {
    response
        .choices
        .first()
        .and_then(|choice| choice.message.content.as_ref())
        .map(|content| tokenizer.encode_with_special_tokens(content.render().as_ref()))
        .unwrap_or_default()
}
