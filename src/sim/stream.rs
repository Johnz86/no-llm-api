//! Deterministic simulation of a streamed chat completion.
//!
//! The stream is consumer driven: nothing is produced unless the client polls,
//! so dropping the response cancels the simulation immediately instead of
//! leaving a detached task pacing tokens into a closed channel.

use std::convert::Infallible;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::response::sse::Event;
use futures::Stream;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use tiktoken_rs::CoreBPE;

use crate::model::{
    ChatCompletionChunk, ChatCompletionChunkDelta, ChatCompletionMessageToolCall,
    ChatCompletionMessageToolCallChunk, ChatCompletionResponse, ChatRole, FunctionCallChunk,
};
use crate::service::{PreparedCompletion, chunk_from_delta, usage_chunk};
use crate::sim::scenario::{Fault, FaultKind, FaultStage, Timing};

/// Counts streams abandoned by the consumer before the terminal frame.
#[derive(Debug, Default)]
pub struct CancelCounter(AtomicU64);

impl CancelCounter {
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    fn record(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// Everything needed to replay one completion as SSE, resolved up front.
///
/// The frame list is the whole simulation: a role-only opener, content-only
/// middles, refusal or tool-call fragments, and a terminal `delta: {}` carrying
/// `finish_reason`. Timing and faults are resolved here too, so the generator
/// only has to obey them.
pub struct StreamPlan {
    pub response: ChatCompletionResponse,
    /// Ordered `(choice index, delta)` pairs. With `n > 1` the per-choice
    /// sequences are interleaved, the way the API emits them.
    pub frames: Vec<(usize, ChatCompletionChunkDelta)>,
    pub include_usage: bool,
    pub gap: Duration,
    pub ttft: Duration,
    pub jitter_ms: u64,
    pub burst_frames: u32,
    pub fault: Fault,
    /// Seed for jitter and fault probability; derived, so pacing is reproducible.
    pub seed: u64,
}

impl StreamPlan {
    /// A plan with no scenario applied: the server-wide token rate only.
    pub fn new(prepared: PreparedCompletion, tokenizer: &CoreBPE, rate: NonZeroU32) -> Self {
        Self::with_profile(
            prepared,
            tokenizer,
            rate,
            &Timing::default(),
            &Fault::default(),
            0,
        )
    }

    pub fn with_profile(
        prepared: PreparedCompletion,
        tokenizer: &CoreBPE,
        rate: NonZeroU32,
        timing: &Timing,
        fault: &Fault,
        seed: u64,
    ) -> Self {
        let per_choice: Vec<Vec<ChatCompletionChunkDelta>> = prepared
            .token_sets
            .iter()
            .enumerate()
            .map(|(index, tokens)| {
                let pieces = token_pieces(tokenizer, tokens);
                let pieces = match timing.chunk_tokens {
                    Some(size) if size > 1 => group_pieces(&pieces, size as usize),
                    _ => pieces,
                };
                build_frames(&prepared.response, index, &pieces, tokenizer)
            })
            .collect();

        let effective_rate = timing
            .tokens_per_second
            .and_then(NonZeroU32::new)
            .unwrap_or(rate);
        Self {
            response: prepared.response,
            frames: interleave(per_choice),
            include_usage: prepared.include_usage_chunk,
            gap: Duration::from_secs_f64(1.0 / f64::from(effective_rate.get())),
            ttft: Duration::from_millis(timing.ttft_ms),
            jitter_ms: timing.jitter_ms,
            burst_frames: timing.burst_frames,
            fault: fault.clone(),
            seed,
        }
    }

    /// Whether keep-alive comments should be sent for this plan.
    ///
    /// Decision D6: off for fidelity with real traffic, on during a stall so the
    /// fault is actually reachable by a client that relies on them.
    pub fn keep_alive(&self) -> bool {
        self.fault.kind == FaultKind::Stall
    }
}

/// Round-robins the per-choice sequences so no choice finishes before another
/// starts, which is what a client assembling `n` alternatives expects to see.
fn interleave(
    per_choice: Vec<Vec<ChatCompletionChunkDelta>>,
) -> Vec<(usize, ChatCompletionChunkDelta)> {
    let rounds = per_choice.iter().map(Vec::len).max().unwrap_or(0);
    let mut frames = Vec::with_capacity(rounds * per_choice.len());
    for round in 0..rounds {
        for (index, sequence) in per_choice.iter().enumerate() {
            if let Some(delta) = sequence.get(round) {
                frames.push((index, delta.clone()));
            }
        }
    }
    frames
}

/// Groups token pieces into larger frames, for clients being tested against
/// providers that batch tokens.
fn group_pieces(pieces: &[String], size: usize) -> Vec<String> {
    pieces.chunks(size).map(|chunk| chunk.concat()).collect()
}

/// Builds the ordered delta sequence for one choice.
fn build_frames(
    response: &ChatCompletionResponse,
    choice_index: usize,
    pieces: &[String],
    tokenizer: &CoreBPE,
) -> Vec<ChatCompletionChunkDelta> {
    let mut frames = vec![ChatCompletionChunkDelta {
        role: Some(ChatRole::Assistant),
        content: Some(String::new()),
        ..Default::default()
    }];

    let choice = response.choices.get(choice_index);

    // Reasoning arrives before visible content, which is the order the GUIs that
    // render a thinking pane expect.
    if let Some(reasoning) = choice.and_then(|choice| choice.message.reasoning_content.as_deref()) {
        let tokens = tokenizer.encode_with_special_tokens(reasoning);
        frames.extend(
            token_pieces(tokenizer, &tokens)
                .into_iter()
                .filter(|piece| !piece.is_empty())
                .map(|piece| ChatCompletionChunkDelta {
                    reasoning_content: Some(piece),
                    ..Default::default()
                }),
        );
    }

    frames.extend(
        pieces
            .iter()
            .filter(|piece| !piece.is_empty())
            .map(|piece| ChatCompletionChunkDelta {
                content: Some(piece.clone()),
                ..Default::default()
            }),
    );

    if let Some(refusal) = choice.and_then(|choice| choice.message.refusal.as_deref()) {
        let tokens = tokenizer.encode_with_special_tokens(refusal);
        frames.extend(
            token_pieces(tokenizer, &tokens)
                .into_iter()
                .filter(|piece| !piece.is_empty())
                .map(|piece| ChatCompletionChunkDelta {
                    refusal: Some(piece),
                    ..Default::default()
                }),
        );
    }

    if let Some(calls) = choice.and_then(|choice| choice.message.tool_calls.as_deref()) {
        frames.extend(tool_call_frames(calls));
    }

    if let Some(function_call) = choice.and_then(|choice| choice.message.function_call.as_ref()) {
        frames.push(ChatCompletionChunkDelta {
            function_call: Some(function_call.clone()),
            ..Default::default()
        });
    }

    frames
}

/// Streams tool calls the way the API does: an opening fragment carrying `id`,
/// `type` and the function name, then argument fragments interleaved across
/// parallel calls. `arguments` is never an empty string, which breaks clients
/// that treat `""` as a complete payload.
fn tool_call_frames(calls: &[ChatCompletionMessageToolCall]) -> Vec<ChatCompletionChunkDelta> {
    let mut frames: Vec<ChatCompletionChunkDelta> = calls
        .iter()
        .enumerate()
        .map(|(index, call)| ChatCompletionChunkDelta {
            tool_calls: Some(vec![ChatCompletionMessageToolCallChunk {
                index,
                id: call.id.clone(),
                r#type: call
                    .r#type
                    .clone()
                    .or(Some(crate::model::ChatCompletionToolType::Function)),
                function: Some(FunctionCallChunk {
                    name: call
                        .function
                        .as_ref()
                        .and_then(|function| function.name.clone()),
                    arguments: None,
                }),
            }]),
            ..Default::default()
        })
        .collect();

    let fragments: Vec<Vec<String>> = calls
        .iter()
        .map(|call| {
            split_arguments(
                call.function
                    .as_ref()
                    .and_then(|function| function.arguments.as_deref())
                    .unwrap_or_default(),
            )
        })
        .collect();

    let rounds = fragments.iter().map(Vec::len).max().unwrap_or(0);
    for round in 0..rounds {
        for (index, call_fragments) in fragments.iter().enumerate() {
            if let Some(fragment) = call_fragments.get(round) {
                frames.push(ChatCompletionChunkDelta {
                    tool_calls: Some(vec![ChatCompletionMessageToolCallChunk {
                        index,
                        id: None,
                        r#type: None,
                        function: Some(FunctionCallChunk {
                            name: None,
                            arguments: Some(fragment.clone()),
                        }),
                    }]),
                    ..Default::default()
                });
            }
        }
    }

    frames
}

/// Splits an argument payload over at least two non-empty fragments when it is
/// long enough, so clients that assemble fragments are actually exercised.
fn split_arguments(arguments: &str) -> Vec<String> {
    if arguments.is_empty() {
        return Vec::new();
    }
    let mut boundary = arguments.len() / 2;
    while boundary > 0 && !arguments.is_char_boundary(boundary) {
        boundary -= 1;
    }
    if boundary == 0 || boundary == arguments.len() {
        return vec![arguments.to_string()];
    }
    vec![
        arguments[..boundary].to_string(),
        arguments[boundary..].to_string(),
    ]
}

/// Splits a token sequence into per-token text pieces that are always valid UTF-8.
///
/// A single token can carry a fragment of a multi-byte character, which is why
/// decoding a growing token buffer fails part way through emoji or CJK text.
/// Bytes are therefore accumulated and only released on a character boundary,
/// so concatenating every piece reproduces the full decode byte for byte.
pub fn token_pieces(tokenizer: &CoreBPE, tokens: &[u32]) -> Vec<String> {
    let mut pieces = Vec::with_capacity(tokens.len());
    let mut pending: Vec<u8> = Vec::new();

    for token in tokens {
        match tokenizer.decode_bytes(std::slice::from_ref(token)) {
            Ok(bytes) => pending.extend_from_slice(&bytes),
            Err(error) => {
                tracing::warn!(target: "no_llm_api", ?error, token, "unknown token in fixture");
                pending.extend_from_slice("\u{FFFD}".as_bytes());
            }
        }
        pieces.push(drain_complete(&mut pending));
    }

    if !pending.is_empty() {
        let tail = String::from_utf8_lossy(&pending).into_owned();
        pending.clear();
        match pieces.last_mut() {
            Some(last) => last.push_str(&tail),
            None => pieces.push(tail),
        }
    }

    pieces
}

/// Removes and returns the longest prefix of `pending` that is valid UTF-8.
///
/// Genuinely invalid sequences are replaced with `U+FFFD` rather than aborting;
/// a trailing incomplete character stays in the buffer for the next token.
fn drain_complete(pending: &mut Vec<u8>) -> String {
    let mut out = String::new();
    loop {
        match std::str::from_utf8(pending) {
            Ok(text) => {
                out.push_str(text);
                pending.clear();
                return out;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                out.push_str(unsafe { std::str::from_utf8_unchecked(&pending[..valid]) });
                match error.error_len() {
                    Some(bad) => {
                        out.push('\u{FFFD}');
                        pending.drain(..valid + bad);
                    }
                    None => {
                        pending.drain(..valid);
                        return out;
                    }
                }
            }
        }
    }
}

/// Renders the plan as SSE frames, guaranteeing a terminal `[DONE]` on every path
/// except the deliberate `drop` fault.
pub fn sse_stream(
    plan: StreamPlan,
    cancels: Arc<CancelCounter>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    async_stream::stream! {
        let mut guard = CancelGuard { cancels, done: false };
        let mut rng = StdRng::seed_from_u64(plan.seed);
        let fires = plan.fault.kind != FaultKind::None
            && (plan.fault.rate >= 1.0 || rng.random::<f64>() < plan.fault.rate);
        let trigger_at = fault_trigger_index(&plan.fault, &plan.frames);

        if !plan.ttft.is_zero() {
            tokio::time::sleep(plan.ttft).await;
        }

        for (index, (choice_index, delta)) in plan.frames.iter().enumerate() {
            if fires && index == trigger_at {
                match plan.fault.kind {
                    FaultKind::Drop => {
                        // Deliberate truncation: no terminal frame, no [DONE].
                        guard.done = true;
                        return;
                    }
                    FaultKind::SseError => {
                        yield Ok(error_event(
                            "The server had an error while processing your request.",
                            "server_error",
                        ));
                        yield Ok(done_event());
                        guard.done = true;
                        return;
                    }
                    FaultKind::Stall => {
                        tokio::time::sleep(Duration::from_millis(
                            plan.fault.after_ms.unwrap_or(30_000),
                        ))
                        .await;
                    }
                    _ => {}
                }
            }

            if index > 0 {
                let bursting = (index as u32) < plan.burst_frames;
                if !bursting {
                    let mut gap = plan.gap;
                    if fires && plan.fault.kind == FaultKind::SlowThenRecover && index < trigger_at {
                        gap *= 5;
                    }
                    if plan.jitter_ms > 0 {
                        gap += Duration::from_millis(rng.random_range(0..=plan.jitter_ms));
                    }
                    if !gap.is_zero() {
                        tokio::time::sleep(gap).await;
                    }
                }
            }

            let chunk = chunk_from_delta(&plan.response, *choice_index, delta.clone(), None);
            match encode(&chunk) {
                Ok(event) => yield Ok(event),
                Err(event) => {
                    yield Ok(event);
                    yield Ok(done_event());
                    guard.done = true;
                    return;
                }
            }
        }


        if fires && trigger_at == plan.frames.len() {
            match plan.fault.kind {
                FaultKind::Drop => {
                    guard.done = true;
                    return;
                }
                FaultKind::SseError => {
                    yield Ok(error_event(
                        "The server had an error while processing your request.",
                        "server_error",
                    ));
                    yield Ok(done_event());
                    guard.done = true;
                    return;
                }
                FaultKind::Stall => {
                    tokio::time::sleep(Duration::from_millis(
                        plan.fault.after_ms.unwrap_or(30_000),
                    ))
                    .await;
                }
                _ => {}
            }
        }

        // One terminal frame per choice, each carrying its own finish_reason.
        for (choice_index, choice) in plan.response.choices.iter().enumerate() {
            let final_chunk = chunk_from_delta(
                &plan.response,
                choice_index,
                ChatCompletionChunkDelta::default(),
                choice.finish_reason.clone(),
            );
            match encode(&final_chunk) {
                Ok(event) | Err(event) => yield Ok(event),
            }
        }

        if plan.include_usage {
            let usage = usage_chunk(&plan.response);
            if let Ok(event) = encode(&usage) {
                yield Ok(event);
            }
        }

        yield Ok(done_event());
        guard.done = true;
    }
}

fn fault_trigger_index(fault: &Fault, frames: &[(usize, ChatCompletionChunkDelta)]) -> usize {
    let Some(stage) = fault.stage else {
        return fault.after_frames.unwrap_or(2) as usize;
    };
    if stage == FaultStage::Terminal {
        return frames.len();
    }
    let offset = fault.after_frames.unwrap_or(0) as usize;
    frames
        .iter()
        .enumerate()
        .filter(|(_, (_, delta))| delta_stage(delta) == Some(stage))
        .nth(offset)
        .map_or(usize::MAX, |(index, _)| index)
}

fn delta_stage(delta: &ChatCompletionChunkDelta) -> Option<FaultStage> {
    if delta.reasoning_content.is_some() {
        return Some(FaultStage::Reasoning);
    }
    let visible_content = delta
        .content
        .as_ref()
        .is_some_and(|content| !content.is_empty());
    (visible_content
        || delta.refusal.is_some()
        || delta.tool_calls.is_some()
        || delta.function_call.is_some())
    .then_some(FaultStage::Output)
}

/// A spec-shaped error delivered inside the stream, the way the API does it.
fn error_event(message: &str, kind: &str) -> Event {
    Event::default().data(
        serde_json::json!({
            "error": {
                "message": message,
                "type": kind,
                "param": serde_json::Value::Null,
                "code": serde_json::Value::Null,
            }
        })
        .to_string(),
    )
}

/// Serialises a chunk, falling back to a spec-shaped error frame.
fn encode(chunk: &ChatCompletionChunk) -> Result<Event, Event> {
    Event::default().json_data(chunk).map_err(|error| {
        tracing::error!(target: "no_llm_api", ?error, "failed to serialize chunk");
        Event::default().data(
            serde_json::json!({
                "error": {
                    "message": "failed to serialize completion chunk",
                    "type": "server_error",
                    "param": serde_json::Value::Null,
                    "code": serde_json::Value::Null,
                }
            })
            .to_string(),
        )
    })
}

fn done_event() -> Event {
    Event::default().data("[DONE]")
}

struct CancelGuard {
    cancels: Arc<CancelCounter>,
    done: bool,
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if !self.done {
            self.cancels.record();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiktoken_rs::cl100k_base;

    fn pieces_for(text: &str) -> Vec<String> {
        let tokenizer = cl100k_base().unwrap();
        let tokens = tokenizer.encode_with_special_tokens(text);
        token_pieces(&tokenizer, &tokens)
    }

    #[test]
    fn pieces_concatenate_to_the_source_text() {
        for text in [
            "plain ascii",
            "cafe\u{301} latte",
            "emoji \u{1F680}\u{1F44D} and \u{4F60}\u{597D}\u{4E16}\u{754C}",
            "\u{1F469}\u{200D}\u{1F4BB}\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}",
        ] {
            let pieces = pieces_for(text);
            assert_eq!(pieces.concat(), text, "round trip failed for {text:?}");
        }
    }

    #[test]
    fn stage_trigger_uses_semantic_boundaries_and_frame_offsets() {
        let frames = vec![
            (
                0,
                ChatCompletionChunkDelta {
                    role: Some(ChatRole::Assistant),
                    content: Some(String::new()),
                    ..Default::default()
                },
            ),
            (
                0,
                ChatCompletionChunkDelta {
                    reasoning_content: Some("think".to_string()),
                    ..Default::default()
                },
            ),
            (
                0,
                ChatCompletionChunkDelta {
                    content: Some("one".to_string()),
                    ..Default::default()
                },
            ),
            (
                0,
                ChatCompletionChunkDelta {
                    content: Some("two".to_string()),
                    ..Default::default()
                },
            ),
        ];
        let fault = Fault {
            stage: Some(FaultStage::Reasoning),
            ..Default::default()
        };
        assert_eq!(fault_trigger_index(&fault, &frames), 1);
        let fault = Fault {
            stage: Some(FaultStage::Output),
            after_frames: Some(1),
            ..Default::default()
        };
        assert_eq!(fault_trigger_index(&fault, &frames), 3);
        let fault = Fault {
            stage: Some(FaultStage::Terminal),
            ..Default::default()
        };
        assert_eq!(fault_trigger_index(&fault, &frames), frames.len());
        let fault = Fault {
            after_frames: Some(2),
            ..Default::default()
        };
        assert_eq!(fault_trigger_index(&fault, &frames), 2);
    }

    #[test]
    fn one_piece_per_token_even_when_empty() {
        let tokenizer = cl100k_base().unwrap();
        let text = "\u{1F680}\u{1F680}\u{1F680}";
        let tokens = tokenizer.encode_with_special_tokens(text);
        let pieces = token_pieces(&tokenizer, &tokens);
        assert_eq!(pieces.len(), tokens.len());
        assert!(
            pieces.iter().any(|piece| piece.is_empty()),
            "expected at least one token to carry only a character fragment"
        );
    }

    #[test]
    fn partial_multi_byte_tokens_never_abort() {
        let tokenizer = cl100k_base().unwrap();
        let tokens = tokenizer.encode_with_special_tokens("\u{1F680}");
        for prefix in 1..=tokens.len() {
            let pieces = token_pieces(&tokenizer, &tokens[..prefix]);
            assert_eq!(pieces.len(), prefix);
        }
    }

    #[test]
    fn invalid_trailing_bytes_become_replacement_characters() {
        let mut pending = vec![0xE2, 0x28, 0xA1];
        let drained = drain_complete(&mut pending);
        assert!(drained.contains('\u{FFFD}'));
    }

    /// The bug this module exists to fix: decoding a growing token buffer errors
    /// as soon as a prefix ends inside a multi-byte character, which used to
    /// abort the stream with no final chunk and no `[DONE]`.
    #[test]
    fn growing_buffer_decode_still_fails_mid_character() {
        let tokenizer = cl100k_base().unwrap();
        let text = "cafe\u{301} \u{1F680}\u{1F44D} \u{4F60}\u{597D}\u{4E16}\u{754C}";
        let tokens = tokenizer.encode_with_special_tokens(text);
        let failing = (1..=tokens.len())
            .filter(|prefix| tokenizer.decode(&tokens[..*prefix]).is_err())
            .count();
        assert!(
            failing > 0,
            "fixture must contain a token boundary that breaks naive decoding"
        );
        assert_eq!(token_pieces(&tokenizer, &tokens).concat(), text);
    }
}
