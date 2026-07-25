//! Six counters, hand-rolled.
//!
//! The budget for observability is deliberately small: this process instruments
//! the GUI under test, not itself. No registry, no histograms, no dependency.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::sim::stream::CancelCounter;

#[derive(Debug, Default)]
pub struct Metrics {
    pub requests: AtomicU64,
    pub completions: AtomicU64,
    pub streams: AtomicU64,
    pub faults: AtomicU64,
    pub errors: AtomicU64,
}

impl Metrics {
    pub fn record_request(&self, status: u16) {
        self.requests.fetch_add(1, Ordering::Relaxed);
        if status >= 400 {
            self.errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn record_completion(&self, streamed: bool) {
        self.completions.fetch_add(1, Ordering::Relaxed);
        if streamed {
            self.streams.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn record_fault(&self) {
        self.faults.fetch_add(1, Ordering::Relaxed);
    }

    /// Prometheus text exposition format, version 0.0.4.
    pub fn render(&self, cancels: &CancelCounter) -> String {
        let lines = [
            (
                "no_llm_api_requests_total",
                "counter",
                "HTTP requests handled.",
                self.requests.load(Ordering::Relaxed),
            ),
            (
                "no_llm_api_completions_total",
                "counter",
                "Chat completions produced.",
                self.completions.load(Ordering::Relaxed),
            ),
            (
                "no_llm_api_streams_total",
                "counter",
                "Streamed completions started.",
                self.streams.load(Ordering::Relaxed),
            ),
            (
                "no_llm_api_streams_cancelled_total",
                "counter",
                "Streams abandoned by the client before the terminal frame.",
                cancels.get(),
            ),
            (
                "no_llm_api_faults_injected_total",
                "counter",
                "Simulated faults that fired.",
                self.faults.load(Ordering::Relaxed),
            ),
            (
                "no_llm_api_errors_total",
                "counter",
                "Responses with a 4xx or 5xx status.",
                self.errors.load(Ordering::Relaxed),
            ),
        ];

        lines
            .iter()
            .map(|(name, kind, help, value)| {
                format!("# HELP {name} {help}\n# TYPE {name} {kind}\n{name} {value}\n")
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendering_is_prometheus_shaped_and_counts_what_happened() {
        let metrics = Metrics::default();
        let cancels = CancelCounter::default();
        metrics.record_request(200);
        metrics.record_request(429);
        metrics.record_completion(true);
        metrics.record_fault();

        let text = metrics.render(&cancels);
        assert!(text.contains("# TYPE no_llm_api_requests_total counter"));
        assert!(text.contains("no_llm_api_requests_total 2"));
        assert!(text.contains("no_llm_api_errors_total 1"));
        assert!(text.contains("no_llm_api_completions_total 1"));
        assert!(text.contains("no_llm_api_streams_total 1"));
        assert!(text.contains("no_llm_api_faults_injected_total 1"));
        assert!(text.contains("no_llm_api_streams_cancelled_total 0"));
        assert_eq!(
            text.lines()
                .filter(|line| line.starts_with("# HELP"))
                .count(),
            6,
            "six counters is the whole budget"
        );
    }
}
