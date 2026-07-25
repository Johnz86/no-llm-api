#![allow(dead_code)]

pub mod sse;

use std::borrow::Cow;
use std::num::NonZeroU32;
use std::sync::Arc;

use no_llm_api::dataset::{ConversationScripts, DatasetRow, write_dataset};
use no_llm_api::http::{RouterOptions, build_router_with_options};
use no_llm_api::service::ChatService;
use no_llm_api::sim::scenario::Scenario;
use no_llm_api::sim::stream::CancelCounter;
use tempfile::TempDir;
use tiktoken_rs::cl100k_base;

/// A reply that spans combining marks, emoji, a ZWJ sequence and CJK, so that
/// single tokens necessarily land inside multi-byte characters.
pub const UNICODE_PROMPT: &str = "unicode please";
pub const UNICODE_REPLY: &str = "cafe\u{301} \u{1F680}\u{1F44D} \u{1F469}\u{200D}\u{1F4BB} \u{4F60}\u{597D}\u{4E16}\u{754C} done";

pub const PLAIN_PROMPT: &str = "plain please";
pub const PLAIN_REPLY: &str = "All good here.";

pub const TOOL_PROMPT: &str = "check the weather in two cities";
pub const REFUSAL_PROMPT: &str = "do something disallowed";
pub const REFUSAL_TEXT: &str = "I'm sorry, but I can't help with that request.";

pub const REASONING_PROMPT: &str = "think it through";
pub const REASONING_TRACE: &str = "The user wants the capital. France's capital is Paris.";
pub const REASONING_ANSWER: &str = "Paris.";

/// Two parallel tool calls with arguments long enough to be split.
pub fn tool_calls_json() -> String {
    serde_json::json!([
        {
            "id": "call_alpha",
            "type": "function",
            "function": {
                "name": "get_weather",
                "arguments": "{\"city\":\"Berlin\",\"unit\":\"celsius\"}"
            }
        },
        {
            "id": "call_beta",
            "type": "function",
            "function": {
                "name": "get_weather",
                "arguments": "{\"city\":\"Tokyo\",\"unit\":\"celsius\"}"
            }
        }
    ])
    .to_string()
}

pub struct Fixture {
    pub app: axum::Router,
    pub cancels: Arc<CancelCounter>,
    pub state: no_llm_api::http::AppState,
    _dir: TempDir,
}

/// Builds a router backed by a purpose-written parquet fixture.
pub fn fixture(tokens_per_second: u32) -> Fixture {
    fixture_with_rows(tokens_per_second, &default_rows())
}

/// A router running a named built-in scenario or an explicit profile.
pub fn fixture_with_scenario(tokens_per_second: u32, scenario: Scenario) -> Fixture {
    build(
        tokens_per_second,
        &default_rows(),
        RouterOptions {
            scenario,
            ..RouterOptions::default()
        },
    )
}

/// A router with arbitrary options, for auth and control-plane tests.
pub fn fixture_with_options(tokens_per_second: u32, options: RouterOptions) -> Fixture {
    build(tokens_per_second, &default_rows(), options)
}

pub fn default_rows() -> Vec<DatasetRow<'static>> {
    let mut tool_reply = row("conv-tools", 1, "assistant", "", Some("tool_calls"));
    tool_reply.content = None;
    tool_reply.tool_calls = Some(Cow::Owned(tool_calls_json()));

    let mut refusal_reply = row("conv-refusal", 1, "assistant", "", Some("content_filter"));
    refusal_reply.content = None;
    refusal_reply.refusal = Some(Cow::Borrowed(REFUSAL_TEXT));

    vec![
        row("conv-unicode", 0, "user", UNICODE_PROMPT, None),
        row("conv-unicode", 1, "assistant", UNICODE_REPLY, Some("stop")),
        row("conv-plain", 0, "user", PLAIN_PROMPT, None),
        row("conv-plain", 1, "assistant", PLAIN_REPLY, Some("stop")),
        row("conv-tools", 0, "user", TOOL_PROMPT, None),
        tool_reply,
        row("conv-refusal", 0, "user", REFUSAL_PROMPT, None),
        refusal_reply,
        row("conv-reasoning", 0, "user", REASONING_PROMPT, None),
        row(
            "conv-reasoning",
            1,
            "assistant",
            &format!("<think>{REASONING_TRACE}</think>{REASONING_ANSWER}"),
            Some("stop"),
        ),
    ]
}

