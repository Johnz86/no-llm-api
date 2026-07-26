//! Chat rendering of schema-v2 text and reasoning plans.

mod support;

use serde_json::{Value, json};
use std::collections::HashSet;
use support::sse::collect_sse;
use support::{assert_error_envelope, fixture, send, send_with_headers};

fn message(model: &str, text: &str) -> Value {
    json!({
        "model": model,
        "messages": [{"role": "user", "content": text}]
    })
}

#[tokio::test]
async fn explicit_text_case_renders_with_plan_diagnostics() {
    let fixture = fixture(1_000);
    let body = json!({
        "model": "mock-gpt-4o",
        "messages": [
            {"role": "developer", "content": "Answer in one sentence."},
            {"role": "user", "content": "Introduce the simulator."}
        ]
    });
    let (status, headers, text) = send_with_headers(
        fixture.app,
        "POST",
        "/v1/chat/completions",
        Some(body),
        &[("x-simulate-case", "basic-text/concise")],
    )
    .await;

    assert_eq!(status, 200);
    let response: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        response["choices"][0]["message"]["content"],
        "no-llm-api replays deterministic, authored model outcomes for client development."
    );
    assert_eq!(headers["x-simulate-match"], "explicit");
    assert_eq!(headers["x-simulate-case"], "concise");
    assert_eq!(headers["x-simulate-variant"], "default");
    assert_eq!(headers["x-simulate-dataset-revision"].as_bytes().len(), 16);
    assert_eq!(headers["x-simulate-plan-digest"].as_bytes().len(), 16);
}

#[tokio::test]
async fn body_selector_overrides_headers_and_renders_reasoning_effort() {
    let fixture = fixture(1_000);
    let mut body = message("mock-reasoner", "Which release should ship?");
    body["reasoning_effort"] = json!("high");
    body["x_simulate"] = json!({
        "case": "reasoning-effort/release-decision",
        "variant": "high"
    });
    let (status, headers, text) = send_with_headers(
        fixture.app,
        "POST",
        "/v1/chat/completions",
        Some(body),
        &[
            ("x-simulate-case", "basic-text/concise"),
            ("x-simulate-variant", "default"),
        ],
    )
    .await;

    assert_eq!(status, 200);
    let response: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        response["choices"][0]["message"]["reasoning_content"],
        "Compared readiness, blockers, rollback coverage, ownership, and recovery time."
    );
    assert_eq!(
        response["choices"][0]["message"]["content"],
        "Ship release B."
    );
    assert_eq!(
        response["usage"]["completion_tokens_details"]["reasoning_tokens"],
        28
    );
    assert_eq!(headers["x-simulate-case"], "release-decision");
    assert_eq!(headers["x-simulate-variant"], "high");
}

#[tokio::test]
async fn structured_output_keeps_authored_json_bytes() {
    let fixture = fixture(1_000);
    let mut body = message("mock-gpt-4o", "Report release status.");
    body["response_format"] = json!({
        "type": "json_schema",
        "json_schema": {
            "name": "release-status",
            "schema": {
                "type": "object",
                "properties": {
                    "status": {"enum": ["green", "amber", "red"]},
                    "blockers": {"type": "integer", "minimum": 0}
                },
                "required": ["status", "blockers"],
                "additionalProperties": false
            },
            "strict": true
        }
    });
    let (status, _, text) = send_with_headers(
        fixture.app,
        "POST",
        "/v1/chat/completions",
        Some(body),
        &[("x-simulate-case", "structured-output/release-status")],
    )
    .await;

    assert_eq!(status, 200);
    let response: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        response["choices"][0]["message"]["content"],
        r#"{"status":"green","blockers":0}"#
    );
}

#[tokio::test]
async fn structured_output_must_satisfy_the_requested_schema() {
    let fixture = fixture(1_000);
    let mut body = message("mock-gpt-4o", "Report release status.");
    body["response_format"] = json!({
        "type": "json_schema",
        "json_schema": {
            "name": "release-status",
            "schema": {
                "type": "object",
                "properties": {"status": {"const": "red"}},
                "required": ["status"]
            },
            "strict": true
        }
    });
    let (status, _, text) = send_with_headers(
        fixture.app,
        "POST",
        "/v1/chat/completions",
        Some(body),
        &[("x-simulate-case", "structured-output/release-status")],
    )
    .await;

    assert_eq!(status, 400);
    let response = assert_error_envelope(&text);
    assert_eq!(response["error"]["param"], "response_format");
    assert_eq!(response["error"]["code"], "semantic_schema_error");
}

