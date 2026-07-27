//! Non-streamed text-first Responses API contract.

mod support;

use no_llm_api::http::RouterOptions;
use no_llm_api::models::{Capabilities, ModelCatalogue, ModelEntry, ModelProfile};
use serde_json::{Value, json};
use support::sse::collect_sse_at;
use support::{
    assert_error_envelope, fixture, fixture_with_options, fixture_with_scenario, send,
    send_with_headers,
};

fn limited_fixture(context_window: u32, max_output_tokens: u32) -> support::Fixture {
    let entry = |id: &str, reasoning: bool| ModelEntry {
        id: id.to_string(),
        created: 1_735_689_600,
        owned_by: "no-llm-api".to_string(),
        profile: ModelProfile {
            context_window: Some(context_window),
            max_output_tokens: Some(max_output_tokens),
            capabilities: Capabilities {
                reasoning,
                reasoning_efforts: if reasoning {
                    ["low", "medium", "high"].map(str::to_string).to_vec()
                } else {
                    Vec::new()
                },
                structured_output: true,
                ..Default::default()
            },
            ..Default::default()
        },
    };
    fixture_with_options(
        100_000,
        RouterOptions {
            models: ModelCatalogue::from_entries(vec![
                entry("mock-gpt-4o", false),
                entry("mock-reasoner", true),
            ]),
            ..RouterOptions::default()
        },
    )
}

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
async fn spec_defaults_empty_text_config_and_instructions_round_trip() {
    let fixture = fixture(10_000);
    let request = json!({
        "model": "mock-gpt-4o",
        "instructions": "Answer in one sentence.",
        "input": "Introduce the simulator.",
        "text": {},
        "x_simulate": {"case": "basic-text/concise"}
    });
    let (status, _, body) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(request.clone()),
    )
    .await;

    assert_eq!(status, 200, "{body}");
    let response: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(response["instructions"], "Answer in one sentence.");
    assert_eq!(response["store"], true);
    assert_eq!(response["parallel_tool_calls"], true);
    assert_eq!(response["text"]["format"]["type"], "text");

    let mut without_instructions = request;
    without_instructions
        .as_object_mut()
        .unwrap()
        .remove("instructions");
    let (_, _, body) = send(
        fixture.app,
        "POST",
        "/v1/responses",
        Some(without_instructions),
    )
    .await;
    let other: Value = serde_json::from_str(&body).unwrap();
    assert_ne!(response["id"], other["id"]);
}

#[tokio::test]
async fn response_metadata_enforces_the_pinned_string_map_limits() {
    let cases = [
        (
            (0..17)
                .map(|index| (format!("key-{index}"), json!("value")))
                .collect::<serde_json::Map<_, _>>(),
            400,
        ),
        (
            [("k".repeat(65), json!("value"))].into_iter().collect(),
            400,
        ),
        (
            [("key".to_string(), json!("v".repeat(513)))]
                .into_iter()
                .collect(),
            400,
        ),
        ([("key".to_string(), json!(42))].into_iter().collect(), 400),
    ];
    for (metadata, expected) in cases {
        let fixture = fixture(10_000);
        let (status, _, body) = send(
            fixture.app,
            "POST",
            "/v1/responses",
            Some(json!({
                "model": "mock-gpt-4o",
                "input": "Introduce the simulator.",
                "metadata": metadata,
                "x_simulate": {"case": "basic-text/concise"}
            })),
        )
        .await;
        assert_eq!(status.as_u16(), expected, "{body}");
        assert_error_envelope(&body);
    }
}

#[tokio::test]
async fn representation_controls_change_resource_identity_not_semantic_selection() {
    let fixture = fixture(100_000);
    let base = json!({
        "model": "mock-reasoner",
        "input": "Which release should ship?",
        "reasoning": {"effort": "high"},
        "store": false
    });
    let variants = [
        base.clone(),
        {
            let mut value = base.clone();
            value["store"] = json!(true);
            value
        },
        {
            let mut value = base.clone();
            value["metadata"] = json!({"suite": "resource-identity"});
            value
        },
        {
            let mut value = base.clone();
            value["parallel_tool_calls"] = json!(false);
            value
        },
        {
            let mut value = base.clone();
            value["reasoning"]["summary"] = json!("auto");
            value
        },
        {
            let mut value = base;
            value["include"] = json!(["reasoning.encrypted_content"]);
            value
        },
    ];

    let mut plan_digests = std::collections::BTreeSet::new();
    let mut resource_ids = std::collections::BTreeSet::new();
    for request in variants {
        let (status, headers, body) =
            send(fixture.app.clone(), "POST", "/v1/responses", Some(request)).await;
        assert_eq!(status, 200, "{body}");
        let response: Value = serde_json::from_str(&body).unwrap();
        plan_digests.insert(
            headers["x-simulate-plan-digest"]
                .to_str()
                .unwrap()
                .to_string(),
        );
        resource_ids.insert(response["id"].as_str().unwrap().to_string());
    }

    assert_eq!(plan_digests.len(), 1);
    assert_eq!(resource_ids.len(), 6);
}

#[tokio::test]
async fn stored_and_stateless_representations_never_share_retrievability() {
    let fixture = fixture(100_000);
    let base = json!({
        "model": "mock-gpt-4o",
        "input": "Introduce the simulator.",
        "x_simulate": {"case": "basic-text/concise"}
    });
    let mut stored_request = base.clone();
    stored_request["store"] = json!(true);
    let (_, stored_headers, stored_body) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(stored_request),
    )
    .await;
    let stored: Value = serde_json::from_str(&stored_body).unwrap();

    let mut stateless_request = base;
    stateless_request["store"] = json!(false);
    let (_, stateless_headers, stateless_body) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(stateless_request),
    )
    .await;
    let stateless: Value = serde_json::from_str(&stateless_body).unwrap();

    assert_eq!(
        stored_headers["x-simulate-plan-digest"],
        stateless_headers["x-simulate-plan-digest"]
    );
    assert_ne!(stored["id"], stateless["id"]);

    let (status, _, retrieved) = send(
        fixture.app.clone(),
        "GET",
        &format!("/v1/responses/{}", stored["id"].as_str().unwrap()),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(serde_json::from_str::<Value>(&retrieved).unwrap(), stored);

    let (status, _, _) = send(
        fixture.app,
        "GET",
        &format!("/v1/responses/{}", stateless["id"].as_str().unwrap()),
        None,
    )
    .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn concurrent_representation_variants_retrieve_their_exact_bodies() {
    let fixture = fixture(100_000);
    let requests = (0..32).map(|index| {
        send(
            fixture.app.clone(),
            "POST",
            "/v1/responses",
            Some(json!({
                "model": "mock-gpt-4o",
                "input": "Introduce the simulator.",
                "metadata": {"variant": index.to_string()},
                "store": true,
                "x_simulate": {"case": "basic-text/concise"}
            })),
        )
    });
    let created = futures::future::join_all(requests).await;
    let mut ids = std::collections::BTreeSet::new();
    for (status, _, body) in created {
        assert_eq!(status, 200, "{body}");
        let response: Value = serde_json::from_str(&body).unwrap();
        assert!(ids.insert(response["id"].as_str().unwrap().to_string()));
        let (status, _, retrieved) = send(
            fixture.app.clone(),
            "GET",
            &format!("/v1/responses/{}", response["id"].as_str().unwrap()),
            None,
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(serde_json::from_str::<Value>(&retrieved).unwrap(), response);
    }
    assert_eq!(ids.len(), 32);
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
    assert!(response["output"][0].get("encrypted_content").is_none());
    assert!(!text.contains("reasoning_content"));
}

#[tokio::test]
async fn encrypted_reasoning_is_opt_in_and_replays_across_fresh_instances() {
    let first = fixture(100_000);
    let request = json!({
        "model": "mock-reasoner",
        "input": "Which release should ship?",
        "reasoning": {"effort": "high", "summary": "auto"},
        "store": false
    });
    let (status, _, default_body) = send(
        first.app.clone(),
        "POST",
        "/v1/responses",
        Some(request.clone()),
    )
    .await;
    assert_eq!(status, 200, "{default_body}");
    let default_response: Value = serde_json::from_str(&default_body).unwrap();
    assert!(
        default_response["output"][0]
            .get("encrypted_content")
            .is_none()
    );

    let mut included_request = request.clone();
    included_request["include"] = json!(["reasoning.encrypted_content"]);
    let (status, _, included_body) = send(
        first.app.clone(),
        "POST",
        "/v1/responses",
        Some(included_request.clone()),
    )
    .await;
    assert_eq!(status, 200, "{included_body}");
    let included: Value = serde_json::from_str(&included_body).unwrap();
    let encrypted = included["output"][0]["encrypted_content"].as_str().unwrap();
    assert!(encrypted.starts_with("enc_v1_"));
    assert!(!included_body.contains("fixture-authored-opaque-reasoning"));
    assert_ne!(default_response["id"], included["id"]);

    let mut streamed_request = included_request;
    streamed_request["stream"] = json!(true);
    let events = collect_sse_at(first.app.clone(), "/v1/responses", streamed_request)
        .await
        .chunks();
    assert_eq!(events.last().unwrap()["response"], included);
    let added = events
        .iter()
        .find(|event| event["type"] == "response.output_item.added")
        .unwrap();
    assert_eq!(added["item"]["encrypted_content"], encrypted);

    let mut default_stream = request;
    default_stream["stream"] = json!(true);
    let default_events = collect_sse_at(first.app, "/v1/responses", default_stream)
        .await
        .chunks();
    assert!(
        !serde_json::to_string(&default_events)
            .unwrap()
            .contains("encrypted_content")
    );

    let second = fixture(100_000);
    let (status, _, replay_body) = send(
        second.app,
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-reasoner",
            "input": [
                included["output"][0].clone(),
                included["output"][1].clone(),
                {"type": "message", "role": "user", "content": "Which release should ship?"}
            ],
            "reasoning": {"effort": "high", "summary": "auto"},
            "store": false,
            "x_simulate": {"case": "reasoning-effort/release-decision"}
        })),
    )
    .await;
    assert_eq!(status, 200, "{replay_body}");
}

