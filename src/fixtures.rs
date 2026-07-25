//! Human-authorable fixture sets.
//!
//! Decision D3: a *fixture set* is conversation data (this file), a *scenario* is
//! a behaviour profile. YAML wins for fixtures because block scalars are the only
//! comfortable way to author markdown replies, and the same format serves both.
//!
//! Fixtures used to be ~330 lines of hand-written `DatasetRow` literals, which is
//! why nobody added coverage to them. These files are the source of truth now, and
//! `cargo run --bin fixtures -- build` turns them into parquet.

use std::borrow::Cow;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::dataset::DatasetRow;

/// The built-in fixture sets, embedded so a fresh clone needs no files.
pub const BUILTINS: [(&str, &str); 12] = [
    ("orion", include_str!("../fixtures/orion.yaml")),
    ("lyra", include_str!("../fixtures/lyra.yaml")),
    ("vega", include_str!("../fixtures/vega.yaml")),
    ("audio", include_str!("../fixtures/audio.yaml")),
    ("builder", include_str!("../fixtures/builder.yaml")),
    ("markdown", include_str!("../fixtures/markdown.yaml")),
    ("length", include_str!("../fixtures/length.yaml")),
    ("json-schema", include_str!("../fixtures/json-schema.yaml")),
    ("reasoning", include_str!("../fixtures/reasoning.yaml")),
    ("empty", include_str!("../fixtures/empty.yaml")),
    ("multibyte", include_str!("../fixtures/multibyte.yaml")),
    ("long", include_str!("../fixtures/long.yaml")),
];

/// One scripted conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureSet {
    pub id: String,
    #[serde(default)]
    pub description: String,
    pub turns: Vec<FixtureTurn>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureTurn {
    pub role: FixtureRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_parts: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function_call: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FixtureFinishReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
    /// Repeat the content this many times when building, for size fixtures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureRole {
    System,
    User,
    Assistant,
    Tool,
}

