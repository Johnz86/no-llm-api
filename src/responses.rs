//! Typed request and response vocabulary for the experimental Responses surface.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::request_types::ReasoningEffort;
use crate::sim::canonical::{CanonicalContent, CanonicalRequest, CanonicalTurn};
use crate::sim::script::Interface;

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
}