#[tokio::test]
async fn opaque_reasoning_and_output_items_replay_as_typed_input() {
    let fixture = fixture(100_000);
    let request = json!({
        "model": "mock-reasoner",
        "input": "Which release should ship?",
        "reasoning": {"effort": "high", "summary": "auto"},
        "include": ["reasoning.encrypted_content"],
        "store": false
    });
    let (status, _, parent_body) =
        send(fixture.app.clone(), "POST", "/v1/responses", Some(request)).await;
    assert_eq!(status, 200, "{parent_body}");
    let parent: Value = serde_json::from_str(&parent_body).unwrap();
    let encrypted = parent["output"][0]["encrypted_content"]
        .as_str()
        .unwrap()
        .to_string();
    let replay_input = json!([
        parent["output"][0].clone(),
        parent["output"][1].clone(),
        {"type": "message", "role": "user", "content": "Which release should ship?"}
    ]);
    let (status, _, replay_body) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-reasoner",
            "input": replay_input,
            "reasoning": {"effort": "high", "summary": "auto"},
            "store": false,
            "x_simulate": {"case": "reasoning-effort/release-decision"}
        })),
    )
    .await;
    assert_eq!(status, 200, "{replay_body}");
    let replay: Value = serde_json::from_str(&replay_body).unwrap();
    assert_eq!(replay["output_text"], parent["output_text"]);
    assert_ne!(replay["id"], parent["id"]);
    assert!(!replay_body.contains(&encrypted));

    let invalid_cases = [
        ("missing", {
            let mut input = replay["output"].clone();
            input[0]
                .as_object_mut()
                .unwrap()
                .remove("encrypted_content");
            input
        }),
        ("corrupt", {
            let mut input = replay["output"].clone();
            input[0]["encrypted_content"] = json!("corrupt");
            input
        }),
        ("wrong-context", {
            let mut input = parent["output"].clone();
            input[1]["id"] = json!("msg_0000000000000000");
            input
        }),
        ("duplicate", {
            let mut input = parent["output"].as_array().unwrap().clone();
            input.insert(1, input[0].clone());
            Value::Array(input)
        }),
    ];
    for (name, input) in invalid_cases {
        let (status, _, body) = send(
            fixture.app.clone(),
            "POST",
            "/v1/responses",
            Some(json!({
                "model": "mock-reasoner",
                "input": input,
                "reasoning": {"effort": "high"},
                "x_simulate": {"case": "reasoning-effort/release-decision"}
            })),
        )
        .await;
        assert_eq!(status, 400, "{name}: {body}");
        assert_eq!(assert_error_envelope(&body)["error"]["param"], "input");
    }
}

#[tokio::test]
async fn reasoning_replay_pairs_are_strictly_adjacent_and_complete() {
    let fixture = fixture(100_000);
    let (_, _, parent_body) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-reasoner",
            "input": "Which release should ship?",
            "reasoning": {"effort": "high", "summary": "auto"},
            "include": ["reasoning.encrypted_content"],
            "store": false
        })),
    )
    .await;
    let parent: Value = serde_json::from_str(&parent_body).unwrap();
    let reasoning = parent["output"][0].clone();
    let message = parent["output"][1].clone();
    let user = json!({"type": "message", "role": "user", "content": "continue"});

    let invalid_cases = [
        (
            "trailing reasoning",
            json!([reasoning.clone()]),
            "replay_context_mismatch",
        ),
        (
            "intervening input",
            json!([reasoning.clone(), user.clone(), message.clone()]),
            "replay_context_mismatch",
        ),
        (
            "consecutive reasoning",
            json!([reasoning.clone(), reasoning.clone(), message.clone()]),
            "replay_context_mismatch",
        ),
        (
            "duplicate assistant",
            json!([message.clone(), message.clone()]),
            "duplicate_replay_item",
        ),
        (
            "partial assistant",
            {
                let mut partial = message.clone();
                partial["status"] = json!("in_progress");
                json!([partial])
            },
            "invalid_replay_item",
        ),
        (
            "raw reasoning",
            {
                let mut raw = reasoning;
                raw["content"] = json!([{
                    "type": "reasoning_text",
                    "text": "private trace"
                }]);
                json!([raw, message])
            },
            "invalid_replay_item",
        ),
    ];

    for (name, input, expected_code) in invalid_cases {
        let (status, _, body) = send(
            fixture.app.clone(),
            "POST",
            "/v1/responses",
            Some(json!({
                "model": "mock-reasoner",
                "input": input,
                "reasoning": {"effort": "high"},
                "x_simulate": {"case": "reasoning-effort/release-decision"}
            })),
        )
        .await;
        assert_eq!(status, 400, "{name}: {body}");
        let error = assert_error_envelope(&body);
        assert_eq!(error["error"]["param"], "input", "{name}");
        assert_eq!(error["error"]["code"], expected_code, "{name}");
    }
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
async fn explicitly_selected_negative_structured_variant_fails_at_runtime() {
    let fixture = fixture(10_000);
    let body = json!({
        "model": "mock-gpt-4o",
        "input": "Report release status.",
        "text": {"format": {
            "type": "json_schema",
            "name": "release-status",
            "schema": {
                "type": "object",
                "properties": {
                    "status": {"type": "string"},
                    "blockers": {"type": "integer"}
                },
                "required": ["status", "blockers"]
            }
        }},
        "x_simulate": {
            "case": "structured-output/release-status",
            "variant": "negative-missing-blockers"
        }
    });
    let (status, _, text) = send(fixture.app, "POST", "/v1/responses", Some(body)).await;

    assert_eq!(status, 400, "{text}");
    let response = assert_error_envelope(&text);
    assert_eq!(response["error"]["param"], "text.format");
    assert_eq!(response["error"]["code"], "semantic_schema_error");
}

