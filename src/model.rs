use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::borrow::Cow;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    System,
    Developer,
    User,
    Assistant,
    Tool,
    Function,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatCompletionToolType {
    Function,
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
    #[serde(default)]
    pub audio: Option<Value>,
    #[serde(default)]
    pub refusal: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ChatCompletionStreamOptions {
    pub include_usage: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    pub stop: Option<Value>,
    #[serde(default)]
    pub max_completion_tokens: Option<u32>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub service_tier: Option<String>,
    #[serde(default)]
    pub response_format: Option<Value>,
    #[serde(default)]
    pub response_prefix: Option<String>,
    #[serde(default)]
    pub logit_bias: Option<Map<String, Value>>,
    #[serde(default)]
    pub metadata: Option<Map<String, Value>>,
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default)]
    pub tools: Option<Vec<Value>>,
    #[serde(default)]
    pub tool_choice: Option<Value>,
    #[serde(default)]
    pub parallel_tool_calls: Option<bool>,
    #[serde(default)]
    pub modalities: Option<Vec<String>>,
    #[serde(default)]
    pub stream_options: Option<ChatCompletionStreamOptions>,
    #[serde(default)]
    pub function_call: Option<Value>,
    #[serde(default)]
    pub audio: Option<Value>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens_details: Option<CompletionTokensDetails>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PromptTokensDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CompletionTokensDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_prediction_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected_prediction_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponseMessage {
    pub role: ChatRole,
    pub content: Option<MessageContent>,
    #[serde(default)]
    pub refusal: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ChatCompletionMessageToolCall>>,
    #[serde(default)]
    pub function_call: Option<FunctionCall>,
    #[serde(default)]
    pub audio: Option<ChatCompletionResponseMessageAudio>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponseMessageAudio {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub expires_at: Option<u32>,
    #[serde(default)]
    pub data: Option<String>,
    #[serde(default)]
    pub transcript: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionChoice {
    pub index: usize,
    pub message: ChatCompletionResponseMessage,
    #[serde(default)]
    pub finish_reason: Option<FinishReason>,
    #[serde(default)]
    pub logprobs: Option<ChatChoiceLogprobs>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChoiceLogprobs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<ChatCompletionTokenLogprob>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<Vec<ChatCompletionTokenLogprob>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionTokenLogprob {
    pub token: String,
    pub logprob: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub top_logprobs: Vec<TopLogprob>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopLogprob {
    pub token: String,
    pub logprob: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<Vec<u8>>,
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
    pub request_id: Option<String>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub frequency_penalty: Option<f32>,
    #[serde(default)]
    pub presence_penalty: Option<f32>,
    #[serde(default)]
    pub stop: Option<Value>,
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default)]
    pub tool_choice: Option<Value>,
    #[serde(default)]
    pub response_format: Option<Value>,
    #[serde(default)]
    pub parallel_tool_calls: Option<bool>,
    #[serde(default)]
    pub modalities: Option<Vec<String>>,
    #[serde(default)]
    pub response_prefix: Option<String>,
    #[serde(default)]
    pub logit_bias: Option<Map<String, Value>>,
    #[serde(default)]
    pub stream_options: Option<ChatCompletionStreamOptions>,
    #[serde(default)]
    pub audio: Option<Value>,
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
    #[serde(default)]
    pub refusal: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ChatCompletionMessageToolCall>>,
    #[serde(default)]
    pub function_call: Option<FunctionCall>,
    #[serde(default)]
    pub audio: Option<ChatCompletionResponseMessageAudio>,
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
    #[serde(default)]
    pub system_fingerprint: Option<String>,
    #[serde(default)]
    pub usage: Option<ChatCompletionUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionChunkChoice {
    pub index: usize,
    pub delta: ChatCompletionChunkDelta,
    pub finish_reason: Option<FinishReason>,
    #[serde(default)]
    pub logprobs: Option<ChatChoiceLogprobs>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionChunkDelta {
    #[serde(default)]
    pub role: Option<ChatRole>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub function_call: Option<FunctionCall>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ChatCompletionMessageToolCall>>,
    #[serde(default)]
    pub refusal: Option<String>,
    #[serde(default)]
    pub audio: Option<ChatCompletionResponseMessageAudio>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionMessageToolCall {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub r#type: Option<ChatCompletionToolType>,
    #[serde(default)]
    pub function: Option<FunctionCall>,
}
