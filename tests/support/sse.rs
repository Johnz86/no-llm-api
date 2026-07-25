//! Assertion helpers for the SSE transcripts the mock produces.
//!
//! Frames are parsed the way real clients parse them: comment lines starting
//! with `:` are ignored, `data:` payloads accumulate, and a blank line ends a
//! frame. Everything here is deliberately dependency free so a broken stream
//! fails on the transcript, not inside a client library.

use std::time::Duration;

use serde_json::Value;

/// One SSE frame plus the moment it was observed.
#[derive(Debug, Clone)]
pub struct Frame {
    pub data: String,
    pub at: Duration,
}

impl Frame {
    pub fn is_done(&self) -> bool {
        self.data == "[DONE]"
    }

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.data)
            .unwrap_or_else(|error| panic!("frame is not JSON ({error}): {:?}", self.data))
    }
}

/// Incrementally parses SSE bytes into frames, tolerating chunk boundaries.
#[derive(Default)]
pub struct SseParser {
    buffer: String,
    pending: Vec<String>,
    comments: usize,
}

impl SseParser {
    pub fn push(&mut self, bytes: &[u8], at: Duration, out: &mut Vec<Frame>) {
        self.buffer.push_str(&String::from_utf8_lossy(bytes));
        while let Some(end) = self.buffer.find('\n') {
            let line = self.buffer[..end].trim_end_matches('\r').to_string();
            self.buffer.drain(..end + 1);
            if line.is_empty() {
                if !self.pending.is_empty() {
                    out.push(Frame {
                        data: std::mem::take(&mut self.pending).join("\n"),
                        at,
                    });
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix(':') {
                let _ = rest;
                self.comments += 1;
                continue;
            }
            if let Some(rest) = line.strip_prefix("data:") {
                self.pending
                    .push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
            }
        }
    }

    pub fn comment_count(&self) -> usize {
        self.comments
    }
}

/// A parsed transcript with the assertions every streaming test repeats.
pub struct Transcript {
    pub frames: Vec<Frame>,
    pub comments: usize,
}

impl Transcript {
    pub fn chunks(&self) -> Vec<Value> {
        self.frames
            .iter()
            .filter(|frame| !frame.is_done())
            .map(|frame| frame.json())
            .collect()
    }

    /// Concatenation of every `choices[0].delta.content` fragment, in order.
    pub fn content(&self) -> String {
        self.chunks()
            .iter()
            .filter_map(|chunk| {
                chunk["choices"]
                    .get(0)?
                    .get("delta")?
                    .get("content")?
                    .as_str()
                    .map(str::to_owned)
            })
            .collect()
    }

    pub fn finish_reasons(&self) -> Vec<String> {
        self.chunks()
            .iter()
            .filter_map(|chunk| {
                chunk["choices"]
                    .get(0)?
                    .get("finish_reason")?
                    .as_str()
                    .map(str::to_owned)
            })
            .collect()
    }

    pub fn usage_frame_positions(&self) -> Vec<usize> {
        self.chunks()
            .iter()
            .enumerate()
            .filter(|(_, chunk)| chunk.get("usage").is_some_and(|usage| !usage.is_null()))
            .map(|(index, _)| index)
            .collect()
    }

    pub fn ids(&self) -> Vec<String> {
        self.chunks()
            .iter()
            .filter_map(|chunk| chunk["id"].as_str().map(str::to_owned))
            .collect()
    }

    /// Median gap between consecutive frames, the pacing signal that survives CI jitter.
    pub fn median_gap(&self) -> Duration {
        let mut gaps: Vec<Duration> = self
            .frames
            .windows(2)
            .map(|pair| pair[1].at.saturating_sub(pair[0].at))
            .collect();
        if gaps.is_empty() {
            return Duration::ZERO;
        }
        gaps.sort();
        gaps[gaps.len() / 2]
    }

    /// The invariants that hold for every well-formed transcript.
    pub fn assert_well_formed(&self) {
        assert!(!self.frames.is_empty(), "transcript is empty");
        let done: Vec<usize> = self
            .frames
            .iter()
            .enumerate()
            .filter(|(_, frame)| frame.is_done())
            .map(|(index, _)| index)
            .collect();
        assert_eq!(done.len(), 1, "expected exactly one [DONE] frame: {done:?}");
        assert_eq!(
            done[0],
            self.frames.len() - 1,
            "[DONE] must be the last frame"
        );
        let finishes = self.finish_reasons();
        assert_eq!(
            finishes.len(),
            1,
            "expected exactly one finish_reason, got {finishes:?}"
        );
        let chunks = self.chunks();
        let last_with_choices = chunks
            .iter()
            .rposition(|chunk| !chunk["choices"].as_array().is_none_or(|a| a.is_empty()));
        let finish_at = chunks.iter().position(|chunk| {
            chunk["choices"]
                .get(0)
                .and_then(|choice| choice.get("finish_reason"))
                .is_some_and(|reason| !reason.is_null())
        });
        assert_eq!(
            finish_at, last_with_choices,
            "finish_reason must be on the last chunk that carries a choice"
        );
        for chunk in &chunks {
            assert_eq!(chunk["object"], "chat.completion.chunk");
        }
        let ids = self.ids();
        assert!(
            ids.windows(2).all(|pair| pair[0] == pair[1]),
            "chunk ids must be stable across the stream"
        );
    }
}

/// Drives an in-process router and collects the SSE transcript for a request body.
pub async fn collect_sse(app: axum::Router, body: serde_json::Value) -> Transcript {
    use axum::body::Body;
    use axum::http::Request;
    use futures::StreamExt;
    use std::time::Instant;
    use tower::ServiceExt;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("router call failed");

    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream"),
        "streamed responses must be SSE"
    );

    let started = Instant::now();
    let mut parser = SseParser::default();
    let mut frames = Vec::new();
    let mut stream = response.into_body().into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.expect("stream error");
        parser.push(&chunk, started.elapsed(), &mut frames);
    }

    Transcript {
        comments: parser.comment_count(),
        frames,
    }
}
