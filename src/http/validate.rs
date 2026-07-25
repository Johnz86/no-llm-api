//! Request validation.
//!
//! The mock answers the same 400s a real backend does, so a GUI's error handling
//! is exercised offline. Ranges come from `CreateChatCompletionRequest`
//! (`openapi.yaml:32658`).

use crate::http::error::ApiError;
use crate::model::{ChatCompletionRequest, ChatRole};
use crate::request_types::{ToolChoice, is_valid_function_name};

/// Validates a request, returning the spec-shaped error for the first problem.
pub fn validate(request: &ChatCompletionRequest) -> Result<(), ApiError> {
    if request.messages.is_empty() {
        return Err(invalid(
            "messages",
            "Invalid value for 'messages': expected a non-empty array.",
        ));
    }

    range("temperature", request.temperature, 0.0, 2.0)?;
    range("top_p", request.top_p, 0.0, 1.0)?;
    range("frequency_penalty", request.frequency_penalty, -2.0, 2.0)?;
    range("presence_penalty", request.presence_penalty, -2.0, 2.0)?;

    if let Some(n) = request.n
        && n == 0
    {
        return Err(invalid("n", "Invalid value for 'n': must be at least 1."));
    }

    if let Some(top_logprobs) = request.top_logprobs {
        if top_logprobs > 20 {
            return Err(invalid(
                "top_logprobs",
                "Invalid value for 'top_logprobs': must be between 0 and 20.",
            ));
        }
        if request.logprobs != Some(true) {
            return Err(invalid(
                "top_logprobs",
                "Invalid value for 'top_logprobs': 'logprobs' must be true to use this parameter.",
            ));
        }
    }

    if let Some(cap) = request.max_completion_tokens.or(request.max_tokens)
        && cap == 0
    {
        return Err(invalid(
            "max_completion_tokens",
            "Invalid value for 'max_completion_tokens': must be at least 1.",
        ));
    }

    if let Some(n) = request.n
        && n > 128
    {
        return Err(invalid("n", "Invalid value for 'n': must be at most 128."));
    }

    if let Some(stop) = &request.stop
        && stop.len() > 4
    {
        return Err(invalid(
            "stop",
            "Invalid value for 'stop': at most 4 stop sequences are allowed.",
        ));
    }

    if let Some(bias) = &request.logit_bias {
        for (token, value) in bias {
            if !(-100..=100).contains(&i32::from(*value)) {
                return Err(invalid(
                    "logit_bias",
                    format!(
                        "Invalid value for 'logit_bias[{token}]': must be between -100 and 100."
                    ),
                ));
            }
        }
    }

    if let Some(tools) = &request.tools {
        for tool in tools {
            if let Some(name) = tool.function_name()
                && !is_valid_function_name(name)
            {
                return Err(invalid(
                    "tools",
                    format!(
                        "Invalid value for 'tools': function name '{name}' must be 1-64 characters \
                         of letters, digits, underscores and dashes."
                    ),
                ));
            }
        }
    }

    // A forced tool that is not declared can never fire, so the request is a bug
    // rather than a scenario worth simulating.
    if let Some(ToolChoice::Named { function, .. }) = &request.tool_choice {
        let declared = request
            .tools
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|tool| tool.function_name() == Some(function.name.as_str()));
        if !declared {
            return Err(invalid(
                "tool_choice",
                format!(
                    "Invalid value for 'tool_choice': function '{}' is not present in 'tools'.",
                    function.name
                ),
            ));
        }
    }

    for (index, message) in request.messages.iter().enumerate() {
        match message.role {
            ChatRole::Tool if message.tool_call_id.is_none() => {
                return Err(invalid(
                    "messages",
                    format!(
                        "Invalid message at index {index}: 'tool_call_id' is required when role is 'tool'."
                    ),
                ));
            }
            ChatRole::User | ChatRole::System | ChatRole::Developer
                if message.content.is_none() =>
            {
                return Err(invalid(
                    "messages",
                    format!(
                        "Invalid message at index {index}: 'content' is required for this role."
                    ),
                ));
            }
            _ => {}
        }
    }

    Ok(())
}

fn range(name: &str, value: Option<f32>, low: f32, high: f32) -> Result<(), ApiError> {
    match value {
        Some(value) if !(low..=high).contains(&value) => Err(invalid(
            name,
            format!("Invalid value for '{name}': must be between {low} and {high}."),
        )),
        _ => Ok(()),
    }
}

