//! Deterministic process-local conversation resources and item ownership.

use std::collections::BTreeMap;
use std::sync::Arc;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::RwLock;

use crate::responses::{
    CreateResponseRequest, ResponseInput, ResponseInputItem, ResponseObject,
    ResponseOutputItemKind, canonical_input_item,
};
use crate::sim::canonical::{CanonicalTurn, canonical_json};
use crate::sim::digest::digest_fields;
use crate::sim::identity::{Identity, IdentityMode, SystemClock};
use crate::store::SortOrder;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateConversationRequest {
    #[serde(default)]
    pub metadata: Map<String, Value>,
    #[serde(default)]
    pub items: Vec<ResponseInputItem>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateConversationItemsRequest {
    pub items: Vec<ResponseInputItem>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ConversationResource {
    pub id: String,
    pub object: &'static str,
    pub created_at: i64,
    pub metadata: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConversationDeleted {
    pub id: String,
    pub object: &'static str,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ConversationItemList {
    pub object: &'static str,
    pub data: Vec<Value>,
    pub first_id: Option<String>,
    pub last_id: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Clone)]
pub struct ConversationSnapshot {
    pub resource: ConversationResource,
    pub turns: Vec<CanonicalTurn>,
    pub item_ids: Vec<String>,
    pub reasoning_tokens: u32,
    pub next_generation: u32,
}

#[derive(Clone, Default)]
pub struct ConversationStore {
    inner: Arc<RwLock<IndexMap<String, ConversationRecord>>>,
}

#[derive(Clone)]
struct ConversationRecord {
    resource: ConversationResource,
    items: BTreeMap<String, StoredConversationItem>,
}

#[derive(Clone)]
struct StoredConversationItem {
    generation: u32,
    group: String,
    ordinal: usize,
    value: Value,
    turn: Option<CanonicalTurn>,
    reasoning_tokens: u32,
}

impl ConversationStore {
    pub async fn create(&self, request: &CreateConversationRequest) -> ConversationResource {
        let source = canonical_json(&json!({
            "metadata": request.metadata,
            "items": request.items,
        }));
        let digest = digest_fields(["conversation-v1", source.as_str()]);
        let identity = Identity::derive(digest, IdentityMode::Derived, &SystemClock);
        let resource = ConversationResource {
            id: format!("conv_{digest:016x}"),
            object: "conversation",
            created_at: identity.created,
            metadata: request.metadata.clone(),
        };
        let items = request
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let id = input_item_id("initial", &resource.id, index, item);
                (
                    id.clone(),
                    StoredConversationItem {
                        generation: 0,
                        group: "initial".to_string(),
                        ordinal: index,
                        value: input_item_value(item, &id),
                        turn: Some(canonical_input_item(item)),
                        reasoning_tokens: 0,
                    },
                )
            })
            .collect();
        let mut guard = self.inner.write().await;
        guard
            .entry(resource.id.clone())
            .or_insert_with(|| ConversationRecord {
                resource: resource.clone(),
                items,
            });
        resource
    }

    pub async fn get(&self, id: &str) -> Option<ConversationResource> {
        self.inner
            .read()
            .await
            .get(id)
            .map(|record| record.resource.clone())
    }

    pub async fn snapshot(&self, id: &str) -> Option<ConversationSnapshot> {
        let guard = self.inner.read().await;
        let record = guard.get(id)?;
        let ordered = ordered_items(record);
        Some(ConversationSnapshot {
            resource: record.resource.clone(),
            turns: ordered
                .iter()
                .filter_map(|(_, item)| item.turn.clone())
                .collect(),
            item_ids: ordered.iter().map(|(id, _)| (*id).clone()).collect(),
            reasoning_tokens: ordered.iter().map(|(_, item)| item.reasoning_tokens).sum(),
            next_generation: next_generation(record),
        })
    }

    pub async fn append_response(
        &self,
        id: &str,
        generation: u32,
        request: &CreateResponseRequest,
        response: &ResponseObject,
        output_turn: CanonicalTurn,
    ) -> bool {
        let mut guard = self.inner.write().await;
        let Some(record) = guard.get_mut(id) else {
            return false;
        };
        if generation != next_generation(record) {
            return false;
        }
        let inputs = response_input_items(request);
        for (index, (item, turn)) in inputs.into_iter().enumerate() {
            let item_id = input_item_id(&response.id, id, index, &item);
            record
                .items
                .entry(item_id.clone())
                .or_insert_with(|| StoredConversationItem {
                    generation,
                    group: response.id.clone(),
                    ordinal: index,
                    value: input_item_value(&item, &item_id),
                    turn: Some(turn),
                    reasoning_tokens: 0,
                });
        }
        let offset = request.canonical_turns().len();
        for (index, output) in response.output.iter().enumerate() {
            let value = serde_json::to_value(output).expect("Response output items serialize");
            let item_id = output.id().to_string();
            let kind = output.kind();
            record
                .items
                .entry(item_id)
                .or_insert(StoredConversationItem {
                    generation,
                    group: response.id.clone(),
                    ordinal: offset + index,
                    value,
                    turn: (kind == ResponseOutputItemKind::Message).then(|| output_turn.clone()),
                    reasoning_tokens: if kind == ResponseOutputItemKind::Reasoning {
                        response.usage.output_tokens_details.reasoning_tokens
                    } else {
                        0
                    },
                });
        }
        true
    }

    pub async fn add_items(
        &self,
        id: &str,
        request: &CreateConversationItemsRequest,
    ) -> Option<ConversationItemList> {
        let mut guard = self.inner.write().await;
        let record = guard.get_mut(id)?;
        let batch = canonical_json(&serde_json::to_value(&request.items).ok()?);
        let generation = next_generation(record);
        let group = format!("batch_{:016x}", digest_fields([id, batch.as_str()]));
        let mut ids = Vec::new();
        for (index, item) in request.items.iter().enumerate() {
            let digest = digest_fields([
                "conversation-added-item-v1",
                id,
                batch.as_str(),
                index.to_string().as_str(),
            ]);
            let item_id = format!("msg_{digest:016x}");
            record
                .items
                .entry(item_id.clone())
                .or_insert_with(|| StoredConversationItem {
                    generation,
                    group: group.clone(),
                    ordinal: index,
                    value: input_item_value(item, &item_id),
                    turn: Some(canonical_input_item(item)),
                    reasoning_tokens: 0,
                });
            ids.push(item_id);
        }
        let data = ids
            .iter()
            .filter_map(|item_id| record.items.get(item_id).map(|item| item.value.clone()))
            .collect();
        Some(item_list(data, false))
    }

    pub async fn list_items(
        &self,
        id: &str,
        order: SortOrder,
        after: Option<&str>,
        limit: usize,
    ) -> Option<ConversationItemList> {
        let guard = self.inner.read().await;
        let record = guard.get(id)?;
        let mut ordered = ordered_items(record);
        if order == SortOrder::Descending {
            ordered.reverse();
        }
        if let Some(after) = after
            && let Some(index) = ordered
                .iter()
                .position(|(item_id, _)| item_id.as_str() == after)
        {
            ordered.drain(0..=index);
        }
        let has_more = ordered.len() > limit;
        ordered.truncate(limit);
        Some(item_list(
            ordered
                .into_iter()
                .map(|(_, item)| item.value.clone())
                .collect(),
            has_more,
        ))
    }

    pub async fn get_item(&self, id: &str, item_id: &str) -> Option<Value> {
        self.inner
            .read()
            .await
            .get(id)?
            .items
            .get(item_id)
            .map(|item| item.value.clone())
    }

    pub async fn delete_item(&self, id: &str, item_id: &str) -> Option<ConversationResource> {
        let mut guard = self.inner.write().await;
        let record = guard.get_mut(id)?;
        record.items.remove(item_id)?;
        Some(record.resource.clone())
    }

    pub async fn delete(&self, id: &str) -> Option<ConversationDeleted> {
        self.inner
            .write()
            .await
            .shift_remove(id)
            .map(|record| ConversationDeleted {
                id: record.resource.id,
                object: "conversation.deleted",
                deleted: true,
            })
    }

    pub async fn clear(&self) {
        self.inner.write().await.clear();
    }
}

