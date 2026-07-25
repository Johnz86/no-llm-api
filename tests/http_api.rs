//! HTTP surface contract: error envelopes, model catalogue, CORS, parity.

mod support;

use serde_json::json;
use support::{PLAIN_PROMPT, assert_error_envelope, body, fixture, send, send_raw};

#[tokio::test]
async fn unknown_route_answers_with_a_json_error_envelope() {
    let fixture = fixture(1000);
    let (status, headers, text) = send(fixture.app.clone(), "GET", "/v1/nope", None).await;

    assert_eq!(status, 404);
    assert_eq!(
        headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .map(|value| value.starts_with("application/json")),
        Some(true),
        "content-type was {:?}",
        headers.get("content-type")
    );
    let value = assert_error_envelope(&text);
    assert_eq!(value["error"]["code"], "unknown_url");
}

#[tokio::test]
async fn wrong_method_answers_405_with_an_envelope() {
    let fixture = fixture(1000);
    let (status, _, text) = send(fixture.app.clone(), "PUT", "/v1/chat/completions", None).await;
    assert_eq!(status, 405);
    assert_error_envelope(&text);
}

#[tokio::test]
async fn missing_json_content_type_answers_415() {
    let fixture = fixture(1000);
    let (status, _, text) = send_raw(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(body(PLAIN_PROMPT).to_string()),
        false,
    )
    .await;
    assert_eq!(status, 415);
    assert_error_envelope(&text);
}

#[tokio::test]
async fn malformed_json_answers_400() {
    let fixture = fixture(1000);
    let (status, _, text) = send_raw(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some("{not json".to_string()),
        true,
    )
    .await;
    assert_eq!(status, 400);
    assert_error_envelope(&text);
}

#[tokio::test]
async fn empty_messages_answers_400_with_the_offending_param() {
    let fixture = fixture(1000);
    let (status, _, text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(json!({ "model": "m", "messages": [] })),
    )
    .await;
    assert_eq!(status, 400);
    let value = assert_error_envelope(&text);
    assert_eq!(value["error"]["param"], "messages");
}

#[tokio::test]
async fn invalid_order_answers_400() {
    let fixture = fixture(1000);
    let (status, _, text) = send(
        fixture.app.clone(),
        "GET",
        "/v1/chat/completions?order=sideways",
        None,
    )
    .await;
    assert_eq!(status, 400);
    let value = assert_error_envelope(&text);
    assert_eq!(value["error"]["param"], "order");
}

#[tokio::test]
async fn model_catalogue_is_spec_shaped_at_both_mount_points() {
    let fixture = fixture(1000);
    for uri in ["/models", "/v1/models"] {
        let (status, _, text) = send(fixture.app.clone(), "GET", uri, None).await;
        assert_eq!(status, 200, "{uri}");
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["object"], "list", "{uri}");
        let data = value["data"].as_array().unwrap();
        assert!(!data.is_empty(), "{uri} returned an empty catalogue");
        for model in data {
            assert_eq!(model["object"], "model");
            assert!(model["id"].as_str().is_some_and(|id| !id.is_empty()));
            assert!(model["created"].as_i64().is_some());
            assert!(model["owned_by"].as_str().is_some());
            assert_eq!(
                model.as_object().unwrap().len(),
                4,
                "only the four spec fields may be serialised: {model}"
            );
        }
    }
}

