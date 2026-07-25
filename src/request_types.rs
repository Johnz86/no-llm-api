//! Typed request vocabulary.
//!
//! A `Value`-shaped field accepts garbage and echoes it back, so a GUI's
//! malformed request looks successful. These types are the shapes the spec and
//! `async-openai` declare, which turns "silently ignored" into a 400.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `stop`: one string or up to four (`CreateChatCompletionRequest`,
/// `openapi.yaml:32658`). More than four must be rejected, not truncated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StopConfiguration {
    Single(String),
    Multiple(Vec<String>),
}

impl StopConfiguration {
    pub fn len(&self) -> usize {
        match self {
            StopConfiguration::Single(_) => 1,
            StopConfiguration::Multiple(values) => values.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn sequences(&self) -> Vec<&str> {
        match self {
            StopConfiguration::Single(value) => vec![value.as_str()],
            StopConfiguration::Multiple(values) => values.iter().map(String::as_str).collect(),
        }
    }
}

/// `response_format`, the field structured-output GUIs depend on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    Text,
    JsonObject,
    JsonSchema { json_schema: JsonSchemaFormat },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonSchemaFormat {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

/// `ServiceTier` (`openapi.yaml:61410`). Echoed into every response, so a wrong
/// value here breaks typed clients rather than the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceTier {
    Auto,
    #[default]
    Default,
    Flex,
    Scale,
    Priority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    Text,
    Audio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    Wav,
    Aac,
    Mp3,
    Flac,
    Opus,
    Pcm16,
}

/// `audio`: the voice list changes often upstream, so it stays a string while
/// the format - which a client must be able to decode - is an enum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioConfig {
    pub voice: String,
    pub format: AudioFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoiceMode {
    None,
    Auto,
    Required,
}

/// `tool_choice`: a mode, or a named function a test wants to force.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolChoice {
    Mode(ToolChoiceMode),
    Named {
        r#type: ToolKind,
        function: FunctionName,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Function,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionName {
    pub name: String,
}

/// One entry of `tools`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Tool {
    Function { function: FunctionDefinition },
    Custom { custom: Value },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

impl Tool {
    pub fn function_name(&self) -> Option<&str> {
        match self {
            Tool::Function { function } => Some(function.name.as_str()),
            Tool::Custom { .. } => None,
        }
    }
}

/// `logit_bias`: token id to bias in -100..=100. A wider type would accept
/// values the API rejects.
pub type LogitBias = HashMap<String, i8>;

/// Whether a function name matches the pattern the API enforces.
pub fn is_valid_function_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stop_accepts_both_forms_and_counts_them() {
        let single: StopConfiguration = serde_json::from_value(json!("END")).unwrap();
        assert_eq!(single.len(), 1);
        let many: StopConfiguration = serde_json::from_value(json!(["a", "b", "c"])).unwrap();
        assert_eq!(many.len(), 3);
        assert_eq!(many.sequences(), vec!["a", "b", "c"]);
    }

    #[test]
    fn response_format_is_tagged_the_way_structured_output_clients_send_it() {
        let text: ResponseFormat = serde_json::from_value(json!({"type": "text"})).unwrap();
        assert_eq!(text, ResponseFormat::Text);

        let schema: ResponseFormat = serde_json::from_value(json!({
            "type": "json_schema",
            "json_schema": { "name": "reply", "strict": true, "schema": { "type": "object" } }
        }))
        .unwrap();
        match schema {
            ResponseFormat::JsonSchema { json_schema } => {
                assert_eq!(json_schema.name, "reply");
                assert_eq!(json_schema.strict, Some(true));
            }
            other => panic!("unexpected: {other:?}"),
        }

        assert!(serde_json::from_value::<ResponseFormat>(json!({"type": "yaml"})).is_err());
    }

    #[test]
    fn enum_fields_reject_values_the_api_would_reject() {
        assert!(serde_json::from_value::<ServiceTier>(json!("flex")).is_ok());
        assert!(serde_json::from_value::<ServiceTier>(json!("turbo")).is_err());
        assert!(serde_json::from_value::<ReasoningEffort>(json!("xhigh")).is_ok());
        assert!(serde_json::from_value::<ReasoningEffort>(json!("extreme")).is_err());
        assert!(serde_json::from_value::<Modality>(json!("audio")).is_ok());
        assert!(serde_json::from_value::<Modality>(json!("video")).is_err());
        assert!(serde_json::from_value::<AudioFormat>(json!("pcm16")).is_ok());
        assert!(serde_json::from_value::<AudioFormat>(json!("ogg")).is_err());
    }

    #[test]
    fn tool_choice_covers_modes_and_forced_functions() {
        let auto: ToolChoice = serde_json::from_value(json!("auto")).unwrap();
        assert_eq!(auto, ToolChoice::Mode(ToolChoiceMode::Auto));

        let forced: ToolChoice = serde_json::from_value(json!({
            "type": "function", "function": { "name": "get_weather" }
        }))
        .unwrap();
        match forced {
            ToolChoice::Named { function, .. } => assert_eq!(function.name, "get_weather"),
            other => panic!("unexpected: {other:?}"),
        }

        assert!(serde_json::from_value::<ToolChoice>(json!("maybe")).is_err());
    }

    #[test]
    fn tools_are_tagged_and_expose_their_function_name() {
        let tool: Tool = serde_json::from_value(json!({
            "type": "function",
            "function": { "name": "get_weather", "parameters": { "type": "object" } }
        }))
        .unwrap();
        assert_eq!(tool.function_name(), Some("get_weather"));
        assert!(serde_json::from_value::<Tool>(json!({"type": "plugin"})).is_err());
    }

    #[test]
    fn logit_bias_rejects_out_of_byte_range_values() {
        assert!(serde_json::from_value::<LogitBias>(json!({"1234": 50})).is_ok());
        assert!(serde_json::from_value::<LogitBias>(json!({"1234": 5000})).is_err());
    }

    #[test]
    fn function_names_follow_the_api_pattern() {
        assert!(is_valid_function_name("get_weather-2"));
        assert!(!is_valid_function_name(""));
        assert!(!is_valid_function_name("get weather"));
        assert!(!is_valid_function_name(&"x".repeat(65)));
    }
}
