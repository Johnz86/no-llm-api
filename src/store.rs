use std::str::FromStr;
use std::sync::Arc;

use indexmap::IndexMap;
use serde_json::{Map, Value};
use tokio::sync::RwLock;

use crate::model::{ChatCompletionDeleted, ChatCompletionResponse, StoredMessage};

#[derive(Clone, Default)]
pub struct CompletionStore {
    inner: Arc<RwLock<IndexMap<String, StoredCompletion>>>,
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
    ) -> Vec<StoredCompletion> {
        let guard = self.inner.read().await;
        let mut items: Vec<StoredCompletion> = match order {
            SortOrder::Ascending => guard.values().cloned().collect(),
            SortOrder::Descending => guard.values().rev().cloned().collect(),
        };

        if let Some(after_id) = after {
            if let Some(idx) = items.iter().position(|item| item.completion.id == after_id) {
                items.drain(0..=idx);
            }
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

        if let Some(after_id) = after {
            if let Some(idx) = messages.iter().position(|message| message.id == after_id) {
                messages.drain(0..=idx);
            }
        }

        if limit == 0 || messages.len() <= limit {
            return Some(messages);
        }

        messages.truncate(limit);
        Some(messages)
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
    pub fn to_list_item(&self) -> ChatCompletionResponse {
        self.completion.clone()
    }
}
