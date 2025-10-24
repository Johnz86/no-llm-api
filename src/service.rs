use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use tiktoken_rs::CoreBPE;
use uuid::Uuid;

use crate::dataset::{AssistantMessage, ConversationScript, ConversationScripts};
use crate::model::{
    ChatCompletionChoice, ChatCompletionChunk, ChatCompletionChunkChoice, ChatCompletionChunkDelta,
    ChatCompletionRequest, ChatCompletionRequestMessage, ChatCompletionResponse,
    ChatCompletionResponseMessage, ChatCompletionUsage, ChatRole, MessageContent, StoredMessage,
};
use crate::store::{CompletionStore, ListFilters, SortOrder, StoredCompletion};

pub struct ChatService {
    scripts: Arc<[ConversationScript]>,
    cursors: Vec<AtomicUsize>,
    rotation: AtomicUsize,
    token_rate: NonZeroU32,
    store: CompletionStore,
    tokenizer: Arc<CoreBPE>,
}

pub struct PreparedCompletion {
    pub response: ChatCompletionResponse,
    pub tokens: Vec<usize>,
    pub include_usage_chunk: bool,
}

impl ChatService {
    pub fn new(
        scripts: ConversationScripts,
        tokenizer: Arc<CoreBPE>,
        token_rate: NonZeroU32,
    ) -> Self {
        let shared = scripts.share();
        let cursors = shared.iter().map(|_| AtomicUsize::new(0)).collect();
        Self {
            scripts: shared,
            cursors,
            rotation: AtomicUsize::new(0),
            token_rate,
            store: CompletionStore::new(),
            tokenizer,
        }
    }

    pub fn tokens_per_second(&self) -> NonZeroU32 {
        self.token_rate
    }

    pub fn tokenizer(&self) -> Arc<CoreBPE> {
        self.tokenizer.clone()
    }

