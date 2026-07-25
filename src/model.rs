use serde::{Deserialize, Serialize};

use crate::request_types::{
    AudioConfig, LogitBias, Modality, ReasoningEffort, ResponseFormat, ServiceTier,
    StopConfiguration, Tool, ToolChoice,
};
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

    /// The parts array, when the content was supplied in structured form.
    pub fn parts(&self) -> Option<&[Value]> {
        match self {
            MessageContent::Text(_) => None,
            MessageContent::Parts(parts) => Some(parts),
        }
    }
}

/// A response `message.content` is a string in the spec (`openapi.yaml:31208`,
/// schema `ChatCompletionResponseMessage`), never an array of parts. Requests may
/// use parts; responses must flatten them or clients reject the payload.
fn serialize_content_as_string<S>(
    value: &Option<MessageContent>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match value {
        Some(content) => serializer.serialize_some(content.render().as_ref()),
        None => serializer.serialize_none(),
    }
}

/// Accepts an explicit `null` where a plain `bool` is expected.
///
/// Several GUIs send `"stream": null` for a non-streamed call.
fn null_as_false<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<bool>::deserialize(deserializer)?.unwrap_or(false))
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
    /// Required when `role` is `tool`; validation enforces that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
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
    #[serde(default, deserialize_with = "null_as_false")]
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
    pub stop: Option<StopConfiguration>,
    #[serde(default)]
    pub max_completion_tokens: Option<u32>,
    #[serde(default)]
    pub reasoning_effort: Option<ReasoningEffort>,
    #[serde(default)]
    pub service_tier: Option<ServiceTier>,
    #[serde(default)]
    pub response_format: Option<ResponseFormat>,
    #[serde(default)]
    pub response_prefix: Option<String>,
    #[serde(default)]
    pub logit_bias: Option<LogitBias>,
    #[serde(default)]
    pub metadata: Option<Map<String, Value>>,
    #[serde(default)]
    pub seed: Option<i64>,
    #[serde(default)]
    pub tools: Option<Vec<Tool>>,
    #[serde(default)]
    pub tool_choice: Option<ToolChoice>,
    #[serde(default)]
    pub parallel_tool_calls: Option<bool>,
    #[serde(default)]
    pub modalities: Option<Vec<Modality>>,
    #[serde(default)]
    pub stream_options: Option<ChatCompletionStreamOptions>,
    #[serde(default)]
    pub function_call: Option<Value>,
    #[serde(default)]
    pub audio: Option<AudioConfig>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub store: Option<bool>,
    /// Per-request simulation directive; never echoed back.
    #[serde(default, skip_serializing)]
    pub x_simulate: Option<Value>,
    /// Accepted and ignored: present in the spec, but they do not change what a
    /// fixture server can answer. Rejecting them would break clients that always
    /// send them, and echoing them would put non-spec keys in the response.
    #[serde(default, skip_serializing)]
    pub logprobs: Option<bool>,
    #[serde(default, skip_serializing)]
    pub top_logprobs: Option<u32>,
    #[serde(default, skip_serializing)]
    pub prediction: Option<Value>,
    #[serde(default, skip_serializing)]
    pub web_search_options: Option<Value>,
    #[serde(default, skip_serializing)]
    pub verbosity: Option<String>,
    #[serde(default, skip_serializing)]
    pub prompt_cache_key: Option<String>,
    #[serde(default, skip_serializing)]
    pub safety_identifier: Option<String>,
    #[serde(default, skip_serializing)]
    pub functions: Option<Vec<Value>>,
    #[serde(default, skip_serializing)]
    pub include_obfuscation: Option<bool>,
    #[serde(default, skip_serializing)]
    pub n: Option<u32>,
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
    #[serde(serialize_with = "serialize_content_as_string")]
    pub content: Option<MessageContent>,
    #[serde(default)]
    pub refusal: Option<String>,
    /// Not in the spec, but what reasoning-model GUIs render. Emitted before
    /// `content` in a stream, and omitted entirely when there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ChatCompletionMessageToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function_call: Option<FunctionCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<ChatCompletionResponseMessageAudio>,
}

