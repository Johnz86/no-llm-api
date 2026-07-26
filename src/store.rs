use std::str::FromStr;
use std::sync::Arc;

use indexmap::IndexMap;
use serde::Serialize;
use serde_json::{Map, Value};
use tokio::sync::RwLock;

use crate::model::{ChatCompletionDeleted, ChatCompletionResponse, StoredMessage};
use crate::responses::{ResponseDeleted, ResponseObject};
use crate::sim::canonical::CanonicalTurn;

#[derive(Debug, Clone, Default)]
pub struct ListFilters {
    pub model: Option<String>,
    pub metadata: Vec<(String, String)>,
}

#[derive(Clone, Default)]
pub struct CompletionStore {
    inner: Arc<RwLock<IndexMap<String, StoredCompletion>>>,
}

#[derive(Clone, Default)]
pub struct ResponseStore {
    inner: Arc<RwLock<IndexMap<String, StoredResponse>>>,
}

#[derive(Clone)]
pub struct StoredResponse {
    pub response: ResponseObject,
    pub turns: Vec<CanonicalTurn>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ResponseStateView {
    pub id: String,
    pub model: String,
    pub status: crate::responses::ResponseStatus,
    pub previous_response_id: Option<String>,
    pub conversation_id: Option<String>,
    pub output_items: usize,
    pub canonical_turns: usize,
}

impl ResponseStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn save(&self, response: ResponseObject, turns: Vec<CanonicalTurn>) {
        let mut guard = self.inner.write().await;
        guard
            .entry(response.id.clone())
            .or_insert(StoredResponse { response, turns });
    }

    pub async fn get(&self, id: &str) -> Option<ResponseObject> {
        self.inner
            .read()
            .await
            .get(id)
            .map(|stored| stored.response.clone())
    }

    pub async fn get_stored(&self, id: &str) -> Option<StoredResponse> {
        self.inner.read().await.get(id).cloned()
    }

    pub async fn delete(&self, id: &str) -> Option<ResponseDeleted> {
        self.inner
            .write()
            .await
            .shift_remove(id)
            .map(|stored| ResponseDeleted {
                id: stored.response.id,
                object: "response",
                deleted: true,
            })
    }

    pub async fn snapshot(&self) -> Vec<ResponseStateView> {
        let guard = self.inner.read().await;
        let mut values: Vec<_> = guard
            .values()
            .map(|stored| ResponseStateView {
                id: stored.response.id.clone(),
                model: stored.response.model.clone(),
                status: stored.response.status,
                previous_response_id: stored.response.previous_response_id.clone(),
                conversation_id: stored
                    .response
                    .conversation
                    .as_ref()
                    .map(|conversation| conversation.id.clone()),
                output_items: stored.response.output.len(),
                canonical_turns: stored.turns.len(),
            })
            .collect();
        values.sort_by(|left, right| left.id.cmp(&right.id));
        values
    }

    pub async fn clear(&self) {
        self.inner.write().await.clear();
    }
}

impl CompletionStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(IndexMap::new())),
        }
    }

    pub async fn save(&self, completion: ChatCompletionResponse, messages: Vec<StoredMessage>) {
        let key = completion.id.clone();
        let stored = StoredCompletion {
            completion,
            messages,
        };
        let mut guard = self.inner.write().await;
        guard.insert(key, stored);
    }

    pub async fn list(
        &self,
        order: SortOrder,
        after: Option<&str>,
        limit: usize,
        filters: &ListFilters,
    ) -> Vec<StoredCompletion> {
        let guard = self.inner.read().await;
        let mut items: Vec<StoredCompletion> = match order {
            SortOrder::Ascending => guard.values().cloned().collect(),
            SortOrder::Descending => guard.values().rev().cloned().collect(),
        };

        if let Some(model) = filters.model.as_ref() {
            items.retain(|item| item.completion.model == *model);
        }

        if !filters.metadata.is_empty() {
            items.retain(|item| {
                metadata_matches(item.completion.metadata.as_ref(), &filters.metadata)
            });
        }

        if let Some(after_id) = after
            && let Some(idx) = items.iter().position(|item| item.completion.id == after_id)
        {
            items.drain(0..=idx);
        }

        if limit == 0 || items.len() <= limit {
            return items;
        }

        items.truncate(limit);
        items
    }

    pub async fn get(&self, id: &str) -> Option<StoredCompletion> {
        let guard = self.inner.read().await;
        guard.get(id).cloned()
    }

    pub async fn update_metadata(
        &self,
        id: &str,
        metadata: Map<String, Value>,
    ) -> Option<StoredCompletion> {
        let mut guard = self.inner.write().await;
        let entry = guard.get_mut(id)?;
        entry.completion.metadata = Some(metadata);
        Some(entry.clone())
    }

    pub async fn delete(&self, id: &str) -> Option<ChatCompletionDeleted> {
        let mut guard = self.inner.write().await;
        guard.shift_remove(id).map(|_| ChatCompletionDeleted {
            object: "chat.completion.deleted".to_string(),
            id: id.to_string(),
            deleted: true,
        })
    }

    pub async fn messages(
        &self,
        id: &str,
        order: SortOrder,
        after: Option<&str>,
        limit: usize,
    ) -> Option<Vec<StoredMessage>> {
        let guard = self.inner.read().await;
        let entry = guard.get(id)?;
        let mut messages: Vec<StoredMessage> = match order {
            SortOrder::Ascending => entry.messages.clone(),
            SortOrder::Descending => entry.messages.iter().cloned().rev().collect(),
        };

        if let Some(after_id) = after
            && let Some(idx) = messages.iter().position(|message| message.id == after_id)
        {
            messages.drain(0..=idx);
        }

        if limit == 0 || messages.len() <= limit {
            return Some(messages);
        }

        messages.truncate(limit);
        Some(messages)
    }
}