#[tokio::test]
async fn unsupported_future_controls_fail_explicitly() {
    for (field, value) in [
        ("tools", json!([{"type": "function"}])),
        ("include", json!(["file_search_call.results"])),
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
async fn stored_responses_are_retrievable_and_deletable() {
    let fixture = fixture(10_000);
    let request = json!({
        "model": "mock-reasoner",
        "input": "Which release should ship?",
        "reasoning": {"effort": "medium", "summary": "auto"},
        "store": true,
        "metadata": {"suite": "persistence"}
    });
    let (status, _, text) = send(fixture.app.clone(), "POST", "/v1/responses", Some(request)).await;
    assert_eq!(status, 200);
    let created: Value = serde_json::from_str(&text).unwrap();
    let id = created["id"].as_str().unwrap();

    let (status, _, retrieved) = send(
        fixture.app.clone(),
        "GET",
        &format!("/v1/responses/{id}"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(serde_json::from_str::<Value>(&retrieved).unwrap(), created);

    let (status, _, deleted) = send(
        fixture.app.clone(),
        "DELETE",
        &format!("/v1/responses/{id}"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_str::<Value>(&deleted).unwrap(),
        json!({"id": id, "object": "response", "deleted": true})
    );

    let (status, _, missing) = send(fixture.app, "GET", &format!("/v1/responses/{id}"), None).await;
    assert_eq!(status, 404);
    assert_eq!(
        assert_error_envelope(&missing)["error"]["type"],
        "invalid_request_error"
    );
}

#[tokio::test]
async fn deleted_predecessors_cannot_be_continued() {
    let fixture = fixture(10_000);
    let (_, _, text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-gpt-4o",
            "input": "Summarize the release in one paragraph.",
            "store": true
        })),
    )
    .await;
    let parent: Value = serde_json::from_str(&text).unwrap();
    let parent_id = parent["id"].as_str().unwrap();
    let (status, _, _) = send(
        fixture.app.clone(),
        "DELETE",
        &format!("/v1/responses/{parent_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200);

    let (status, _, text) = send(
        fixture.app,
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-gpt-4o",
            "input": "Correction: use exactly five words.",
            "previous_response_id": parent_id
        })),
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(
        assert_error_envelope(&text)["error"]["code"],
        "previous_response_not_found"
    );
}

#[tokio::test]
async fn stateless_responses_are_not_retrievable() {
    let fixture = fixture(10_000);
    let (status, _, text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-gpt-4o",
            "input": "Perform the disallowed deployment action.",
            "store": false
        })),
    )
    .await;
    assert_eq!(status, 200);
    let created: Value = serde_json::from_str(&text).unwrap();

    let (status, _, _) = send(
        fixture.app,
        "GET",
        &format!("/v1/responses/{}", created["id"].as_str().unwrap()),
        None,
    )
    .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn stored_predecessor_continues_the_canonical_turn_sequence() {
    let fixture = fixture(10_000);
    let (status, _, parent_text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-gpt-4o",
            "input": "Summarize the release in one paragraph.",
            "store": true
        })),
    )
    .await;
    assert_eq!(status, 200);
    let parent: Value = serde_json::from_str(&parent_text).unwrap();

    let (status, headers, child_text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-gpt-4o",
            "input": "Correction: use exactly five words.",
            "previous_response_id": parent["id"],
            "store": true
        })),
    )
    .await;
    assert_eq!(status, 200);
    let child: Value = serde_json::from_str(&child_text).unwrap();
    assert_eq!(headers["x-simulate-match"], "exact");
    assert_eq!(child["previous_response_id"], parent["id"]);
    assert_eq!(
        child["output_text"],
        "Release validated; deployment is ready."
    );
    assert!(
        child["usage"]["input_tokens"].as_u64().unwrap()
            > parent["usage"]["total_tokens"].as_u64().unwrap()
    );

    let (status, _, retrieved) = send(
        fixture.app,
        "GET",
        &format!("/v1/responses/{}", child["id"].as_str().unwrap()),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(serde_json::from_str::<Value>(&retrieved).unwrap(), child);
}

#[tokio::test]
async fn state_modes_select_the_same_authored_continuation() {
    let fixture = fixture(100_000);
    let (_, _, parent_body) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-gpt-4o",
            "input": "Summarize the release in one paragraph.",
            "store": true
        })),
    )
    .await;
    let parent: Value = serde_json::from_str(&parent_body).unwrap();
    let correction = "Correction: use exactly five words.";
    let (_, _, predecessor_body) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-gpt-4o",
            "input": correction,
            "previous_response_id": parent["id"],
            "store": false
        })),
    )
    .await;
    let predecessor: Value = serde_json::from_str(&predecessor_body).unwrap();

    let (_, _, replay_body) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-gpt-4o",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": "Summarize the release in one paragraph."
                },
                parent["output"][0].clone(),
                {"type": "message", "role": "user", "content": correction}
            ],
            "store": false
        })),
    )
    .await;
    let replay: Value = serde_json::from_str(&replay_body).unwrap();

    let (status, _, conversation_body) = send(
        fixture.app.clone(),
        "POST",
        "/v1/conversations",
        Some(json!({
            "metadata": {"suite": "state-equivalence"},
            "items": [
                {
                    "type": "message",
                    "role": "user",
                    "content": "Summarize the release in one paragraph."
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "content": "The release is ready after validation."
                }
            ]
        })),
    )
    .await;
    assert_eq!(status, 200, "{conversation_body}");
    let conversation: Value = serde_json::from_str(&conversation_body).unwrap();
    let (_, _, conversation_response_body) = send(
        fixture.app,
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-gpt-4o",
            "input": correction,
            "conversation": conversation["id"],
            "store": false
        })),
    )
    .await;
    let conversation_response: Value = serde_json::from_str(&conversation_response_body).unwrap();

    for response in [&predecessor, &replay, &conversation_response] {
        assert_eq!(
            response["output_text"],
            "Release validated; deployment is ready."
        );
    }
}

#[tokio::test]
async fn reasoning_usage_is_equal_across_all_continuation_modes() {
    let fixture = fixture(100_000);
    let original = "Which release should ship?";
    let next = "Which release should ship?";
    let (_, _, parent_body) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-reasoner",
            "input": original,
            "reasoning": {"effort": "high", "summary": "auto"},
            "include": ["reasoning.encrypted_content"],
            "store": true
        })),
    )
    .await;
    let parent: Value = serde_json::from_str(&parent_body).unwrap();

    let (_, _, predecessor_body) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-reasoner",
            "input": next,
            "previous_response_id": parent["id"],
            "reasoning": {"effort": "high", "summary": "auto"},
            "store": false,
            "x_simulate": {"case": "reasoning-effort/release-decision"}
        })),
    )
    .await;
    let predecessor: Value = serde_json::from_str(&predecessor_body).unwrap();

    let history = json!([
        {"type": "message", "role": "user", "content": original},
        parent["output"][0].clone(),
        parent["output"][1].clone(),
        {"type": "message", "role": "user", "content": next}
    ]);
    let (_, _, stateless_body) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-reasoner",
            "input": history,
            "reasoning": {"effort": "high", "summary": "auto"},
            "store": false,
            "x_simulate": {"case": "reasoning-effort/release-decision"}
        })),
    )
    .await;
    let stateless: Value = serde_json::from_str(&stateless_body).unwrap();

    let (_, _, conversation_body) = send(
        fixture.app.clone(),
        "POST",
        "/v1/conversations",
        Some(json!({
            "items": [
                {"type": "message", "role": "user", "content": original},
                parent["output"][0].clone(),
                parent["output"][1].clone()
            ]
        })),
    )
    .await;
    let conversation: Value = serde_json::from_str(&conversation_body).unwrap();
    let (_, _, conversation_response_body) = send(
        fixture.app,
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-reasoner",
            "input": next,
            "conversation": conversation["id"],
            "reasoning": {"effort": "high", "summary": "auto"},
            "store": false,
            "x_simulate": {"case": "reasoning-effort/release-decision"}
        })),
    )
    .await;
    let conversation_response: Value = serde_json::from_str(&conversation_response_body).unwrap();

    let expected_input_tokens = predecessor["usage"]["input_tokens"].clone();
    assert!(expected_input_tokens.as_u64().unwrap() > 28);
    for response in [&predecessor, &stateless, &conversation_response] {
        assert_eq!(response["output_text"], "Ship release B.");
        assert_eq!(response["usage"]["input_tokens"], expected_input_tokens);
    }
}