impl FixtureRole {
    pub fn as_str(self) -> &'static str {
        match self {
            FixtureRole::System => "system",
            FixtureRole::User => "user",
            FixtureRole::Assistant => "assistant",
            FixtureRole::Tool => "tool",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureFinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    FunctionCall,
}

impl FixtureFinishReason {
    pub fn as_str(self) -> &'static str {
        match self {
            FixtureFinishReason::Stop => "stop",
            FixtureFinishReason::Length => "length",
            FixtureFinishReason::ToolCalls => "tool_calls",
            FixtureFinishReason::ContentFilter => "content_filter",
            FixtureFinishReason::FunctionCall => "function_call",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FixtureError {
    #[error("io error reading {0}: {1}")]
    Io(String, #[source] std::io::Error),
    #[error("invalid fixture yaml in {0}: {1}")]
    Yaml(String, #[source] serde_yaml_ng::Error),
    #[error("fixture '{0}' is invalid: {1}")]
    Lint(String, String),
}

impl FixtureSet {
    pub fn from_yaml(name: &str, text: &str) -> Result<Self, FixtureError> {
        serde_yaml_ng::from_str(text).map_err(|error| FixtureError::Yaml(name.to_string(), error))
    }

    /// Every rule a fixture must satisfy to be usable by the simulator.
    pub fn lint(&self) -> Result<(), FixtureError> {
        let fail = |message: String| Err(FixtureError::Lint(self.id.clone(), message));

        if self.id.trim().is_empty() {
            return fail("id must not be empty".to_string());
        }
        if self.turns.is_empty() {
            return fail("a fixture needs at least one turn".to_string());
        }
        if !matches!(
            self.turns.last().map(|turn| turn.role),
            Some(FixtureRole::Assistant)
        ) {
            return fail("the last turn must be an assistant reply".to_string());
        }
        if !self.turns.iter().any(|turn| turn.role == FixtureRole::User) {
            return fail("a fixture needs at least one user turn to match on".to_string());
        }

        for (index, turn) in self.turns.iter().enumerate() {
            let empty = turn.content.as_deref().unwrap_or("").is_empty();
            match turn.role {
                FixtureRole::Assistant => {
                    let has_payload = !empty
                        || turn.refusal.is_some()
                        || turn.tool_calls.is_some()
                        || turn.function_call.is_some();
                    if !has_payload {
                        return fail(format!(
                            "assistant turn {index} has no content, refusal, tool call or function call"
                        ));
                    }
                    if turn.finish_reason.is_none() {
                        return fail(format!("assistant turn {index} needs a finish_reason"));
                    }
                }
                _ => {
                    if empty {
                        return fail(format!(
                            "{} turn {index} has no content",
                            turn.role.as_str()
                        ));
                    }
                    if turn.finish_reason.is_some() {
                        return fail(format!(
                            "{} turn {index} must not declare a finish_reason",
                            turn.role.as_str()
                        ));
                    }
                }
            }

            if let Some(calls) = &turn.tool_calls {
                let entries = calls.as_array().ok_or_else(|| {
                    FixtureError::Lint(
                        self.id.clone(),
                        format!("turn {index}: tool_calls must be a list"),
                    )
                })?;
                for call in entries {
                    let arguments = call["function"]["arguments"].as_str().ok_or_else(|| {
                        FixtureError::Lint(
                            self.id.clone(),
                            format!(
                                "turn {index}: every tool call needs function.arguments as a string"
                            ),
                        )
                    })?;
                    if serde_json::from_str::<Value>(arguments).is_err() {
                        return fail(format!(
                            "turn {index}: tool call arguments must be valid JSON, got {arguments:?}"
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    /// Converts the set into parquet rows.
    pub fn to_rows(&self) -> Vec<DatasetRow<'static>> {
        self.turns
            .iter()
            .enumerate()
            .map(|(index, turn)| DatasetRow {
                conversation_id: Cow::Owned(self.id.clone()),
                turn_index: index as u32,
                role: Cow::Owned(turn.role.as_str().to_string()),
                content: turn.content.as_ref().map(|text| {
                    Cow::Owned(match turn.repeat {
                        Some(times) if times > 1 => text.repeat(times as usize),
                        _ => text.clone(),
                    })
                }),
                content_parts: turn.content_parts.as_ref().map(json_string),
                refusal: turn.refusal.as_ref().map(|value| Cow::Owned(value.clone())),
                tool_calls: turn.tool_calls.as_ref().map(json_string),
                function_call: turn.function_call.as_ref().map(json_string),
                audio: turn.audio.as_ref().map(json_string),
                finish_reason: turn
                    .finish_reason
                    .map(|reason| Cow::Owned(reason.as_str().to_string())),
                usage: turn.usage.as_ref().map(json_string),
            })
            .collect()
    }
}

fn json_string(value: &Value) -> Cow<'static, str> {
    Cow::Owned(value.to_string())
}

/// Loads and lints the built-in fixture sets.
pub fn builtin_sets() -> Vec<FixtureSet> {
    BUILTINS
        .iter()
        .map(|(name, body)| {
            let set = FixtureSet::from_yaml(name, body)
                .unwrap_or_else(|error| panic!("built-in fixture {name} is broken: {error}"));
            set.lint().unwrap_or_else(|error| {
                panic!("built-in fixture {name} fails its own lint: {error}")
            });
            set
        })
        .collect()
}

/// The rows the built-in fixture sets produce.
pub fn builtin_rows() -> Vec<DatasetRow<'static>> {
    builtin_sets()
        .iter()
        .flat_map(|set| set.to_rows())
        .collect()
}

/// Loads every `*.yaml` in a directory, linting as it goes.
pub fn load_dir(path: &Path) -> Result<Vec<FixtureSet>, FixtureError> {
    let entries = std::fs::read_dir(path)
        .map_err(|error| FixtureError::Io(path.display().to_string(), error))?;
    let mut sets = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| FixtureError::Io(path.display().to_string(), error))?;
        let file = entry.path();
        if file.extension().and_then(|value| value.to_str()) != Some("yaml") {
            continue;
        }
        let name = file.display().to_string();
        let text = std::fs::read_to_string(&file)
            .map_err(|error| FixtureError::Io(name.clone(), error))?;
        let set = FixtureSet::from_yaml(&name, &text)?;
        set.lint()?;
        sets.push(set);
    }
    sets.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(sets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_parses_and_passes_the_lint() {
        let sets = builtin_sets();
        assert_eq!(sets.len(), BUILTINS.len());
        let mut ids: Vec<&str> = sets.iter().map(|set| set.id.as_str()).collect();
        ids.sort();
        let unique = ids.len();
        ids.dedup();
        assert_eq!(unique, ids.len(), "fixture ids must be unique");
        for set in &sets {
            assert!(!set.description.is_empty(), "{} has no description", set.id);
        }
    }

    #[test]
    fn the_builtins_cover_the_behaviours_a_gui_needs_to_render() {
        let rows = builtin_rows();
        let finishes: std::collections::BTreeSet<String> = rows
            .iter()
            .filter_map(|row| row.finish_reason.as_ref().map(|value| value.to_string()))
            .collect();
        for reason in [
            "stop",
            "length",
            "tool_calls",
            "content_filter",
            "function_call",
        ] {
            assert!(
                finishes.contains(reason),
                "no fixture finishes with {reason}"
            );
        }
        assert!(
            rows.iter().any(|row| row.refusal.is_some()),
            "no refusal fixture"
        );
        assert!(
            rows.iter().any(|row| row.audio.is_some()),
            "no audio fixture"
        );
        assert!(
            rows.iter().any(|row| row.tool_calls.is_some()),
            "no tool-call fixture"
        );
        assert!(
            rows.iter().any(|row| row
                .content
                .as_deref()
                .is_some_and(|text| text.contains("```"))),
            "no markdown fixture"
        );
        assert!(
            rows.iter().any(|row| row
                .content
                .as_deref()
                .is_some_and(|text| text.contains("<think>"))),
            "no reasoning fixture"
        );
        assert!(
            rows.iter()
                .any(|row| row.content.as_deref().is_some_and(|text| !text.is_ascii())),
            "no multi-byte fixture"
        );
        assert!(
            rows.iter()
                .any(|row| row.content.as_deref().is_some_and(|text| text.len() > 4000)),
            "no large fixture"
        );
    }

    #[test]
    fn a_fixture_that_does_not_end_with_an_assistant_reply_is_rejected() {
        let set = FixtureSet::from_yaml(
            "test",
            "id: x\ndescription: d\nturns:\n  - role: user\n    content: hi\n",
        )
        .unwrap();
        assert!(set.lint().is_err());
    }

    #[test]
    fn an_assistant_turn_without_a_finish_reason_is_rejected() {
        let set = FixtureSet::from_yaml(
            "test",
            "id: x\ndescription: d\nturns:\n  - role: user\n    content: hi\n  - role: assistant\n    content: yes\n",
        )
        .unwrap();
        assert!(set.lint().is_err());
    }

    #[test]
    fn tool_call_arguments_must_be_valid_json() {
        let yaml = "id: x\ndescription: d\nturns:\n  - role: user\n    content: hi\n  - role: assistant\n    finish_reason: tool_calls\n    tool_calls:\n      - id: call_1\n        type: function\n        function:\n          name: f\n          arguments: 'not json'\n";
        let set = FixtureSet::from_yaml("test", yaml).unwrap();
        let error = set.lint().unwrap_err().to_string();
        assert!(error.contains("valid JSON"), "{error}");
    }

    #[test]
    fn unknown_fields_are_rejected_rather_than_silently_dropped() {
        assert!(
            FixtureSet::from_yaml("test", "id: x\nturnz: []\n").is_err(),
            "a typo in a fixture must fail the build"
        );
    }

    #[test]
    fn repeat_expands_content_at_build_time() {
        let set = FixtureSet::from_yaml(
            "test",
            "id: x\ndescription: d\nturns:\n  - role: user\n    content: hi\n  - role: assistant\n    content: 'ab'\n    repeat: 3\n    finish_reason: stop\n",
        )
        .unwrap();
        let rows = set.to_rows();
        assert_eq!(rows[1].content.as_deref(), Some("ababab"));
    }
}
