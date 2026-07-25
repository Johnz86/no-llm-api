//! Safety rules for the one code path that can spend money.
//!
//! Credentials are wrapped so they cannot be printed by accident, recorded rows
//! pass one redaction pass before they touch parquet, and requests are capped so
//! a runaway loop cannot bill indefinitely.

use std::fmt;

use serde::Serialize;

/// A secret that refuses to render itself.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Redacted(String);

impl Redacted {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The only way to read the value, named so it is visible in review.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Redacted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Redacted(\"[redacted]\")")
    }
}

impl fmt::Display for Redacted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[redacted]")
    }
}

impl Serialize for Redacted {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("[redacted]")
    }
}

/// Caps that bound what a live session can cost.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct LiveCaps {
    pub request_timeout_secs: u64,
    pub max_concurrent_requests: usize,
    pub max_requests_per_session: u64,
}

impl Default for LiveCaps {
    fn default() -> Self {
        Self {
            request_timeout_secs: 60,
            max_concurrent_requests: 4,
            max_requests_per_session: 500,
        }
    }
}

/// Patterns that must never reach a recorded fixture.
const SECRET_PREFIXES: [&str; 4] = ["sk-", "sk_", "azsk-", "Bearer "];

/// Replaces anything that looks like a credential in recorded text.
///
/// Recording writes model output to disk, and model output can quote a key that
/// a user pasted into a prompt. One pass here is cheaper than auditing every
/// column at every call site.
pub fn redact_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'outer: while !rest.is_empty() {
        for prefix in SECRET_PREFIXES {
            if let Some(position) = rest.find(prefix) {
                let (before, tail) = rest.split_at(position);
                out.push_str(before);
                let token_end = tail[prefix.len()..]
                    .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'))
                    .map(|offset| offset + prefix.len())
                    .unwrap_or(tail.len());
                if token_end > prefix.len() + 4 {
                    out.push_str("[redacted]");
                } else {
                    out.push_str(&tail[..token_end]);
                }
                rest = &tail[token_end..];
                continue 'outer;
            }
        }
        out.push_str(rest);
        break;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_never_prints_itself() {
        let secret = Redacted::new("sk-verysecretvalue");
        assert_eq!(format!("{secret}"), "[redacted]");
        assert_eq!(format!("{secret:?}"), "Redacted(\"[redacted]\")");
        assert_eq!(
            serde_json::to_string(&secret).unwrap(),
            "\"[redacted]\"",
            "serialising settings must not leak the key"
        );
        assert_eq!(secret.expose(), "sk-verysecretvalue");
    }

    #[test]
    fn recorded_text_loses_anything_that_looks_like_a_key() {
        let redacted = redact_text("my key is sk-abcdef1234567890 and that is all");
        assert!(!redacted.contains("abcdef1234567890"), "{redacted}");
        assert!(redacted.contains("[redacted]"), "{redacted}");
        assert!(redacted.starts_with("my key is "));
        assert!(redacted.ends_with(" and that is all"));
    }

    #[test]
    fn bearer_headers_quoted_in_text_are_redacted() {
        let redacted = redact_text("Authorization: Bearer sk-1234567890abcdef");
        assert!(!redacted.contains("1234567890abcdef"), "{redacted}");
    }

    #[test]
    fn ordinary_text_is_untouched() {
        let text = "Sprint closed 14 tickets, shipped analytics, and stabilized the API.";
        assert_eq!(redact_text(text), text);
    }

    #[test]
    fn short_lookalikes_are_left_alone() {
        assert_eq!(redact_text("sk-ab"), "sk-ab");
    }

    #[test]
    fn caps_default_to_something_bounded() {
        let caps = LiveCaps::default();
        assert!(caps.request_timeout_secs > 0);
        assert!(caps.max_concurrent_requests > 0);
        assert!(caps.max_requests_per_session > 0);
    }
}