#[tokio::test]
async fn multi_generation_predecessors_do_not_double_count_prior_input() {
    let fixture = fixture(100_000);
    let input = "Which release should ship?";
    let create = |previous_response_id: Option<Value>| {
        let mut request = json!({
            "model": "mock-reasoner",
            "input": input,
            "reasoning": {"effort": "high", "summary": "auto"},
            "include": ["reasoning.encrypted_content"],
            "store": true,
            "x_simulate": {"case": "reasoning-effort/release-decision"}
        });
        if let Some(previous_response_id) = previous_response_id {
            request["previous_response_id"] = previous_response_id;
        }
        request
    };
    let (_, _, parent_body) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(create(None)),
    )
    .await;
    let parent: Value = serde_json::from_str(&parent_body).unwrap();
    let (_, _, child_body) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(create(Some(parent["id"].clone()))),
    )
    .await;
    let child: Value = serde_json::from_str(&child_body).unwrap();
    let (_, _, grandchild_body) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(create(Some(child["id"].clone()))),
    )
    .await;
    let grandchild: Value = serde_json::from_str(&grandchild_body).unwrap();

    let (_, _, replay_body) = send(
        fixture.app,
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-reasoner",
            "input": [
                {"type": "message", "role": "user", "content": input},
                parent["output"][0].clone(),
                parent["output"][1].clone(),
                {"type": "message", "role": "user", "content": input},
                child["output"][0].clone(),
                child["output"][1].clone(),
                {"type": "message", "role": "user", "content": input}
            ],
            "reasoning": {"effort": "high", "summary": "auto"},
            "store": false,
            "x_simulate": {"case": "reasoning-effort/release-decision"}
        })),
    )
    .await;
    let replay: Value = serde_json::from_str(&replay_body).unwrap();

    assert_eq!(
        grandchild["usage"]["input_tokens"],
        replay["usage"]["input_tokens"]
    );
    assert_eq!(grandchild["output_text"], replay["output_text"]);
}

#[tokio::test]
async fn reasoning_only_predecessors_carry_nonzero_reasoning_usage() {
    let fixture = fixture(100_000);
    let (_, _, parent_body) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-reasoner",
            "input": "Return reasoning without visible output.",
            "reasoning": {"effort": "high", "summary": "auto"},
            "store": true
        })),
    )
    .await;
    let parent: Value = serde_json::from_str(&parent_body).unwrap();
    assert_eq!(
        parent["usage"]["output_tokens_details"]["reasoning_tokens"],
        12
    );

    let (_, _, child_body) = send(
        fixture.app,
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-reasoner",
            "input": "Which release should ship?",
            "previous_response_id": parent["id"],
            "reasoning": {"effort": "high", "summary": "auto"},
            "store": false,
            "x_simulate": {"case": "reasoning-effort/release-decision"}
        })),
    )
    .await;
    let child: Value = serde_json::from_str(&child_body).unwrap();
    assert!(child["usage"]["input_tokens"].as_u64().unwrap() > 12);
}

#[tokio::test]
async fn ordinary_assistant_history_selects_the_authored_continuation() {
    let fixture = fixture(100_000);
    let (status, headers, body) = send(
        fixture.app,
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-gpt-4o",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": "Summarize the release in one paragraph."
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "content": "The release is ready after validation."
                },
                {
                    "type": "message",
                    "role": "user",
                    "content": "Correction: use exactly five words."
                }
            ],
            "store": false
        })),
    )
    .await;

    assert_eq!(status, 200, "{body}");
    assert_eq!(headers["x-simulate-match"], "exact");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["output_text"],
        "Release validated; deployment is ready."
    );
}

#[tokio::test]
async fn assistant_prefill_input_parts_are_accepted_as_history() {
    let fixture = fixture(100_000);
    let (status, _, body) = send(
        fixture.app,
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-gpt-4o",
            "input": [
                {"type": "message", "role": "user", "content": "Introduce the simulator."},
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "input_text", "text": "no-llm-api"}]
                }
            ],
            "store": false,
            "x_simulate": {"case": "basic-text/concise"}
        })),
    )
    .await;

    assert_eq!(status, 200, "{body}");
}

#[tokio::test]
async fn continuation_requires_an_available_stored_predecessor() {
    let fixture = fixture(10_000);
    let (status, _, text) = send(
        fixture.app,
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-gpt-4o",
            "input": "Correction: use exactly five words.",
            "previous_response_id": "resp_missing"
        })),
    )
    .await;

    assert_eq!(status, 404);
    let response = assert_error_envelope(&text);
    assert_eq!(response["error"]["param"], "previous_response_id");
    assert_eq!(response["error"]["code"], "previous_response_not_found");
}

#[tokio::test]
async fn state_expired_scenario_rejects_an_existing_predecessor_reproducibly() {
    let fixture = fixture_with_scenario(
        10_000,
        no_llm_api::sim::scenario::Scenario::resolve("state-expired").unwrap(),
    );
    let (status, _, text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-gpt-4o",
            "input": "Summarize the release in one paragraph.",
            "store": true
        })),
    )
    .await;
    assert_eq!(status, 200);
    let parent: Value = serde_json::from_str(&text).unwrap();
    let parent_id = parent["id"].as_str().unwrap();

    let (status, _, retrieved) = send(
        fixture.app.clone(),
        "GET",
        &format!("/v1/responses/{parent_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(serde_json::from_str::<Value>(&retrieved).unwrap(), parent);

    for _ in 0..2 {
        let (status, _, text) = send(
            fixture.app.clone(),
            "POST",
            "/v1/responses",
            Some(json!({
                "model": "mock-gpt-4o",
                "input": "Correction: use exactly five words.",
                "previous_response_id": parent_id
            })),
        )
        .await;
        assert_eq!(status, 404);
        let error = assert_error_envelope(&text);
        assert_eq!(error["error"]["param"], "previous_response_id");
        assert_eq!(error["error"]["code"], "previous_response_expired");
    }
}

#[tokio::test]
async fn concurrent_identical_continuations_produce_one_immutable_child() {
    let fixture = fixture(10_000);
    let (_, _, parent_text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-gpt-4o",
            "input": "Summarize the release in one paragraph.",
            "store": true
        })),
    )
    .await;
    let parent: Value = serde_json::from_str(&parent_text).unwrap();
    let body = json!({
        "model": "mock-gpt-4o",
        "input": "Correction: use exactly five words.",
        "previous_response_id": parent["id"],
        "store": true
    });
    let tasks = (0..64).map(|_| {
        send(
            fixture.app.clone(),
            "POST",
            "/v1/responses",
            Some(body.clone()),
        )
    });
    let results = futures::future::join_all(tasks).await;
    let bodies: std::collections::BTreeSet<_> = results
        .into_iter()
        .map(|(status, _, body)| {
            assert_eq!(status, 200);
            body
        })
        .collect();

    assert_eq!(bodies.len(), 1);
    let child: Value = serde_json::from_str(bodies.first().unwrap()).unwrap();
    let (status, _, retrieved) = send(
        fixture.app,
        "GET",
        &format!("/v1/responses/{}", child["id"].as_str().unwrap()),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(serde_json::from_str::<Value>(&retrieved).unwrap(), child);
}

