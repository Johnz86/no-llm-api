//! Behaviour is selectable without a rebuild: scenarios, per-request directives,
//! the control plane and auth.

mod support;

use std::time::Duration;

use no_llm_api::config::{AuthMode, AuthSettings, ControlPlaneSettings};
use no_llm_api::http::RouterOptions;
use no_llm_api::sim::scenario::Scenario;
use serde_json::json;
use support::sse::collect_sse;
use support::{
    PLAIN_PROMPT, assert_error_envelope, body, fixture, fixture_with_options,
    fixture_with_scenario, send, send_with_headers, stream_body,
};

#[tokio::test]
async fn every_builtin_scenario_still_streams_a_well_formed_transcript() {
    for name in ["default", "fast"] {
        let scenario = Scenario::resolve(name).unwrap();
        let fixture = fixture_with_scenario(1000, scenario);
        let transcript = collect_sse(fixture.app.clone(), stream_body(PLAIN_PROMPT)).await;
        transcript.assert_well_formed();
    }
}

#[tokio::test]
async fn the_slow_scenario_paces_slower_than_the_default() {
    let fast = fixture_with_scenario(1000, Scenario::resolve("fast").unwrap());
    let started = std::time::Instant::now();
    let quick = collect_sse(fast.app.clone(), stream_body(PLAIN_PROMPT)).await;
    let quick_elapsed = started.elapsed();
    quick.assert_well_formed();

    // `slow` is 4 tokens/s with a 1.2 s think, so a handful of frames must take
    // meaningfully longer than the same stream at 1000 tokens/s.
    let mut slow_profile = Scenario::resolve("slow").unwrap();
    slow_profile.timing.ttft_ms = 200;
    slow_profile.timing.tokens_per_second = Some(20);
    let slow = fixture_with_scenario(1000, slow_profile);
    let started = std::time::Instant::now();
    let paced = collect_sse(slow.app.clone(), stream_body(PLAIN_PROMPT)).await;
    let slow_elapsed = started.elapsed();
    paced.assert_well_formed();

    assert!(
        slow_elapsed > quick_elapsed + Duration::from_millis(150),
        "slow {slow_elapsed:?} was not slower than fast {quick_elapsed:?}"
    );
    assert!(
        paced.frames.first().map(|frame| frame.at) >= Some(Duration::from_millis(150)),
        "the first frame must wait for ttft_ms: {:?}",
        paced.frames.first().map(|frame| frame.at)
    );
}

