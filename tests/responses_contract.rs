//! Executable contract corpus for the first Responses text/reasoning slice.

use std::collections::BTreeMap;

use serde_json::Value;

const ROOT: &str = "docs/spec/responses-text-contract";

fn read_json(path: &str) -> Value {
    let path = format!("{ROOT}/{path}");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn cases() -> Vec<Value> {
    let manifest = read_json("manifest.json");
    manifest["cases"]
        .as_array()
        .expect("manifest cases")
        .iter()
        .map(|path| read_json(path.as_str().expect("case path")))
        .collect()
}

#[test]
fn corpus_is_pinned_to_provenance_and_one_official_client() {
    let manifest = read_json("manifest.json");
    let provenance_text =
        std::fs::read_to_string("openapi.provenance.json").expect("openapi.provenance.json");
    let provenance: Value =
        serde_json::from_str(provenance_text.trim_start_matches('\u{feff}')).expect("provenance");

    assert_eq!(manifest["openapi"]["source"], provenance["source"]);
    assert_eq!(manifest["openapi"]["commit"], provenance["upstream_commit"]);
    assert_eq!(
        manifest["openapi"]["commit_date"],
        provenance["upstream_commit_date"]
    );
    assert_eq!(manifest["openapi"]["sha256"], provenance["sha256"]);

    let targets = manifest["sdk_targets"].as_array().expect("sdk targets");
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0]["language"], "JavaScript/TypeScript");
    assert_eq!(targets[0]["package"], "openai");
    assert_eq!(targets[0]["version"], "6.49.0");
}

#[test]
fn every_stream_has_one_contiguous_lifecycle_and_matches_the_sync_response() {
    for case in cases() {
        let name = case["case"].as_str().expect("case name");
        let events = case["events"].as_array().expect("events");
        assert!(!events.is_empty(), "{name}");

        for (sequence, event) in events.iter().enumerate() {
            assert!(
                matches!(
                    event["type"].as_str(),
                    Some(
                        "response.created"
                            | "response.in_progress"
                            | "response.output_item.added"
                            | "response.reasoning_summary_part.added"
                            | "response.reasoning_summary_text.delta"
                            | "response.reasoning_summary_text.done"
                            | "response.reasoning_summary_part.done"
                            | "response.content_part.added"
                            | "response.output_text.delta"
                            | "response.output_text.done"
                            | "response.content_part.done"
                            | "response.output_item.done"
                            | "response.completed"
                    )
                ),
                "{name} has an event outside the frozen text surface: {}",
                event["type"]
            );
            assert_eq!(
                event["sequence_number"].as_u64(),
                Some(sequence as u64),
                "{name} sequence {sequence}"
            );
        }
        assert_eq!(events[0]["type"], "response.created", "{name}");
        assert_eq!(events[1]["type"], "response.in_progress", "{name}");

        let terminals: Vec<&Value> = events
            .iter()
            .filter(|event| {
                matches!(
                    event["type"].as_str(),
                    Some(
                        "response.completed"
                            | "response.incomplete"
                            | "response.failed"
                            | "response.cancelled"
                            | "error"
                    )
                )
            })
            .collect();
        assert_eq!(terminals.len(), 1, "{name}");
        assert_eq!(events.last(), terminals.first().copied(), "{name}");
        assert_eq!(terminals[0]["response"], case["response"], "{name}");

        let done_items: Vec<&Value> = events
            .iter()
            .filter(|event| event["type"] == "response.output_item.done")
            .collect();
        let output = case["response"]["output"].as_array().expect("output");
        assert_eq!(done_items.len(), output.len(), "{name}");
        for event in done_items {
            let index = event["output_index"].as_u64().expect("output index") as usize;
            assert_eq!(event["item"], output[index], "{name} output {index}");
        }
    }
}

#[test]
fn text_deltas_reconstruct_each_non_streamed_output_part() {
    for case in cases() {
        let name = case["case"].as_str().expect("case name");
        let mut streamed: BTreeMap<(u64, u64), String> = BTreeMap::new();
        for event in case["events"].as_array().expect("events") {
            if event["type"] == "response.output_text.delta" {
                let key = (
                    event["output_index"].as_u64().expect("output index"),
                    event["content_index"].as_u64().expect("content index"),
                );
                let delta = event["delta"].as_str().expect("text delta");
                assert!(!delta.is_empty(), "{name} has an empty text delta");
                streamed.entry(key).or_default().push_str(delta);
            }
        }

        for ((output_index, content_index), text) in streamed {
            assert_eq!(
                text,
                case["response"]["output"][output_index as usize]["content"]
                    [content_index as usize]["text"],
                "{name} output {output_index} content {content_index}"
            );
        }
    }
}

#[test]
fn reasoning_summary_is_public_but_raw_reasoning_is_absent() {
    let case = read_json("reasoning-summary.json");
    let events = case["events"].as_array().expect("events");
    let mut summary = String::new();
    let mut last_summary = 0;
    let mut first_output = usize::MAX;

    for (index, event) in events.iter().enumerate() {
        match event["type"].as_str() {
            Some("response.reasoning_summary_text.delta") => {
                summary.push_str(event["delta"].as_str().expect("summary delta"));
                last_summary = index;
            }
            Some("response.output_text.delta") => first_output = first_output.min(index),
            _ => {}
        }
    }

    assert_eq!(summary, case["response"]["output"][0]["summary"][0]["text"]);
    assert!(last_summary < first_output);
    assert_eq!(
        case["response"]["usage"]["output_tokens_details"]["reasoning_tokens"],
        14
    );

    let encoded = serde_json::to_string(&case).expect("encode case");
    assert!(!encoded.contains("reasoning_trace"));
    assert!(!encoded.contains("chain_of_thought"));
}

#[test]
fn structured_output_preserves_wire_bytes_and_semantic_value() {
    let case = read_json("structured-output.json");
    let text = case["response"]["output"][0]["content"][0]["text"]
        .as_str()
        .expect("structured text");
    let parsed: Value = serde_json::from_str(text).expect("structured output JSON");
    assert_eq!(parsed, case["semantic_output"]);
    assert_eq!(case["request"]["text"]["format"]["type"], "json_schema");
    assert_eq!(case["request"]["text"]["format"]["strict"], true);
    assert_eq!(case["response"]["text"]["format"]["name"], "release_status");
}