#[tokio::test]
async fn one_predecessor_supports_distinct_reproducible_branches() {
    let fixture = fixture(10_000);
    let (_, _, text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-gpt-4o",
            "input": "Summarize the release in one paragraph.",
            "store": true
        })),
    )
    .await;
    let parent: Value = serde_json::from_str(&text).unwrap();
    let parent_id = parent["id"].clone();
    let requests = [
        json!({
            "model": "mock-gpt-4o",
            "input": "Correction: use exactly five words.",
            "previous_response_id": parent_id,
            "store": true
        }),
        json!({
            "model": "mock-gpt-4o",
            "input": "Perform the disallowed deployment action.",
            "previous_response_id": parent_id,
            "store": true
        }),
    ];
    let mut branches = Vec::new();
    for request in requests {
        let (_, _, first) = send(
            fixture.app.clone(),
            "POST",
            "/v1/responses",
            Some(request.clone()),
        )
        .await;
        let (_, _, retry) = send(fixture.app.clone(), "POST", "/v1/responses", Some(request)).await;
        assert_eq!(retry, first);
        branches.push(serde_json::from_str::<Value>(&first).unwrap());
    }

    assert_ne!(branches[0]["id"], branches[1]["id"]);
    assert_eq!(branches[0]["previous_response_id"], parent_id);
    assert_eq!(branches[1]["previous_response_id"], parent_id);
}

#[tokio::test]
async fn state_diagnostics_are_redacted_sorted_and_reset_clears_storage() {
    let fixture = fixture(10_000);
    for input in [
        "Perform the disallowed deployment action.",
        "Summarize the release in one paragraph.",
    ] {
        let (status, _, _) = send(
            fixture.app.clone(),
            "POST",
            "/v1/responses",
            Some(json!({
                "model": "mock-gpt-4o",
                "input": input,
                "store": true,
                "metadata": {"secret": "must-not-appear"}
            })),
        )
        .await;
        assert_eq!(status, 200);
    }

    let (status, _, text) = send(fixture.app.clone(), "GET", "/_mock/responses", None).await;
    assert_eq!(status, 200);
    assert!(!text.contains("must-not-appear"));
    assert!(!text.contains("Perform the disallowed"));
    let state: Value = serde_json::from_str(&text).unwrap();
    let data = state["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
    assert!(data[0]["id"].as_str().unwrap() < data[1]["id"].as_str().unwrap());
    assert!(data.iter().all(|item| item["canonical_turns"] == 2));

    let (status, _, _) = send(fixture.app.clone(), "POST", "/_mock/reset", None).await;
    assert_eq!(status, 200);
    let (_, _, text) = send(fixture.app, "GET", "/_mock/responses", None).await;
    let state: Value = serde_json::from_str(&text).unwrap();
    assert!(state["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn conversation_resource_owns_an_automatically_appended_turn_log() {
    let fixture = fixture(10_000);
    let request = json!({"metadata": {"suite": "conversation-state"}});
    let (status, _, created_text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/conversations",
        Some(request.clone()),
    )
    .await;
    assert_eq!(status, 200);
    let conversation: Value = serde_json::from_str(&created_text).unwrap();
    assert_eq!(conversation["object"], "conversation");
    assert!(conversation["id"].as_str().unwrap().starts_with("conv_"));

    let (_, _, duplicate_text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/conversations",
        Some(request),
    )
    .await;
    assert_eq!(duplicate_text, created_text);

    let conversation_id = conversation["id"].as_str().unwrap();
    let (status, _, parent_text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-gpt-4o",
            "input": "Summarize the release in one paragraph.",
            "conversation": conversation_id
        })),
    )
    .await;
    assert_eq!(status, 200);
    let parent: Value = serde_json::from_str(&parent_text).unwrap();
    assert_eq!(parent["conversation"]["id"], conversation_id);

    let (status, headers, child_text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-gpt-4o",
            "input": "Correction: use exactly five words.",
            "conversation": {"id": conversation_id}
        })),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(headers["x-simulate-match"], "exact");
    let child: Value = serde_json::from_str(&child_text).unwrap();
    assert_eq!(
        child["output_text"],
        "Release validated; deployment is ready."
    );
    assert_eq!(child["conversation"]["id"], conversation_id);
    assert!(
        child["usage"]["input_tokens"].as_u64().unwrap()
            > parent["usage"]["total_tokens"].as_u64().unwrap()
    );

    let (status, _, retrieved) = send(
        fixture.app.clone(),
        "GET",
        &format!("/v1/conversations/{conversation_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(retrieved, created_text);

    let (status, _, deleted) = send(
        fixture.app.clone(),
        "DELETE",
        &format!("/v1/conversations/{conversation_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_str::<Value>(&deleted).unwrap(),
        json!({"id": conversation_id, "object": "conversation.deleted", "deleted": true})
    );

    let (status, _, missing) = send(
        fixture.app,
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-gpt-4o",
            "input": "Correction: use exactly five words.",
            "conversation": conversation_id
        })),
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(
        assert_error_envelope(&missing)["error"]["code"],
        "conversation_not_found"
    );
}

