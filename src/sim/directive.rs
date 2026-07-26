//! Per-request simulation overrides.
//!
//! Decision D2: the scenario supplies defaults, a request overrides them. Only
//! per-request directives are safe when tests run in parallel, and only scenario
//! files are shareable, so both exist - with a fixed precedence:
//! request body `x_simulate` > `X-Simulate-*` headers > scenario > flags.

use serde::Deserialize;
use serde_json::Value;

use crate::sim::scenario::{Fault, FaultKind, FaultStage, Scenario, Timing};

/// Overrides parsed from one request.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Directive {
    pub case: Option<String>,
    pub variant: Option<String>,
    pub ttft_ms: Option<u64>,
    pub tokens_per_second: Option<u32>,
    pub jitter_ms: Option<u64>,
    pub chunk_tokens: Option<u32>,
    pub burst_frames: Option<u32>,
    pub commit_delay_ms: Option<u64>,
    pub fault: Option<String>,
    pub stage: Option<String>,
    pub status: Option<u16>,
    pub retry_after: Option<u64>,
    pub after_ms: Option<u64>,
    pub after_frames: Option<u32>,
    /// Force the fault on or off regardless of the scenario's probability.
    pub rate: Option<f64>,
}

impl Directive {
    pub fn is_empty(&self) -> bool {
        *self == Directive::default()
    }

    /// Parses the `x_simulate` object from a request body.
    pub fn from_value(value: &Value) -> Self {
        serde_json::from_value(value.clone()).unwrap_or_default()
    }