#[tokio::test]
async fn single_model_lookup_and_unknown_model_404() {
    let fixture = fixture(1000);
    let (_, _, list) = send(fixture.app.clone(), "GET", "/v1/models", None).await;
    let value: serde_json::Value = serde_json::from_str(&list).unwrap();
    let first = value["data"][0]["id"].as_str().unwrap().to_string();

    let (status, _, text) = send(
        fixture.app.clone(),
        "GET",
        &format!("/v1/models/{first}"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let model: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(model["id"], first.as_str());

    let (status, _, text) = send(fixture.app.clone(), "GET", "/v1/models/nope", None).await;
    assert_eq!(status, 404);
    let value = assert_error_envelope(&text);
    assert_eq!(value["error"]["code"], "model_not_found");
    assert_eq!(value["error"]["param"], "model");
}

#[tokio::test]
async fn health_reports_liveness_without_touching_the_dataset() {
    let fixture = fixture(1000);
    let (status, _, text) = send(fixture.app.clone(), "GET", "/health", None).await;
    assert_eq!(status, 200);
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["status"], "ok");
    assert!(value["models"].as_u64().unwrap() >= 1);
}

#[tokio::test]
async fn index_is_served_from_the_binary_not_the_working_directory() {
    let fixture = fixture(1000);
    let original = std::env::current_dir().unwrap();
    let temp = tempfile::tempdir().unwrap();
    std::env::set_current_dir(temp.path()).unwrap();
    let (status, _, text) = send(fixture.app.clone(), "GET", "/", None).await;
    std::env::set_current_dir(original).unwrap();

    assert_eq!(status, 200);
    assert!(text.contains("<html") || text.contains("<!DOCTYPE"));
}

#[tokio::test]
async fn preflight_mirrors_origin_and_requested_headers() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let fixture = fixture(1000);
    let request = Request::builder()
        .method("OPTIONS")
        .uri("/v1/chat/completions")
        .header("origin", "http://localhost:5173")
        .header("access-control-request-method", "POST")
        .header(
            "access-control-request-headers",
            "authorization,content-type,x-stainless-lang",
        )
        .body(Body::empty())
        .unwrap();

    let response = fixture.app.clone().oneshot(request).await.unwrap();
    let headers = response.headers();
    assert!(response.status().is_success(), "{:?}", response.status());
    assert_eq!(
        headers
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some("http://localhost:5173")
    );
    let allowed = headers
        .get("access-control-allow-headers")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    for header in ["authorization", "content-type", "x-stainless-lang"] {
        assert!(allowed.contains(header), "{header} not in {allowed:?}");
    }
    assert_eq!(
        headers
            .get("access-control-max-age")
            .and_then(|value| value.to_str().ok()),
        Some("600")
    );
}

#[tokio::test]
async fn actual_responses_expose_the_headers_clients_read() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let fixture = fixture(1000);
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/models")
                .header("origin", "http://localhost:5173")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let exposed = response
        .headers()
        .get("access-control-expose-headers")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    for header in ["x-request-id", "retry-after", "x-ratelimit-limit-requests"] {
        assert!(exposed.contains(header), "{header} not in {exposed:?}");
    }
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some("http://localhost:5173")
    );
}

#[tokio::test]
async fn request_id_is_echoed_when_supplied_and_minted_otherwise() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let fixture = fixture(1000);
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/models")
                .header("x-request-id", "req_supplied")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok()),
        Some("req_supplied")
    );

    let (_, headers, _) = send(fixture.app.clone(), "GET", "/v1/models", None).await;
    let minted = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(minted.starts_with("req_"), "minted id was {minted:?}");
}

#[tokio::test]
async fn stored_completions_paginate_and_report_cursors() {
    let fixture = fixture(1000);
    for index in 0..3 {
        let mut request = body(PLAIN_PROMPT);
        request["store"] = json!(true);
        request["metadata"] = json!({ "run": format!("r{index}") });
        let (status, _, _) = send(
            fixture.app.clone(),
            "POST",
            "/v1/chat/completions",
            Some(request),
        )
        .await;
        assert_eq!(status, 200);
    }

    let (status, _, text) = send(
        fixture.app.clone(),
        "GET",
        "/v1/chat/completions?limit=2",
        None,
    )
    .await;
    assert_eq!(status, 200);
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["object"], "list");
    assert_eq!(value["data"].as_array().unwrap().len(), 2);
    assert_eq!(value["has_more"], true);
    assert_eq!(value["first_id"], value["data"][0]["id"]);
    assert_eq!(value["last_id"], value["data"][1]["id"]);

    let (_, _, filtered) = send(
        fixture.app.clone(),
        "GET",
        "/v1/chat/completions?metadata[run]=r1",
        None,
    )
    .await;
    let value: serde_json::Value = serde_json::from_str(&filtered).unwrap();
    assert_eq!(
        value["data"].as_array().unwrap().len(),
        1,
        "metadata filter did not narrow the list: {filtered}"
    );
}

