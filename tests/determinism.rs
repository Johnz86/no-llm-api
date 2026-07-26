//! Determinism: the same request must produce the same bytes, everywhere, always.

mod support;

use std::collections::BTreeSet;

use serde_json::json;
use support::sse::{collect_sse, collect_sse_at};
use support::{PLAIN_PROMPT, TOOL_PROMPT, UNICODE_PROMPT, body, fixture, send, stream_body};

#[tokio::test]
async fn identical_requests_return_identical_bodies() {
    let fixture = fixture(1000);
    let (_, _, first) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(body(PLAIN_PROMPT)),
    )
    .await;
    let (_, _, second) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(body(PLAIN_PROMPT)),
    )
    .await;
    assert_eq!(first, second, "identical requests must be byte-identical");
}

#[tokio::test]
async fn sixty_four_concurrent_identical_requests_agree() {
    let fixture = fixture(1000);
    let mut handles = Vec::new();
    for _ in 0..64 {
        let app = fixture.app.clone();
        handles.push(tokio::spawn(async move {
            let (status, _, text) = send(
                app,
                "POST",
                "/v1/chat/completions",
                Some(body(PLAIN_PROMPT)),
            )
            .await;
            assert_eq!(status, 200);
            text
        }));
    }

    let mut distinct = BTreeSet::new();
    for handle in handles {
        distinct.insert(handle.await.expect("task panicked"));
    }
    assert_eq!(
        distinct.len(),
        1,
        "concurrency produced {} distinct bodies",
        distinct.len()
    );
}

#[tokio::test]
async fn sixty_four_responses_requests_agree_in_sync_and_stream_modes() {
    let fixture = fixture(100_000);
    let body = json!({
        "model": "mock-reasoner",
        "input": "Which release should ship?",
        "reasoning": {"effort": "high", "summary": "auto"},
        "store": false
    });
    let sync = futures::future::join_all((0..64).map(|_| {
        send(
            fixture.app.clone(),
            "POST",
            "/v1/responses",
            Some(body.clone()),
        )
    }))
    .await;
    let sync_bodies: BTreeSet<_> = sync
        .into_iter()
        .map(|(status, _, body)| {
            assert_eq!(status, 200);
            body
        })
        .collect();
    assert_eq!(sync_bodies.len(), 1);

    let mut stream_body = body;
    stream_body["stream"] = json!(true);
    let streams = futures::future::join_all(
        (0..64).map(|_| collect_sse_at(fixture.app.clone(), "/v1/responses", stream_body.clone())),
    )
    .await;
    let transcripts: BTreeSet<_> = streams
        .into_iter()
        .map(|transcript| serde_json::to_string(&transcript.chunks()).unwrap())
        .collect();
    assert_eq!(transcripts.len(), 1);
}

#[tokio::test]
async fn identical_streamed_requests_produce_identical_transcripts() {
    let fixture = fixture(1000);
    for prompt in [PLAIN_PROMPT, UNICODE_PROMPT, TOOL_PROMPT] {
        let first = collect_sse(fixture.app.clone(), stream_body(prompt)).await;
        let second = collect_sse(fixture.app.clone(), stream_body(prompt)).await;
        let first_data: Vec<&str> = first.frames.iter().map(|f| f.data.as_str()).collect();
        let second_data: Vec<&str> = second.frames.iter().map(|f| f.data.as_str()).collect();
        assert_eq!(
            first_data, second_data,
            "streamed output differed between runs for {prompt:?}"
        );
    }
}

#[tokio::test]
async fn different_prompts_produce_different_identities() {
    let fixture = fixture(1000);
    let mut ids = BTreeSet::new();
    for prompt in [PLAIN_PROMPT, UNICODE_PROMPT, TOOL_PROMPT] {
        let (_, _, text) = send(
            fixture.app.clone(),
            "POST",
            "/v1/chat/completions",
            Some(body(prompt)),
        )
        .await;
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        ids.insert(value["id"].as_str().unwrap().to_string());
    }
    assert_eq!(ids.len(), 3, "each prompt must have its own id: {ids:?}");
}

