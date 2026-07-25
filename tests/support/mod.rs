#![allow(dead_code)]

pub mod sse;

use std::borrow::Cow;
use std::num::NonZeroU32;
use std::sync::Arc;

use no_llm_api::dataset::{ConversationScripts, DatasetRow, write_dataset};
use no_llm_api::http::build_router_with_state;
use no_llm_api::service::ChatService;
use no_llm_api::sim::stream::CancelCounter;
use tempfile::TempDir;
use tiktoken_rs::cl100k_base;

/// A reply that spans combining marks, emoji, a ZWJ sequence and CJK, so that
/// single tokens necessarily land inside multi-byte characters.
pub const UNICODE_PROMPT: &str = "unicode please";
pub const UNICODE_REPLY: &str = "cafe\u{301} \u{1F680}\u{1F44D} \u{1F469}\u{200D}\u{1F4BB} \u{4F60}\u{597D}\u{4E16}\u{754C} done";

pub const PLAIN_PROMPT: &str = "plain please";
pub const PLAIN_REPLY: &str = "All good here.";

pub struct Fixture {
    pub app: axum::Router,
    pub cancels: Arc<CancelCounter>,
    _dir: TempDir,
}

/// Builds a router backed by a purpose-written parquet fixture.
pub fn fixture(tokens_per_second: u32) -> Fixture {
    fixture_with_rows(tokens_per_second, &default_rows())
}

pub fn default_rows() -> Vec<DatasetRow<'static>> {
    vec![
        row("conv-unicode", 0, "user", UNICODE_PROMPT, None),
        row("conv-unicode", 1, "assistant", UNICODE_REPLY, Some("stop")),
        row("conv-plain", 0, "user", PLAIN_PROMPT, None),
        row("conv-plain", 1, "assistant", PLAIN_REPLY, Some("stop")),
    ]
}

pub fn row(
    conversation: &str,
    turn: u32,
    role: &str,
    content: &str,
    finish: Option<&str>,
) -> DatasetRow<'static> {
    DatasetRow {
        conversation_id: Cow::Owned(conversation.to_string()),
        turn_index: turn,
        role: Cow::Owned(role.to_string()),
        content: Some(Cow::Owned(content.to_string())),
        content_parts: None,
        refusal: None,
        tool_calls: None,
        function_call: None,
        audio: None,
        finish_reason: finish.map(|value| Cow::Owned(value.to_string())),
        usage: None,
    }
}

pub fn fixture_with_rows(tokens_per_second: u32, rows: &[DatasetRow<'_>]) -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("fixture.parquet");
    write_dataset(&path, rows).expect("write fixture dataset");

    let tokenizer = Arc::new(cl100k_base().expect("tokenizer"));
    let scripts = ConversationScripts::load(&path, &tokenizer).expect("load fixture dataset");
    let service = Arc::new(ChatService::new(
        scripts,
        tokenizer,
        NonZeroU32::new(tokens_per_second).expect("non-zero rate"),
    ));
    let (app, cancels) = build_router_with_state(service);

    Fixture {
        app,
        cancels,
        _dir: dir,
    }
}

/// A minimal streamed chat request body.
pub fn stream_body(prompt: &str) -> serde_json::Value {
    serde_json::json!({
        "model": "gpt-4o-mini",
        "stream": true,
        "messages": [{ "role": "user", "content": prompt }]
    })
}