#[tokio::test]
async fn concurrent_conversation_writes_have_one_winner_and_explicit_conflicts() {
    let fixture = fixture(100_000);
    let (_, _, created) = send(
        fixture.app.clone(),
        "POST",
        "/v1/conversations",
        Some(json!({"metadata": {"suite": "concurrency"}})),
    )
    .await;
    let conversation: Value = serde_json::from_str(&created).unwrap();
    let conversation_id = conversation["id"].as_str().unwrap().to_string();
    let requests = (0..64).map(|index| {
        let (input, case) = if index % 2 == 0 {
            ("Introduce the simulator.", "basic-text/concise")
        } else {
            (
                "Perform the disallowed deployment action.",
                "response-refusal/policy-refusal",
            )
        };
        send(
            fixture.app.clone(),
            "POST",
            "/v1/responses",
            Some(json!({
                "model": "mock-gpt-4o",
                "input": input,
                "conversation": conversation_id,
                "x_simulate": {"case": case}
            })),
        )
    });
    let results = futures::future::join_all(requests).await;
    let successes: Vec<_> = results
        .iter()
        .filter(|(status, _, _)| *status == 200)
        .collect();
    assert_eq!(successes.len(), 1);
    for (status, _, body) in results {
        if status == 200 {
            continue;
        }
        assert_eq!(status, 409, "{body}");
        let error = assert_error_envelope(&body);
        assert_eq!(error["error"]["param"], "conversation");
        assert_eq!(error["error"]["code"], "conversation_conflict");
    }

    let (status, _, items) = send(
        fixture.app,
        "GET",
        &format!("/v1/conversations/{conversation_id}/items?order=asc"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_str::<Value>(&items).unwrap()["data"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn sixty_four_identical_conversation_writes_commit_once() {
    let fixture = fixture(100_000);
    let (_, _, created) = send(
        fixture.app.clone(),
        "POST",
        "/v1/conversations",
        Some(json!({"metadata": {"suite": "identical-concurrency"}})),
    )
    .await;
    let conversation: Value = serde_json::from_str(&created).unwrap();
    let conversation_id = conversation["id"].as_str().unwrap().to_string();
    let body = json!({
        "model": "mock-gpt-4o",
        "input": "Introduce the simulator.",
        "conversation": conversation_id,
        "x_simulate": {"case": "basic-text/concise"}
    });
    let results = futures::future::join_all((0..64).map(|_| {
        send(
            fixture.app.clone(),
            "POST",
            "/v1/responses",
            Some(body.clone()),
        )
    }))
    .await;
    let success_bodies: std::collections::BTreeSet<_> = results
        .iter()
        .filter(|(status, _, _)| *status == 200)
        .map(|(_, _, body)| body)
        .collect();
    assert_eq!(success_bodies.len(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|(status, _, _)| *status == 409)
            .count(),
        63
    );
}

#[tokio::test]
async fn sequential_identical_conversation_requests_are_distinct_turns() {
    let fixture = fixture(100_000);
    let (_, _, created) = send(
        fixture.app.clone(),
        "POST",
        "/v1/conversations",
        Some(json!({"metadata": {"suite": "sequential"}})),
    )
    .await;
    let conversation: Value = serde_json::from_str(&created).unwrap();
    let conversation_id = conversation["id"].as_str().unwrap();
    let body = json!({
        "model": "mock-gpt-4o",
        "input": "Introduce the simulator.",
        "conversation": conversation_id,
        "x_simulate": {"case": "basic-text/concise"}
    });
    let mut ids = Vec::new();
    for _ in 0..2 {
        let (status, _, response) = send(
            fixture.app.clone(),
            "POST",
            "/v1/responses",
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, 200, "{response}");
        ids.push(
            serde_json::from_str::<Value>(&response).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    assert_ne!(ids[0], ids[1]);
}

#[tokio::test]
async fn conversation_and_predecessor_linkage_are_mutually_exclusive() {
    let fixture = fixture(10_000);
    let (status, _, text) = send(
        fixture.app,
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-gpt-4o",
            "input": "hello",
            "conversation": "conv_test",
            "previous_response_id": "resp_test"
        })),
    )
    .await;

    assert_eq!(status, 400);
    let error = assert_error_envelope(&text);
    assert_eq!(error["error"]["param"], "conversation");
    assert_eq!(error["error"]["code"], "invalid_value");
}

#[tokio::test]
async fn initial_conversation_items_seed_semantic_history() {
    let fixture = fixture(10_000);
    let (status, _, text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/conversations",
        Some(json!({
            "items": [
                {
                    "type": "message",
                    "role": "user",
                    "content": "Summarize the release in one paragraph."
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "content": "The release is ready after validation."
                }
            ]
        })),
    )
    .await;
    assert_eq!(status, 200);
    let conversation: Value = serde_json::from_str(&text).unwrap();

    let (status, headers, response) = send(
        fixture.app,
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-gpt-4o",
            "input": "Correction: use exactly five words.",
            "conversation": conversation["id"]
        })),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(headers["x-simulate-match"], "exact");
    assert_eq!(
        serde_json::from_str::<Value>(&response).unwrap()["output_text"],
        "Release validated; deployment is ready."
    );
}

#[tokio::test]
async fn conversation_items_support_deterministic_crud_and_pagination() {
    let fixture = fixture(10_000);
    let (_, _, text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/conversations",
        Some(json!({
            "items": [
                {"type": "message", "role": "user", "content": "First"},
                {"type": "message", "role": "assistant", "content": "Second"}
            ]
        })),
    )
    .await;
    let conversation: Value = serde_json::from_str(&text).unwrap();
    let conversation_id = conversation["id"].as_str().unwrap();
    let items_path = format!("/v1/conversations/{conversation_id}/items");

    let (status, _, text) = send(
        fixture.app.clone(),
        "GET",
        &format!("{items_path}?order=asc"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let ascending: Value = serde_json::from_str(&text).unwrap();
    let initial = ascending["data"].as_array().unwrap();
    assert_eq!(initial.len(), 2);
    assert_eq!(initial[0]["content"][0]["text"], "First");
    assert_eq!(initial[1]["content"][0]["text"], "Second");
    assert_eq!(ascending["first_id"], initial[0]["id"]);
    assert_eq!(ascending["last_id"], initial[1]["id"]);
    assert_eq!(ascending["has_more"], false);

    let (_, _, text) = send(
        fixture.app.clone(),
        "GET",
        &format!("{items_path}?limit=1"),
        None,
    )
    .await;
    let first_page: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(first_page["data"][0]["id"], initial[1]["id"]);
    assert_eq!(first_page["has_more"], true);
    let cursor = first_page["last_id"].as_str().unwrap();

    let (_, _, text) = send(
        fixture.app.clone(),
        "GET",
        &format!("{items_path}?limit=1&after={cursor}"),
        None,
    )
    .await;
    let second_page: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(second_page["data"][0]["id"], initial[0]["id"]);
    assert_eq!(second_page["has_more"], false);

    let added_request = json!({
        "items": [{"type": "message", "role": "user", "content": "Third"}]
    });
    let (_, _, first_add) = send(
        fixture.app.clone(),
        "POST",
        &items_path,
        Some(added_request.clone()),
    )
    .await;
    let (_, _, retry_add) = send(
        fixture.app.clone(),
        "POST",
        &items_path,
        Some(added_request),
    )
    .await;
    assert_eq!(retry_add, first_add);
    let added: Value = serde_json::from_str(&first_add).unwrap();
    let added_id = added["data"][0]["id"].as_str().unwrap();

    let (status, _, retrieved) = send(
        fixture.app.clone(),
        "GET",
        &format!("{items_path}/{added_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_str::<Value>(&retrieved).unwrap(),
        added["data"][0]
    );

    let (_, _, text) = send(
        fixture.app.clone(),
        "GET",
        &format!("{items_path}?order=asc"),
        None,
    )
    .await;
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap()["data"]
            .as_array()
            .unwrap()
            .len(),
        3
    );

    let (status, _, deleted) = send(
        fixture.app.clone(),
        "DELETE",
        &format!("{items_path}/{added_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_str::<Value>(&deleted).unwrap(),
        conversation
    );
    let (status, _, _) = send(
        fixture.app,
        "GET",
        &format!("{items_path}/{added_id}"),
        None,
    )
    .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn responses_append_typed_conversation_items_and_deleted_turns_leave_context() {
    let fixture = fixture(10_000);
    let (_, _, text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/conversations",
        Some(json!({
            "items": [
                {"type": "message", "role": "user", "content": "Summarize the release in one paragraph."},
                {"type": "message", "role": "assistant", "content": "The release is ready after validation."}
            ]
        })),
    )
    .await;
    let conversation: Value = serde_json::from_str(&text).unwrap();
    let conversation_id = conversation["id"].as_str().unwrap();
    let items_path = format!("/v1/conversations/{conversation_id}/items");

    let (_, _, text) = send(
        fixture.app.clone(),
        "GET",
        &format!("{items_path}?order=asc"),
        None,
    )
    .await;
    let initial: Value = serde_json::from_str(&text).unwrap();
    let assistant_id = initial["data"][1]["id"].as_str().unwrap();
    let (status, _, _) = send(
        fixture.app.clone(),
        "DELETE",
        &format!("{items_path}/{assistant_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200);

    let (status, headers, _) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-gpt-4o",
            "input": "Correction: use exactly five words.",
            "conversation": conversation_id
        })),
    )
    .await;
    assert_eq!(status, 200);
    assert_ne!(headers["x-simulate-match"], "exact");

    let (_, _, text) = send(fixture.app, "GET", &format!("{items_path}?order=asc"), None).await;
    let items: Value = serde_json::from_str(&text).unwrap();
    let data = items["data"].as_array().unwrap();
    assert_eq!(data.len(), 3);
    assert_eq!(data[1]["type"], "message");
    assert_eq!(data[1]["role"], "user");
    assert_eq!(data[2]["type"], "message");
    assert_eq!(data[2]["role"], "assistant");
}

#[tokio::test]
async fn conversation_item_batches_enforce_the_supported_size() {
    for items in [
        json!([]),
        json!(vec![
            json!({
                "type": "message",
                "role": "user",
                "content": "overflow"
            });
            21
        ]),
    ] {
        let fixture = fixture(10_000);
        let (_, _, text) = send(
            fixture.app.clone(),
            "POST",
            "/v1/conversations",
            Some(json!({})),
        )
        .await;
        let conversation: Value = serde_json::from_str(&text).unwrap();
        let (status, _, text) = send(
            fixture.app,
            "POST",
            &format!(
                "/v1/conversations/{}/items",
                conversation["id"].as_str().unwrap()
            ),
            Some(json!({"items": items})),
        )
        .await;
        assert_eq!(status, 400);
        let error = assert_error_envelope(&text);
        assert_eq!(error["error"]["param"], "items");
        assert_eq!(error["error"]["code"], "invalid_value");
    }
}

#[tokio::test]
async fn reasoning_responses_append_input_reasoning_and_output_items() {
    let fixture = fixture(10_000);
    let (_, _, text) = send(
        fixture.app.clone(),
        "POST",
        "/v1/conversations",
        Some(json!({})),
    )
    .await;
    let conversation: Value = serde_json::from_str(&text).unwrap();
    let conversation_id = conversation["id"].as_str().unwrap();

    let (status, _, _) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-reasoner",
            "input": "Which release should ship?",
            "reasoning": {"effort": "high", "summary": "auto"},
            "conversation": conversation_id
        })),
    )
    .await;
    assert_eq!(status, 200);

    let (_, _, text) = send(
        fixture.app,
        "GET",
        &format!("/v1/conversations/{conversation_id}/items?order=asc"),
        None,
    )
    .await;
    let items: Value = serde_json::from_str(&text).unwrap();
    let data = items["data"].as_array().unwrap();
    assert_eq!(data.len(), 3);
    assert_eq!(data[0]["type"], "message");
    assert_eq!(data[0]["role"], "user");
    assert_eq!(data[1]["type"], "reasoning");
    assert_eq!(data[2]["type"], "message");
    assert_eq!(data[2]["role"], "assistant");
}

