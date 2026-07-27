//! Cross-module reproducibility contracts for schema-v2 semantic fixtures.

use std::collections::BTreeSet;
use std::process::Command;
use std::sync::Arc;

use no_llm_api::model::{
    ChatCompletionRequest, ChatCompletionRequestMessage, ChatRole, MessageContent,
};
use no_llm_api::request_types::ReasoningEffort;
use no_llm_api::sim::canonical::CanonicalRequest;
use no_llm_api::sim::plan::{SelectionControls, SemanticCapabilities, compile};
use no_llm_api::sim::script::builtin_fixtures;

fn fixtures_binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_fixtures"))
}

#[test]
fn semantic_artifact_reproduces_across_fresh_processes() {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first.json");
    let second = directory.path().join("second.json");

    for output in [&first, &second] {
        let result = fixtures_binary()
            .args(["build-semantic", "--output"])
            .arg(output)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    assert_eq!(
        std::fs::read(first).unwrap(),
        std::fs::read(second).unwrap()
    );
}

#[test]
fn responses_plan_reproduces_across_fresh_processes() {
    let directory = tempfile::tempdir().unwrap();
    let request = directory.path().join("responses.json");
    std::fs::write(
        &request,
        serde_json::to_vec(&serde_json::json!({
            "model": "mock-reasoner",
            "input": "Which release should ship?",
            "reasoning": {"effort": "high", "summary": "auto"},
            "store": false
        }))
        .unwrap(),
    )
    .unwrap();
    let run = || {
        fixtures_binary()
            .args(["snapshot-semantic", "--request"])
            .arg(&request)
            .output()
            .unwrap()
    };
    let first = run();
    let second = run();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(first.stdout, second.stdout);
    let plan: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(plan["interface"], "responses");
}

#[test]
fn compatibility_report_command_is_machine_readable_and_clean_for_same_artifact() {
    let directory = tempfile::tempdir().unwrap();
    let artifact = directory.path().join("semantic.json");
    let build = fixtures_binary()
        .args(["build-semantic", "--output"])
        .arg(&artifact)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let report = fixtures_binary()
        .args(["compatibility-report", "--baseline"])
        .arg(&artifact)
        .arg("--candidate")
        .arg(&artifact)
        .output()
        .unwrap();
    assert!(
        report.status.success(),
        "{}",
        String::from_utf8_lossy(&report.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&report.stdout).unwrap();
    assert_eq!(value["compatible"], true);
    assert_eq!(value["fallback_assignment_changes"], serde_json::json!([]));
    assert_eq!(
        value["fallback_candidate_set_changes"],
        serde_json::json!([])
    );
    assert_eq!(value["legacy_byte_changes"], serde_json::json!([]));
}

#[test]
fn explain_command_is_stable_and_redacts_prompt_text() {
    let directory = tempfile::tempdir().unwrap();
    let request_path = directory.path().join("request.json");
    std::fs::write(
        &request_path,
        serde_json::to_vec(&reasoning_request()).unwrap(),
    )
    .unwrap();

    let run = || {
        fixtures_binary()
            .args(["explain-semantic", "--request"])
            .arg(&request_path)
            .output()
            .unwrap()
    };
    let first = run();
    let second = run();
    assert!(first.status.success());
    assert_eq!(first.stdout, second.stdout);
    let explanation: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(explanation["fixture_id"], "reasoning-effort");
    assert_eq!(explanation["variant_id"], "high");
    assert!(!String::from_utf8_lossy(&first.stdout).contains("Which release should ship?"));
}

#[test]
fn semantic_plan_matches_the_golden_snapshot() {
    let result = fixtures_binary()
        .args([
            "snapshot-semantic",
            "--request",
            "tests/fixtures/semantic-reasoning-request.json",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let actual: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    let expected: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string("tests/snapshots/semantic_reasoning_plan.json").unwrap(),
    )
    .unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn concurrent_identical_requests_produce_one_plan() {
    let fixtures = Arc::new(builtin_fixtures());
    let request = Arc::new(CanonicalRequest::from_chat(&reasoning_request()));
    let capabilities = Arc::new(SemanticCapabilities {
        reasoning_efforts: ["low", "medium", "high"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        structured_output: true,
    });
    let tasks: Vec<_> = (0..64)
        .map(|_| {
            let fixtures = Arc::clone(&fixtures);
            let request = Arc::clone(&request);
            let capabilities = Arc::clone(&capabilities);
            std::thread::spawn(move || {
                serde_json::to_string(
                    &compile(
                        &fixtures,
                        &request,
                        &SelectionControls::default(),
                        &capabilities,
                    )
                    .unwrap(),
                )
                .unwrap()
            })
        })
        .collect();
    let plans: BTreeSet<_> = tasks.into_iter().map(|task| task.join().unwrap()).collect();
    assert_eq!(plans.len(), 1);
}

fn reasoning_request() -> ChatCompletionRequest {
    ChatCompletionRequest {
        model: "mock-reasoner".to_string(),
        messages: vec![ChatCompletionRequestMessage {
            role: ChatRole::User,
            content: Some(MessageContent::Text(
                "Which release should ship?".to_string(),
            )),
            name: None,
            tool_call_id: None,
            tool_calls: None,
            function_call: None,
            audio: None,
            refusal: None,
        }],
        reasoning_effort: Some(ReasoningEffort::High),
        ..Default::default()
    }
}
