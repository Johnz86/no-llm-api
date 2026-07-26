//! Non-streamed text-first Responses API contract.

mod support;

use serde_json::{Value, json};
use support::sse::collect_sse_at;
use support::{assert_error_envelope, fixture, send, send_with_headers};

#[tokio::test]
async fn explicit_text_plan_renders_a_response_message() {
    let fixture = fixture(1_000);
    let body = json!({
        "model": "mock-gpt-4o",
        "input": "Introduce the simulator."
    });
    let (status, headers, text) = send_with_headers(
        fixture.app,
        "POST",
        "/v1/responses",
        Some(body),
        &[("x-simulate-case", "basic-text/concise")],
    )
    .await;

    assert_eq!(status, 200);
    let response: Value = serde_json::from_str(&text).unwrap();
    assert!(response["id"].as_str().unwrap().starts_with("resp_"));
    assert_eq!(response["object"], "response");
    assert_eq!(response["status"], "completed");
    assert_eq!(response["output"][0]["type"], "message");
    assert_eq!(
        response["output"][0]["content"][0]["text"],
        "no-llm-api replays deterministic, authored model outcomes for client development."
    );
    assert_eq!(headers["x-simulate-match"], "explicit");
    assert_eq!(headers["x-simulate-case"], "concise");
    assert_eq!(headers["x-simulate-plan-digest"].as_bytes().len(), 16);
}

#[tokio::test]
async fn reasoning_summary_is_public_but_chat_trace_is_absent() {
    let fixture = fixture(1_000);
    let body = json!({
        "model": "mock-reasoner",
        "input": "Which release should ship?",
        "reasoning": {"effort": "high", "summary": "auto"}
    });
    let (status, headers, text) = send(fixture.app, "POST", "/v1/responses", Some(body)).await;

    assert_eq!(status, 200);
    let response: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(headers["x-simulate-match"], "exact");
    assert_eq!(response["output"][0]["type"], "reasoning");
    assert_eq!(
        response["output"][0]["summary"][0]["text"],
        "Compared readiness, blockers, rollback coverage, ownership, and recovery time."
    );
    assert_eq!(
        response["output"][1]["content"][0]["text"],
        "Ship release B."
    );
    assert_eq!(
        response["usage"]["output_tokens_details"]["reasoning_tokens"],
        28
    );
    assert!(!text.contains("reasoning_content"));
}

#[tokio::test]
async fn structured_response_preserves_bytes_after_schema_validation() {
    let fixture = fixture(1_000);
    let body = json!({
        "model": "mock-gpt-4o",
        "input": "Report release status.",
        "text": {"format": {
            "type": "json_schema",
            "name": "release-status",
            "strict": true,
            "schema": {
                "type": "object",
                "properties": {
                    "status": {"const": "green"},
                    "blockers": {"type": "integer"}
                },
                "required": ["status", "blockers"],
                "additionalProperties": false
            }
        }}
    });
    let (status, _, text) = send_with_headers(
        fixture.app,
        "POST",
        "/v1/responses",
        Some(body),
        &[("x-simulate-case", "structured-output/release-status")],
    )
    .await;

    assert_eq!(status, 200);
    let response: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        response["output"][0]["content"][0]["text"],
        r#"{"status":"green","blockers":0}"#
    );
    assert_eq!(response["text"]["format"]["type"], "json_schema");
    assert!(response["text"]["format"].get("schema").is_none());
}

#[tokio::test]
async fn schema_mismatch_uses_the_common_error_envelope() {
    let fixture = fixture(1_000);
    let body = json!({
        "model": "mock-gpt-4o",
        "input": "Report release status.",
        "text": {"format": {
            "type": "json_schema",
            "name": "release-status",
            "schema": {"type": "object", "properties": {"status": {"const": "red"}}}
        }},
        "x_simulate": {"case": "structured-output/release-status"}
    });
    let (status, _, text) = send(fixture.app, "POST", "/v1/responses", Some(body)).await;

    assert_eq!(status, 400);
    let response = assert_error_envelope(&text);
    assert_eq!(response["error"]["param"], "text.format");
    assert_eq!(response["error"]["code"], "semantic_schema_error");
}

#[tokio::test]
async fn unsupported_future_controls_fail_explicitly() {
    for (field, value) in [
        ("store", json!(true)),
        ("previous_response_id", json!("resp_parent")),
        ("tools", json!([{"type": "function"}])),
        ("max_output_tokens", json!(10)),
    ] {
        let fixture = fixture(1_000);
        let mut body = json!({
            "model": "mock-reasoner",
            "input": "Which release should ship?"
        });
        body[field] = value;
        let (status, _, text) = send(fixture.app, "POST", "/v1/responses", Some(body)).await;
        assert_eq!(status, 400, "field {field}");
        let response = assert_error_envelope(&text);
        assert_eq!(response["error"]["param"], field);
        assert_eq!(response["error"]["code"], "unsupported_parameter");
    }
}

#[tokio::test]
async fn stream_reconstructs_reasoning_then_text_without_a_done_sentinel() {
    let fixture = fixture(10_000);
    let transcript = collect_sse_at(
        fixture.app,
        "/v1/responses",
        json!({
            "model": "mock-reasoner",
            "input": "Which release should ship?",
            "reasoning": {"effort": "high", "summary": "auto"},
            "stream": true
        }),
    )
    .await;
    let events = transcript.chunks();

    assert!(transcript.frames.iter().all(|frame| !frame.is_done()));
    for (sequence, event) in events.iter().enumerate() {
        assert_eq!(event["sequence_number"], sequence);
    }
    assert_eq!(events[0]["type"], "response.created");
    assert_eq!(events.last().unwrap()["type"], "response.completed");
    let summary_end = events
        .iter()
        .rposition(|event| event["type"] == "response.reasoning_summary_text.delta")
        .unwrap();
    let output_start = events
        .iter()
        .position(|event| event["type"] == "response.output_text.delta")
        .unwrap();
    assert!(summary_end < output_start);
    let text: String = events
        .iter()
        .filter(|event| event["type"] == "response.output_text.delta")
        .map(|event| event["delta"].as_str().unwrap())
        .collect();
    assert_eq!(text, "Ship release B.");
    assert_eq!(
        events.last().unwrap()["response"]["output_text"],
        "Ship release B."
    );
}

#[tokio::test]
async fn both_mount_points_return_identical_response_objects() {
    let fixture = fixture(1_000);
    let body = json!({
        "model": "mock-reasoner",
        "input": "Which release should ship?",
        "reasoning": {"effort": "low", "summary": "auto"}
    });
    let (_, _, root) = send(
        fixture.app.clone(),
        "POST",
        "/responses",
        Some(body.clone()),
    )
    .await;
    let (_, _, versioned) = send(fixture.app, "POST", "/v1/responses", Some(body)).await;

    assert_eq!(root, versioned);
}
