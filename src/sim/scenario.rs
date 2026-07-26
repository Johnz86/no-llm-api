//! Behaviour profiles: pacing and failure, selectable without a rebuild.
//!
//! Decision D3: a *scenario* is a behaviour profile (this file); a *fixture set*
//! is conversation data. Both are YAML. Built-ins are embedded so `--scenario
//! slow` works with no files on disk.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// The seven built-in profiles, embedded at compile time.
pub const BUILTINS: [(&str, &str); 7] = [
    ("default", include_str!("../../scenarios/default.yaml")),
    ("fast", include_str!("../../scenarios/fast.yaml")),
    ("slow", include_str!("../../scenarios/slow.yaml")),
    ("realistic", include_str!("../../scenarios/realistic.yaml")),
    ("flaky", include_str!("../../scenarios/flaky.yaml")),
    (
        "rate-limited",
        include_str!("../../scenarios/rate-limited.yaml"),
    ),
    ("outage", include_str!("../../scenarios/outage.yaml")),
];

/// A complete behaviour profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub timing: Timing,
    #[serde(default)]
    pub fault: Fault,
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            description: "Deterministic pacing, no faults.".to_string(),
            timing: Timing::default(),
            fault: Fault::default(),
        }
    }
}

/// Pacing knobs. `None` means "inherit the server-wide setting".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Timing {
    /// Delay before the first frame.
    pub ttft_ms: u64,
    /// Per-request override for the token rate.
    pub tokens_per_second: Option<u32>,
    /// Maximum jitter added to each gap, drawn from the seeded generator.
    pub jitter_ms: u64,
    /// Emit this many tokens per frame.
    pub chunk_tokens: Option<u32>,
    /// Frames emitted with no delay at the start of the stream.
    pub burst_frames: u32,
}

/// Failure injection. Exactly one kind is active at a time.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Fault {
    pub kind: FaultKind,
    /// Optional semantic boundary; absent preserves global frame indexing.
    pub stage: Option<FaultStage>,
    /// Probability in `0.0..=1.0`, evaluated against the seeded generator.
    pub rate: f64,
    /// Status for `http_error`.
    pub status: Option<u16>,
    /// `Retry-After` seconds for `http_error`.
    pub retry_after: Option<u64>,
    /// Milliseconds for `stall`, or before `drop` / `sse_error`.
    pub after_ms: Option<u64>,
    /// Frames to emit before `drop`, `sse_error` or `slow_then_recover` acts.
    pub after_frames: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultStage {
    Reasoning,
    Output,
    Terminal,
}

impl FaultStage {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "reasoning" => Some(Self::Reasoning),
            "output" => Some(Self::Output),
            "terminal" => Some(Self::Terminal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultKind {
    #[default]
    None,
    /// Hold the stream open, emitting nothing.
    Stall,
    /// Close the connection mid-stream with no terminal frame.
    Drop,
    /// Emit a spec-shaped error frame, then `[DONE]`.
    SseError,
    /// Answer the request with an HTTP error before streaming starts.
    HttpError,
    /// Pace slowly for a while, then return to the configured rate.
    SlowThenRecover,
}

impl FaultKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FaultKind::None => "none",
            FaultKind::Stall => "stall",
            FaultKind::Drop => "drop",
            FaultKind::SseError => "sse_error",
            FaultKind::HttpError => "http_error",
            FaultKind::SlowThenRecover => "slow_then_recover",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" | "" => Some(FaultKind::None),
            "stall" => Some(FaultKind::Stall),
            "drop" => Some(FaultKind::Drop),
            "sse_error" | "sse-error" => Some(FaultKind::SseError),
            "http_error" | "http-error" => Some(FaultKind::HttpError),
            "slow_then_recover" | "slow-then-recover" => Some(FaultKind::SlowThenRecover),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScenarioError {
    #[error("unknown scenario '{0}'; built-ins are: {1}")]
    Unknown(String, String),
    #[error("io error reading {0}: {1}")]
    Io(String, #[source] std::io::Error),
    #[error("invalid scenario yaml: {0}")]
    Yaml(#[from] serde_yaml_ng::Error),
}

impl Scenario {
    /// Resolves a built-in name or a path to a YAML file.
    pub fn resolve(name_or_path: &str) -> Result<Self, ScenarioError> {
        let trimmed = name_or_path.trim();
        if let Some((_, body)) = BUILTINS
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(trimmed))
        {
            return Self::from_yaml(body);
        }

        let path = Path::new(trimmed);
        if path.exists() {
            let text = std::fs::read_to_string(path)
                .map_err(|error| ScenarioError::Io(path.display().to_string(), error))?;
            return Self::from_yaml(&text);
        }

        Err(ScenarioError::Unknown(
            trimmed.to_string(),
            BUILTINS
                .iter()
                .map(|(name, _)| *name)
                .collect::<Vec<_>>()
                .join(", "),
        ))
    }

    pub fn from_yaml(text: &str) -> Result<Self, ScenarioError> {
        Ok(serde_yaml_ng::from_str(text)?)
    }

    pub fn builtin_names() -> Vec<&'static str> {
        BUILTINS.iter().map(|(name, _)| *name).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_parses_and_keeps_its_name() {
        for (name, _) in BUILTINS {
            let scenario = Scenario::resolve(name).expect(name);
            assert_eq!(scenario.name, name, "{name} declares a different name");
            assert!(
                !scenario.description.is_empty(),
                "{name} has no description"
            );
        }
    }

    #[test]
    fn the_default_scenario_injects_nothing() {
        let scenario = Scenario::resolve("default").unwrap();
        assert_eq!(scenario.fault.kind, FaultKind::None);
        assert_eq!(scenario.timing.ttft_ms, 0);
        assert_eq!(scenario.timing.jitter_ms, 0);
    }

    #[test]
    fn slow_is_slower_than_fast() {
        let slow = Scenario::resolve("slow").unwrap();
        let fast = Scenario::resolve("fast").unwrap();
        assert!(
            slow.timing.tokens_per_second < fast.timing.tokens_per_second,
            "slow {:?} fast {:?}",
            slow.timing.tokens_per_second,
            fast.timing.tokens_per_second
        );
        assert!(slow.timing.ttft_ms > fast.timing.ttft_ms);
    }

    #[test]
    fn failure_scenarios_declare_a_fault() {
        for name in ["flaky", "rate-limited", "outage"] {
            let scenario = Scenario::resolve(name).unwrap();
            assert_ne!(scenario.fault.kind, FaultKind::None, "{name}");
            assert!(scenario.fault.rate > 0.0, "{name}");
        }
        let limited = Scenario::resolve("rate-limited").unwrap();
        assert_eq!(limited.fault.status, Some(429));
        assert!(limited.fault.retry_after.is_some());
    }

    #[test]
    fn unknown_names_list_the_builtins() {
        let error = Scenario::resolve("nope").unwrap_err().to_string();
        assert!(error.contains("default"), "{error}");
    }

    #[test]
    fn unknown_fields_are_rejected_rather_than_ignored() {
        let error = Scenario::from_yaml("name: x\nnot_a_field: 1\n").unwrap_err();
        assert!(matches!(error, ScenarioError::Yaml(_)));
    }

    #[test]
    fn fault_kinds_round_trip_through_their_wire_names() {
        for kind in [
            FaultKind::None,
            FaultKind::Stall,
            FaultKind::Drop,
            FaultKind::SseError,
            FaultKind::HttpError,
            FaultKind::SlowThenRecover,
        ] {
            assert_eq!(FaultKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(FaultKind::parse("nonsense"), None);
    }
}