    /// Parses `X-Simulate-*` headers.
    ///
    /// `X-Simulate-Fault: http_error;status=429;retry_after=3` carries its own
    /// parameters, so one header covers the whole fault case.
    pub fn from_headers<'a, I>(headers: I) -> Self
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        let mut directive = Directive::default();
        for (name, value) in headers {
            let name = name.to_ascii_lowercase();
            let Some(key) = name.strip_prefix("x-simulate-") else {
                continue;
            };
            match key {
                "case" => directive.case = non_empty(value),
                "variant" => directive.variant = non_empty(value),
                "fault" => directive.absorb_fault_spec(value),
                "stage" => directive.stage = non_empty(value),
                "ttft-ms" => directive.ttft_ms = value.trim().parse().ok(),
                "tps" | "tokens-per-second" => {
                    directive.tokens_per_second = value.trim().parse().ok();
                }
                "jitter-ms" => directive.jitter_ms = value.trim().parse().ok(),
                "chunk-tokens" => directive.chunk_tokens = value.trim().parse().ok(),
                "burst-frames" => directive.burst_frames = value.trim().parse().ok(),
                "commit-delay-ms" => directive.commit_delay_ms = value.trim().parse().ok(),
                _ => {}
            }
        }
        directive
    }

    fn absorb_fault_spec(&mut self, spec: &str) {
        let mut parts = spec
            .split(';')
            .map(str::trim)
            .filter(|part| !part.is_empty());
        if let Some(kind) = parts.next() {
            self.fault = Some(kind.to_string());
        }
        for part in parts {
            let Some((key, value)) = part.split_once('=') else {
                continue;
            };
            match key.trim().to_ascii_lowercase().as_str() {
                "status" => self.status = value.trim().parse().ok(),
                "retry_after" | "retry-after" => self.retry_after = value.trim().parse().ok(),
                "after_ms" | "after-ms" => self.after_ms = value.trim().parse().ok(),
                "after_frames" | "after-frames" => self.after_frames = value.trim().parse().ok(),
                "rate" => self.rate = value.trim().parse().ok(),
                "stage" => self.stage = non_empty(value),
                _ => {}
            }
        }
    }

    /// Later directives win, member by member.
    pub fn merge(self, higher: Directive) -> Directive {
        Directive {
            case: higher.case.or(self.case),
            variant: higher.variant.or(self.variant),
            ttft_ms: higher.ttft_ms.or(self.ttft_ms),
            tokens_per_second: higher.tokens_per_second.or(self.tokens_per_second),
            jitter_ms: higher.jitter_ms.or(self.jitter_ms),
            chunk_tokens: higher.chunk_tokens.or(self.chunk_tokens),
            burst_frames: higher.burst_frames.or(self.burst_frames),
            commit_delay_ms: higher.commit_delay_ms.or(self.commit_delay_ms),
            fault: higher.fault.or(self.fault),
            stage: higher.stage.or(self.stage),
            status: higher.status.or(self.status),
            retry_after: higher.retry_after.or(self.retry_after),
            after_ms: higher.after_ms.or(self.after_ms),
            after_frames: higher.after_frames.or(self.after_frames),
            rate: higher.rate.or(self.rate),
        }
    }

    /// Applies the directive on top of a scenario, producing the effective profile.
    pub fn apply(&self, scenario: &Scenario) -> (Timing, Fault) {
        let mut timing = scenario.timing.clone();
        if let Some(value) = self.ttft_ms {
            timing.ttft_ms = value;
        }
        if let Some(value) = self.tokens_per_second {
            timing.tokens_per_second = Some(value);
        }
        if let Some(value) = self.jitter_ms {
            timing.jitter_ms = value;
        }
        if let Some(value) = self.chunk_tokens {
            timing.chunk_tokens = Some(value);
        }
        if let Some(value) = self.burst_frames {
            timing.burst_frames = value;
        }

        let mut fault = scenario.fault.clone();
        if let Some(stage) = self.stage.as_deref().and_then(FaultStage::parse) {
            fault.stage = Some(stage);
        }
        if let Some(kind) = self.fault.as_deref().and_then(FaultKind::parse) {
            fault.kind = kind;
            // An explicit per-request fault is meant to fire.
            fault.rate = self.rate.unwrap_or(1.0);
        } else if let Some(rate) = self.rate {
            fault.rate = rate;
        }
        if let Some(value) = self.status {
            fault.status = Some(value);
        }
        if let Some(value) = self.retry_after {
            fault.retry_after = Some(value);
        }
        if let Some(value) = self.after_ms {
            fault.after_ms = Some(value);
        }
        if let Some(value) = self.after_frames {
            fault.after_frames = Some(value);
        }
        (timing, fault)
    }
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_fault_spec_carries_its_parameters() {
        let directive = Directive::from_headers([(
            "X-Simulate-Fault",
            "http_error;status=429;retry_after=3;stage=output",
        )]);
        assert_eq!(directive.fault.as_deref(), Some("http_error"));
        assert_eq!(directive.status, Some(429));
        assert_eq!(directive.retry_after, Some(3));
        assert_eq!(directive.stage.as_deref(), Some("output"));
    }

    #[test]
    fn timing_headers_are_parsed() {
        let directive = Directive::from_headers([
            ("x-simulate-ttft-ms", "250"),
            ("x-simulate-tps", "7"),
            ("x-simulate-jitter-ms", "10"),
            ("x-simulate-commit-delay-ms", "25"),
        ]);
        assert_eq!(directive.ttft_ms, Some(250));
        assert_eq!(directive.tokens_per_second, Some(7));
        assert_eq!(directive.jitter_ms, Some(10));
        assert_eq!(directive.commit_delay_ms, Some(25));
    }

    #[test]
    fn semantic_selector_headers_are_parsed() {
        let directive = Directive::from_headers([
            ("x-simulate-case", " reasoning-effort/release-decision "),
            ("x-simulate-variant", "high"),
            ("x-simulate-stage", "reasoning"),
        ]);
        assert_eq!(
            directive.case.as_deref(),
            Some("reasoning-effort/release-decision")
        );
        assert_eq!(directive.variant.as_deref(), Some("high"));
        assert_eq!(directive.stage.as_deref(), Some("reasoning"));
    }

    #[test]
    fn unrelated_headers_and_garbage_values_are_ignored() {
        let directive = Directive::from_headers([
            ("authorization", "Bearer x"),
            ("x-simulate-tps", "not a number"),
        ]);
        assert!(directive.is_empty());
    }

    #[test]
    fn body_directive_overrides_headers() {
        let header = Directive::from_headers([
            ("x-simulate-case", "basic-text/concise"),
            ("x-simulate-variant", "default"),
            ("x-simulate-tps", "5"),
        ]);
        let body = Directive::from_value(&serde_json::json!({
            "case": "reasoning-effort/release-decision",
            "variant": "high",
            "tokens_per_second": 9
        }));
        let merged = header.merge(body);
        assert_eq!(
            merged.case.as_deref(),
            Some("reasoning-effort/release-decision")
        );
        assert_eq!(merged.variant.as_deref(), Some("high"));
        assert_eq!(merged.tokens_per_second, Some(9));
    }

    #[test]
    fn applying_a_fault_directive_forces_it_to_fire() {
        let scenario = Scenario::default();
        assert_eq!(scenario.fault.kind, FaultKind::None);
        let directive = Directive::from_headers([("x-simulate-fault", "sse_error")]);
        let (_, fault) = directive.apply(&scenario);
        assert_eq!(fault.kind, FaultKind::SseError);
        assert_eq!(fault.rate, 1.0);
    }

    #[test]
    fn a_directive_without_a_fault_keeps_the_scenario_profile() {
        let scenario = Scenario::resolve("rate-limited").unwrap();
        let directive = Directive::from_headers([("x-simulate-tps", "3")]);
        let (timing, fault) = directive.apply(&scenario);
        assert_eq!(timing.tokens_per_second, Some(3));
        assert_eq!(fault.kind, FaultKind::HttpError);
        assert_eq!(fault.status, Some(429));
    }
}