#[tokio::test]
async fn output_budget_is_spent_on_reasoning_before_visible_text() {
    for (limit, expected_reasoning, expect_text) in
        [(16, 16, false), (28, 28, false), (30, 28, true)]
    {
        let fixture = fixture(10_000);
        let transcript = collect_sse_at(
            fixture.app,
            "/v1/responses",
            json!({
                "model": "mock-reasoner",
                "input": "Which release should ship?",
                "reasoning": {"effort": "high", "summary": "auto"},
                "max_output_tokens": limit,
                "stream": true
            }),
        )
        .await;
        let events = transcript.chunks();
        let terminal = &events.last().unwrap()["response"];
        let output_text = terminal["output_text"].as_str().unwrap();

        assert_eq!(events.last().unwrap()["type"], "response.incomplete");
        assert_eq!(terminal["status"], "incomplete");
        assert_eq!(
            terminal["incomplete_details"]["reason"],
            "max_output_tokens"
        );
        assert_eq!(terminal["max_output_tokens"], limit);
        assert_eq!(
            terminal["usage"]["output_tokens_details"]["reasoning_tokens"],
            expected_reasoning
        );
        assert_eq!(terminal["usage"]["output_tokens"], limit);
        assert_eq!(!output_text.is_empty(), expect_text);
        assert!("Ship release B.".starts_with(output_text));

        let streamed_text: String = events
            .iter()
            .filter(|event| event["type"] == "response.output_text.delta")
            .map(|event| event["delta"].as_str().unwrap())
            .collect();
        assert_eq!(streamed_text, output_text);
    }
}

#[tokio::test]
async fn output_budget_below_the_schema_minimum_is_rejected() {
    let fixture = fixture(10_000);
    let (status, _, text) = send(
        fixture.app,
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-reasoner",
            "input": "Which release should ship?",
            "max_output_tokens": 15
        })),
    )
    .await;

    assert_eq!(status, 400);
    let response = assert_error_envelope(&text);
    assert_eq!(response["error"]["param"], "max_output_tokens");
    assert_eq!(response["error"]["code"], "invalid_value");
}

#[tokio::test]
async fn model_output_ceiling_rejects_oversized_caps_and_bounds_defaults() {
    let fixture = limited_fixture(1_000, 16);
    let base = json!({
        "model": "mock-reasoner",
        "input": "Which release should ship?",
        "reasoning": {"effort": "high", "summary": "auto"},
        "store": false
    });
    let mut oversized = base.clone();
    oversized["max_output_tokens"] = json!(17);
    let (status, _, body) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(oversized),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    let error = assert_error_envelope(&body);
    assert_eq!(error["error"]["param"], "max_output_tokens");
    assert_eq!(error["error"]["code"], "max_output_tokens_exceeded");

    let mut boundary = base.clone();
    boundary["max_output_tokens"] = json!(16);
    let (status, _, body) =
        send(fixture.app.clone(), "POST", "/v1/responses", Some(boundary)).await;
    assert_eq!(status, 200, "{body}");

    let (status, _, default_body) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(base.clone()),
    )
    .await;
    assert_eq!(status, 200, "{default_body}");
    let response: Value = serde_json::from_str(&default_body).unwrap();
    assert_eq!(response["status"], "incomplete");
    assert_eq!(response["max_output_tokens"], Value::Null);
    assert_eq!(response["usage"]["output_tokens"], 16);
    assert_eq!(
        response["incomplete_details"]["reason"],
        "max_output_tokens"
    );

    let mut streamed = base;
    streamed["stream"] = json!(true);
    let events = collect_sse_at(fixture.app, "/v1/responses", streamed)
        .await
        .chunks();
    assert_eq!(events.last().unwrap()["type"], "response.incomplete");
    assert_eq!(events.last().unwrap()["response"], response);
}

#[tokio::test]
async fn model_context_window_accepts_the_boundary_and_rejects_excess() {
    let fixture = limited_fixture(5, 64);
    let base = json!({
        "model": "mock-reasoner",
        "input": "Which release should ship?",
        "reasoning": {"effort": "high", "summary": "auto"},
        "store": false
    });
    let (status, _, body) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(base.clone()),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["usage"]["input_tokens"],
        5
    );

    let mut oversized = base;
    oversized["input"] = json!("Which release should ship? Add detail.");
    oversized["x_simulate"] = json!({"case": "reasoning-effort/release-decision"});
    let (status, _, body) = send(fixture.app, "POST", "/v1/responses", Some(oversized)).await;
    assert_eq!(status, 400, "{body}");
    let error = assert_error_envelope(&body);
    assert_eq!(error["error"]["param"], "input");
    assert_eq!(error["error"]["code"], "context_length_exceeded");
}