fn invalid(param: &str, message: impl Into<String>) -> ApiError {
    ApiError::invalid_request(message).with_param(param)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChatCompletionRequestMessage, MessageContent};
    use crate::request_types::{
        FunctionDefinition, FunctionName, StopConfiguration, Tool, ToolKind,
    };

    fn request() -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "m".to_string(),
            messages: vec![ChatCompletionRequestMessage {
                role: ChatRole::User,
                content: Some(MessageContent::Text("hi".to_string())),
                name: None,
                tool_call_id: None,
                tool_calls: None,
                function_call: None,
                audio: None,
                refusal: None,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn a_plain_request_is_valid() {
        assert!(validate(&request()).is_ok());
    }

    #[test]
    fn empty_messages_is_rejected() {
        let mut request = request();
        request.messages.clear();
        let error = validate(&request).unwrap_err();
        assert_eq!(error.body.param.as_deref(), Some("messages"));
    }

    #[test]
    fn scalar_ranges_are_enforced_at_both_ends() {
        for (mutate, param) in [
            (
                Box::new(|r: &mut ChatCompletionRequest| r.temperature = Some(2.5))
                    as Box<dyn Fn(&mut ChatCompletionRequest)>,
                "temperature",
            ),
            (
                Box::new(|r: &mut ChatCompletionRequest| r.top_p = Some(-0.1)),
                "top_p",
            ),
            (
                Box::new(|r: &mut ChatCompletionRequest| r.frequency_penalty = Some(3.0)),
                "frequency_penalty",
            ),
            (
                Box::new(|r: &mut ChatCompletionRequest| r.presence_penalty = Some(-9.0)),
                "presence_penalty",
            ),
        ] {
            let mut request = request();
            mutate(&mut request);
            let error = validate(&request).unwrap_err();
            assert_eq!(error.body.param.as_deref(), Some(param));
        }
    }

    #[test]
    fn boundary_values_are_accepted() {
        let mut request = request();
        request.temperature = Some(0.0);
        request.top_p = Some(1.0);
        request.frequency_penalty = Some(-2.0);
        request.presence_penalty = Some(2.0);
        assert!(validate(&request).is_ok());
    }

    #[test]
    fn top_logprobs_requires_logprobs() {
        let mut request = request();
        request.top_logprobs = Some(3);
        let error = validate(&request).unwrap_err();
        assert_eq!(error.body.param.as_deref(), Some("top_logprobs"));

        request.logprobs = Some(true);
        assert!(validate(&request).is_ok());

        request.top_logprobs = Some(21);
        assert!(validate(&request).is_err());
    }

    #[test]
    fn zero_caps_are_rejected() {
        let mut request = request();
        request.max_completion_tokens = Some(0);
        assert!(validate(&request).is_err());
        request.max_completion_tokens = Some(1);
        assert!(validate(&request).is_ok());
        request.n = Some(0);
        assert!(validate(&request).is_err());
        request.n = Some(129);
        assert!(validate(&request).is_err());
        request.n = Some(128);
        assert!(validate(&request).is_ok());
    }

    #[test]
    fn more_than_four_stop_sequences_is_rejected() {
        let mut request = request();
        request.stop = Some(StopConfiguration::Multiple(
            ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect(),
        ));
        assert!(validate(&request).is_ok());
        request.stop = Some(StopConfiguration::Multiple(
            ["a", "b", "c", "d", "e"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        ));
        let error = validate(&request).unwrap_err();
        assert_eq!(error.body.param.as_deref(), Some("stop"));
    }

    #[test]
    fn logit_bias_outside_the_documented_range_is_rejected() {
        let mut request = request();
        request.logit_bias = Some([("42".to_string(), 100i8)].into_iter().collect());
        assert!(validate(&request).is_ok());
        request.logit_bias = Some([("42".to_string(), 120i8)].into_iter().collect());
        let error = validate(&request).unwrap_err();
        assert_eq!(error.body.param.as_deref(), Some("logit_bias"));
    }

    #[test]
    fn a_malformed_function_name_is_rejected() {
        let mut request = request();
        request.tools = Some(vec![Tool::Function {
            function: FunctionDefinition {
                name: "get weather".to_string(),
                description: None,
                parameters: None,
                strict: None,
            },
        }]);
        let error = validate(&request).unwrap_err();
        assert_eq!(error.body.param.as_deref(), Some("tools"));
    }

    #[test]
    fn forcing_an_undeclared_tool_is_rejected() {
        let mut request = request();
        request.tool_choice = Some(ToolChoice::Named {
            r#type: ToolKind::Function,
            function: FunctionName {
                name: "get_weather".to_string(),
            },
        });
        let error = validate(&request).unwrap_err();
        assert_eq!(error.body.param.as_deref(), Some("tool_choice"));

        request.tools = Some(vec![Tool::Function {
            function: FunctionDefinition {
                name: "get_weather".to_string(),
                description: None,
                parameters: None,
                strict: None,
            },
        }]);
        assert!(validate(&request).is_ok());
    }

    #[test]
    fn a_tool_message_needs_the_call_it_answers() {
        let mut request = request();
        request.messages[0].role = ChatRole::Tool;
        let error = validate(&request).unwrap_err();
        assert_eq!(error.body.param.as_deref(), Some("messages"));

        request.messages[0].tool_call_id = Some("call_alpha".to_string());
        assert!(validate(&request).is_ok());
    }

    #[test]
    fn a_user_message_without_content_is_rejected() {
        let mut request = request();
        request.messages[0].content = None;
        let error = validate(&request).unwrap_err();
        assert_eq!(error.body.param.as_deref(), Some("messages"));
    }
}
