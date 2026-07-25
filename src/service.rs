use std::num::NonZeroU32;
use std::sync::Arc;

use serde_json::{Map, Value};
use tiktoken_rs::CoreBPE;

use crate::dataset::ConversationScripts;
#[cfg(feature = "live")]
use crate::live::{LiveBackend, LiveBackendError};
use crate::model::{
    ChatCompletionChoice, ChatCompletionChunk, ChatCompletionChunkChoice, ChatCompletionChunkDelta,
    ChatCompletionRequest, ChatCompletionRequestMessage, ChatCompletionResponse,
    ChatCompletionResponseMessage, ChatCompletionUsage, ChatRole, FinishReason, MessageContent,
    StoredMessage,
};
use crate::sim::digest::Digest;
use crate::sim::identity::{Clock, Identity, IdentityMode, SystemClock};
use crate::sim::select::{MatchKind, ScriptIndex, SelectionKey};
use crate::store::{CompletionStore, ListFilters, SortOrder, StoredCompletion};
use thiserror::Error;

enum ChatBackend {
    Dataset(ScriptIndex),
    #[cfg(feature = "live")]
    Live(Box<LiveBackend>),
}

#[derive(Debug, Error)]
pub enum ServiceError {
    #[cfg(feature = "live")]
    #[error("live backend error: {0}")]
    Live(#[from] LiveBackendError),
    #[error("no choices returned from completion")]
    EmptyResponse,
}

pub struct ChatService {
    backend: ChatBackend,
    token_rate: NonZeroU32,
    store: CompletionStore,
    tokenizer: Arc<CoreBPE>,
    identity_mode: IdentityMode,
    clock: Arc<dyn Clock>,
    tokenizer_name: String,
}

pub struct PreparedCompletion {
    pub response: ChatCompletionResponse,
    pub tokens: Vec<u32>,
    pub include_usage_chunk: bool,
    pub match_kind: MatchKind,
}

impl ChatService {
    pub fn new(
        scripts: ConversationScripts,
        tokenizer: Arc<CoreBPE>,
        token_rate: NonZeroU32,
    ) -> Self {
        Self {
            backend: ChatBackend::Dataset(ScriptIndex::build(scripts.share())),
            token_rate,
            store: CompletionStore::new(),
            tokenizer,
            identity_mode: IdentityMode::default(),
            clock: Arc::new(SystemClock),
            tokenizer_name: "cl100k_base".to_string(),
        }
    }

    #[cfg(feature = "live")]
    pub fn with_live(
        backend: LiveBackend,
        tokenizer: Arc<CoreBPE>,
        token_rate: NonZeroU32,
    ) -> Self {
        Self {
            backend: ChatBackend::Live(Box::new(backend)),
            token_rate,
            store: CompletionStore::new(),
            tokenizer,
            identity_mode: IdentityMode::default(),
            clock: Arc::new(SystemClock),
            tokenizer_name: "cl100k_base".to_string(),
        }
    }

    /// Records which tokenizer preset is in use, for readiness reporting.
    pub fn with_tokenizer_name(mut self, name: impl Into<String>) -> Self {
        self.tokenizer_name = name.into();
        self
    }

    /// Overrides where `created` comes from; ids stay plan-derived either way.
    pub fn with_identity_mode(mut self, mode: IdentityMode) -> Self {
        self.identity_mode = mode;
        self
    }

    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    pub fn tokens_per_second(&self) -> NonZeroU32 {
        self.token_rate
    }

    /// How many fixture conversations are loaded; zero means not ready.
    pub fn script_count(&self) -> usize {
        match &self.backend {
            ChatBackend::Dataset(scripts) => scripts.len(),
            #[cfg(feature = "live")]
            ChatBackend::Live(_) => usize::MAX,
        }
    }

    /// The tokenizer preset name, for readiness reporting.
    pub fn tokenizer_name(&self) -> &str {
        &self.tokenizer_name
    }

    pub fn tokenizer(&self) -> Arc<CoreBPE> {
        self.tokenizer.clone()
    }

    pub async fn create_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<PreparedCompletion, ServiceError> {
        match &self.backend {
            ChatBackend::Dataset(dataset) => self.create_completion_dataset(dataset, request).await,
            #[cfg(feature = "live")]
            ChatBackend::Live(live) => self.create_completion_live(live, request).await,
        }
    }