#[tokio::test]
async fn oversized_state_modes_fail_identically_without_mutating_state() {
    let fixture = limited_fixture(10, 64);
    let original = "Introduce the simulator.";
    let (_, _, parent_body) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(json!({
            "model": "mock-gpt-4o",
            "input": original,
            "store": true,
            "x_simulate": {"case": "basic-text/concise"}
        })),
    )
    .await;
    let parent: Value = serde_json::from_str(&parent_body).unwrap();
    assert_eq!(fixture.state.responses.snapshot().await.len(), 1);

    let (_, _, conversation_body) = send(
        fixture.app.clone(),
        "POST",
        "/v1/conversations",
        Some(json!({
            "items": [
                {"type": "message", "role": "user", "content": original},
                parent["output"][0].clone()
            ]
        })),
    )
    .await;
    let conversation: Value = serde_json::from_str(&conversation_body).unwrap();

    let requests = [
        json!({
            "model": "mock-gpt-4o",
            "input": original,
            "previous_response_id": parent["id"],
            "store": true
        }),
        json!({
            "model": "mock-gpt-4o",
            "input": [
                {"type": "message", "role": "user", "content": original},
                parent["output"][0].clone(),
                {"type": "message", "role": "user", "content": original}
            ],
            "store": true
        }),
        json!({
            "model": "mock-gpt-4o",
            "input": original,
            "conversation": conversation["id"],
            "store": true
        }),
    ];
    let mut errors = Vec::new();
    for request in requests {
        let (status, _, body) =
            send(fixture.app.clone(), "POST", "/v1/responses", Some(request)).await;
        assert_eq!(status, 400, "{body}");
        let error = assert_error_envelope(&body);
        assert_eq!(error["error"]["param"], "input");
        assert_eq!(error["error"]["code"], "context_length_exceeded");
        errors.push(body);
    }
    assert_eq!(errors[0], errors[1]);
    assert_eq!(errors[1], errors[2]);
    assert_eq!(fixture.state.responses.snapshot().await.len(), 1);

    let (status, _, items) = send(
        fixture.app,
        "GET",
        &format!(
            "/v1/conversations/{}/items?order=asc",
            conversation["id"].as_str().unwrap()
        ),
        None,
    )
    .await;
    assert_eq!(status, 200, "{items}");
    assert_eq!(
        serde_json::from_str::<Value>(&items).unwrap()["data"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
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
async fn terminal_lifecycle_variants_use_response_and_item_specific_statuses() {
    for (input, terminal, item_status) in [
        (
            "Return a partial deployment summary.",
            "response.incomplete",
            "incomplete",
        ),
        (
            "Simulate a failed deployment summary.",
            "response.failed",
            "incomplete",
        ),
        (
            "Simulate a cancelled deployment summary.",
            "response.cancelled",
            "incomplete",
        ),
    ] {
        let fixture = fixture(10_000);
        let transcript = collect_sse_at(
            fixture.app,
            "/v1/responses",
            json!({
                "model": "mock-gpt-4o",
                "input": input,
                "stream": true
            }),
        )
        .await;
        let events = transcript.chunks();
        let terminal_event = events.last().unwrap();

        assert_eq!(terminal_event["type"], terminal);
        assert_eq!(terminal_event["response"]["status"], &terminal[9..]);
        assert_eq!(
            terminal_event["response"]["output"][0]["status"],
            item_status
        );
        assert!(terminal_event["response"]["completed_at"].is_null());
        match terminal {
            "response.incomplete" => assert_eq!(
                terminal_event["response"]["incomplete_details"]["reason"],
                "max_output_tokens"
            ),
            "response.failed" => {
                assert_eq!(terminal_event["response"]["error"]["code"], "server_error")
            }
            "response.cancelled" => {
                assert!(terminal_event["response"]["error"].is_null());
                assert!(terminal_event["response"]["incomplete_details"].is_null());
            }
            _ => unreachable!(),
        }
    }
}

#[tokio::test]
async fn staged_error_and_drop_faults_end_without_a_response_terminal_event() {
    for (fault, expected_last) in [("sse_error", Some("error")), ("drop", None)] {
        let fixture = fixture(10_000);
        let transcript = collect_sse_at(
            fixture.app,
            "/v1/responses",
            json!({
                "model": "mock-reasoner",
                "input": "Which release should ship?",
                "reasoning": {"effort": "high", "summary": "auto"},
                "stream": true,
                "x_simulate": {"fault": fault, "stage": "output"}
            }),
        )
        .await;
        let events = transcript.chunks();

        assert!(!events.is_empty());
        assert!(events.iter().all(|event| {
            !matches!(
                event["type"].as_str(),
                Some(
                    "response.completed"
                        | "response.incomplete"
                        | "response.failed"
                        | "response.cancelled"
                )
            )
        }));
        if let Some(expected_last) = expected_last {
            assert_eq!(events.last().unwrap()["type"], expected_last);
            assert_eq!(events.last().unwrap()["code"], "server_error");
        } else {
            assert_ne!(events.last().unwrap()["type"], "error");
        }
    }
}

#[tokio::test]
async fn reasoning_output_terminal_and_reserved_tool_fault_stages_are_distinct() {
    let cases = [
        (
            "reasoning",
            json!({
                "model": "mock-reasoner",
                "input": "Which release should ship?",
                "reasoning": {"effort": "high", "summary": "auto"},
                "stream": true,
                "store": false,
                "x_simulate": {"fault": "sse_error", "stage": "reasoning"}
            }),
            "error",
        ),
        (
            "output",
            json!({
                "model": "mock-gpt-4o",
                "input": "Introduce the simulator.",
                "stream": true,
                "store": false,
                "x_simulate": {
                    "case": "basic-text/concise",
                    "fault": "sse_error",
                    "stage": "output"
                }
            }),
            "error",
        ),
        (
            "terminal",
            json!({
                "model": "mock-gpt-4o",
                "input": "Introduce the simulator.",
                "stream": true,
                "store": false,
                "x_simulate": {
                    "case": "basic-text/concise",
                    "fault": "sse_error",
                    "stage": "terminal"
                }
            }),
            "error",
        ),
        (
            "tool-reserved",
            json!({
                "model": "mock-gpt-4o",
                "input": "Introduce the simulator.",
                "stream": true,
                "store": false,
                "x_simulate": {
                    "case": "basic-text/concise",
                    "fault": "sse_error",
                    "stage": "tool"
                }
            }),
            "response.completed",
        ),
    ];
    for (name, request, expected_last) in cases {
        let fixture = fixture(100_000);
        let events = collect_sse_at(fixture.app, "/v1/responses", request)
            .await
            .chunks();
        assert_eq!(events.last().unwrap()["type"], expected_last, "{name}");
    }
}

#[tokio::test]
async fn refusal_events_reconstruct_the_authored_refusal() {
    let fixture = fixture(10_000);
    let transcript = collect_sse_at(
        fixture.app,
        "/v1/responses",
        json!({
            "model": "mock-gpt-4o",
            "input": "Perform the disallowed deployment action.",
            "stream": true
        }),
    )
    .await;
    let events = transcript.chunks();
    let refusal: String = events
        .iter()
        .filter(|event| event["type"] == "response.refusal.delta")
        .map(|event| event["delta"].as_str().unwrap())
        .collect();

    assert_eq!(
        refusal,
        "I cannot perform that action, but I can help review a safe deployment plan."
    );
    assert_eq!(
        events.last().unwrap()["response"]["output"][0]["content"][0]["refusal"],
        refusal
    );
    assert_eq!(events.last().unwrap()["response"]["output_text"], "");
}

#[tokio::test]
async fn unicode_and_zero_visible_output_preserve_response_lifecycle() {
    let fixture = fixture(100_000);
    let unicode = "cafe\u{301} \u{1F680}\u{1F44D} \u{1F469}\u{200D}\u{1F4BB} \u{4F60}\u{597D}\u{4E16}\u{754C} done";
    let request = json!({
        "model": "mock-gpt-4o",
        "input": "Return the Unicode boundary sample.",
        "store": false
    });
    let (status, _, body) = send(
        fixture.app.clone(),
        "POST",
        "/v1/responses",
        Some(request.clone()),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let response: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(response["output_text"], unicode);
    let mut streamed = request;
    streamed["stream"] = json!(true);
    let events = collect_sse_at(fixture.app.clone(), "/v1/responses", streamed)
        .await
        .chunks();
    let reconstructed: String = events
        .iter()
        .filter(|event| event["type"] == "response.output_text.delta")
        .map(|event| event["delta"].as_str().unwrap())
        .collect();
    assert_eq!(reconstructed, unicode);
    assert_eq!(events.last().unwrap()["response"], response);

    let reasoning = collect_sse_at(
        fixture.app,
        "/v1/responses",
        json!({
            "model": "mock-reasoner",
            "input": "Return reasoning without visible output.",
            "reasoning": {"effort": "high", "summary": "auto"},
            "store": false,
            "stream": true
        }),
    )
    .await
    .chunks();
    let terminal = reasoning.last().unwrap();
    assert_eq!(terminal["type"], "response.completed");
    assert_eq!(terminal["response"]["output_text"], "");
    assert_eq!(terminal["response"]["output"].as_array().unwrap().len(), 1);
    assert_eq!(terminal["response"]["output"][0]["type"], "reasoning");
    assert!(
        reasoning
            .iter()
            .all(|event| event["type"] != "response.output_text.delta")
    );
}

#[tokio::test]
async fn abandoning_a_responses_stream_records_cancellation() {
    use axum::body::Body;
    use axum::http::Request;
    use futures::StreamExt;
    use tower::ServiceExt;

    let fixture = fixture(2);
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "model": "mock-reasoner",
                        "input": "Which release should ship?",
                        "reasoning": {"effort": "high", "summary": "auto"},
                        "stream": true
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    stream.next().await.unwrap().unwrap();
    drop(stream);

    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1_500);
    while fixture.cancels.get() == 0 && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(fixture.cancels.get(), 1);
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