#[tokio::test]
async fn delete_then_get_reports_a_not_found_envelope() {
    let fixture = fixture(1000);
    let mut request = body(PLAIN_PROMPT);
    request["store"] = json!(true);
    let (_, _, text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(request),
    )
    .await;
    let id = serde_json::from_str::<serde_json::Value>(&text).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let (status, _, _) = send(
        fixture.app.clone(),
        "DELETE",
        &format!("/v1/chat/completions/{id}"),
        None,
    )
    .await;
    assert_eq!(status, 200);

    let (status, _, text) = send(
        fixture.app.clone(),
        "GET",
        &format!("/v1/chat/completions/{id}"),
        None,
    )
    .await;
    assert_eq!(status, 404);
    assert_error_envelope(&text);
}

#[tokio::test]
async fn root_and_v1_mount_points_agree() {
    let fixture = fixture(1000);
    for (root, versioned) in [
        ("/chat/completions", "/v1/chat/completions"),
        ("/models", "/v1/models"),
    ] {
        let (root_status, _, _) = send(fixture.app.clone(), "GET", root, None).await;
        let (v1_status, _, _) = send(fixture.app.clone(), "GET", versioned, None).await;
        assert_eq!(root_status, v1_status, "{root} vs {versioned}");
    }
}

#[tokio::test]
async fn messages_pagination_reports_cursors() {
    let fixture = fixture(1000);
    let mut request = body(PLAIN_PROMPT);
    request["store"] = json!(true);
    let (_, _, text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(request),
    )
    .await;
    let id = serde_json::from_str::<serde_json::Value>(&text).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let (status, _, text) = send(
        fixture.app.clone(),
        "GET",
        &format!("/v1/chat/completions/{id}/messages"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["object"], "list");
    assert!(!value["data"].as_array().unwrap().is_empty());
    assert_eq!(value["first_id"], value["data"][0]["id"]);
}

#[tokio::test]
async fn ready_reports_what_a_probe_needs() {
    let fixture = fixture(1000);
    let (status, _, text) = send(fixture.app.clone(), "GET", "/ready", None).await;
    assert_eq!(status, 200);
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["status"], "ready");
    assert!(value["scripts"].as_u64().unwrap() >= 1);
    assert_eq!(value["scenario"], "default");
    assert_eq!(value["tokenizer"], "cl100k_base");
    assert!(value["models"].as_u64().unwrap() >= 1);
}

#[tokio::test]
async fn spec_fields_we_do_not_simulate_are_accepted_and_never_echoed() {
    let fixture = fixture(1000);
    let request = json!({
        "model": "mock-gpt-4o",
        "messages": [{ "role": "user", "content": PLAIN_PROMPT }],
        "logprobs": true,
        "top_logprobs": 3,
        "prediction": { "type": "content", "content": "x" },
        "web_search_options": {},
        "verbosity": "low",
        "prompt_cache_key": "cache-key",
        "safety_identifier": "user-123",
        "functions": [],
        "include_obfuscation": true,
        "n": 1,
        "user": "someone"
    });
    let (status, _, text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(request),
    )
    .await;
    assert_eq!(status, 200, "{text}");
    for field in [
        "logprobs",
        "top_logprobs",
        "prediction",
        "web_search_options",
        "verbosity",
        "prompt_cache_key",
        "safety_identifier",
        "functions",
        "include_obfuscation",
        "x_simulate",
    ] {
        assert!(
            !text.contains(&format!("\"{field}\"")),
            "'{field}' must not be echoed in the response: {text}"
        );
    }
}

