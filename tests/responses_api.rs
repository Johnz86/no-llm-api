//! Non-streamed text-first Responses API contract.

mod support;

use serde_json::{Value, json};
use support::sse::collect_sse_at;
use support::{assert_error_envelope, fixture, fixture_with_scenario, send, send_with_headers};

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
    for (field, value) in [("tools", json!([{"type": "function"}]))] {
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
            "input": "Perform the disallowed deployment action."
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