#[tokio::test]
async fn identity_fields_are_plan_derived_and_shaped_like_the_real_api() {
    let fixture = fixture(1000);
    let (_, _, text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(body(PLAIN_PROMPT)),
    )
    .await;
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();

    let id = value["id"].as_str().unwrap();
    assert!(id.starts_with("chatcmpl-"), "{id}");
    let fingerprint = value["system_fingerprint"].as_str().unwrap();
    assert!(fingerprint.starts_with("fp_"), "{fingerprint}");
    assert_eq!(
        fingerprint.len(),
        13,
        "fp_ plus ten hex digits: {fingerprint}"
    );
    assert!(
        value.get("request_id").is_none(),
        "request_id belongs to the stored object, not the lean POST response: {text}"
    );
    assert!(value["created"].as_i64().unwrap() > 1_700_000_000);

    // The stored object carries the derived request_id, as the spec's own list
    // example does.
    let mut stored_request = body(PLAIN_PROMPT);
    stored_request["store"] = json!(true);
    let (_, _, created) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(stored_request),
    )
    .await;
    let stored_id = serde_json::from_str::<serde_json::Value>(&created).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, _, stored) = send(
        fixture.app.clone(),
        "GET",
        &format!("/v1/chat/completions/{stored_id}"),
        None,
    )
    .await;
    let stored: serde_json::Value = serde_json::from_str(&stored).unwrap();
    assert!(stored["request_id"].as_str().unwrap().starts_with("req_"));
    assert_eq!(stored["id"], stored_id.as_str());
}

#[tokio::test]
async fn metadata_changes_the_identity_but_not_the_reply() {
    let fixture = fixture(1000);
    let mut with_metadata = body(PLAIN_PROMPT);
    with_metadata["metadata"] = json!({ "run": "a" });
    let (_, _, plain) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(body(PLAIN_PROMPT)),
    )
    .await;
    let (_, _, tagged) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(with_metadata),
    )
    .await;

    let plain: serde_json::Value = serde_json::from_str(&plain).unwrap();
    let tagged: serde_json::Value = serde_json::from_str(&tagged).unwrap();
    assert_ne!(plain["id"], tagged["id"]);
    assert_eq!(
        plain["choices"][0]["message"]["content"], tagged["choices"][0]["message"]["content"],
        "metadata must not steer fixture selection"
    );
}

#[tokio::test]
async fn the_match_ladder_is_reported_in_a_header() {
    let fixture = fixture(1000);
    let (_, headers, _) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(body(PLAIN_PROMPT)),
    )
    .await;
    assert_eq!(
        headers
            .get("x-simulate-match")
            .and_then(|value| value.to_str().ok()),
        Some("conversation_prefix")
    );

    let (_, headers, _) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(body("something no fixture mentions at all")),
    )
    .await;
    assert_eq!(
        headers
            .get("x-simulate-match")
            .and_then(|value| value.to_str().ok()),
        Some("fallback")
    );
}

#[tokio::test]
async fn off_script_prompts_still_answer_the_same_way_every_time() {
    let fixture = fixture(1000);
    let prompt = "write a title for this conversation";
    let (_, _, first) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(body(prompt)),
    )
    .await;
    let (_, _, second) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(body(prompt)),
    )
    .await;
    assert_eq!(
        first, second,
        "the fallback must not consume a rotating cursor"
    );
}

#[tokio::test]
async fn a_second_unrelated_completion_does_not_disturb_the_first() {
    // LibreChat's titleConvo fires an extra completion; with a rotating cursor it
    // used to steal the answer meant for the conversation.
    let fixture = fixture(1000);
    let (_, _, before) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(body(PLAIN_PROMPT)),
    )
    .await;
    let _ = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(body("write a short title")),
    )
    .await;
    let (_, _, after) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(body(PLAIN_PROMPT)),
    )
    .await;
    assert_eq!(before, after);
}