    async fn create_completion_dataset(
        &self,
        scripts: &ScriptIndex,
        request: ChatCompletionRequest,
    ) -> Result<PreparedCompletion, ServiceError> {
        let store_enabled = request.store.unwrap_or(false);
        let include_usage_chunk = request
            .stream_options
            .as_ref()
            .map(|options| options.include_usage)
            .unwrap_or(false);

        let key = SelectionKey::from_request(&request);
        let selection = scripts.select(&key);
        let assistant = selection.message;

        let mut tokens: Vec<u32> = assistant.tokens.iter().copied().collect();
        let mut assistant_text = assistant.rendered_text.as_ref().to_string();
        let mut finish_reason = assistant
            .finish_reason
            .clone()
            .unwrap_or(FinishReason::Stop);
        let mut truncated = false;

        if let Some(cap) = request
            .max_completion_tokens
            .or(request.max_tokens)
            .map(|value| value as usize)
            .filter(|cap| *cap < tokens.len())
        {
            tokens.truncate(cap);
            match self.tokenizer.decode(&tokens) {
                Ok(decoded) => assistant_text = decoded,
                Err(error) => {
                    tracing::error!(
                        target: "no_llm_api",
                        ?error,
                        "failed to decode truncated completion; using full response"
                    );
                    tokens = assistant.tokens.iter().copied().collect();
                    assistant_text = assistant.rendered_text.as_ref().to_string();
                }
            }
            truncated = true;
            finish_reason = FinishReason::Length;
        }

        let computed_prompt_tokens = count_prompt_tokens(&request.messages, &self.tokenizer);
        let mut usage = assistant.usage.clone().unwrap_or_else(|| {
            let completion_tokens = tokens.len() as u32;
            ChatCompletionUsage {
                prompt_tokens: computed_prompt_tokens,
                completion_tokens,
                total_tokens: computed_prompt_tokens + completion_tokens,
                prompt_tokens_details: None,
                completion_tokens_details: None,
            }
        });
        if truncated || assistant.usage.is_none() {
            usage.completion_tokens = tokens.len() as u32;
            usage.total_tokens = usage.prompt_tokens + usage.completion_tokens;
        }

        let plan_digest = {
            let mut digest = Digest::new();
            digest.field(&key.digest().to_le_bytes());
            digest.field(selection.script_id.as_bytes());
            digest.field(assistant.rendered_text.as_bytes());
            digest.field(&(tokens.len() as u64).to_le_bytes());
            // Metadata does not steer fixture selection, but it is echoed back, so
            // two requests that differ only in metadata are different responses.
            if let Some(metadata) = &request.metadata {
                digest.field(Value::Object(metadata.clone()).to_string().as_bytes());
            }
            digest.finish()
        };
        let identity = Identity::derive(plan_digest, self.identity_mode, self.clock.as_ref());
        let created = identity.created;
        let identifier = identity.id.clone();
        let request_id = identity.request_id.clone();

        let message_content = if truncated {
            if assistant_text.is_empty() {
                None
            } else {
                Some(MessageContent::Text(assistant_text.clone()))
            }
        } else if let Some(content) = assistant.content.clone() {
            Some(content)
        } else if assistant_text.is_empty() {
            None
        } else {
            Some(MessageContent::Text(assistant_text.clone()))
        };

        // A `<think>...</think>` prefix becomes reasoning_content, and the tokens
        // are re-derived so pacing and usage still describe what is on the wire.
        let (reasoning_content, message_content, reasoning_tokens) = match &message_content {
            Some(MessageContent::Text(text)) => match crate::model::split_reasoning(text) {
                (Some(reasoning), visible) => {
                    let reasoning_token_count =
                        self.tokenizer.encode_with_special_tokens(&reasoning).len() as u32;
                    tokens = self.tokenizer.encode_with_special_tokens(&visible);
                    (
                        Some(reasoning),
                        Some(MessageContent::Text(visible)),
                        Some(reasoning_token_count),
                    )
                }
                (None, _) => (None, message_content.clone(), None),
            },
            other => (None, other.clone(), None),
        };

        if let Some(reasoning_token_count) = reasoning_tokens {
            usage.completion_tokens = tokens.len() as u32 + reasoning_token_count;
            usage.total_tokens = usage.prompt_tokens + usage.completion_tokens;
            let details = usage.completion_tokens_details.get_or_insert_default();
            details.reasoning_tokens = Some(reasoning_token_count);
        }

        let completion_message = ChatCompletionResponseMessage {
            role: ChatRole::Assistant,
            content: message_content,
            refusal: assistant.refusal.clone(),
            reasoning_content,
            tool_calls: assistant.tool_calls.clone(),
            function_call: assistant.function_call.clone(),
            audio: assistant.audio.clone(),
        };

        let choice = ChatCompletionChoice {
            index: 0,
            message: completion_message.clone(),
            finish_reason: Some(finish_reason.clone()),
            logprobs: None,
        };

        let temperature = request.temperature.or(Some(1.0));
        let top_p = request.top_p.or(Some(1.0));
        let frequency_penalty = request.frequency_penalty.or(Some(0.0));
        let presence_penalty = request.presence_penalty.or(Some(0.0));
        let service_tier = request
            .service_tier
            .or(Some(crate::request_types::ServiceTier::Default));

        let response = ChatCompletionResponse {
            id: identifier.clone(),
            object: "chat.completion".to_string(),
            created,
            model: request.model.clone(),
            usage,
            choices: vec![choice],
            metadata: request.metadata.clone(),
            system_fingerprint: Some(identity.system_fingerprint.clone()),
            service_tier,
            request_id: Some(request_id),
            temperature,
            top_p,
            frequency_penalty,
            presence_penalty,
            stop: request.stop.clone(),
            seed: request.seed,
            tool_choice: request.tool_choice.clone(),
            response_format: request.response_format.clone(),
            parallel_tool_calls: request.parallel_tool_calls,
            modalities: request.modalities.clone(),
            response_prefix: request.response_prefix.clone(),
            logit_bias: request.logit_bias.clone(),
            stream_options: request.stream_options.clone(),
            audio: request.audio.clone(),
            tools: request.tools.clone(),
            input_user: request.user.clone(),
        };

        if store_enabled {
            let stored_messages =
                build_stored_messages(&identifier, &request.messages, &completion_message);
            self.store.save(response.clone(), stored_messages).await;
        }

        Ok(PreparedCompletion {
            response,
            tokens,
            include_usage_chunk,
            match_kind: selection.kind,
        })
    }

