//! Snapshots of the wire shape. Identity is plan-derived, so nothing is redacted:
//! a diff here means the bytes a GUI sees actually changed.

mod support;

use serde_json::json;
use support::sse::collect_sse;
use support::{
    PLAIN_PROMPT, REFUSAL_PROMPT, TOOL_PROMPT, UNICODE_PROMPT, body, fixture, send, send_raw,
    stream_body,
};

async fn post_body(value: serde_json::Value) -> String {
    let fixture = fixture(1000);
    let (_, _, text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(value),
    )
    .await;
    pretty(&text)
}

fn pretty(text: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value) => serde_json::to_string_pretty(&value).unwrap(),
        Err(_) => text.to_string(),
    }
}

#[tokio::test]
async fn snapshot_plain_completion() {
    insta::assert_snapshot!(post_body(body(PLAIN_PROMPT)).await);
}

#[tokio::test]
async fn snapshot_unicode_completion() {
    insta::assert_snapshot!(post_body(body(UNICODE_PROMPT)).await);
}

#[tokio::test]
async fn snapshot_tool_call_completion() {
    insta::assert_snapshot!(post_body(body(TOOL_PROMPT)).await);
}

#[tokio::test]
async fn snapshot_refusal_completion() {
    insta::assert_snapshot!(post_body(body(REFUSAL_PROMPT)).await);
}

#[tokio::test]
async fn snapshot_completion_with_parts_content() {
    insta::assert_snapshot!(
        post_body(json!({
            "model": "mock-gpt-4o",
            "messages": [{
                "role": "user",
                "content": [{ "type": "text", "text": PLAIN_PROMPT }]
            }]
        }))
        .await
    );
}

#[tokio::test]
async fn snapshot_completion_with_token_cap() {
    insta::assert_snapshot!(
        post_body(json!({
            "model": "mock-gpt-4o",
            "max_completion_tokens": 3,
            "messages": [{ "role": "user", "content": UNICODE_PROMPT }]
        }))
        .await
    );
}

async fn transcript(value: serde_json::Value) -> String {
    let fixture = fixture(1000);
    let transcript = collect_sse(fixture.app.clone(), value).await;
    transcript
        .frames
        .iter()
        .map(|frame| format!("data: {}", frame.data))
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn snapshot_plain_transcript() {
    insta::assert_snapshot!(transcript(stream_body(PLAIN_PROMPT)).await);
}

#[tokio::test]
async fn snapshot_tool_call_transcript() {
    insta::assert_snapshot!(transcript(stream_body(TOOL_PROMPT)).await);
}

#[tokio::test]
async fn snapshot_transcript_with_usage() {
    let mut request = stream_body(PLAIN_PROMPT);
    request["stream_options"] = json!({ "include_usage": true });
    insta::assert_snapshot!(transcript(request).await);
}

#[tokio::test]
async fn snapshot_unknown_route_error() {
    let fixture = fixture(1000);
    let (_, _, text) = send(fixture.app.clone(), "GET", "/v1/nope", None).await;
    insta::assert_snapshot!(pretty(&text));
}

#[tokio::test]
async fn snapshot_unknown_model_error() {
    let fixture = fixture(1000);
    let (_, _, text) = send(fixture.app.clone(), "GET", "/v1/models/nope", None).await;
    insta::assert_snapshot!(pretty(&text));
}

#[tokio::test]
async fn snapshot_malformed_body_error() {
    let fixture = fixture(1000);
    let (_, _, text) = send_raw(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some("{".to_string()),
        true,
    )
    .await;
    insta::assert_snapshot!(pretty(&text));
}