#[tokio::test]
async fn unknown_request_fields_do_not_break_the_call() {
    let fixture = fixture(1000);
    let (status, _, _) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(json!({
            "model": "mock-gpt-4o",
            "messages": [{ "role": "user", "content": PLAIN_PROMPT }],
            "some_future_openai_field": { "nested": true }
        })),
    )
    .await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn the_post_response_is_lean_and_the_stored_object_is_enriched() {
    let fixture = fixture(1000);
    let mut request = body(PLAIN_PROMPT);
    request["store"] = json!(true);
    request["temperature"] = json!(0.7);
    request["user"] = json!("tester");
    let (status, _, text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(request),
    )
    .await;
    assert_eq!(status, 200);

    let lean: serde_json::Value = serde_json::from_str(&text).unwrap();
    let keys: Vec<&String> = lean.as_object().unwrap().keys().collect();
    for spec_field in ["id", "object", "created", "model", "choices", "usage"] {
        assert!(
            lean.get(spec_field).is_some(),
            "missing {spec_field}: {text}"
        );
    }
    for echo in [
        "temperature",
        "top_p",
        "request_id",
        "stop",
        "stream_options",
    ] {
        assert!(
            lean.get(echo).is_none(),
            "the lean POST response must not echo '{echo}': {keys:?}"
        );
    }

    let id = lean["id"].as_str().unwrap();
    let (status, _, stored) = send(
        fixture.app.clone(),
        "GET",
        &format!("/v1/chat/completions/{id}"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let stored: serde_json::Value = serde_json::from_str(&stored).unwrap();
    // The spec's own list example carries these, with explicit nulls.
    for field in [
        "request_id",
        "tool_choice",
        "seed",
        "top_p",
        "temperature",
        "presence_penalty",
        "frequency_penalty",
        "system_fingerprint",
        "input_user",
        "service_tier",
        "tools",
        "metadata",
        "response_format",
    ] {
        assert!(
            stored.as_object().unwrap().contains_key(field),
            "the stored object must carry '{field}': {stored}"
        );
    }
    assert_eq!(stored["temperature"], 0.7);
    assert_eq!(stored["input_user"], "tester");
    assert!(stored["tool_choice"].is_null());
}

#[tokio::test]
async fn out_of_range_parameters_answer_400_naming_the_parameter() {
    let fixture = fixture(1000);
    for (field, value, expected) in [
        ("temperature", json!(2.5), "temperature"),
        ("top_p", json!(-0.2), "top_p"),
        ("frequency_penalty", json!(9), "frequency_penalty"),
        ("presence_penalty", json!(-9), "presence_penalty"),
        ("max_completion_tokens", json!(0), "max_completion_tokens"),
        ("top_logprobs", json!(3), "top_logprobs"),
    ] {
        let mut request = body(PLAIN_PROMPT);
        request[field] = value;
        let (status, _, text) = send(
            fixture.app.clone(),
            "POST",
            "/v1/chat/completions",
            Some(request),
        )
        .await;
        assert_eq!(status, 400, "{field} should have been rejected: {text}");
        let envelope = assert_error_envelope(&text);
        assert_eq!(envelope["error"]["param"], expected);
    }
}

#[tokio::test]
async fn valid_boundary_parameters_are_accepted() {
    let fixture = fixture(1000);
    let mut request = body(PLAIN_PROMPT);
    request["temperature"] = json!(0);
    request["top_p"] = json!(1);
    request["frequency_penalty"] = json!(-2);
    request["presence_penalty"] = json!(2);
    request["logprobs"] = json!(true);
    request["top_logprobs"] = json!(20);
    let (status, _, text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(request),
    )
    .await;
    assert_eq!(status, 200, "{text}");
}

#[tokio::test]
async fn wrongly_typed_fields_are_rejected_instead_of_echoed() {
    let fixture = fixture(1000);
    for (field, value) in [
        ("service_tier", json!("turbo")),
        ("reasoning_effort", json!("extreme")),
        ("modalities", json!(["video"])),
        ("response_format", json!({ "type": "yaml" })),
        ("tool_choice", json!("maybe")),
        ("tools", json!([{ "type": "plugin" }])),
        ("logit_bias", json!({ "42": 5000 })),
        ("audio", json!({ "voice": "alloy", "format": "ogg" })),
        ("stop", json!(42)),
    ] {
        let mut request = body(PLAIN_PROMPT);
        request[field] = value;
        let (status, _, text) = send(
            fixture.app.clone(),
            "POST",
            "/v1/chat/completions",
            Some(request),
        )
        .await;
        assert_eq!(status, 400, "'{field}' should have been rejected: {text}");
        assert_error_envelope(&text);
    }
}

#[tokio::test]
async fn well_typed_fields_are_accepted_and_the_stored_object_keeps_them() {
    let fixture = fixture(1000);
    let mut request = body(PLAIN_PROMPT);
    request["store"] = json!(true);
    request["service_tier"] = json!("flex");
    request["reasoning_effort"] = json!("high");
    request["modalities"] = json!(["text"]);
    request["response_format"] = json!({
        "type": "json_schema",
        "json_schema": { "name": "reply", "strict": true, "schema": { "type": "object" } }
    });
    request["stop"] = json!(["END", "STOP"]);
    request["seed"] = json!(-42);
    request["logit_bias"] = json!({ "42": -100 });
    request["tools"] = json!([{
        "type": "function",
        "function": { "name": "get_weather", "parameters": { "type": "object" } }
    }]);
    request["tool_choice"] = json!({ "type": "function", "function": { "name": "get_weather" } });
    request["audio"] = json!({ "voice": "alloy", "format": "pcm16" });

    let (status, _, text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(request),
    )
    .await;
    assert_eq!(status, 200, "{text}");
    let lean: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(lean["service_tier"], "flex");

    let id = lean["id"].as_str().unwrap();
    let (_, _, stored) = send(
        fixture.app.clone(),
        "GET",
        &format!("/v1/chat/completions/{id}"),
        None,
    )
    .await;
    let stored: serde_json::Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(stored["seed"], -42, "a negative seed must round-trip");
    assert_eq!(stored["response_format"]["type"], "json_schema");
    assert_eq!(stored["tools"][0]["function"]["name"], "get_weather");
    assert_eq!(stored["tool_choice"]["function"]["name"], "get_weather");
}

#[tokio::test]
async fn role_specific_message_requirements_are_enforced() {
    let fixture = fixture(1000);
    let (status, _, text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(json!({
            "model": "mock-gpt-4o",
            "messages": [
                { "role": "user", "content": PLAIN_PROMPT },
                { "role": "tool", "content": "42" }
            ]
        })),
    )
    .await;
    assert_eq!(status, 400, "{text}");
    let envelope = assert_error_envelope(&text);
    assert_eq!(envelope["error"]["param"], "messages");

    let (status, _, _) = send(
        fixture.app.clone(),
        "POST",
        "/v1/chat/completions",
        Some(json!({
            "model": "mock-gpt-4o",
            "messages": [
                { "role": "user", "content": PLAIN_PROMPT },
                { "role": "tool", "content": "42", "tool_call_id": "call_alpha" }
            ]
        })),
    )
    .await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn streamed_responses_disable_proxy_buffering() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let fixture = fixture(1000);
    let mut request = body(PLAIN_PROMPT);
    request["stream"] = json!(true);
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from(request.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response
            .headers()
            .get("x-accel-buffering")
            .and_then(|value| value.to_str().ok()),
        Some("no")
    );
    assert!(
        response.headers().get("content-encoding").is_none(),
        "SSE must never be compressed"
    );
}
