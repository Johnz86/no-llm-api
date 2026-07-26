//! Protocol-neutral request canonicalization for deterministic semantic plans.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{ChatCompletionRequest, ChatRole, MessageContent};
use crate::sim::digest::digest_fields;
use crate::sim::script::{Interface, SemanticRole};

pub const CHAT_SCHEMA_REVISION: &str = "chat-completions-2026-07-23";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanonicalRequest {
    pub interface: Interface,
    pub schema_revision: String,
    pub model: String,
    pub turns: Vec<CanonicalTurn>,
    pub reasoning_effort: Option<String>,
    pub response_format: Option<Value>,
    pub max_output_tokens: Option<u32>,
    pub stream: bool,
    pub store: bool,
    pub seed: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanonicalTurn {
    pub role: String,
    pub content: CanonicalContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_call: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum CanonicalContent {
    Empty,
    Text(String),
    Parts(Value),
}

impl CanonicalRequest {
    pub fn from_chat(request: &ChatCompletionRequest) -> Self {
        Self {
            interface: Interface::ChatCompletions,
            schema_revision: CHAT_SCHEMA_REVISION.to_string(),
            model: request.model.clone(),
            turns: request
                .messages
                .iter()
                .map(|message| CanonicalTurn {
                    role: chat_role(message.role.clone()).to_string(),
                    content: match &message.content {
                        None => CanonicalContent::Empty,
                        Some(MessageContent::Text(text)) => CanonicalContent::Text(text.clone()),
                        Some(MessageContent::Parts(parts)) => {
                            CanonicalContent::Parts(Value::Array(parts.clone()))
                        }
                    },
                    name: message.name.clone(),
                    tool_call_id: message.tool_call_id.clone(),
                    tool_calls: message
                        .tool_calls
                        .as_ref()
                        .map(|calls| Value::Array(calls.clone())),
                    function_call: message.function_call.clone(),
                    audio: message.audio.clone(),
                    refusal: message.refusal.clone(),
                })
                .collect(),
            reasoning_effort: request
                .reasoning_effort
                .and_then(|effort| serde_json::to_value(effort).ok())
                .and_then(|value| value.as_str().map(str::to_owned)),
            response_format: request.response_format.as_ref().and_then(|format| {
                serde_json::to_value(format)
                    .ok()
                    .map(|value| canonicalize_json(&value))
            }),
            max_output_tokens: request.max_completion_tokens.or(request.max_tokens),
            stream: request.stream,
            store: request.store.unwrap_or(false),
            seed: request.seed,
        }
    }

    pub fn canonical_json(&self) -> String {
        canonical_json(&serde_json::to_value(self).expect("canonical request is serializable"))
    }

    pub fn digest(&self) -> u64 {
        let json = self.canonical_json();
        digest_fields(["semantic-request-v1", json.as_str()])
    }

    pub fn match_turns(&self) -> Option<Vec<(SemanticRole, &str)>> {
        self.turns
            .iter()
            .map(|turn| {
                let role = match turn.role.as_str() {
                    "system" => SemanticRole::System,
                    "developer" => SemanticRole::Developer,
                    "user" => SemanticRole::User,
                    "assistant" => SemanticRole::Assistant,
                    _ => return None,
                };
                let CanonicalContent::Text(text) = &turn.content else {
                    return None;
                };
                Some((role, text.as_str()))
            })
            .collect()
    }
}

fn chat_role(role: ChatRole) -> &'static str {
    match role {
        ChatRole::System => "system",
        ChatRole::Developer => "developer",
        ChatRole::User => "user",
        ChatRole::Assistant => "assistant",
        ChatRole::Tool => "tool",
        ChatRole::Function => "function",
    }
}

pub fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            serde_json::to_string(value).expect("JSON scalar is serializable")
        }
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(values) => {
            let mut entries: Vec<_> = values.iter().collect();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            format!(
                "{{{}}}",
                entries
                    .into_iter()
                    .map(|(key, value)| format!(
                        "{}:{}",
                        serde_json::to_string(key).expect("JSON key is serializable"),
                        canonical_json(value)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

fn canonicalize_json(value: &Value) -> Value {
    serde_json::from_str(&canonical_json(value)).expect("canonical JSON round trips")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChatCompletionRequestMessage, ChatRole};
    use crate::request_types::{JsonSchemaFormat, ReasoningEffort, ResponseFormat};

    #[test]
    fn object_key_order_never_changes_canonical_json() {
        let left = serde_json::json!({"z": 1, "a": {"d": 2, "b": 3}});
        let right = serde_json::json!({"a": {"b": 3, "d": 2}, "z": 1});
        assert_eq!(canonical_json(&left), canonical_json(&right));
    }

    #[test]
    fn chat_controls_and_role_boundaries_participate() {
        let request = ChatCompletionRequest {
            model: "mock-reasoner".to_string(),
            messages: vec![
                ChatCompletionRequestMessage {
                    role: ChatRole::Developer,
                    content: Some(MessageContent::Text("Be concise.".to_string())),
                    name: None,
                    tool_call_id: None,
                    tool_calls: None,
                    function_call: None,
                    audio: None,
                    refusal: None,
                },
                ChatCompletionRequestMessage {
                    role: ChatRole::User,
                    content: Some(MessageContent::Text("Status?".to_string())),
                    name: None,
                    tool_call_id: None,
                    tool_calls: None,
                    function_call: None,
                    audio: None,
                    refusal: None,
                },
            ],
            reasoning_effort: Some(ReasoningEffort::High),
            response_format: Some(ResponseFormat::JsonSchema {
                json_schema: JsonSchemaFormat {
                    name: "status".to_string(),
                    description: None,
                    schema: Some(serde_json::json!({"required": ["ok"], "type": "object"})),
                    strict: Some(true),
                },
            }),
            stream: true,
            ..Default::default()
        };
        let canonical = CanonicalRequest::from_chat(&request);
        assert_eq!(canonical.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(canonical.turns[0].role, "developer");
        assert!(canonical.canonical_json().contains("json_schema"));
        assert_eq!(canonical.digest(), canonical.digest());
    }

    #[test]
    fn framing_and_roles_prevent_semantic_collisions() {
        let mut first = CanonicalRequest::from_chat(&ChatCompletionRequest {
            model: "m".to_string(),
            messages: Vec::new(),
            ..Default::default()
        });
        let mut second = first.clone();
        first.turns.push(CanonicalTurn {
            role: "user".to_string(),
            content: CanonicalContent::Text("ab".to_string()),
            name: None,
            tool_call_id: None,
            tool_calls: None,
            function_call: None,
            audio: None,
            refusal: None,
        });
        second.turns.push(CanonicalTurn {
            role: "assistant".to_string(),
            content: CanonicalContent::Text("ab".to_string()),
            name: None,
            tool_call_id: None,
            tool_calls: None,
            function_call: None,
            audio: None,
            refusal: None,
        });
        assert_ne!(first.digest(), second.digest());
    }
}