#[tokio::test]
async fn invalid_requested_schema_fails_before_rendering() {
    let fixture = fixture(1_000);
    let mut body = message("mock-gpt-4o", "Report release status.");
    body["store"] = json!(true);
    body["response_format"] = json!({
        "type": "json_schema",
        "json_schema": {
            "name": "release-status",
            "schema": {"type": 17},
            "strict": true
        }
    });
    let (status, _, text) = send_with_headers(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(body),
        &[("x-simulate-case", "structured-output/release-status")],
    )
    .await;

    assert_eq!(status, 400);
    let response = assert_error_envelope(&text);
    assert_eq!(response["error"]["param"], "response_format");
    assert_eq!(response["error"]["code"], "semantic_schema_error");

    let (status, _, text) = send(fixture.app, "GET", "/v1/chat/completions", None).await;
    assert_eq!(status, 200);
    let stored: Value = serde_json::from_str(&text).unwrap();
    assert!(stored["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn semantic_reasoning_stream_uses_existing_sse_contract() {
    let fixture = fixture(1_000);
    let mut body = message("mock-reasoner", "Which release should ship?");
    body["stream"] = json!(true);
    body["reasoning_effort"] = json!("low");
    body["x_simulate"] = json!({"case": "reasoning-effort/release-decision"});
    let transcript = collect_sse(fixture.app, body).await;

    transcript.assert_well_formed();
    assert_eq!(transcript.content(), "Ship release B.");
    let reasoning: String = transcript
        .chunks()
        .iter()
        .filter_map(|chunk| chunk["choices"][0]["delta"]["reasoning_content"].as_str())
        .collect();
    assert_eq!(reasoning, "Checked readiness and blockers.");
}

#[tokio::test]
async fn unknown_semantic_case_uses_the_common_error_envelope() {
    let fixture = fixture(1_000);
    let (status, _, text) = send_with_headers(
        fixture.app,
        "POST",
        "/v1/chat/completions",
        Some(message("mock-gpt-4o", "hello")),
        &[("x-simulate-case", "missing/case")],
    )
    .await;

    assert_eq!(status, 400);
    let response = assert_error_envelope(&text);
    assert_eq!(response["error"]["param"], "x_simulate.case");
    assert_eq!(response["error"]["code"], "semantic_selection_error");
}

#[tokio::test]
async fn incompatible_explicit_variant_is_rejected() {
    let fixture = fixture(1_000);
    let mut body = message("mock-reasoner", "Which release should ship?");
    body["reasoning_effort"] = json!("low");
    let (status, _, text) = send_with_headers(
        fixture.app,
        "POST",
        "/v1/chat/completions",
        Some(body),
        &[
            ("x-simulate-case", "reasoning-effort/release-decision"),
            ("x-simulate-variant", "high"),
        ],
    )
    .await;

    assert_eq!(status, 400);
    let response = assert_error_envelope(&text);
    assert_eq!(response["error"]["param"], "x_simulate.variant");
}

#[tokio::test]
async fn omitted_effort_uses_the_declared_default_variant() {
    let fixture = fixture(1_000);
    let (status, headers, text) = send_with_headers(
        fixture.app,
        "POST",
        "/v1/chat/completions",
        Some(message("mock-reasoner", "Which release should ship?")),
        &[("x-simulate-case", "reasoning-effort/release-decision")],
    )
    .await;

    assert_eq!(status, 200);
    let response: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(headers["x-simulate-variant"], "medium");
    assert_eq!(
        response["usage"]["completion_tokens_details"]["reasoning_tokens"],
        16
    );
}

#[tokio::test]
async fn effort_is_rejected_when_the_model_profile_does_not_support_it() {
    let fixture = fixture(1_000);
    let body = json!({
        "model": "mock-gpt-4o",
        "messages": [
            {"role": "developer", "content": "Answer in one sentence."},
            {"role": "user", "content": "Introduce the simulator."}
        ],
        "reasoning_effort": "high"
    });
    let (status, _, text) = send_with_headers(
        fixture.app,
        "POST",
        "/v1/chat/completions",
        Some(body),
        &[("x-simulate-case", "basic-text/concise")],
    )
    .await;

    assert_eq!(status, 400);
    let response = assert_error_envelope(&text);
    assert_eq!(response["error"]["param"], "reasoning_effort");
    assert_eq!(response["error"]["code"], "semantic_selection_error");
}

#[tokio::test]
async fn requests_without_semantic_selectors_keep_the_legacy_ladder() {
    let fixture = fixture(1_000);
    let (status, headers, text) = send(
        fixture.app,
        "POST",
        "/v1/chat/completions",
        Some(message("mock-gpt-4o", support::PLAIN_PROMPT)),
    )
    .await;

    assert_eq!(status, 200);
    let response: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        response["choices"][0]["message"]["content"],
        support::PLAIN_REPLY
    );
    assert_eq!(headers["x-simulate-match"], "conversation_prefix");
    assert!(headers.get("x-simulate-plan-digest").is_none());
}

#[tokio::test]
async fn semantic_completion_uses_existing_storage_routes() {
    let fixture = fixture(1_000);
    let mut body = message("mock-reasoner", "Which release should ship?");
    body["store"] = json!(true);
    body["reasoning_effort"] = json!("medium");
    body["x_simulate"] = json!({"case": "reasoning-effort/release-decision"});
    let (status, _, text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(body),
    )
    .await;
    assert_eq!(status, 200);
    let created: Value = serde_json::from_str(&text).unwrap();
    let id = created["id"].as_str().unwrap();

    let (status, _, text) = send(
        fixture.app,
        "GET",
        &format!("/v1/chat/completions/{id}"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let stored: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        stored["choices"][0]["message"]["content"],
        "Ship release B."
    );
    assert_eq!(
        stored["choices"][0]["message"]["reasoning_content"],
        "Compared readiness, blockers, and rollback coverage."
    );
}

fn reasoning_stream_with_fault(stage: &str) -> Value {
    json!({
        "model": "mock-reasoner",
        "messages": [{"role": "user", "content": "Which release should ship?"}],
        "stream": true,
        "reasoning_effort": "low",
        "x_simulate": {
            "case": "reasoning-effort/release-decision",
            "fault": "sse_error",
            "stage": stage
        }
    })
}

fn streamed_reasoning(transcript: &support::sse::Transcript) -> String {
    transcript
        .chunks()
        .iter()
        .filter_map(|chunk| chunk["choices"][0]["delta"]["reasoning_content"].as_str())
        .collect()
}

#[tokio::test]
async fn stage_fault_can_fire_before_reasoning() {
    let fixture = fixture(1_000);
    let transcript = collect_sse(fixture.app, reasoning_stream_with_fault("reasoning")).await;

    assert!(streamed_reasoning(&transcript).is_empty());
    assert!(transcript.content().is_empty());
    assert!(
        transcript
            .frames
            .iter()
            .any(|frame| frame.data.contains("server_error"))
    );
    assert_eq!(transcript.frames.last().unwrap().data, "[DONE]");
}

#[tokio::test]
async fn stage_fault_can_fire_between_reasoning_and_output() {
    let fixture = fixture(1_000);
    let transcript = collect_sse(fixture.app, reasoning_stream_with_fault("output")).await;

    assert_eq!(
        streamed_reasoning(&transcript),
        "Checked readiness and blockers."
    );
    assert!(transcript.content().is_empty());
    assert!(
        transcript
            .frames
            .iter()
            .any(|frame| frame.data.contains("server_error"))
    );
}

#[tokio::test]
async fn stage_fault_can_fire_before_the_terminal_chunk() {
    let fixture = fixture(1_000);
    let transcript = collect_sse(fixture.app, reasoning_stream_with_fault("terminal")).await;

    assert_eq!(transcript.content(), "Ship release B.");
    assert!(transcript.finish_reasons().is_empty());
    assert!(
        transcript
            .frames
            .iter()
            .any(|frame| frame.data.contains("server_error"))
    );
    assert_eq!(transcript.frames.last().unwrap().data, "[DONE]");
}

#[tokio::test]
async fn completion_cap_is_spent_on_reasoning_before_visible_output() {
    let fixture = fixture(1_000);
    for (cap, full_reasoning, has_output) in [(4, false, false), (8, true, false), (9, true, true)]
    {
        let mut body = message("mock-reasoner", "Which release should ship?");
        body["reasoning_effort"] = json!("low");
        body["max_completion_tokens"] = json!(cap);
        body["x_simulate"] = json!({"case": "reasoning-effort/release-decision"});
        let (status, _, text) = send(
            fixture.app.clone(),
            "POST",
            "/v1/chat/completions",
            Some(body),
        )
        .await;

        assert_eq!(status, 200);
        let response: Value = serde_json::from_str(&text).unwrap();
        let choice = &response["choices"][0];
        let reasoning = choice["message"]["reasoning_content"].as_str().unwrap();
        assert_eq!(
            reasoning == "Checked readiness and blockers.",
            full_reasoning
        );
        assert_eq!(choice["message"]["content"].is_string(), has_output);
        assert_eq!(choice["finish_reason"], "length");
        assert_eq!(response["usage"]["completion_tokens"], cap);
        assert_eq!(
            response["usage"]["completion_tokens_details"]["reasoning_tokens"],
            cap.min(8)
        );
    }
}

#[tokio::test]
async fn capped_stream_reconstructs_the_capped_completion_and_usage() {
    let fixture = fixture(1_000);
    let mut body = message("mock-reasoner", "Which release should ship?");
    body["reasoning_effort"] = json!("low");
    body["max_completion_tokens"] = json!(9);
    body["x_simulate"] = json!({"case": "reasoning-effort/release-decision"});
    let (status, _, text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, 200);
    let completion: Value = serde_json::from_str(&text).unwrap();

    body["stream"] = json!(true);
    body["stream_options"] = json!({"include_usage": true});
    let transcript = collect_sse(fixture.app, body).await;

    transcript.assert_well_formed();
    assert_eq!(
        transcript.content(),
        completion["choices"][0]["message"]["content"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        streamed_reasoning(&transcript),
        completion["choices"][0]["message"]["reasoning_content"]
            .as_str()
            .unwrap()
    );
    let chunks = transcript.chunks();
    let streamed_usage = &chunks
        .iter()
        .find(|chunk| chunk.get("usage").is_some_and(|usage| !usage.is_null()))
        .unwrap()["usage"];
    assert_eq!(streamed_usage, &completion["usage"]);
}

#[tokio::test]
async fn concurrent_semantic_requests_produce_one_body_and_plan_digest() {
    let fixture = fixture(1_000);
    let body = json!({
        "model": "mock-reasoner",
        "messages": [{"role": "user", "content": "Which release should ship?"}],
        "reasoning_effort": "high"
    });
    let calls = (0..64).map(|_| {
        send_with_headers(
            fixture.app.clone(),
            "POST",
            "/v1/chat/completions",
            Some(body.clone()),
            &[("x-simulate-case", "reasoning-effort/release-decision")],
        )
    });
    let results = futures::future::join_all(calls).await;
    let bodies: HashSet<_> = results.iter().map(|(_, _, body)| body).collect();
    let digests: HashSet<_> = results
        .iter()
        .map(|(status, headers, _)| {
            assert_eq!(*status, 200);
            headers["x-simulate-plan-digest"].to_str().unwrap()
        })
        .collect();

    assert_eq!(bodies.len(), 1);
    assert_eq!(digests.len(), 1);
}

#[tokio::test]
async fn concurrent_semantic_streams_produce_one_transcript() {
    let fixture = fixture(1_000);
    let body = json!({
        "model": "mock-reasoner",
        "messages": [{"role": "user", "content": "Which release should ship?"}],
        "reasoning_effort": "high",
        "stream": true,
        "x_simulate": {"case": "reasoning-effort/release-decision"}
    });
    let calls = (0..64).map(|_| collect_sse(fixture.app.clone(), body.clone()));
    let transcripts: HashSet<_> = futures::future::join_all(calls)
        .await
        .into_iter()
        .map(|transcript| {
            transcript
                .frames
                .into_iter()
                .map(|frame| frame.data)
                .collect::<Vec<_>>()
        })
        .collect();

    assert_eq!(transcripts.len(), 1);
}
