//! Deterministic process-local conversation resources and turn ownership.

use std::collections::BTreeMap;
use std::sync::Arc;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::RwLock;

use crate::responses::ResponseInputItem;
use crate::sim::canonical::{CanonicalTurn, canonical_json};
use crate::sim::digest::digest_fields;
use crate::sim::identity::{Identity, IdentityMode, SystemClock};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateConversationRequest {
    #[serde(default)]
    pub metadata: Map<String, Value>,
    #[serde(default)]
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

#[derive(Debug, Clone)]
pub struct ConversationSnapshot {
    pub resource: ConversationResource,
    pub turns: Vec<CanonicalTurn>,
    pub response_ids: Vec<String>,
    pub reasoning_tokens: u32,
}

#[derive(Clone, Default)]
pub struct ConversationStore {
    inner: Arc<RwLock<IndexMap<String, ConversationRecord>>>,
}

#[derive(Clone)]
struct ConversationRecord {
    resource: ConversationResource,
    initial_turns: Vec<CanonicalTurn>,
    exchanges: BTreeMap<String, ConversationExchange>,
}

#[derive(Clone)]
struct ConversationExchange {
    turns: Vec<CanonicalTurn>,
    reasoning_tokens: u32,
}

impl ConversationStore {
    pub async fn create(
        &self,
        request: &CreateConversationRequest,
        initial_turns: Vec<CanonicalTurn>,
    ) -> ConversationResource {
        let value = serde_json::json!({
            "metadata": request.metadata,
            "turns": initial_turns,
        });
        let digest = digest_fields(["conversation-v1", canonical_json(&value).as_str()]);
        let identity = Identity::derive(digest, IdentityMode::Derived, &SystemClock);
        let resource = ConversationResource {
            id: format!("conv_{digest:016x}"),
            object: "conversation",
            created_at: identity.created,
            metadata: request.metadata.clone(),
        };
        let mut guard = self.inner.write().await;
        guard
            .entry(resource.id.clone())
            .or_insert_with(|| ConversationRecord {
                resource: resource.clone(),
                initial_turns,
                exchanges: BTreeMap::new(),
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
        let mut turns = record.initial_turns.clone();
        for exchange in record.exchanges.values() {
            turns.extend(exchange.turns.clone());
        }
        Some(ConversationSnapshot {
            resource: record.resource.clone(),
            turns,
            response_ids: record.exchanges.keys().cloned().collect(),
            reasoning_tokens: record
                .exchanges
                .values()
                .map(|exchange| exchange.reasoning_tokens)
                .sum(),
        })
    }

    pub async fn append(
        &self,
        id: &str,
        response_id: String,
        turns: Vec<CanonicalTurn>,
        reasoning_tokens: u32,
    ) -> bool {
        let mut guard = self.inner.write().await;
        let Some(record) = guard.get_mut(id) else {
            return false;
        };
        record
            .exchanges
            .entry(response_id)
            .or_insert(ConversationExchange {
                turns,
                reasoning_tokens,
            });
        true
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