    pub async fn create_completion(&self, request: ChatCompletionRequest) -> PreparedCompletion {
        let store_enabled = request.store.unwrap_or(false);
        let include_usage_chunk = request
            .stream_options
            .as_ref()
            .map(|options| options.include_usage)
            .unwrap_or(false);

        let assistant = match self.match_assistant_response(&request) {
            Some(message) => message,
            None => {
                let script_index =
                    self.rotation.fetch_add(1, Ordering::Relaxed) % self.scripts.len();
                let script = &self.scripts[script_index];
                let turn_index = self.cursors[script_index].fetch_add(1, Ordering::Relaxed);
                script.assistant_at(turn_index)
            }
        };

        let mut tokens: Vec<usize> = assistant.tokens.iter().copied().collect();
        let mut assistant_text = assistant.text.as_ref().to_string();
        let mut finish_reason = "stop".to_string();

        if let Some(cap) = request
            .max_completion_tokens
            .or(request.max_tokens)
            .map(|value| value as usize)
            .filter(|cap| *cap < tokens.len())
        {
            tokens.truncate(cap);
            match self.tokenizer.decode(tokens.clone()) {
                Ok(decoded) => assistant_text = decoded,
                Err(error) => {
                    tracing::error!(
                        target: "no_llm_api",
                        ?error,
                        "failed to decode truncated completion; using full response"
                    );
                    tokens = assistant.tokens.iter().copied().collect();
                    assistant_text = assistant.text.as_ref().to_string();
                }
            }
            finish_reason = "length".to_string();
        }

        let completion_token_count = tokens.len() as u32;
        let prompt_tokens = count_prompt_tokens(&request.messages, &self.tokenizer);

        let usage = ChatCompletionUsage {
            prompt_tokens,
            completion_tokens: completion_token_count,
            total_tokens: prompt_tokens + completion_token_count,
        };

        let created = unix_timestamp();
        let identifier = format!("chatcmpl-{}", Uuid::new_v4());
        let request_id = format!("req_{}", Uuid::new_v4().as_simple());

        let completion_message = ChatCompletionResponseMessage {
            role: ChatRole::Assistant,
            content: Some(MessageContent::Text(assistant_text.clone())),
            refusal: None,
            tool_calls: None,
            function_call: None,
            audio: None,
        };

        let choice = ChatCompletionChoice {
            index: 0,
            message: completion_message,
            finish_reason: finish_reason.clone(),
            logprobs: None,
        };

        let temperature = request.temperature.or(Some(1.0));
        let top_p = request.top_p.or(Some(1.0));
        let frequency_penalty = request.frequency_penalty.or(Some(0.0));
        let presence_penalty = request.presence_penalty.or(Some(0.0));
        let service_tier = request
            .service_tier
            .clone()
            .or_else(|| Some("default".to_string()));

        let response = ChatCompletionResponse {
            id: identifier.clone(),
            object: "chat.completion".to_string(),
            created,
            model: request.model.clone(),
            usage,
            choices: vec![choice],
            metadata: request.metadata.clone(),
            system_fingerprint: Some("fp_mock".to_string()),
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
        };

        if store_enabled {
            let stored_messages =
                build_stored_messages(&identifier, &request.messages, assistant_text.clone());
            self.store.save(response.clone(), stored_messages).await;
        }

        PreparedCompletion {
            response,
            tokens,
            include_usage_chunk,
        }
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

    fn match_assistant_response(
        &self,
        request: &ChatCompletionRequest,
    ) -> Option<AssistantMessage> {
        let user_message = last_user_message(&request.messages)?;
        self.scripts
            .iter()
            .find_map(|script| script.response_for_user(&user_message))
    }
}

fn build_stored_messages(
    completion_id: &str,
    messages: &[ChatCompletionRequestMessage],
    assistant_content: String,
) -> Vec<StoredMessage> {
    let mut stored = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        stored.push(StoredMessage {
            id: format!("{}-{}", completion_id, index),
            role: message.role.clone(),
            content: message.content.clone(),
            name: message.name.clone(),
        });
    }
    stored.push(StoredMessage {
        id: format!("{}-{}", completion_id, stored.len()),
        role: ChatRole::Assistant,
        content: Some(MessageContent::Text(assistant_content)),
        name: None,
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

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn last_user_message(messages: &[ChatCompletionRequestMessage]) -> Option<String> {
    messages.iter().rev().find_map(|message| {
        if matches!(message.role, ChatRole::User) {
            message
                .content
                .as_ref()
                .map(|content| content.render().into_owned())
        } else {
            None
        }
    })
}

pub fn chunk_from_delta(
    response: &ChatCompletionResponse,
    delta: ChatCompletionChunkDelta,
    finish: Option<&str>,
) -> ChatCompletionChunk {
    let choice = ChatCompletionChunkChoice {
        index: 0,
        delta,
        finish_reason: finish.map(|reason| reason.to_string()),
        logprobs: None,
    };

    ChatCompletionChunk {
        id: response.id.clone(),
        object: "chat.completion.chunk".to_string(),
        created: response.created,
        model: response.model.clone(),
        choices: vec![choice],
        system_fingerprint: response.system_fingerprint.clone(),
        usage: None,
    }
}

pub fn delta_for_text(content: String, include_role: bool) -> ChatCompletionChunkDelta {
    ChatCompletionChunkDelta {
        role: include_role.then_some(ChatRole::Assistant),
        content: Some(content),
        function_call: None,
        tool_calls: None,
        refusal: None,
    }
}

pub fn empty_delta() -> ChatCompletionChunkDelta {
    ChatCompletionChunkDelta {
        role: None,
        content: None,
        function_call: None,
        tool_calls: None,
        refusal: None,
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
        usage: Some(response.usage.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::{ConversationScripts, ensure_sample_dataset};
    use crate::model::{
        ChatCompletionRequest, ChatCompletionRequestMessage, ChatRole, MessageContent,
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
                tool_calls: None,
                function_call: None,
                audio: None,
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
        let result = service.create_completion(request.clone()).await;
        assert!(!result.tokens.is_empty());
        let stored = service
            .list(SortOrder::Ascending, None, 10, ListFilters::default())
            .await;
        assert!(stored.is_empty());

        let mut second = request.clone();
        second.store = Some(true);
        let stored_result = service.create_completion(second).await;
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
        let result = service.create_completion(request).await;
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
        let result = service.create_completion(request).await;
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
                .map(|choice| choice.finish_reason.as_str()),
            Some("length")
        );
    }
}