fn metadata_matches(metadata: Option<&Map<String, Value>>, filters: &[(String, String)]) -> bool {
    let Some(map) = metadata else {
        return false;
    };

    filters.iter().all(|(key, expected)| match map.get(key) {
        Some(value) => metadata_value_eq(value, expected),
        None => false,
    })
}

fn metadata_value_eq(value: &Value, expected: &str) -> bool {
    match value {
        Value::String(actual) => actual == expected,
        Value::Number(number) => number.to_string() == expected,
        Value::Bool(flag) => {
            if expected.eq_ignore_ascii_case("true") {
                *flag
            } else if expected.eq_ignore_ascii_case("false") {
                !flag
            } else {
                false
            }
        }
        Value::Null => expected.eq_ignore_ascii_case("null"),
        Value::Array(_) | Value::Object(_) => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    Ascending,
    Descending,
}

impl FromStr for SortOrder {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "asc" | "ASC" => Ok(SortOrder::Ascending),
            "desc" | "DESC" => Ok(SortOrder::Descending),
            _ => Err(()),
        }
    }
}

#[derive(Clone)]
pub struct StoredCompletion {
    pub completion: ChatCompletionResponse,
    pub messages: Vec<StoredMessage>,
}

impl StoredCompletion {
    /// The enriched object the list and retrieve routes return.
    pub fn to_list_item(&self) -> crate::model::StoredChatCompletionView {
        self.completion.stored_view()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChatRole, FinishReason, MessageContent};
    use serde_json::json;

    fn sample_response(id: &str, created: i64) -> ChatCompletionResponse {
        ChatCompletionResponse {
            id: id.to_string(),
            object: "chat.completion".to_string(),
            created,
            model: "test-model".to_string(),
            usage: crate::model::ChatCompletionUsage {
                prompt_tokens: 2,
                completion_tokens: 3,
                total_tokens: 5,
                prompt_tokens_details: None,
                completion_tokens_details: None,
            },
            choices: vec![crate::model::ChatCompletionChoice {
                index: 0,
                message: crate::model::ChatCompletionResponseMessage {
                    role: ChatRole::Assistant,
                    content: Some(MessageContent::Text("ok".to_string())),
                    refusal: None,
                    reasoning_content: None,
                    tool_calls: None,
                    function_call: None,
                    audio: None,
                },
                finish_reason: Some(FinishReason::Stop),
                logprobs: None,
            }],
            metadata: None,
            system_fingerprint: Some("fp_test".to_string()),
            service_tier: Some(crate::request_types::ServiceTier::Default),
            request_id: Some(format!("req_{id}")),
            temperature: Some(1.0),
            top_p: Some(1.0),
            frequency_penalty: Some(0.0),
            presence_penalty: Some(0.0),
            stop: None,
            seed: None,
            tool_choice: None,
            response_format: None,
            parallel_tool_calls: None,
            modalities: None,
            response_prefix: None,
            logit_bias: None,
            stream_options: None,
            audio: None,
            tools: None,
            input_user: None,
        }
    }

    fn sample_messages(root: &str) -> Vec<StoredMessage> {
        vec![
            StoredMessage {
                id: format!("{root}-0"),
                role: ChatRole::User,
                content: Some(MessageContent::Text("hi".to_string())),
                content_parts: None,
                name: None,
                refusal: None,
                tool_calls: None,
                function_call: None,
                audio: None,
            },
            StoredMessage {
                id: format!("{root}-1"),
                role: ChatRole::Assistant,
                content: Some(MessageContent::Text("ok".to_string())),
                content_parts: None,
                name: None,
                refusal: None,
                tool_calls: None,
                function_call: None,
                audio: None,
            },
        ]
    }

    #[tokio::test]
    async fn responses_are_immutable_and_delete_is_explicit() {
        let store = ResponseStore::new();
        let mut first = sample_response_object("resp_test");
        store.save(first.clone(), Vec::new()).await;
        first.model = "changed-model".to_string();
        store.save(first, Vec::new()).await;

        assert_eq!(store.get("resp_test").await.unwrap().model, "test-model");
        assert_eq!(
            serde_json::to_value(store.delete("resp_test").await.unwrap()).unwrap(),
            json!({"id": "resp_test", "object": "response", "deleted": true})
        );
        assert!(store.get("resp_test").await.is_none());
        assert!(store.delete("resp_test").await.is_none());
    }

