//! Wire-level assertions for the streamed chat completion transcript.

mod support;

use std::time::Duration;

use support::sse::collect_sse;
use support::{
    PLAIN_PROMPT, PLAIN_REPLY, REFUSAL_PROMPT, REFUSAL_TEXT, TOOL_PROMPT, UNICODE_PROMPT,
    UNICODE_REPLY, fixture, stream_body,
};

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
async fn opening_frame_is_role_only_and_terminal_frame_is_empty() {
    let fixture = fixture(1000);
    let transcript = collect_sse(fixture.app.clone(), stream_body(PLAIN_PROMPT)).await;
    transcript.assert_well_formed();

    let chunks = transcript.chunks();
    let first = &chunks[0]["choices"][0]["delta"];
    assert_eq!(first["role"], "assistant");
    assert_eq!(first["content"], "", "the opener carries an empty content");

    let last = &chunks[chunks.len() - 1];
    assert_eq!(
        last["choices"][0]["delta"]
            .as_object()
            .expect("delta object")
            .len(),
        0,
        "the terminal frame must be delta: {{}}: {last}"
    );
    assert!(!last["choices"][0]["finish_reason"].is_null());
}

#[tokio::test]
async fn deltas_never_carry_null_members_or_audio() {
    let fixture = fixture(1000);
    let transcript = collect_sse(fixture.app.clone(), stream_body(PLAIN_PROMPT)).await;

    for chunk in transcript.chunks() {
        let delta = chunk["choices"][0]["delta"].as_object().unwrap().clone();
        for (key, value) in delta {
            assert!(
                !value.is_null(),
                "delta member '{key}' was serialised as null: {chunk}"
            );
            assert_ne!(key, "audio", "audio has no place in a delta: {chunk}");
        }
    }
}

#[tokio::test]
async fn chunks_carry_the_service_tier() {
    let fixture = fixture(1000);
    let transcript = collect_sse(fixture.app.clone(), stream_body(PLAIN_PROMPT)).await;
    for chunk in transcript.chunks() {
        assert_eq!(chunk["service_tier"], "default", "{chunk}");
    }
}

#[tokio::test]
async fn tool_calls_stream_as_indexed_fragments() {
    let fixture = fixture(1000);
    let transcript = collect_sse(fixture.app.clone(), stream_body(TOOL_PROMPT)).await;
    transcript.assert_well_formed();

    assert_eq!(transcript.finish_reasons(), vec!["tool_calls".to_string()]);

    let mut openers = 0;
    let mut argument_frames: std::collections::BTreeMap<u64, Vec<String>> = Default::default();
    for chunk in transcript.chunks() {
        let Some(calls) = chunk["choices"][0]["delta"]["tool_calls"].as_array() else {
            continue;
        };
        for call in calls {
            let index = call["index"]
                .as_u64()
                .unwrap_or_else(|| panic!("index is required on every fragment: {chunk}"));
            match call.get("id").and_then(|value| value.as_str()) {
                Some(id) => {
                    openers += 1;
                    assert!(!id.is_empty());
                    assert_eq!(call["type"], "function");
                    assert!(
                        call["function"]["name"].as_str().is_some(),
                        "the opening fragment names the function: {chunk}"
                    );
                    assert!(
                        call["function"].get("arguments").is_none(),
                        "the opening fragment must not send empty arguments: {chunk}"
                    );
                }
                None => {
                    assert!(
                        call.get("type").is_none(),
                        "only the first fragment carries type: {chunk}"
                    );
                    let arguments = call["function"]["arguments"]
                        .as_str()
                        .unwrap_or_else(|| panic!("expected an arguments fragment: {chunk}"));
                    assert!(
                        !arguments.is_empty(),
                        "arguments fragments are never empty: {chunk}"
                    );
                    argument_frames
                        .entry(index)
                        .or_default()
                        .push(arguments.to_string());
                }
            }
        }
    }

    assert_eq!(openers, 2, "both parallel calls must open");
    assert_eq!(argument_frames.len(), 2, "both calls must send arguments");
    for (index, fragments) in &argument_frames {
        assert!(
            fragments.len() >= 2,
            "call {index} arguments must span at least two frames: {fragments:?}"
        );
        let joined = fragments.concat();
        assert!(
            serde_json::from_str::<serde_json::Value>(&joined).is_ok(),
            "call {index} fragments must reassemble into JSON: {joined}"
        );
    }
}

#[tokio::test]
async fn refusals_stream_as_refusal_deltas() {
    let fixture = fixture(1000);
    let transcript = collect_sse(fixture.app.clone(), stream_body(REFUSAL_PROMPT)).await;
    transcript.assert_well_formed();

    let refusal: String = transcript
        .chunks()
        .iter()
        .filter_map(|chunk| {
            chunk["choices"][0]["delta"]["refusal"]
                .as_str()
                .map(str::to_owned)
        })
        .collect();
    assert_eq!(refusal, REFUSAL_TEXT);
    assert_eq!(
        transcript.finish_reasons(),
        vec!["content_filter".to_string()]
    );
}

