//! The tracked spec extract must stay self-contained and cover our surface.

use std::collections::BTreeSet;

use serde_yaml_ng::Value;

const EXTRACT: &str = "docs/spec/chat-completions.openapi.yaml";

fn extract() -> Value {
    let text = std::fs::read_to_string(EXTRACT).expect(EXTRACT);
    serde_yaml_ng::from_str(&text).expect("extract is valid YAML")
}

#[test]
fn the_extract_covers_every_path_the_mock_implements() {
    let spec = extract();
    let paths = spec["paths"].as_mapping().expect("paths");
    for path in [
        "/chat/completions",
        "/chat/completions/{completion_id}",
        "/chat/completions/{completion_id}/messages",
        "/models",
        "/models/{model}",
        "/responses",
        "/responses/{response_id}",
        "/conversations",
        "/conversations/{conversation_id}",
        "/conversations/{conversation_id}/items",
        "/conversations/{conversation_id}/items/{item_id}",
    ] {
        assert!(
            paths.contains_key(Value::String(path.to_string())),
            "{path} is missing from {EXTRACT}"
        );
    }
    assert_eq!(paths.len(), 11, "the extract must stay pruned");
}

#[test]
fn the_extract_carries_the_schemas_the_implementation_uses() {
    let spec = extract();
    let schemas = spec["components"]["schemas"].as_mapping().expect("schemas");
    for schema in [
        "CreateChatCompletionRequest",
        "CreateChatCompletionResponse",
        "CreateChatCompletionStreamResponse",
        "ChatCompletionStreamResponseDelta",
        "ChatCompletionMessageToolCallChunk",
        "ChatCompletionResponseMessage",
        "ChatCompletionStreamOptions",
        "ServiceTier",
        "Model",
        "ListModelsResponse",
        "Error",
        "ErrorResponse",
    ] {
        assert!(
            schemas.contains_key(Value::String(schema.to_string())),
            "{schema} is missing from {EXTRACT}"
        );
    }
}

#[test]
fn the_extract_has_no_dangling_references() {
    let spec = extract();
    let schemas = spec["components"]["schemas"].as_mapping().expect("schemas");
    let present: BTreeSet<String> = schemas
        .keys()
        .filter_map(|key| key.as_str().map(str::to_owned))
        .collect();

    let mut referenced = BTreeSet::new();
    collect_refs(&spec, &mut referenced);

    let missing: Vec<&String> = referenced.difference(&present).collect();
    assert!(
        missing.is_empty(),
        "the extract references schemas it does not contain: {missing:?}"
    );
}

#[test]
fn the_extract_is_small_enough_to_review_in_a_diff() {
    let contents = std::fs::read(EXTRACT).expect(EXTRACT);
    assert!(
        !contents.windows(2).any(|pair| pair == b"\r\n"),
        "{EXTRACT} must stay LF-only so the reviewability guard is platform-independent"
    );
    let bytes = contents.len();
    assert!(
        bytes < 512 * 1024,
        "{EXTRACT} is {bytes} bytes; the point of pruning is reviewability"
    );
}

#[test]
fn the_extract_records_its_exact_upstream_revision() {
    let extract = std::fs::read_to_string(EXTRACT).expect(EXTRACT);
    let provenance_text =
        std::fs::read_to_string("openapi.provenance.json").expect("openapi.provenance.json");
    let provenance: serde_json::Value =
        serde_json::from_str(provenance_text.trim_start_matches('\u{feff}'))
            .expect("valid provenance JSON");
    for field in ["source", "upstream_commit", "sha256"] {
        let value = provenance[field]
            .as_str()
            .unwrap_or_else(|| panic!("provenance field {field}"));
        assert!(
            extract.lines().take(5).any(|line| line.contains(value)),
            "{EXTRACT} does not record current provenance field {field}"
        );
    }
}

#[test]
fn the_extract_keeps_seed_bounds_numeric_and_valid() {
    let spec = extract();
    let seed = &spec["components"]["schemas"]["CreateChatCompletionRequest"]["allOf"][1]["properties"]
        ["seed"];
    assert_eq!(seed["minimum"].as_i64(), Some(i64::MIN));
    assert_eq!(seed["maximum"].as_i64(), Some(i64::MAX));
}

#[test]
fn maintained_docs_do_not_line_cite_the_untracked_full_spec() {
    for path in [
        "README.md",
        "docs/README.md",
        "docs/spec/chat-completions-scope.md",
        "docs/spec/upstream-openapi.md",
        "docs/versioning.md",
    ] {
        let document = std::fs::read_to_string(path).expect(path);
        assert!(
            !document.contains("`openapi.yaml:"),
            "{path} still has a line citation into the untracked full spec"
        );
    }
}

fn collect_refs(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::Mapping(map) => {
            for (key, entry) in map {
                if key.as_str() == Some("$ref")
                    && let Some(target) = entry.as_str()
                    && let Some(name) = target.strip_prefix("#/components/schemas/")
                {
                    out.insert(name.to_string());
                    continue;
                }
                collect_refs(entry, out);
            }
        }
        Value::Sequence(items) => {
            for item in items {
                collect_refs(item, out);
            }
        }
        _ => {}
    }
}