    #[cfg(feature = "live")]
    async fn create_completion_live(
        &self,
        backend: &LiveBackend,
        request: ChatCompletionRequest,
    ) -> Result<PreparedCompletion, ServiceError> {
        let store_enabled = request.store.unwrap_or(false);
        let include_usage_chunk = request
            .stream_options
            .as_ref()
            .map(|options| options.include_usage)
            .unwrap_or(false);

        let live = backend.create_completion(&request, &self.tokenizer).await?;
        if live.response.choices.is_empty() {
            return Err(ServiceError::EmptyResponse);
        }

        let response = live.response;
        if store_enabled {
            let identifier = response.id.clone();
            if let Some(choice) = response.choices.first() {
                let stored_messages =
                    build_stored_messages(&identifier, &request.messages, &choice.message);
                self.store.save(response.clone(), stored_messages).await;
            }
        }

        Ok(PreparedCompletion {
            response,
            tokens: live.tokens,
            include_usage_chunk,
            match_kind: MatchKind::Fallback,
        })
    }

    pub async fn list(
        &self,
        order: SortOrder,
        after: Option<&str>,
        limit: usize,
        filters: ListFilters,
    ) -> Vec<StoredCompletion> {
        self.store.list(order, after, limit, &filters).await
    }

    pub async fn get(&self, id: &str) -> Option<StoredCompletion> {
        self.store.get(id).await
    }

    pub async fn update_metadata(
        &self,
        id: &str,
        metadata: Map<String, Value>,
    ) -> Option<StoredCompletion> {
        self.store.update_metadata(id, metadata).await
    }

    pub async fn delete(&self, id: &str) -> Option<crate::model::ChatCompletionDeleted> {
        self.store.delete(id).await
    }

    pub async fn messages(
        &self,
        id: &str,
        order: SortOrder,
        after: Option<&str>,
        limit: usize,
    ) -> Option<Vec<StoredMessage>> {
        self.store.messages(id, order, after, limit).await
    }
}

fn build_stored_messages(
    completion_id: &str,
    messages: &[ChatCompletionRequestMessage],
    assistant: &ChatCompletionResponseMessage,
) -> Vec<StoredMessage> {
    let mut stored = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        stored.push(StoredMessage {
            id: format!("{}-{}", completion_id, index),
            role: message.role.clone(),
            content: message.content.clone(),
            content_parts: message
                .content
                .as_ref()
                .and_then(|content| content.parts().map(<[_]>::to_vec)),
            name: message.name.clone(),
            refusal: message.refusal.clone(),
            tool_calls: None,
            function_call: None,
            audio: None,
        });
    }
    stored.push(StoredMessage {
        id: format!("{}-{}", completion_id, stored.len()),
        role: ChatRole::Assistant,
        content: assistant.content.clone(),
        content_parts: assistant
            .content
            .as_ref()
            .and_then(|content| content.parts().map(<[_]>::to_vec)),
        name: None,
        refusal: assistant.refusal.clone(),
        tool_calls: assistant.tool_calls.clone(),
        function_call: assistant.function_call.clone(),
        audio: assistant.audio.clone(),
    });
    stored
}