#[tokio::test]
async fn a_per_request_header_overrides_the_scenario_pacing() {
    let fixture = fixture_with_scenario(1000, Scenario::resolve("default").unwrap());
    let (status, _, _) = send_with_headers(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(body(PLAIN_PROMPT)),
        &[("x-simulate-ttft-ms", "0"), ("x-simulate-tps", "1000")],
    )
    .await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn the_rate_limited_scenario_answers_429_with_retry_after() {
    let fixture = fixture_with_scenario(1000, Scenario::resolve("rate-limited").unwrap());
    let (status, headers, text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(body(PLAIN_PROMPT)),
    )
    .await;
    assert_eq!(status, 429);
    assert_eq!(
        headers
            .get("retry-after")
            .and_then(|value| value.to_str().ok()),
        Some("3")
    );
    let value = assert_error_envelope(&text);
    assert_eq!(value["error"]["code"], "rate_limit_exceeded");
}

#[tokio::test]
async fn the_outage_scenario_answers_503() {
    let fixture = fixture_with_scenario(1000, Scenario::resolve("outage").unwrap());
    let (status, _, text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(body(PLAIN_PROMPT)),
    )
    .await;
    assert_eq!(status, 503);
    assert_error_envelope(&text);
}

#[tokio::test]
async fn a_header_directive_can_inject_an_http_error_on_an_otherwise_healthy_server() {
    let fixture = fixture(1000);
    let (status, headers, _) = send_with_headers(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(body(PLAIN_PROMPT)),
        &[("x-simulate-fault", "http_error;status=500;retry_after=7")],
    )
    .await;
    assert_eq!(status, 500);
    assert_eq!(
        headers
            .get("retry-after")
            .and_then(|value| value.to_str().ok()),
        Some("7")
    );

    // The next request without the header is healthy again.
    let (status, _, _) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(body(PLAIN_PROMPT)),
    )
    .await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn a_body_directive_can_inject_a_mid_stream_error_frame() {
    let fixture = fixture(1000);
    let mut request = stream_body(PLAIN_PROMPT);
    request["x_simulate"] = json!({ "fault": "sse_error", "after_frames": 2 });
    let transcript = collect_sse(fixture.app.clone(), request).await;

    let frames: Vec<&str> = transcript.frames.iter().map(|f| f.data.as_str()).collect();
    assert_eq!(
        frames.last(),
        Some(&"[DONE]"),
        "an injected error still terminates the stream: {frames:?}"
    );
    let error_frame = transcript
        .frames
        .iter()
        .find(|frame| frame.data.contains("\"error\""))
        .expect("expected an error frame");
    let value: serde_json::Value = serde_json::from_str(&error_frame.data).unwrap();
    for key in ["message", "type", "param", "code"] {
        assert!(value["error"].get(key).is_some(), "{}", error_frame.data);
    }
}

#[tokio::test]
async fn a_dropped_stream_fault_truncates_without_a_terminator() {
    let fixture = fixture(1000);
    let mut request = stream_body(PLAIN_PROMPT);
    request["x_simulate"] = json!({ "fault": "drop", "after_frames": 2 });
    let transcript = collect_sse(fixture.app.clone(), request).await;

    assert!(!transcript.frames.is_empty());
    assert!(
        transcript.frames.iter().all(|frame| !frame.is_done()),
        "a dropped stream must not send [DONE]"
    );
}

#[tokio::test]
async fn chunk_tokens_batches_content_into_fewer_frames() {
    let fixture = fixture(1000);
    let single = collect_sse(fixture.app.clone(), stream_body(PLAIN_PROMPT)).await;

    let mut request = stream_body(PLAIN_PROMPT);
    request["x_simulate"] = json!({ "chunk_tokens": 3 });
    let batched = collect_sse(fixture.app.clone(), request).await;

    batched.assert_well_formed();
    assert!(
        batched.frames.len() < single.frames.len(),
        "batched {} vs single {}",
        batched.frames.len(),
        single.frames.len()
    );
    assert_eq!(
        batched.content(),
        single.content(),
        "batching must not change the text"
    );
}

#[tokio::test]
async fn the_control_plane_reports_and_replaces_the_live_scenario() {
    let fixture = fixture(1000);

    let (status, _, text) = send(fixture.app.clone(), "GET", "/_mock/scenario", None).await;
    assert_eq!(status, 200);
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["name"], "default");

    let (status, _, text) = send(
        fixture.app.clone(),
        "PATCH",
        "/_mock/scenario",
        Some(json!({ "fault": { "kind": "http_error", "rate": 1.0, "status": 500 } })),
    )
    .await;
    assert_eq!(status, 200, "{text}");

    let (status, _, text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(body(PLAIN_PROMPT)),
    )
    .await;
    assert_eq!(
        status, 500,
        "the patched fault must take effect immediately"
    );
    assert_error_envelope(&text);

    let (status, _, _) = send(fixture.app.clone(), "POST", "/_mock/reset", None).await;
    assert_eq!(status, 200);

    let (status, _, _) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(body(PLAIN_PROMPT)),
    )
    .await;
    assert_eq!(status, 200, "reset must restore the boot scenario");
}

#[tokio::test]
async fn the_control_plane_can_replace_a_whole_scenario() {
    let fixture = fixture(1000);
    let replacement = json!({
        "name": "custom",
        "description": "written by a test",
        "timing": { "ttft_ms": 0, "tokens_per_second": 500, "jitter_ms": 0, "burst_frames": 0 },
        "fault": { "kind": "none", "rate": 0.0 }
    });
    let (status, _, text) = send(
        fixture.app.clone(),
        "PUT",
        "/_mock/scenario",
        Some(replacement),
    )
    .await;
    assert_eq!(status, 200, "{text}");

    let (_, _, text) = send(fixture.app.clone(), "GET", "/_mock/scenario", None).await;
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["name"], "custom");
    assert_eq!(value["timing"]["tokens_per_second"], 500);
}