#[tokio::test]
async fn non_streamed_response_content_is_always_a_string() {
    let fixture = fixture(1000);
    let (status, _, text) = support::send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(serde_json::json!({
            "model": "mock-gpt-4o",
            "messages": [{
                "role": "user",
                "content": [{ "type": "text", "text": PLAIN_PROMPT }]
            }]
        })),
    )
    .await;
    assert_eq!(status, 200);
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(
        value["choices"][0]["message"]["content"].is_string(),
        "message.content must serialise as a string: {text}"
    );
}

#[tokio::test]
async fn stream_null_is_treated_as_not_streaming() {
    let fixture = fixture(1000);
    let (status, headers, _) = support::send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(serde_json::json!({
            "model": "mock-gpt-4o",
            "stream": null,
            "messages": [{ "role": "user", "content": PLAIN_PROMPT }]
        })),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .map(|value| value.starts_with("application/json")),
        Some(true)
    );
}

#[tokio::test]
async fn reasoning_is_separated_from_content_and_counted() {
    let fixture = fixture(1000);
    let (status, _, text) = support::send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(support::body(support::REASONING_PROMPT)),
    )
    .await;
    assert_eq!(status, 200, "{text}");
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    let message = &value["choices"][0]["message"];
    assert_eq!(message["reasoning_content"], support::REASONING_TRACE);
    assert_eq!(message["content"], support::REASONING_ANSWER);
    assert!(
        !message["content"].as_str().unwrap().contains("<think>"),
        "the thinking tag must not leak into content: {text}"
    );
    let reasoning_tokens = value["usage"]["completion_tokens_details"]["reasoning_tokens"]
        .as_u64()
        .expect("reasoning_tokens must be reported");
    assert!(reasoning_tokens > 0);
    assert!(
        value["usage"]["completion_tokens"].as_u64().unwrap() > reasoning_tokens,
        "completion tokens must include both reasoning and content: {text}"
    );
}

#[tokio::test]
async fn reasoning_streams_before_content() {
    let fixture = fixture(1000);
    let transcript = collect_sse(fixture.app.clone(), stream_body(support::REASONING_PROMPT)).await;
    transcript.assert_well_formed();

    let chunks = transcript.chunks();
    let last_reasoning = chunks.iter().rposition(|chunk| {
        chunk["choices"][0]["delta"]
            .get("reasoning_content")
            .is_some()
    });
    let first_content = chunks.iter().position(|chunk| {
        chunk["choices"][0]["delta"]
            .get("content")
            .and_then(|value| value.as_str())
            .is_some_and(|value| !value.is_empty())
    });
    assert!(
        last_reasoning.is_some(),
        "no reasoning frames were streamed"
    );
    assert!(first_content.is_some(), "no content frames were streamed");
    assert!(
        last_reasoning < first_content,
        "reasoning {last_reasoning:?} must precede content {first_content:?}"
    );

    let reasoning: String = chunks
        .iter()
        .filter_map(|chunk| {
            chunk["choices"][0]["delta"]["reasoning_content"]
                .as_str()
                .map(str::to_owned)
        })
        .collect();
    assert_eq!(reasoning, support::REASONING_TRACE);
    assert_eq!(transcript.content(), support::REASONING_ANSWER);
}

#[tokio::test]
async fn multiple_choices_stream_interleaved_with_one_terminal_frame_each() {
    let fixture = fixture(1000);
    let mut request = stream_body(PLAIN_PROMPT);
    request["n"] = serde_json::json!(2);
    let transcript = collect_sse(fixture.app.clone(), request).await;

    let chunks = transcript.chunks();
    assert_eq!(
        transcript.frames.last().map(|frame| frame.data.as_str()),
        Some("[DONE]")
    );

    let mut per_choice_content = std::collections::BTreeMap::<u64, String>::new();
    let mut finishes = std::collections::BTreeMap::<u64, usize>::new();
    for chunk in &chunks {
        let Some(choice) = chunk["choices"].get(0) else {
            continue;
        };
        let index = choice["index"].as_u64().expect("every choice has an index");
        if let Some(text) = choice["delta"]["content"].as_str() {
            per_choice_content.entry(index).or_default().push_str(text);
        }
        if !choice["finish_reason"].is_null() {
            *finishes.entry(index).or_default() += 1;
        }
    }

    assert_eq!(
        per_choice_content.len(),
        2,
        "both choices must stream content: {per_choice_content:?}"
    );
    assert_eq!(
        finishes.values().copied().collect::<Vec<_>>(),
        vec![1, 1],
        "each choice ends exactly once: {finishes:?}"
    );
    assert_eq!(per_choice_content[&0], PLAIN_REPLY);

    // Interleaving: choice 1 must start before choice 0 finishes.
    let first_choice_one = chunks
        .iter()
        .position(|chunk| chunk["choices"][0]["index"] == 1)
        .expect("choice 1 never appeared");
    let last_choice_zero = chunks
        .iter()
        .rposition(|chunk| chunk["choices"][0]["index"] == 0)
        .expect("choice 0 never appeared");
    assert!(
        first_choice_one < last_choice_zero,
        "choices must interleave: {first_choice_one} vs {last_choice_zero}"
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
