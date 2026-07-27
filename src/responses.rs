//! Typed request and response vocabulary for the experimental Responses surface.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tiktoken_rs::CoreBPE;

use crate::request_types::ReasoningEffort;
use crate::sim::canonical::{CanonicalContent, CanonicalRequest, CanonicalTurn};
use crate::sim::digest::digest_fields;
use crate::sim::identity::{Identity, IdentityMode, SystemClock};
use crate::sim::plan::{SemanticOutput, SemanticResponsePlan};
use crate::sim::script::Interface;
use crate::sim::script::TerminalStatus;
use crate::sim::stream::token_pieces;

pub const RESPONSES_SCHEMA_REVISION: &str = "responses-2026-07-23";

#[derive(Debug, Clone, Deserialize)]
pub struct CreateResponseRequest {
    pub model: String,
    pub input: ResponseInput,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub reasoning: Option<ResponseReasoningConfig>,
    #[serde(default, deserialize_with = "include_or_default")]
    pub include: Vec<ResponseInclude>,
    #[serde(default)]
    pub text: Option<ResponseTextConfig>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default = "default_true", deserialize_with = "bool_or_default_true")]
    pub store: bool,
    #[serde(default, deserialize_with = "metadata_or_default")]
    pub metadata: BTreeMap<String, String>,
    #[serde(default)]
    pub previous_response_id: Option<String>,
    #[serde(default)]
    pub conversation: Option<ResponseConversationParam>,
    #[serde(default = "default_true", deserialize_with = "bool_or_default_true")]
    pub parallel_tool_calls: bool,
    #[serde(default)]
    pub tools: Vec<Value>,
    #[serde(default)]
    pub x_simulate: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ResponseInput {
    Text(String),
    Items(Vec<ResponseInputItem>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseInputItem {
    Message(ResponseInputMessage),
    Reasoning(ResponseReplayReasoning),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseInputMessage {
    pub r#type: ResponseMessageType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub role: ResponseRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ResponseItemStatus>,
    pub content: ResponseInputContent,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseMessageType {
    Message,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseReplayReasoning {
    pub r#type: ResponseReasoningType,
    pub id: String,
    #[serde(default)]
    pub summary: Vec<ResponseSummaryPart>,
    #[serde(default)]
    pub content: Vec<ResponseReasoningTextPart>,
    #[serde(default)]
    pub encrypted_content: Option<String>,
    #[serde(default)]
    pub status: Option<ResponseItemStatus>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseReasoningType {
    Reasoning,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseReasoningTextPart {
    pub r#type: ResponseReasoningTextType,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseReasoningTextType {
    ReasoningText,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseInputContent {
    Text(String),
    Parts(Vec<ResponseInputPart>),
    OutputParts(Vec<ResponseContentPart>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseInputPart {
    InputText { text: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseRole {
    User,
    Assistant,
    System,
    Developer,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ResponseConversationParam {
    Id(String),
    Object { id: String },
}

impl ResponseConversationParam {
    pub fn id(&self) -> &str {
        match self {
            Self::Id(id) | Self::Object { id } => id,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ResponseConversation {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResponseReasoningConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<ReasoningEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<ReasoningSummary>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningSummary {
    Auto,
    Concise,
    Detailed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ResponseInclude {
    #[serde(rename = "file_search_call.results")]
    FileSearchCallResults,
    #[serde(rename = "web_search_call.results")]
    WebSearchCallResults,
    #[serde(rename = "web_search_call.action.sources")]
    WebSearchCallActionSources,
    #[serde(rename = "message.input_image.image_url")]
    MessageInputImageImageUrl,
    #[serde(rename = "computer_call_output.output.image_url")]
    ComputerCallOutputImageUrl,
    #[serde(rename = "code_interpreter_call.outputs")]
    CodeInterpreterCallOutputs,
    #[serde(rename = "reasoning.encrypted_content")]
    ReasoningEncryptedContent,
    #[serde(rename = "message.output_text.logprobs")]
    MessageOutputTextLogprobs,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResponseTextConfig {
    #[serde(default)]
    pub format: ResponseTextFormat,
}

impl Default for ResponseTextConfig {
    fn default() -> Self {
        Self {
            format: ResponseTextFormat::Text,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseTextFormat {
    Text,
    JsonObject,
    JsonSchema {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        schema: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        strict: Option<bool>,
    },
}

impl Default for ResponseTextFormat {
    fn default() -> Self {
        Self::Text
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResponseObject {
    pub id: String,
    pub object: &'static str,
    pub created_at: i64,
    pub output_text: String,
    pub completed_at: Option<i64>,
    pub status: ResponseStatus,
    pub error: Option<Value>,
    pub incomplete_details: Option<Value>,
    pub instructions: Option<String>,
    pub max_output_tokens: Option<u32>,
    pub model: String,
    pub output: Vec<ResponseOutputItem>,
    pub parallel_tool_calls: bool,
    pub temperature: Option<f32>,
    pub tool_choice: Value,
    pub previous_response_id: Option<String>,
    pub conversation: Option<ResponseConversation>,
    pub reasoning: Option<ResponseReasoningConfig>,
    pub store: bool,
    pub text: ResponseTextSettings,
    pub tools: Vec<Value>,
    pub top_p: Option<f32>,
    pub usage: ResponseUsage,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ResponseDeleted {
    pub id: String,
    pub object: &'static str,
    pub deleted: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    Completed,
    Incomplete,
    Failed,
    Cancelled,
    InProgress,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseItemStatus {
    Completed,
    Incomplete,
    InProgress,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseOutputItem {
    Message {
        id: String,
        role: &'static str,
        status: ResponseItemStatus,
        content: Vec<ResponseContentPart>,
    },
    Reasoning {
        id: String,
        status: ResponseItemStatus,
        summary: Vec<ResponseSummaryPart>,
        #[serde(skip_serializing_if = "Option::is_none")]
        encrypted_content: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseOutputItemKind {
    Message,
    Reasoning,
}

impl ResponseOutputItem {
    pub fn id(&self) -> &str {
        match self {
            Self::Message { id, .. } | Self::Reasoning { id, .. } => id,
        }
    }

    pub fn kind(&self) -> ResponseOutputItemKind {
        match self {
            Self::Message { .. } => ResponseOutputItemKind::Message,
            Self::Reasoning { .. } => ResponseOutputItemKind::Reasoning,
        }
    }

    pub fn message_content(&self) -> Option<&[ResponseContentPart]> {
        match self {
            Self::Message { content, .. } => Some(content),
            Self::Reasoning { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseContentPart {
    OutputText {
        text: String,
        annotations: Vec<Value>,
        logprobs: Vec<Value>,
    },
    Refusal {
        refusal: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResponseSummaryPart {
    pub r#type: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResponseTextSettings {
    pub format: ResponseTextSettingsFormat,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseTextSettingsFormat {
    Text,
    JsonObject,
    JsonSchema {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        strict: Option<bool>,
    },
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ResponseUsage {
    pub input_tokens: u32,
    pub input_tokens_details: ResponseInputTokensDetails,
    pub output_tokens: u32,
    pub output_tokens_details: ResponseOutputTokensDetails,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ResponseInputTokensDetails {
    pub cached_tokens: u32,
    pub cache_write_tokens: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ResponseOutputTokensDetails {
    pub reasoning_tokens: u32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedResponseContext {
    pub turns: Vec<CanonicalTurn>,
    pub carried_reasoning_tokens: u32,
}

impl ResolvedResponseContext {
    pub fn from_request(request: &CreateResponseRequest) -> Self {
        Self {
            turns: request.canonical_turns(),
            carried_reasoning_tokens: request.replayed_reasoning_tokens(),
        }
    }

    pub fn prepend(&mut self, turns: &[CanonicalTurn], carried_reasoning_tokens: u32) {
        self.turns.splice(0..0, turns.iter().cloned());
        self.carried_reasoning_tokens = self
            .carried_reasoning_tokens
            .saturating_add(carried_reasoning_tokens);
    }
}

pub fn render_response(
    request: &CreateResponseRequest,
    plan: &SemanticResponsePlan,
    tokenizer: &CoreBPE,
) -> ResponseObject {
    render_response_with_context(
        request,
        plan,
        tokenizer,
        &ResolvedResponseContext::from_request(request),
    )
}

pub fn render_response_with_context(
    request: &CreateResponseRequest,
    plan: &SemanticResponsePlan,
    tokenizer: &CoreBPE,
    context: &ResolvedResponseContext,
) -> ResponseObject {
    let digest = response_resource_digest(request, plan, context.carried_reasoning_tokens);
    let identity = Identity::derive(digest, IdentityMode::Derived, &SystemClock);
    let suffix = identity.id.trim_start_matches("chatcmpl-");
    let id = format!("resp_{suffix}");
    let budget = budget_outputs(request.max_output_tokens, plan, tokenizer);
    let status = if budget.exhausted && plan.terminal == TerminalStatus::Completed {
        ResponseStatus::Incomplete
    } else {
        terminal_status(plan.terminal)
    };
    let item_status = if status == ResponseStatus::Completed {
        ResponseItemStatus::Completed
    } else {
        ResponseItemStatus::Incomplete
    };
    let mut output = Vec::new();
    let mut message_parts = Vec::new();
    let mut summary_text = None;
    let mut authored_encrypted = None;
    let mut has_reasoning = false;
    for node in &budget.output {
        match node {
            SemanticOutput::Reasoning {
                summary, encrypted, ..
            } => {
                has_reasoning = true;
                summary_text = summary.clone();
                authored_encrypted = encrypted.as_deref();
            }
            SemanticOutput::Text { text } | SemanticOutput::Structured { json: text, .. } => {
                message_parts.push(ResponseContentPart::OutputText {
                    text: text.clone(),
                    annotations: Vec::new(),
                    logprobs: Vec::new(),
                });
            }
            SemanticOutput::Refusal { text } => {
                message_parts.push(ResponseContentPart::Refusal {
                    refusal: text.clone(),
                });
            }
        }
    }
    let summary_requested = request
        .reasoning
        .as_ref()
        .and_then(|reasoning| reasoning.summary)
        .is_some();
    if has_reasoning {
        let summary = summary_text
            .clone()
            .filter(|_| summary_requested)
            .map(|text| {
                vec![ResponseSummaryPart {
                    r#type: "summary_text".to_string(),
                    text,
                }]
            })
            .unwrap_or_default();
        output.push(ResponseOutputItem::Reasoning {
            id: format!("rs_{digest:016x}"),
            status: item_status,
            summary,
            encrypted_content: request.includes_encrypted_reasoning().then(|| {
                encode_reasoning_envelope(digest, budget.reasoning_tokens, authored_encrypted)
            }),
        });
    }
    if !message_parts.is_empty() {
        output.push(ResponseOutputItem::Message {
            id: format!("msg_{digest:016x}"),
            role: "assistant",
            status: item_status,
            content: message_parts,
        });
    }

    let input_tokens = canonical_input_tokens(&context.turns, tokenizer)
        .saturating_add(context.carried_reasoning_tokens);
    let visible_tokens: u32 = output
        .iter()
        .filter_map(ResponseOutputItem::message_content)
        .map(|content| {
            content
                .iter()
                .map(|part| match part {
                    ResponseContentPart::OutputText { text, .. } => {
                        tokenizer.encode_with_special_tokens(text).len() as u32
                    }
                    ResponseContentPart::Refusal { refusal } => {
                        tokenizer.encode_with_special_tokens(refusal).len() as u32
                    }
                })
                .sum::<u32>()
        })
        .sum();
    let reasoning_tokens = budget.reasoning_tokens;
    let output_tokens = visible_tokens + reasoning_tokens;
    let output_text = output
        .iter()
        .filter_map(ResponseOutputItem::message_content)
        .flatten()
        .filter_map(|part| match part {
            ResponseContentPart::OutputText { text, .. } => Some(text.as_str()),
            ResponseContentPart::Refusal { .. } => None,
        })
        .collect();
    ResponseObject {
        id,
        object: "response",
        created_at: identity.created,
        output_text,
        completed_at: (status == ResponseStatus::Completed).then_some(identity.created + 1),
        status,
        error: response_error(status),
        incomplete_details: incomplete_details(status),
        instructions: request.instructions.clone(),
        max_output_tokens: request.max_output_tokens,
        model: request.model.clone(),
        output,
        parallel_tool_calls: request.parallel_tool_calls,
        temperature: None,
        tool_choice: Value::String("auto".to_string()),
        previous_response_id: request.previous_response_id.clone(),
        conversation: request
            .conversation
            .as_ref()
            .map(|conversation| ResponseConversation {
                id: conversation.id().to_string(),
            }),
        reasoning: request.reasoning.clone(),
        store: request.store,
        text: response_text_settings(request.text.as_ref()),
        tools: request.tools.clone(),
        top_p: None,
        usage: ResponseUsage {
            input_tokens,
            input_tokens_details: ResponseInputTokensDetails {
                cached_tokens: 0,
                cache_write_tokens: 0,
            },
            output_tokens,
            output_tokens_details: ResponseOutputTokensDetails { reasoning_tokens },
            total_tokens: input_tokens + output_tokens,
        },
        metadata: request.metadata.clone(),
    }
}

fn response_resource_digest(
    request: &CreateResponseRequest,
    plan: &SemanticResponsePlan,
    carried_reasoning_tokens: u32,
) -> u64 {
    let representation = json!({
        "schema_revision": RESPONSES_SCHEMA_REVISION,
        "plan_digest": plan.plan_digest,
        "instructions": request.instructions,
        "max_output_tokens": request.max_output_tokens,
        "model": request.model,
        "parallel_tool_calls": request.parallel_tool_calls,
        "previous_response_id": request.previous_response_id,
        "conversation_id": request.conversation.as_ref().map(ResponseConversationParam::id),
        "reasoning": request.reasoning,
        "include_encrypted_reasoning": request.includes_encrypted_reasoning(),
        "store": request.store,
        "text": response_text_settings(request.text.as_ref()),
        "tools": request.tools,
        "carried_reasoning_tokens": carried_reasoning_tokens,
        "metadata": request.metadata,
    });
    let canonical = crate::sim::canonical::canonical_json(&representation);
    digest_fields(["responses-resource-v2", canonical.as_str()])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OpaqueReasoningMetadata {
    pub resource_digest: u64,
    pub reasoning_tokens: u32,
}

fn encode_reasoning_envelope(
    resource_digest: u64,
    reasoning_tokens: u32,
    authored: Option<&str>,
) -> String {
    let material_digest = digest_fields([
        "responses-reasoning-material-v1",
        authored.unwrap_or_default(),
    ]);
    format_reasoning_envelope(resource_digest, reasoning_tokens, material_digest)
}

fn format_reasoning_envelope(
    resource_digest: u64,
    reasoning_tokens: u32,
    material_digest: u64,
) -> String {
    let resource = format!("{resource_digest:016x}");
    let tokens = format!("{reasoning_tokens:08x}");
    let material = format!("{material_digest:016x}");
    let seal = digest_fields([
        "responses-reasoning-envelope-v1",
        resource.as_str(),
        tokens.as_str(),
        material.as_str(),
    ]);
    format!("enc_v1_{resource}_{tokens}_{material}_{seal:016x}")
}

pub(crate) fn decode_reasoning_envelope(value: &str) -> Option<OpaqueReasoningMetadata> {
    let parts: Vec<_> = value.split('_').collect();
    let ["enc", "v1", resource, tokens, material, seal] = parts.as_slice() else {
        return None;
    };
    let resource_digest = u64::from_str_radix(resource, 16).ok()?;
    let reasoning_tokens = u32::from_str_radix(tokens, 16).ok()?;
    let material_digest = u64::from_str_radix(material, 16).ok()?;
    let parsed_seal = u64::from_str_radix(seal, 16).ok()?;
    let canonical = format_reasoning_envelope(resource_digest, reasoning_tokens, material_digest);
    (canonical == value && canonical.ends_with(&format!("{parsed_seal:016x}"))).then_some(
        OpaqueReasoningMetadata {
            resource_digest,
            reasoning_tokens,
        },
    )
}

struct ResponseBudget {
    output: Vec<SemanticOutput>,
    reasoning_tokens: u32,
    exhausted: bool,
}

fn budget_outputs(
    cap: Option<u32>,
    plan: &SemanticResponsePlan,
    tokenizer: &CoreBPE,
) -> ResponseBudget {
    let full_reasoning_tokens = if plan.usage.reasoning_tokens > 0 {
        plan.usage.reasoning_tokens
    } else {
        plan.output
            .iter()
            .find_map(|node| match node {
                SemanticOutput::Reasoning { summary, .. } => summary.as_deref(),
                _ => None,
            })
            .map(|text| tokenizer.encode_with_special_tokens(text).len() as u32)
            .unwrap_or(0)
    };
    let visible_tokens: u32 = plan
        .output
        .iter()
        .map(|node| match node {
            SemanticOutput::Text { text }
            | SemanticOutput::Refusal { text }
            | SemanticOutput::Structured { json: text, .. } => {
                tokenizer.encode_with_special_tokens(text).len() as u32
            }
            SemanticOutput::Reasoning { .. } => 0,
        })
        .sum();
    let full_tokens = full_reasoning_tokens + visible_tokens;
    let Some(cap) = cap.filter(|cap| *cap < full_tokens) else {
        return ResponseBudget {
            output: plan.output.clone(),
            reasoning_tokens: full_reasoning_tokens,
            exhausted: false,
        };
    };

    let reasoning_tokens = full_reasoning_tokens.min(cap);
    let mut remaining = cap.saturating_sub(reasoning_tokens);
    let output = plan
        .output
        .iter()
        .filter_map(|node| match node {
            SemanticOutput::Reasoning {
                summary,
                trace,
                encrypted,
            } => Some(SemanticOutput::Reasoning {
                summary: truncate_reasoning_summary(
                    summary.as_deref(),
                    reasoning_tokens,
                    full_reasoning_tokens,
                    tokenizer,
                ),
                trace: trace.clone(),
                encrypted: (reasoning_tokens == full_reasoning_tokens)
                    .then(|| encrypted.clone())
                    .flatten(),
            }),
            SemanticOutput::Text { text } => take_visible(text, &mut remaining, tokenizer)
                .map(|text| SemanticOutput::Text { text }),
            SemanticOutput::Refusal { text } => take_visible(text, &mut remaining, tokenizer)
                .map(|text| SemanticOutput::Refusal { text }),
            SemanticOutput::Structured { json, value } => {
                take_visible(json, &mut remaining, tokenizer).map(|json| {
                    SemanticOutput::Structured {
                        json,
                        value: value.clone(),
                    }
                })
            }
        })
        .collect();
    ResponseBudget {
        output,
        reasoning_tokens,
        exhausted: true,
    }
}

fn truncate_reasoning_summary(
    summary: Option<&str>,
    used_tokens: u32,
    full_tokens: u32,
    tokenizer: &CoreBPE,
) -> Option<String> {
    let summary = summary?;
    if used_tokens == 0 {
        return None;
    }
    if used_tokens >= full_tokens || full_tokens == 0 {
        return Some(summary.to_string());
    }
    let tokens = tokenizer.encode_with_special_tokens(summary);
    let used =
        ((tokens.len() as u64 * u64::from(used_tokens)).div_ceil(u64::from(full_tokens))) as usize;
    Some(token_pieces(tokenizer, &tokens[..used.max(1)]).concat())
}

fn take_visible(text: &str, remaining: &mut u32, tokenizer: &CoreBPE) -> Option<String> {
    if *remaining == 0 {
        return None;
    }
    let tokens = tokenizer.encode_with_special_tokens(text);
    let used = (*remaining as usize).min(tokens.len());
    *remaining -= used as u32;
    Some(token_pieces(tokenizer, &tokens[..used]).concat())
}

fn terminal_status(status: TerminalStatus) -> ResponseStatus {
    match status {
        TerminalStatus::Completed => ResponseStatus::Completed,
        TerminalStatus::Incomplete => ResponseStatus::Incomplete,
        TerminalStatus::Failed => ResponseStatus::Failed,
        TerminalStatus::Cancelled => ResponseStatus::Cancelled,
    }
}

fn response_error(status: ResponseStatus) -> Option<Value> {
    (status == ResponseStatus::Failed).then(|| {
        json!({
            "code": "server_error",
            "message": "The response failed during deterministic simulation."
        })
    })
}

fn incomplete_details(status: ResponseStatus) -> Option<Value> {
    (status == ResponseStatus::Incomplete).then(|| json!({"reason": "max_output_tokens"}))
}

fn response_text_settings(config: Option<&ResponseTextConfig>) -> ResponseTextSettings {
    let format = match config.map(|config| &config.format) {
        None | Some(ResponseTextFormat::Text) => ResponseTextSettingsFormat::Text,
        Some(ResponseTextFormat::JsonObject) => ResponseTextSettingsFormat::JsonObject,
        Some(ResponseTextFormat::JsonSchema { name, strict, .. }) => {
            ResponseTextSettingsFormat::JsonSchema {
                name: name.clone(),
                strict: *strict,
            }
        }
    };
    ResponseTextSettings { format }
}

impl CreateResponseRequest {
    pub(crate) fn includes_encrypted_reasoning(&self) -> bool {
        self.include
            .contains(&ResponseInclude::ReasoningEncryptedContent)
    }

    fn replayed_reasoning_tokens(&self) -> u32 {
        let ResponseInput::Items(items) = &self.input else {
            return 0;
        };
        items.iter().fold(0, |total, item| {
            total.saturating_add(input_item_reasoning_tokens(item))
        })
    }

    pub fn canonical_request(&self) -> CanonicalRequest {
        CanonicalRequest {
            interface: Interface::Responses,
            schema_revision: RESPONSES_SCHEMA_REVISION.to_string(),
            model: self.model.clone(),
            turns: self.canonical_turns(),
            reasoning_effort: self.reasoning.as_ref().and_then(|reasoning| {
                reasoning
                    .effort
                    .and_then(|effort| serde_json::to_value(effort).ok())
                    .and_then(|value| value.as_str().map(str::to_owned))
            }),
            response_format: self
                .text
                .as_ref()
                .and_then(|text| serde_json::to_value(&text.format).ok()),
            max_output_tokens: self.max_output_tokens,
            stream: self.stream,
            store: self.store,
            seed: None,
            context: self.replay_context(),
        }
    }

    pub(crate) fn canonical_turns(&self) -> Vec<CanonicalTurn> {
        let input = match &self.input {
            ResponseInput::Text(text) => vec![canonical_turn("user", text)],
            ResponseInput::Items(items) => items
                .iter()
                .filter_map(|item| match item {
                    ResponseInputItem::Message(message) => Some(canonical_content_turn(
                        message.role.as_str(),
                        &message.content,
                    )),
                    ResponseInputItem::Reasoning(_) => None,
                })
                .collect(),
        };
        self.instructions
            .iter()
            .map(|instructions| canonical_turn("developer", instructions))
            .chain(input)
            .collect()
    }

    fn replay_context(&self) -> Option<Value> {
        let ResponseInput::Items(items) = &self.input else {
            return None;
        };
        let replay: Vec<_> = items
            .iter()
            .filter(|item| match item {
                ResponseInputItem::Message(message) => message.id.is_some(),
                ResponseInputItem::Reasoning(_) => true,
            })
            .collect();
        (!replay.is_empty()).then(|| {
            serde_json::to_value(replay).expect("typed replay items serialize canonically")
        })
    }
}

fn default_true() -> bool {
    true
}

fn bool_or_default_true<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<bool>::deserialize(deserializer)?.unwrap_or(true))
}

fn metadata_or_default<'de, D>(deserializer: D) -> Result<BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<BTreeMap<String, String>>::deserialize(deserializer)?.unwrap_or_default())
}

fn include_or_default<'de, D>(deserializer: D) -> Result<Vec<ResponseInclude>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<Vec<ResponseInclude>>::deserialize(deserializer)?.unwrap_or_default())
}

fn canonical_content_turn(role: &str, content: &ResponseInputContent) -> CanonicalTurn {
    let content = match content {
        ResponseInputContent::Text(text) => CanonicalContent::Text(text.clone()),
        ResponseInputContent::Parts(parts) => CanonicalContent::Parts(
            serde_json::to_value(parts).expect("Responses input parts serialize"),
        ),
        ResponseInputContent::OutputParts(parts) => {
            if parts.len() == 1 {
                match &parts[0] {
                    ResponseContentPart::OutputText { text, .. } => {
                        CanonicalContent::Text(text.clone())
                    }
                    ResponseContentPart::Refusal { refusal } => {
                        CanonicalContent::Text(refusal.clone())
                    }
                }
            } else {
                CanonicalContent::Parts(
                    serde_json::to_value(parts).expect("Responses output parts serialize"),
                )
            }
        }
    };
    CanonicalTurn {
        role: role.to_string(),
        content,
        name: None,
        tool_call_id: None,
        tool_calls: None,
        function_call: None,
        audio: None,
        refusal: None,
    }
}

impl ResponseInputContent {
    pub(crate) fn is_empty(&self) -> bool {
        match self {
            Self::Text(text) => text.is_empty(),
            Self::Parts(parts) => {
                parts.is_empty()
                    || parts.iter().all(|part| match part {
                        ResponseInputPart::InputText { text } => text.is_empty(),
                    })
            }
            Self::OutputParts(parts) => {
                parts.is_empty()
                    || parts.iter().all(|part| match part {
                        ResponseContentPart::OutputText { text, .. } => text.is_empty(),
                        ResponseContentPart::Refusal { refusal } => refusal.is_empty(),
                    })
            }
        }
    }

    pub(crate) fn is_output(&self) -> bool {
        matches!(self, Self::OutputParts(_))
    }
}

impl ResponseInputMessage {
    pub(crate) fn is_replay(&self) -> bool {
        self.id.is_some() || self.status.is_some() || self.content.is_output()
    }
}

impl ResponseRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::System => "system",
            Self::Developer => "developer",
        }
    }
}

fn canonical_turn(role: &str, text: &str) -> CanonicalTurn {
    CanonicalTurn {
        role: role.to_string(),
        content: CanonicalContent::Text(text.to_string()),
        name: None,
        tool_call_id: None,
        tool_calls: None,
        function_call: None,
        audio: None,
        refusal: None,
    }
}

pub(crate) fn canonical_input_item(item: &ResponseInputItem) -> Option<CanonicalTurn> {
    match item {
        ResponseInputItem::Message(message) => Some(canonical_content_turn(
            message.role.as_str(),
            &message.content,
        )),
        ResponseInputItem::Reasoning(_) => None,
    }
}

pub(crate) fn input_item_reasoning_tokens(item: &ResponseInputItem) -> u32 {
    let ResponseInputItem::Reasoning(reasoning) = item else {
        return 0;
    };
    reasoning
        .encrypted_content
        .as_deref()
        .and_then(decode_reasoning_envelope)
        .map_or(0, |metadata| metadata.reasoning_tokens)
}

pub(crate) fn canonical_input_tokens(turns: &[CanonicalTurn], tokenizer: &CoreBPE) -> u32 {
    turns
        .iter()
        .map(|turn| match &turn.content {
            CanonicalContent::Text(text) => tokenizer.encode_with_special_tokens(text).len() as u32,
            CanonicalContent::Empty => 0,
            CanonicalContent::Parts(value) => tokenizer
                .encode_with_special_tokens(&value.to_string())
                .len() as u32,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ModelCatalogue;
    use crate::sim::artifact::builtin_artifact;
    use crate::sim::plan::{SemanticCapabilities, compile};

    #[test]
    fn contract_request_shapes_are_typed_and_canonical() {
        let text: CreateResponseRequest = serde_json::from_value(serde_json::json!({
            "model": "mock-gpt-4o",
            "input": "Say hello."
        }))
        .unwrap();
        let canonical = text.canonical_request();
        assert_eq!(canonical.interface, Interface::Responses);
        assert_eq!(canonical.turns[0].role, "user");

        let structured: CreateResponseRequest = serde_json::from_value(serde_json::json!({
            "model": "mock-gpt-4o",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "Report release status."}]
            }],
            "reasoning": {"effort": "high", "summary": "auto"},
            "text": {"format": {
                "type": "json_schema",
                "name": "release-status",
                "schema": {"type": "object"},
                "strict": true
            }}
        }))
        .unwrap();
        let canonical = structured.canonical_request();
        assert_eq!(canonical.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(
            canonical.response_format.as_ref().unwrap()["name"],
            "release-status"
        );
        assert!(canonical.canonical_json().contains("responses-2026-07-23"));
    }

    #[test]
    fn known_invalid_response_fields_fail_deserialization() {
        assert!(
            serde_json::from_value::<CreateResponseRequest>(serde_json::json!({
                "model": "mock-gpt-4o",
                "input": "hello",
                "reasoning": {"effort": "extreme"}
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<CreateResponseRequest>(serde_json::json!({
                "model": "mock-gpt-4o",
                "input": "hello",
                "text": {"format": {"type": "xml"}}
            }))
            .is_err()
        );
    }

    #[test]
    fn omitted_and_null_persistence_controls_use_spec_defaults() {
        for body in [
            serde_json::json!({"model": "mock-gpt-4o", "input": "hello"}),
            serde_json::json!({
                "model": "mock-gpt-4o",
                "input": "hello",
                "store": null,
                "parallel_tool_calls": null,
                "metadata": null,
                "include": null,
                "text": {}
            }),
        ] {
            let request: CreateResponseRequest = serde_json::from_value(body).unwrap();
            assert!(request.store);
            assert!(request.parallel_tool_calls);
            assert!(request.include.is_empty());
            assert_eq!(
                request.text.unwrap_or_default().format,
                ResponseTextFormat::Text
            );
        }
    }

    #[test]
    fn pinned_include_values_are_typed_before_scope_validation() {
        let request: CreateResponseRequest = serde_json::from_value(serde_json::json!({
            "model": "mock-reasoner",
            "input": "hello",
            "include": [
                "reasoning.encrypted_content",
                "file_search_call.results"
            ]
        }))
        .unwrap();

        assert!(request.includes_encrypted_reasoning());
        assert_eq!(request.include.len(), 2);
    }

    #[test]
    fn instructions_and_input_part_boundaries_are_canonical() {
        let request = |parts: &[&str]| {
            serde_json::from_value::<CreateResponseRequest>(serde_json::json!({
                "model": "mock-gpt-4o",
                "instructions": "Be exact.",
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": parts.iter().map(|text| serde_json::json!({
                        "type": "input_text",
                        "text": text
                    })).collect::<Vec<_>>()
                }]
            }))
            .unwrap()
            .canonical_request()
        };
        let first = request(&["ab", "c"]);
        let second = request(&["a", "bc"]);

        assert_eq!(first.turns[0].role, "developer");
        assert_eq!(
            first.turns[0].content,
            CanonicalContent::Text("Be exact.".into())
        );
        assert!(matches!(first.turns[1].content, CanonicalContent::Parts(_)));
        assert_ne!(first.digest(), second.digest());
    }

    #[test]
    fn reasoning_plan_renders_public_summary_before_message() {
        let request: CreateResponseRequest = serde_json::from_value(serde_json::json!({
            "model": "mock-reasoner",
            "input": "Which release should ship?",
            "reasoning": {"effort": "high", "summary": "auto"},
            "x_simulate": {"case": "reasoning-effort/release-decision"}
        }))
        .unwrap();
        let artifact = builtin_artifact();
        let controls = artifact
            .selection_controls(Some("reasoning-effort/release-decision".to_string()), None);
        let models = ModelCatalogue::builtin();
        let plan = compile(
            &artifact.fixtures,
            &request.canonical_request(),
            &controls,
            &SemanticCapabilities::from(models.profile("mock-reasoner").unwrap()),
        )
        .unwrap();
        let response = render_response(&request, &plan, &tiktoken_rs::cl100k_base().unwrap());
        let value = serde_json::to_value(response).unwrap();

        assert_eq!(value["object"], "response");
        assert_eq!(value["status"], "completed");
        assert_eq!(value["output"][0]["type"], "reasoning");
        assert_eq!(
            value["output"][0]["summary"][0]["text"],
            "Compared readiness, blockers, rollback coverage, ownership, and recovery time."
        );
        assert_eq!(value["output"][1]["type"], "message");
        assert_eq!(value["output"][1]["content"][0]["text"], "Ship release B.");
        assert_eq!(
            value["usage"]["output_tokens_details"]["reasoning_tokens"],
            28
        );
        assert!(!value.to_string().contains("reasoning_content"));
    }
}