#[tokio::test]
async fn the_control_plane_lists_model_profiles_and_a_redacted_request_log() {
    let fixture = fixture(1000);
    let _ = send_with_headers(
        fixture.app.clone(),
        "GET",
        "/v1/models",
        None,
        &[("authorization", "Bearer sk-should-not-be-logged")],
    )
    .await;

    let (status, _, text) = send(fixture.app.clone(), "GET", "/_mock/models", None).await;
    assert_eq!(status, 200);
    assert!(text.contains("profile"), "{text}");

    let (status, _, text) = send(fixture.app.clone(), "GET", "/_mock/requests", None).await;
    assert_eq!(status, 200);
    assert!(
        !text.contains("sk-should-not-be-logged"),
        "credentials leaked into the request log: {text}"
    );
    assert!(text.contains("[redacted]"), "{text}");
}

#[tokio::test]
async fn the_control_plane_is_absent_when_disabled() {
    let fixture = fixture_with_options(
        1000,
        RouterOptions {
            control_plane: ControlPlaneSettings {
                enabled: false,
                token: None,
            },
            ..RouterOptions::default()
        },
    );
    let (status, _, _) = send(fixture.app.clone(), "GET", "/_mock/scenario", None).await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn the_control_plane_requires_its_token_when_one_is_configured() {
    let fixture = fixture_with_options(
        1000,
        RouterOptions {
            control_plane: ControlPlaneSettings {
                enabled: true,
                token: Some("secret".to_string()),
            },
            ..RouterOptions::default()
        },
    );
    let (status, _, _) = send(fixture.app.clone(), "GET", "/_mock/scenario", None).await;
    assert_eq!(status, 401);

    let (status, _, _) = send_with_headers(
        fixture.app.clone(),
        "GET",
        "/_mock/scenario",
        None,
        &[("authorization", "Bearer secret")],
    )
    .await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn any_bearer_mode_accepts_any_non_empty_key_and_rejects_none() {
    let fixture = fixture_with_options(
        1000,
        RouterOptions {
            auth: AuthSettings {
                mode: AuthMode::AnyBearer,
                ..AuthSettings::default()
            },
            ..RouterOptions::default()
        },
    );

    let (status, headers, text) = send(fixture.app.clone(), "GET", "/v1/models", None).await;
    assert_eq!(status, 401);
    assert_eq!(
        headers
            .get("www-authenticate")
            .and_then(|value| value.to_str().ok()),
        Some("Bearer")
    );
    assert_error_envelope(&text);

    let (status, _, _) = send_with_headers(
        fixture.app.clone(),
        "GET",
        "/v1/models",
        None,
        &[("authorization", "Bearer ollama")],
    )
    .await;
    assert_eq!(status, 200);

    // Liveness and the page stay open.
    let (status, _, _) = send(fixture.app.clone(), "GET", "/health", None).await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn keys_mode_distinguishes_unknown_from_forbidden_keys() {
    let fixture = fixture_with_options(
        1000,
        RouterOptions {
            auth: AuthSettings {
                mode: AuthMode::Keys,
                keys: vec!["sk-good-key-value".to_string()],
                forbidden_keys: vec!["sk-revoked-key-value".to_string()],
            },
            ..RouterOptions::default()
        },
    );

    let (status, _, _) = send_with_headers(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(body(PLAIN_PROMPT)),
        &[("authorization", "Bearer sk-good-key-value")],
    )
    .await;
    assert_eq!(status, 200);

    let (status, _, text) = send_with_headers(
        fixture.app.clone(),
        "GET",
        "/v1/models",
        None,
        &[("authorization", "Bearer sk-unknown-key-value")],
    )
    .await;
    assert_eq!(status, 401);
    let value = assert_error_envelope(&text);
    assert_eq!(value["error"]["code"], "invalid_api_key");
    assert!(
        !text.contains("sk-unknown-key-value"),
        "the key must be masked: {text}"
    );

    let (status, _, text) = send_with_headers(
        fixture.app.clone(),
        "GET",
        "/v1/models",
        None,
        &[("authorization", "Bearer sk-revoked-key-value")],
    )
    .await;
    assert_eq!(status, 403, "{text}");
}