fn ordered_items(record: &ConversationRecord) -> Vec<(&String, &StoredConversationItem)> {
    let mut items: Vec<_> = record.items.iter().collect();
    items.sort_by(|(left_id, left), (right_id, right)| {
        (left.generation, &left.group, left.ordinal, *left_id).cmp(&(
            right.generation,
            &right.group,
            right.ordinal,
            *right_id,
        ))
    });
    items
}

fn next_generation(record: &ConversationRecord) -> u32 {
    record
        .items
        .values()
        .map(|item| item.generation)
        .max()
        .map_or(0, |generation| generation.saturating_add(1))
}

fn input_item_id(
    domain: &str,
    conversation_id: &str,
    index: usize,
    item: &ResponseInputItem,
) -> String {
    let value = serde_json::to_value(item).expect("typed input items serialize");
    let index = index.to_string();
    let digest = digest_fields([
        "conversation-input-item-v1",
        domain,
        conversation_id,
        index.as_str(),
        canonical_json(&value).as_str(),
    ]);
    format!("msg_{digest:016x}")
}

fn input_item_value(item: &ResponseInputItem, id: &str) -> Value {
    let mut value = serde_json::to_value(item).expect("typed input items serialize");
    value["id"] = json!(id);
    value["status"] = json!("completed");
    if let Some(text) = value["content"].as_str().map(str::to_string) {
        value["content"] = json!([{"type": "input_text", "text": text}]);
    }
    value
}

fn response_input_items(
    request: &CreateResponseRequest,
) -> Vec<(ResponseInputItem, CanonicalTurn)> {
    match &request.input {
        ResponseInput::Text(text) => {
            let item: ResponseInputItem = serde_json::from_value(json!({
                "type": "message",
                "role": "user",
                "content": text,
            }))
            .expect("text input maps to a typed message");
            vec![(item.clone(), canonical_input_item(&item))]
        }
        ResponseInput::Items(items) => items
            .iter()
            .cloned()
            .map(|item| {
                let turn = canonical_input_item(&item);
                (item, turn)
            })
            .collect(),
    }
}

fn item_list(data: Vec<Value>, has_more: bool) -> ConversationItemList {
    ConversationItemList {
        object: "list",
        first_id: data
            .first()
            .and_then(|item| item["id"].as_str().map(str::to_string)),
        last_id: data
            .last()
            .and_then(|item| item["id"].as_str().map(str::to_string)),
        data,
        has_more,
    }
}