fn count_prompt_tokens(messages: &[ChatCompletionRequestMessage], tokenizer: &CoreBPE) -> u32 {
    messages
        .iter()
        .map(|message| match &message.content {
            Some(content) => tokenizer
                .encode_with_special_tokens(&content.render())
                .len() as u32,
            None => 0,
        })
        .sum()
}

pub fn chunk_from_delta(
    response: &ChatCompletionResponse,
    delta: ChatCompletionChunkDelta,
    finish: Option<FinishReason>,
) -> ChatCompletionChunk {
    let choice = ChatCompletionChunkChoice {
        index: 0,
        delta,
        finish_reason: finish.clone(),
        logprobs: None,
    };

    ChatCompletionChunk {
        id: response.id.clone(),
        object: "chat.completion.chunk".to_string(),
        created: response.created,
        model: response.model.clone(),
        choices: vec![choice],
        system_fingerprint: response.system_fingerprint.clone(),
        service_tier: response.service_tier,
        usage: None,
    }
}

pub fn usage_chunk(response: &ChatCompletionResponse) -> ChatCompletionChunk {
    ChatCompletionChunk {
        id: response.id.clone(),
        object: "chat.completion.chunk".to_string(),
        created: response.created,
        model: response.model.clone(),
        choices: Vec::new(),
        system_fingerprint: response.system_fingerprint.clone(),
        service_tier: response.service_tier,
        usage: Some(response.usage.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::{ConversationScripts, ensure_sample_dataset};
    use crate::model::{
        ChatCompletionRequest, ChatCompletionRequestMessage, ChatRole, FinishReason, MessageContent,
    };
    use tempfile::tempdir;
    use tiktoken_rs::cl100k_base;

    fn request_with_text(text: &str) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "test-model".to_string(),
            messages: vec![ChatCompletionRequestMessage {
                role: ChatRole::User,
                content: Some(MessageContent::Text(text.to_string())),
                name: None,
                tool_call_id: None,
                tool_calls: None,
                function_call: None,
                audio: None,
                refusal: None,
            }],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn stores_completion_when_requested() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sample.parquet");
        ensure_sample_dataset(&path).unwrap();
        let tokenizer = Arc::new(cl100k_base().unwrap());
        let scripts = ConversationScripts::load(&path, &tokenizer).unwrap();
        let service = ChatService::new(scripts, tokenizer, NonZeroU32::new(5).unwrap());

        let request = request_with_text("hello");
        let result = service.create_completion(request.clone()).await.unwrap();
        assert!(!result.tokens.is_empty());
        let stored = service
            .list(SortOrder::Ascending, None, 10, ListFilters::default())
            .await;
        assert!(stored.is_empty());

        let mut second = request.clone();
        second.store = Some(true);
        let stored_result = service.create_completion(second).await.unwrap();
        let stored = service
            .list(SortOrder::Ascending, None, 10, ListFilters::default())
            .await;
        assert_eq!(stored.len(), 1);
        let retrieved = service.get(&stored_result.response.id).await.unwrap();
        assert_eq!(retrieved.completion.id, stored_result.response.id);
    }

    #[tokio::test]
    async fn matches_user_prompt_to_dataset() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sample.parquet");
        ensure_sample_dataset(&path).unwrap();
        let tokenizer = Arc::new(cl100k_base().unwrap());
        let scripts = ConversationScripts::load(&path, &tokenizer).unwrap();
        let service = ChatService::new(scripts, tokenizer, NonZeroU32::new(30).unwrap());

        let request = request_with_text("Summarize the sprint update.");
        let result = service.create_completion(request).await.unwrap();
        let response_text = match &result.response.choices[0].message.content {
            Some(MessageContent::Text(text)) => text.as_str(),
            _ => panic!("unexpected response type"),
        };
        assert_eq!(
            response_text,
            "Sprint closed 14 tickets, shipped analytics, and stabilized the API."
        );
    }

    #[tokio::test]
    async fn applies_max_completion_token_cap() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sample.parquet");
        ensure_sample_dataset(&path).unwrap();
        let tokenizer = Arc::new(cl100k_base().unwrap());
        let scripts = ConversationScripts::load(&path, &tokenizer).unwrap();
        let service = ChatService::new(scripts, tokenizer, NonZeroU32::new(30).unwrap());

        let mut request = request_with_text("Summarize the sprint update.");
        request.max_completion_tokens = Some(5);
        let result = service.create_completion(request).await.unwrap();
        assert!(result.response.usage.completion_tokens <= 5);
        assert_eq!(
            result.tokens.len() as u32,
            result.response.usage.completion_tokens
        );
        assert_eq!(
            result
                .response
                .choices
                .first()
                .and_then(|choice| choice.finish_reason.clone()),
            Some(FinishReason::Length)
        );
    }
}
