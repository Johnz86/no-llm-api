//! Guards the tracked parquet fixture against drift from its source of truth.

use std::path::Path;

use no_llm_api::dataset::{DatasetRow, bundled_rows, read_dataset_rows, write_sample_dataset};

fn key(row: &DatasetRow<'_>) -> (String, u32, String, Option<String>, Option<String>) {
    (
        row.conversation_id.to_string(),
        row.turn_index,
        row.role.to_string(),
        row.content.as_ref().map(|value| value.to_string()),
        row.finish_reason.as_ref().map(|value| value.to_string()),
    )
}

#[test]
fn tracked_dataset_matches_the_bundled_fixture_source() {
    let path = Path::new("data/conversations.parquet");
    assert!(
        path.exists(),
        "data/conversations.parquet is expected to be tracked"
    );

    let tracked = read_dataset_rows(path).expect("read tracked dataset");
    let source = bundled_rows();

    assert_eq!(
        tracked.len(),
        source.len(),
        "row count drifted: run `cargo run --bin regenerate_dataset -- --force`"
    );

    let mut tracked_keys: Vec<_> = tracked.iter().map(key).collect();
    let mut source_keys: Vec<_> = source.iter().map(key).collect();
    tracked_keys.sort();
    source_keys.sort();

    for (tracked, source) in tracked_keys.iter().zip(source_keys.iter()) {
        assert_eq!(
            tracked, source,
            "fixture drifted; run `cargo run --bin regenerate_dataset -- --force`"
        );
    }
}

#[test]
fn regeneration_replaces_an_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conversations.parquet");
    std::fs::write(&path, b"not a parquet file").unwrap();

    write_sample_dataset(&path).expect("regeneration must overwrite");

    let rows = read_dataset_rows(&path).expect("read regenerated dataset");
    assert_eq!(rows.len(), bundled_rows().len());
}

#[test]
fn the_embedded_fixture_list_matches_the_directory() {
    let on_disk = no_llm_api::fixtures::load_dir(Path::new("fixtures")).expect("load fixtures dir");
    let embedded = no_llm_api::fixtures::builtin_sets();

    let mut disk_ids: Vec<&str> = on_disk.iter().map(|set| set.id.as_str()).collect();
    let mut embedded_ids: Vec<&str> = embedded.iter().map(|set| set.id.as_str()).collect();
    disk_ids.sort();
    embedded_ids.sort();
    assert_eq!(
        disk_ids, embedded_ids,
        "fixtures/ and fixtures::BUILTINS disagree; add the file to BUILTINS"
    );

    for disk in &on_disk {
        let embedded = embedded
            .iter()
            .find(|set| set.id == disk.id)
            .unwrap_or_else(|| panic!("{} is not embedded", disk.id));
        assert_eq!(
            disk.to_rows().len(),
            embedded.to_rows().len(),
            "{} differs between disk and the embedded copy",
            disk.id
        );
    }
}

#[test]
fn every_conversation_ends_with_an_assistant_turn() {
    let source = bundled_rows();
    let mut by_conversation: std::collections::BTreeMap<String, Vec<&DatasetRow<'_>>> =
        Default::default();
    for row in &source {
        by_conversation
            .entry(row.conversation_id.to_string())
            .or_default()
            .push(row);
    }
    assert!(!by_conversation.is_empty());
    for (conversation, mut rows) in by_conversation {
        rows.sort_by_key(|row| row.turn_index);
        assert_eq!(
            rows.last().map(|row| row.role.as_ref()),
            Some("assistant"),
            "conversation {conversation} does not end with an assistant reply"
        );
    }
}
