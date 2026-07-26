//! Typed request and response vocabulary for the experimental Responses surface.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tiktoken_rs::CoreBPE;

use crate::request_types::ReasoningEffort;
use crate::sim::canonical::{CanonicalContent, CanonicalRequest, CanonicalTurn};
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
    pub reasoning: Option<ResponseReasoningConfig>,
    #[serde(default)]
    pub text: Option<ResponseTextConfig>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub store: bool,
    #[serde(default)]
    pub metadata: Map<String, Value>,
    #[serde(default)]
    pub previous_response_id: Option<String>,
    #[serde(default)]
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

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseInputItem {
    Message {
        role: ResponseRole,
        content: ResponseInputContent,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ResponseInputContent {
    Text(String),
    Parts(Vec<ResponseInputPart>),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseInputPart {
    InputText { text: String },
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseRole {
    User,
    Assistant,
    System,
    Developer,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResponseTextConfig {
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
    pub instructions: Option<Value>,
    pub max_output_tokens: Option<u32>,
    pub model: String,
    pub output: Vec<ResponseOutputItem>,
    pub parallel_tool_calls: bool,
    pub temperature: Option<f32>,
    pub tool_choice: Value,
    pub previous_response_id: Option<String>,
    pub reasoning: Option<ResponseReasoningConfig>,
    pub store: bool,
    pub text: ResponseTextSettings,
    pub tools: Vec<Value>,
    pub top_p: Option<f32>,
    pub usage: ResponseUsage,
    pub metadata: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ResponseDeleted {
    pub id: String,
    pub object: &'static str,
    pub deleted: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    Completed,
    Incomplete,
    Failed,
    Cancelled,
    InProgress,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, PartialEq)]
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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ResponseSummaryPart {
    pub r#type: &'static str,
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

pub fn render_response(
    request: &CreateResponseRequest,
    plan: &SemanticResponsePlan,
    tokenizer: &CoreBPE,
) -> ResponseObject {
    render_response_with_context(request, plan, tokenizer, 0)
}

pub fn render_response_with_context(
    request: &CreateResponseRequest,
    plan: &SemanticResponsePlan,
    tokenizer: &CoreBPE,
    prior_input_tokens: u32,
) -> ResponseObject {
    let digest = u64::from_str_radix(&plan.plan_digest, 16)
        .expect("semantic plan digest is a hexadecimal u64");
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
    let mut encrypted_content = None;
    for node in &budget.output {
        match node {
            SemanticOutput::Reasoning {
                summary, encrypted, ..
            } => {
                summary_text = summary.clone();
                encrypted_content = encrypted.clone();
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
    if (summary_requested && summary_text.is_some()) || encrypted_content.is_some() {
        let summary = summary_text
            .clone()
            .filter(|_| summary_requested)
            .map(|text| {
                vec![ResponseSummaryPart {
                    r#type: "summary_text",
                    text,
                }]
            })
            .unwrap_or_default();
        output.push(ResponseOutputItem::Reasoning {
            id: format!("rs_{digest:016x}"),
            status: item_status,
            summary,
            encrypted_content,
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

    let input_tokens = prior_input_tokens
        + request
            .canonical_request()
            .turns
            .iter()
            .map(|turn| match &turn.content {
                CanonicalContent::Text(text) => {
                    tokenizer.encode_with_special_tokens(text).len() as u32
                }
                CanonicalContent::Empty => 0,
                CanonicalContent::Parts(value) => tokenizer
                    .encode_with_special_tokens(&value.to_string())
                    .len() as u32,
            })
            .sum::<u32>();
    let visible_tokens: u32 = output
        .iter()
        .map(|item| match item {
            ResponseOutputItem::Message { content, .. } => content
                .iter()
                .map(|part| match part {
                    ResponseContentPart::OutputText { text, .. } => {
                        tokenizer.encode_with_special_tokens(text).len() as u32
                    }
                    ResponseContentPart::Refusal { refusal } => {
                        tokenizer.encode_with_special_tokens(refusal).len() as u32
                    }
                })
                .sum(),
            ResponseOutputItem::Reasoning { .. } => 0,
        })
        .sum();
    let reasoning_tokens = budget.reasoning_tokens;
    let output_tokens = visible_tokens + reasoning_tokens;
    let output_text = output
        .iter()
        .filter_map(|item| match item {
            ResponseOutputItem::Message { content, .. } => Some(content),
            ResponseOutputItem::Reasoning { .. } => None,
        })
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
        instructions: None,
        max_output_tokens: request.max_output_tokens,
        model: request.model.clone(),
        output,
        parallel_tool_calls: request.parallel_tool_calls,
        temperature: None,
        tool_choice: Value::String("auto".to_string()),
        previous_response_id: request.previous_response_id.clone(),
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
            context: None,
        }
    }

    fn canonical_turns(&self) -> Vec<CanonicalTurn> {
        match &self.input {
            ResponseInput::Text(text) => vec![canonical_turn("user", text)],
            ResponseInput::Items(items) => items
                .iter()
                .map(|item| match item {
                    ResponseInputItem::Message { role, content } => {
                        canonical_turn(role.as_str(), &content.text())
                    }
                })
                .collect(),
        }
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

impl ResponseInputContent {
    fn text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Parts(parts) => parts
                .iter()
                .map(|part| match part {
                    ResponseInputPart::InputText { text } => text.as_str(),
                })
                .collect(),
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
