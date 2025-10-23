use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use uuid::Uuid;

use crate::dataset::{ConversationScript, ConversationScripts};
use crate::model::{
    ChatCompletionChoice, ChatCompletionChunk, ChatCompletionChunkChoice, ChatCompletionChunkDelta,
    ChatCompletionRequest, ChatCompletionRequestMessage, ChatCompletionResponse,
    ChatCompletionResponseMessage, ChatCompletionUsage, ChatRole, MessageContent, StoredMessage,
};
use crate::store::{CompletionStore, SortOrder, StoredCompletion};

pub struct ChatService {
    scripts: Arc<[ConversationScript]>,
    cursors: Vec<AtomicUsize>,
    rotation: AtomicUsize,
    token_rate: NonZeroU32,
    store: CompletionStore,
}

pub struct PreparedCompletion {
    pub response: ChatCompletionResponse,
    pub tokens: Vec<String>,
}

impl ChatService {
    pub fn new(scripts: ConversationScripts, token_rate: NonZeroU32) -> Self {
        let shared = scripts.share();
        let cursors = shared.iter().map(|_| AtomicUsize::new(0)).collect();
        Self {
            scripts: shared,
            cursors,
            rotation: AtomicUsize::new(0),
            token_rate,
            store: CompletionStore::new(),
        }
    }

    pub fn tokens_per_second(&self) -> NonZeroU32 {
        self.token_rate
    }

    pub async fn create_completion(&self, request: ChatCompletionRequest) -> PreparedCompletion {
        let store_enabled = request.store.unwrap_or(false);
        let script_index = self.rotation.fetch_add(1, Ordering::Relaxed) % self.scripts.len();
        let script = &self.scripts[script_index];
        let turn_index = self.cursors[script_index].fetch_add(1, Ordering::Relaxed);
        let assistant_content = script.assistant_at(turn_index).to_string();

        let completion_tokens = tokenize(&assistant_content);
        let completion_token_count = completion_tokens.len() as u32;
        let prompt_tokens = count_prompt_tokens(&request.messages);
        let usage = ChatCompletionUsage {
            prompt_tokens,
            completion_tokens: completion_token_count,
            total_tokens: prompt_tokens + completion_token_count,
        };

        let created = unix_timestamp();
        let identifier = format!("chatcmpl-{}", Uuid::new_v4());

        let completion_message = ChatCompletionResponseMessage {
            role: ChatRole::Assistant,
            content: Some(MessageContent::Text(assistant_content.clone())),
            refusal: None,
            tool_calls: None,
            function_call: None,
            audio: None,
        };

        let choice = ChatCompletionChoice {
            index: 0,
            message: completion_message,
            finish_reason: "stop".to_string(),
            logprobs: None,
        };

        let response = ChatCompletionResponse {
            id: identifier.clone(),
            object: "chat.completion".to_string(),
            created,
            model: request.model.clone(),
            usage,
            choices: vec![choice],
            metadata: request.metadata.clone(),
            system_fingerprint: Some("fp_mock".to_string()),
            service_tier: Some("default".to_string()),
            tool_choice: request.tool_choice.clone(),
            response_format: request.response_format.clone(),
        };

        if store_enabled {
            let stored_messages =
                build_stored_messages(&identifier, &request.messages, assistant_content.clone());
            self.store.save(response.clone(), stored_messages).await;
        }

        PreparedCompletion {
            response,
            tokens: completion_tokens,
        }
    }

    pub async fn list(
        &self,
        order: SortOrder,
        after: Option<&str>,
        limit: usize,
    ) -> Vec<StoredCompletion> {
        self.store.list(order, after, limit).await
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

fn count_prompt_tokens(messages: &[ChatCompletionRequestMessage]) -> u32 {
    messages
        .iter()
        .map(|message| match &message.content {
            Some(content) => count_tokens(&content.render()),
            None => 0,
        })
        .sum()
}

fn tokenize(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|token| token.to_string())
        .collect()
}

fn count_tokens(text: &str) -> u32 {
    tokenize(text).len() as u32
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
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
    }
}

pub fn delta_for_token(token: String, include_role: bool) -> ChatCompletionChunkDelta {
    ChatCompletionChunkDelta {
        role: include_role.then_some(ChatRole::Assistant),
        content: Some(token),
        function_call: None,
        tool_calls: None,
    }
}

pub fn empty_delta() -> ChatCompletionChunkDelta {
    ChatCompletionChunkDelta {
        role: None,
        content: None,
        function_call: None,
        tool_calls: None,
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

    fn sample_request() -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "test-model".to_string(),
            messages: vec![ChatCompletionRequestMessage {
                role: ChatRole::User,
                content: Some(MessageContent::Text("hello".to_string())),
                name: None,
                tool_calls: None,
                function_call: None,
            }],
            stream: false,
            max_tokens: None,
            temperature: None,
            top_p: None,
            frequency_penalty: None,
            presence_penalty: None,
            response_format: None,
            metadata: None,
            seed: None,
            tools: None,
            tool_choice: None,
            modalities: None,
            user: None,
            store: None,
        }
    }

    #[tokio::test]
    async fn stores_completion_when_requested() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sample.parquet");
        ensure_sample_dataset(&path).unwrap();
        let scripts = ConversationScripts::load(&path).unwrap();
        let service = ChatService::new(scripts, NonZeroU32::new(5).unwrap());

        let request = sample_request();
        let result = service.create_completion(request.clone()).await;
        assert!(!result.tokens.is_empty());
        let stored = service.list(SortOrder::Ascending, None, 10).await;
        assert!(stored.is_empty());

        let mut second = request.clone();
        second.store = Some(true);
        let stored_result = service.create_completion(second).await;
        let stored = service.list(SortOrder::Ascending, None, 10).await;
        assert_eq!(stored.len(), 1);
        let retrieved = service.get(&stored_result.response.id).await.unwrap();
        assert_eq!(retrieved.completion.id, stored_result.response.id);
    }
}
