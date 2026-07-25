//! Wire-level assertions for the streamed chat completion transcript.

mod support;

use std::time::Duration;

use support::sse::collect_sse;
use support::{PLAIN_PROMPT, PLAIN_REPLY, UNICODE_PROMPT, UNICODE_REPLY, fixture, stream_body};

#[tokio::test]
async fn multi_byte_reply_streams_byte_exact_and_terminates() {
    let fixture = fixture(1000);
    let transcript = collect_sse(fixture.app.clone(), stream_body(UNICODE_PROMPT)).await;

    transcript.assert_well_formed();
    assert_eq!(
        transcript.content(),
        UNICODE_REPLY,
        "concatenated deltas must equal the fixture byte for byte"
    );
    assert_eq!(
        transcript.frames.last().map(|frame| frame.data.as_str()),
        Some("[DONE]")
    );
}

#[tokio::test]
async fn ascii_reply_streams_byte_exact() {
    let fixture = fixture(1000);
    let transcript = collect_sse(fixture.app.clone(), stream_body(PLAIN_PROMPT)).await;

    transcript.assert_well_formed();
    assert_eq!(transcript.content(), PLAIN_REPLY);
}

#[tokio::test]
async fn first_content_chunk_carries_the_role_and_later_ones_do_not() {
    let fixture = fixture(1000);
    let transcript = collect_sse(fixture.app.clone(), stream_body(PLAIN_PROMPT)).await;

    let chunks = transcript.chunks();
    let with_content: Vec<_> = chunks
        .iter()
        .filter(|chunk| {
            chunk["choices"][0]["delta"]
                .get("content")
                .is_some_and(|value| value.is_string())
        })
        .collect();
    assert!(with_content.len() > 1, "expected several content chunks");
    assert_eq!(with_content[0]["choices"][0]["delta"]["role"], "assistant");
    for chunk in &with_content[1..] {
        assert!(
            chunk["choices"][0]["delta"].get("role").is_none()
                || chunk["choices"][0]["delta"]["role"].is_null(),
            "only the first content chunk may carry a role: {chunk}"
        );
    }
}

#[tokio::test]
async fn usage_chunk_is_the_last_chunk_before_done_when_requested() {
    let fixture = fixture(1000);
    let mut body = stream_body(PLAIN_PROMPT);
    body["stream_options"] = serde_json::json!({ "include_usage": true });
    let transcript = collect_sse(fixture.app.clone(), body).await;

    transcript.assert_well_formed();
    let positions = transcript.usage_frame_positions();
    assert_eq!(positions.len(), 1, "exactly one usage frame expected");
    assert_eq!(
        positions[0],
        transcript.chunks().len() - 1,
        "the usage frame must be the final chunk before [DONE]"
    );
    let usage = &transcript.chunks()[positions[0]]["usage"];
    assert!(usage["completion_tokens"].as_u64().unwrap() > 0);
    assert!(
        transcript.chunks()[positions[0]]["choices"]
            .as_array()
            .unwrap()
            .is_empty(),
        "the usage frame carries no choices"
    );
}

#[tokio::test]
async fn no_usage_chunk_when_not_requested() {
    let fixture = fixture(1000);
    let transcript = collect_sse(fixture.app.clone(), stream_body(PLAIN_PROMPT)).await;
    assert!(transcript.usage_frame_positions().is_empty());
}

#[tokio::test]
async fn transcript_carries_no_keep_alive_comments() {
    let fixture = fixture(1000);
    let transcript = collect_sse(fixture.app.clone(), stream_body(PLAIN_PROMPT)).await;
    assert_eq!(
        transcript.comments, 0,
        "keep-alive comments are off by default for fidelity with real traffic"
    );
}

#[tokio::test]
async fn pacing_follows_the_configured_token_rate() {
    let fixture = fixture(20);
    let transcript = collect_sse(fixture.app.clone(), stream_body(PLAIN_PROMPT)).await;

    transcript.assert_well_formed();
    let gap = transcript.median_gap();
    let expected = Duration::from_millis(50);
    assert!(
        gap >= expected / 2 && gap <= expected * 3,
        "median gap {gap:?} is not within tolerance of {expected:?}"
    );
}

#[tokio::test]
async fn dropping_the_stream_cancels_the_simulation() {
    use axum::body::Body;
    use axum::http::Request;
    use futures::StreamExt;
    use tower::ServiceExt;

    let fixture = fixture(2);
    assert_eq!(fixture.cancels.get(), 0);

    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from(stream_body(PLAIN_PROMPT).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    let mut stream = response.into_body().into_data_stream();
    let mut seen = 0;
    while let Some(chunk) = stream.next().await {
        chunk.unwrap();
        seen += 1;
        if seen == 2 {
            break;
        }
    }
    drop(stream);

    let deadline = std::time::Instant::now() + Duration::from_millis(1500);
    while fixture.cancels.get() == 0 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        fixture.cancels.get(),
        1,
        "abandoning the response must register a cancellation"
    );
}
