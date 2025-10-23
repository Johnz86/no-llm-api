use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::borrow::Cow;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<Value>),
}

impl MessageContent {
    pub fn render(&self) -> Cow<'_, str> {
        match self {
            MessageContent::Text(text) => Cow::Borrowed(text),
            MessageContent::Parts(parts) => Cow::Owned(render_parts(parts)),
        }
    }
}

fn render_parts(parts: &[Value]) -> String {
    let mut acc = Vec::new();
    for part in parts {
        if let Some(text) = part.get("text").and_then(|value| value.as_str()) {
            acc.push(text.to_owned());
            continue;
        }
        if let Some(text) = part.get("content").and_then(|value| value.as_str()) {
            acc.push(text.to_owned());
            continue;
        }
        acc.push(part.to_string());
    }
    acc.join(" ")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequestMessage {
    pub role: ChatRole,
    #[serde(default)]
    pub content: Option<MessageContent>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<Value>>,
    #[serde(default)]
    pub function_call: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatCompletionRequestMessage>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub frequency_penalty: Option<f32>,
    #[serde(default)]
    pub presence_penalty: Option<f32>,
    #[serde(default)]
    pub response_format: Option<Value>,
    #[serde(default)]
    pub metadata: Option<Map<String, Value>>,
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default)]
    pub tools: Option<Vec<Value>>,
    #[serde(default)]
    pub tool_choice: Option<Value>,
    #[serde(default)]
    pub modalities: Option<Vec<String>>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub store: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponseMessage {
    pub role: ChatRole,
    pub content: Option<MessageContent>,
    #[serde(default)]
    pub refusal: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<Value>>,
    #[serde(default)]
    pub function_call: Option<Value>,
    #[serde(default)]
    pub audio: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionChoice {
    pub index: usize,
    pub message: ChatCompletionResponseMessage,
    pub finish_reason: String,
    #[serde(default)]
    pub logprobs: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub usage: ChatCompletionUsage,
    pub choices: Vec<ChatCompletionChoice>,
    #[serde(default)]
    pub metadata: Option<Map<String, Value>>,
    #[serde(default)]
    pub system_fingerprint: Option<String>,
    #[serde(default)]
    pub service_tier: Option<String>,
    #[serde(default)]
    pub tool_choice: Option<Value>,
    #[serde(default)]
    pub response_format: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionList {
    pub object: String,
    pub data: Vec<ChatCompletionResponse>,
    pub first_id: Option<String>,
    pub last_id: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionMessageList {
    pub object: String,
    pub data: Vec<StoredMessage>,
    pub first_id: Option<String>,
    pub last_id: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: String,
    pub role: ChatRole,
    pub content: Option<MessageContent>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionDeleted {
    pub object: String,
    pub id: String,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<ChatCompletionChunkChoice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionChunkChoice {
    pub index: usize,
    pub delta: ChatCompletionChunkDelta,
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub logprobs: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionChunkDelta {
    #[serde(default)]
    pub role: Option<ChatRole>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub function_call: Option<Value>,
    #[serde(default)]
    pub tool_calls: Option<Vec<Value>>,
}