    fn sample_response_object(id: &str) -> ResponseObject {
        serde_json::from_value::<crate::responses::CreateResponseRequest>(json!({
            "model": "test-model",
            "input": "hello"
        }))
        .map(|request| {
            let plan = crate::sim::plan::SemanticResponsePlan {
                version: "test".to_string(),
                fixture_id: "test".to_string(),
                case_id: "test".to_string(),
                variant_id: "test".to_string(),
                interface: crate::sim::script::Interface::Responses,
                model: "test-model".to_string(),
                output: vec![crate::sim::plan::SemanticOutput::Text {
                    text: "ok".to_string(),
                }],
                terminal: crate::sim::script::TerminalStatus::Completed,
                usage: crate::sim::plan::SemanticUsage::default(),
                plan_digest: "0000000000000001".to_string(),
                explanation: crate::sim::plan::PlanExplanation {
                    match_kind: crate::sim::plan::SemanticMatchKind::Exact,
                    fixture_id: "test".to_string(),
                    case_id: "test".to_string(),
                    variant_id: "test".to_string(),
                    variant_reason: crate::sim::plan::VariantReason::Default,
                    canonical_request_digest: "test".to_string(),
                    turn_shapes: Vec::new(),
                    effective_controls: crate::sim::plan::EffectiveControls {
                        dataset_revision: "test".to_string(),
                        scenario_revision: "test".to_string(),
                        model_profile_revision: "test".to_string(),
                        simulation_seed: None,
                    },
                },
            };
            crate::responses::render_response(&request, &plan, &tiktoken_rs::cl100k_base().unwrap())
        })
        .map(|mut response| {
            response.id = id.to_string();
            response
        })
        .unwrap()
    }

    #[tokio::test]
    async fn list_respects_order_and_pagination() {
        let store = CompletionStore::new();
        store
            .save(sample_response("a", 1), sample_messages("a"))
            .await;
        let mut with_meta = sample_response("b", 2);
        let mut metadata = Map::new();
        metadata.insert("tag".to_string(), json!("beta"));
        with_meta.metadata = Some(metadata);
        store.save(with_meta, sample_messages("b")).await;
        store
            .save(sample_response("c", 3), sample_messages("c"))
            .await;

        let asc = store
            .list(SortOrder::Ascending, None, 2, &ListFilters::default())
            .await
            .into_iter()
            .map(|item| item.completion.id)
            .collect::<Vec<_>>();
        assert_eq!(asc, vec!["a", "b"]);

        let desc = store
            .list(SortOrder::Descending, None, 2, &ListFilters::default())
            .await
            .into_iter()
            .map(|item| item.completion.id)
            .collect::<Vec<_>>();
        assert_eq!(desc, vec!["c", "b"]);

        let after = store
            .list(SortOrder::Ascending, Some("a"), 2, &ListFilters::default())
            .await
            .into_iter()
            .map(|item| item.completion.id)
            .collect::<Vec<_>>();
        assert_eq!(after, vec!["b", "c"]);

        let mut filters = ListFilters::default();
        filters
            .metadata
            .push(("tag".to_string(), "beta".to_string()));
        let filtered = store
            .list(SortOrder::Ascending, None, 10, &filters)
            .await
            .into_iter()
            .map(|item| item.completion.id)
            .collect::<Vec<_>>();
        assert_eq!(filtered, vec!["b"]);
    }

    #[tokio::test]
    async fn messages_follow_ordering() {
        let store = CompletionStore::new();
        store
            .save(sample_response("chat", 1), sample_messages("chat"))
            .await;

        let asc = store
            .messages("chat", SortOrder::Ascending, None, 10)
            .await
            .unwrap()
            .into_iter()
            .map(|message| message.id)
            .collect::<Vec<_>>();
        assert_eq!(asc, vec!["chat-0", "chat-1"]);

        let desc = store
            .messages("chat", SortOrder::Descending, None, 10)
            .await
            .unwrap()
            .into_iter()
            .map(|message| message.id)
            .collect::<Vec<_>>();
        assert_eq!(desc, vec!["chat-1", "chat-0"]);

        let after = store
            .messages("chat", SortOrder::Ascending, Some("chat-0"), 10)
            .await
            .unwrap()
            .into_iter()
            .map(|message| message.id)
            .collect::<Vec<_>>();
        assert_eq!(after, vec!["chat-1"]);
    }

    #[tokio::test]
    async fn update_metadata_overwrites() {
        let store = CompletionStore::new();
        store
            .save(sample_response("meta", 42), sample_messages("meta"))
            .await;

        let mut metadata = Map::new();
        metadata.insert("key".to_string(), json!(1));
        store.update_metadata("meta", metadata).await.unwrap();

        let updated = store.get("meta").await.unwrap();
        assert_eq!(
            updated
                .completion
                .metadata
                .as_ref()
                .and_then(|map| map.get("key"))
                .and_then(|value| value.as_i64()),
            Some(1)
        );
    }
}