pub fn row(
    conversation: &str,
    turn: u32,
    role: &str,
    content: &str,
    finish: Option<&str>,
) -> DatasetRow<'static> {
    DatasetRow {
        conversation_id: Cow::Owned(conversation.to_string()),
        turn_index: turn,
        role: Cow::Owned(role.to_string()),
        content: Some(Cow::Owned(content.to_string())),
        content_parts: None,
        refusal: None,
        tool_calls: None,
        function_call: None,
        audio: None,
        finish_reason: finish.map(|value| Cow::Owned(value.to_string())),
        usage: None,
    }
}

pub fn fixture_with_rows(tokens_per_second: u32, rows: &[DatasetRow<'_>]) -> Fixture {
    build(tokens_per_second, rows, RouterOptions::default())
}

fn build(tokens_per_second: u32, rows: &[DatasetRow<'_>], options: RouterOptions) -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("fixture.parquet");
    write_dataset(&path, rows).expect("write fixture dataset");

    let tokenizer = Arc::new(cl100k_base().expect("tokenizer"));
    let scripts = ConversationScripts::load(&path, &tokenizer).expect("load fixture dataset");
    let service = Arc::new(ChatService::new(
        scripts,
        tokenizer,
        NonZeroU32::new(tokens_per_second).expect("non-zero rate"),
    ));
    let (app, state) = build_router_with_options(service, options);

    Fixture {
        app,
        cancels: state.cancels.clone(),
        state,
        _dir: dir,
    }
}

/// A minimal streamed chat request body.
pub fn stream_body(prompt: &str) -> serde_json::Value {
    serde_json::json!({
        "model": "gpt-4o-mini",
        "stream": true,
        "messages": [{ "role": "user", "content": prompt }]
    })
}

/// A minimal non-streamed chat request body.
pub fn body(prompt: &str) -> serde_json::Value {
    serde_json::json!({
        "model": "gpt-4o-mini",
        "messages": [{ "role": "user", "content": prompt }]
    })
}

/// One request against the in-process router, returning status, headers and body text.
pub async fn send(
    app: axum::Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (axum::http::StatusCode, axum::http::HeaderMap, String) {
    send_raw(app, method, uri, body.map(|value| value.to_string()), true).await
}

pub async fn send_raw(
    app: axum::Router,
    method: &str,
    uri: &str,
    body: Option<String>,
    json_content_type: bool,
) -> (axum::http::StatusCode, axum::http::HeaderMap, String) {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() && json_content_type {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(body.map(Body::from).unwrap_or_else(Body::empty))
        .expect("request");

    let response = app.oneshot(request).await.expect("router call");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

/// Parses a response body and asserts it is a spec-shaped error envelope.
pub fn assert_error_envelope(text: &str) -> serde_json::Value {
    let value: serde_json::Value =
        serde_json::from_str(text).unwrap_or_else(|error| panic!("not JSON ({error}): {text}"));
    let error = value
        .get("error")
        .unwrap_or_else(|| panic!("no error member: {text}"));
    for key in ["message", "type", "param", "code"] {
        assert!(
            error.get(key).is_some(),
            "error envelope is missing '{key}': {text}"
        );
    }
    assert!(
        error["message"].as_str().is_some_and(|m| !m.is_empty()),
        "error message must be non-empty: {text}"
    );
    value
}

/// A request with extra headers, for auth and simulation-directive tests.
pub async fn send_with_headers(
    app: axum::Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
    extra: &[(&str, &str)],
) -> (axum::http::StatusCode, axum::http::HeaderMap, String) {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    for (name, value) in extra {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(
            body.map(|value| Body::from(value.to_string()))
                .unwrap_or_else(Body::empty),
        )
        .expect("request");

    let response = app.oneshot(request).await.expect("router call");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}