/// Splits a `<think>...</think>` prefix into reasoning and visible content.
///
/// This is the "tag" thinking style: fixtures stay a single text column, and the
/// wire shape still separates the two the way a reasoning model does.
pub fn split_reasoning(text: &str) -> (Option<String>, String) {
    const OPEN: &str = "<think>";
    const CLOSE: &str = "</think>";
    let trimmed = text.trim_start();
    if !trimmed.starts_with(OPEN) {
        return (None, text.to_string());
    }
    match trimmed.find(CLOSE) {
        Some(end) => {
            let reasoning = trimmed[OPEN.len()..end].trim().to_string();
            let rest = trimmed[end + CLOSE.len()..].trim_start().to_string();
            (Some(reasoning).filter(|value| !value.is_empty()), rest)
        }
        None => (None, text.to_string()),
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionChoice {
    pub index: usize,
    pub message: ChatCompletionResponseMessage,
    #[serde(default)]
    pub finish_reason: Option<FinishReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<ServiceTier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<StopConfiguration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modalities: Option<Vec<Modality>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logit_bias: Option<LogitBias>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<ChatCompletionStreamOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioConfig>,
    /// Stored-object members from the spec's list example; not part of the lean
    /// `POST` response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_user: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionList {
    pub object: String,
    pub data: Vec<StoredChatCompletionView>,
    pub first_id: Option<String>,
    pub last_id: Option<String>,
    pub has_more: bool,
}

/// The lean object `POST /chat/completions` returns: exactly the members of
/// `CreateChatCompletionResponse` (`openapi.yaml:33058`). Request echoes belong
/// to the stored object, not to the completion that was just created.
#[derive(Debug, Clone, Serialize)]
pub struct LeanChatCompletionView {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<ChatCompletionChoice>,
    pub usage: ChatCompletionUsage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<ServiceTier>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_fingerprint: Option<String>,
}

/// The enriched object `GET`/`list` return, mirroring the spec's own
/// `listChatCompletions` example (`openapi.yaml:2061-2092`): request parameters
/// are present, with explicit nulls, because that is what clients read back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredChatCompletionView {
    pub object: String,
    pub id: String,
    pub model: String,
    pub created: i64,
    pub request_id: Option<String>,
    pub tool_choice: Option<ToolChoice>,
    pub usage: ChatCompletionUsage,
    pub seed: Option<i64>,
    pub top_p: Option<f32>,
    pub temperature: Option<f32>,
    pub presence_penalty: Option<f32>,
    pub frequency_penalty: Option<f32>,
    pub system_fingerprint: Option<String>,
    pub input_user: Option<String>,
    pub service_tier: Option<ServiceTier>,
    pub tools: Option<Vec<Tool>>,
    pub metadata: Option<Map<String, Value>>,
    pub choices: Vec<ChatCompletionChoice>,
    pub response_format: Option<ResponseFormat>,
}

impl ChatCompletionResponse {
    pub fn lean(&self) -> LeanChatCompletionView {
        LeanChatCompletionView {
            id: self.id.clone(),
            object: self.object.clone(),
            created: self.created,
            model: self.model.clone(),
            choices: self.choices.clone(),
            usage: self.usage.clone(),
            service_tier: self.service_tier,
            system_fingerprint: self.system_fingerprint.clone(),
        }
    }

    pub fn stored_view(&self) -> StoredChatCompletionView {
        StoredChatCompletionView {
            object: self.object.clone(),
            id: self.id.clone(),
            model: self.model.clone(),
            created: self.created,
            request_id: self.request_id.clone(),
            tool_choice: self.tool_choice.clone(),
            usage: self.usage.clone(),
            seed: self.seed,
            top_p: self.top_p,
            temperature: self.temperature,
            presence_penalty: self.presence_penalty,
            frequency_penalty: self.frequency_penalty,
            system_fingerprint: self.system_fingerprint.clone(),
            input_user: self.input_user.clone(),
            service_tier: self.service_tier,
            tools: self.tools.clone(),
            metadata: self.metadata.clone(),
            choices: self.choices.clone(),
            response_format: self.response_format.clone(),
        }
    }
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
    #[serde(serialize_with = "serialize_content_as_string")]
    pub content: Option<MessageContent>,
    /// Structured parts, kept out of `content` so it always reads as a string
    /// (`listChatCompletionMessages` example, `openapi.yaml:3325-3333`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_parts: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub refusal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ChatCompletionMessageToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function_call: Option<FunctionCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<ServiceTier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ChatCompletionUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionChunkChoice {
    pub index: usize,
    pub delta: ChatCompletionChunkDelta,
    pub finish_reason: Option<FinishReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<ChatChoiceLogprobs>,
}

/// A streamed delta. Absent members are omitted rather than sent as null, which
/// is what `ChatCompletionStreamResponseDelta` (`openapi.yaml:31398`) describes
/// and what keeps zod-validated clients from rejecting the frame.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatCompletionChunkDelta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<ChatRole>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function_call: Option<FunctionCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ChatCompletionMessageToolCallChunk>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionMessageToolCall {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default, rename = "type")]
    pub r#type: Option<ChatCompletionToolType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<FunctionCall>,
}

/// `ChatCompletionMessageToolCallChunk` (`openapi.yaml:30619`): `index` is the
/// only required member, `id` and `type` appear on the first fragment of a call,
/// and `arguments` arrives in fragments that are never empty strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionMessageToolCallChunk {
    pub index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub r#type: Option<ChatCompletionToolType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<FunctionCallChunk>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FunctionCallChunk {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}
