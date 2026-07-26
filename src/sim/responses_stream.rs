//! Deterministic event scheduling for streamed Responses objects.

use std::convert::Infallible;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use axum::response::sse::Event;
use futures::Stream;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use serde_json::{Value, json};
use tiktoken_rs::CoreBPE;

use crate::responses::{
    ResponseContentPart, ResponseObject, ResponseOutputItem, ResponseStatus, ResponseSummaryPart,
};
use crate::sim::scenario::{Fault, FaultKind, FaultStage, Timing};
use crate::sim::stream::CancelCounter;
use crate::sim::stream::token_pieces;

pub struct ResponsesStreamPlan {
    pub events: Vec<Value>,
    pub gap: Duration,
    pub ttft: Duration,
    pub jitter_ms: u64,
    pub burst_frames: u32,
    pub fault: Fault,
    pub seed: u64,
}

impl ResponsesStreamPlan {
    pub fn with_profile(
        response: &ResponseObject,
        tokenizer: &CoreBPE,
        rate: NonZeroU32,
        timing: &Timing,
        fault: &Fault,
        seed: u64,
    ) -> Self {
        let effective_rate = timing
            .tokens_per_second
            .and_then(NonZeroU32::new)
            .unwrap_or(rate);
        Self {
            events: response_events(response, tokenizer),
            gap: Duration::from_secs_f64(1.0 / f64::from(effective_rate.get())),
            ttft: Duration::from_millis(timing.ttft_ms),
            jitter_ms: timing.jitter_ms,
            burst_frames: timing.burst_frames,
            fault: fault.clone(),
            seed,
        }
    }

    pub fn keep_alive(&self) -> bool {
        self.fault.kind == FaultKind::Stall
    }
}

pub fn responses_sse_stream(
    plan: ResponsesStreamPlan,
    cancels: Arc<CancelCounter>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    async_stream::stream! {
        let mut guard = ResponsesCancelGuard { cancels, done: false };
        let mut rng = StdRng::seed_from_u64(plan.seed);
        let fires = plan.fault.kind != FaultKind::None
            && (plan.fault.rate >= 1.0 || rng.random::<f64>() < plan.fault.rate);
        let trigger_at = responses_fault_trigger_index(&plan.fault, &plan.events);

        if !plan.ttft.is_zero() {
            tokio::time::sleep(plan.ttft).await;
        }

        for (index, event) in plan.events.iter().enumerate() {
            if fires && index == trigger_at {
                match plan.fault.kind {
                    FaultKind::Drop => {
                        guard.done = true;
                        return;
                    }
                    FaultKind::SseError => {
                        yield Ok(responses_error_event(index));
                        guard.done = true;
                        return;
                    }
                    FaultKind::Stall => {
                        tokio::time::sleep(Duration::from_millis(
                            plan.fault.after_ms.unwrap_or(30_000),
                        )).await;
                    }
                    _ => {}
                }
            }

            if index > 0 && (index as u32) >= plan.burst_frames {
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

            yield Ok(encode_response_event(event));
        }
        guard.done = true;
    }
}

struct ResponsesCancelGuard {
    cancels: Arc<CancelCounter>,
    done: bool,
}

impl Drop for ResponsesCancelGuard {
    fn drop(&mut self) {
        if !self.done {
            self.cancels.record();
        }
    }
}

fn encode_response_event(value: &Value) -> Event {
    let event_type = value["type"]
        .as_str()
        .expect("scheduled Responses events have a type");
    Event::default()
        .event(event_type)
        .data(serde_json::to_string(value).expect("Responses events serialize"))
}

fn responses_error_event(sequence_number: usize) -> Event {
    encode_response_event(&json!({
        "type": "error",
        "code": "server_error",
        "message": "The server had an error while processing your request.",
        "param": null,
        "sequence_number": sequence_number,
    }))
}

fn responses_fault_trigger_index(fault: &Fault, events: &[Value]) -> usize {
    let Some(stage) = fault.stage else {
        return fault.after_frames.unwrap_or(2) as usize;
    };
    let offset = fault.after_frames.unwrap_or(0) as usize;
    events
        .iter()
        .enumerate()
        .filter(|(_, event)| response_event_stage(event) == Some(stage))
        .nth(offset)
        .map_or(usize::MAX, |(index, _)| index)
}

fn response_event_stage(event: &Value) -> Option<FaultStage> {
    match event["type"].as_str() {
        Some("response.reasoning_summary_text.delta") => Some(FaultStage::Reasoning),
        Some("response.output_text.delta" | "response.refusal.delta") => Some(FaultStage::Output),
        Some(
            "response.completed" | "response.incomplete" | "response.failed" | "response.cancelled",
        ) => Some(FaultStage::Terminal),
        _ => None,
    }
}

/// Produces the complete ordered event schedule for a Responses stream.
///
/// Transport, pacing, and injected faults consume this schedule later. Keeping
/// event construction pure makes reconstruction and identity invariants
/// independently testable.
pub fn response_events(response: &ResponseObject, tokenizer: &CoreBPE) -> Vec<Value> {
    let mut events = EventSchedule::default();
    events.push(json!({
        "type": "response.created",
        "response": initial_response(response),
    }));
    events.push(json!({
        "type": "response.in_progress",
        "response": initial_response(response),
    }));

    for (output_index, item) in response.output.iter().enumerate() {
        match item {
            ResponseOutputItem::Reasoning {
                id,
                summary,
                encrypted_content,
                ..
            } => schedule_reasoning(
                &mut events,
                output_index,
                id,
                summary,
                encrypted_content.as_deref(),
                tokenizer,
            ),
            ResponseOutputItem::Message { id, content, .. } => {
                schedule_message(&mut events, output_index, id, content, tokenizer);
            }
        }
        events.push(json!({
            "type": "response.output_item.done",
            "output_index": output_index,
            "item": item,
        }));
    }

    events.push(json!({
        "type": terminal_event(response.status),
        "response": response,
    }));
    events.values
}

#[derive(Default)]
struct EventSchedule {
    values: Vec<Value>,
}

impl EventSchedule {
    fn push(&mut self, mut event: Value) {
        event["sequence_number"] = json!(self.values.len());
        self.values.push(event);
    }
}

fn initial_response(response: &ResponseObject) -> Value {
    json!({
        "id": response.id,
        "object": response.object,
        "status": "in_progress",
        "output": [],
    })
}

fn schedule_reasoning(
    events: &mut EventSchedule,
    output_index: usize,
    item_id: &str,
    summary: &[ResponseSummaryPart],
    encrypted_content: Option<&str>,
    tokenizer: &CoreBPE,
) {
    events.push(json!({
        "type": "response.output_item.added",
        "output_index": output_index,
        "item": {
            "id": item_id,
            "type": "reasoning",
            "status": "in_progress",
            "summary": [],
            "encrypted_content": encrypted_content,
        },
    }));
    for (summary_index, part) in summary.iter().enumerate() {
        events.push(json!({
            "type": "response.reasoning_summary_part.added",
            "item_id": item_id,
            "output_index": output_index,
            "summary_index": summary_index,
            "part": {"type": "summary_text", "text": ""},
        }));
        for delta in text_pieces(tokenizer, &part.text) {
            events.push(json!({
                "type": "response.reasoning_summary_text.delta",
                "item_id": item_id,
                "output_index": output_index,
                "summary_index": summary_index,
                "delta": delta,
            }));
        }
        events.push(json!({
            "type": "response.reasoning_summary_text.done",
            "item_id": item_id,
            "output_index": output_index,
            "summary_index": summary_index,
            "text": part.text,
        }));
        events.push(json!({
            "type": "response.reasoning_summary_part.done",
            "item_id": item_id,
            "output_index": output_index,
            "summary_index": summary_index,
            "part": part,
        }));
    }
}

fn schedule_message(
    events: &mut EventSchedule,
    output_index: usize,
    item_id: &str,
    content: &[ResponseContentPart],
    tokenizer: &CoreBPE,
) {
    events.push(json!({
        "type": "response.output_item.added",
        "output_index": output_index,
        "item": {
            "id": item_id,
            "type": "message",
            "role": "assistant",
            "status": "in_progress",
            "content": [],
        },
    }));
    for (content_index, part) in content.iter().enumerate() {
        match part {
            ResponseContentPart::OutputText { text, .. } => schedule_content_part(
                events,
                output_index,
                content_index,
                item_id,
                "output_text",
                "text",
                text,
                part,
                tokenizer,
            ),
            ResponseContentPart::Refusal { refusal } => schedule_content_part(
                events,
                output_index,
                content_index,
                item_id,
                "refusal",
                "refusal",
                refusal,
                part,
                tokenizer,
            ),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn schedule_content_part(
    events: &mut EventSchedule,
    output_index: usize,
    content_index: usize,
    item_id: &str,
    event_name: &str,
    value_name: &str,
    value: &str,
    part: &ResponseContentPart,
    tokenizer: &CoreBPE,
) {
    let empty_part = match part {
        ResponseContentPart::OutputText { .. } => {
            json!({"type": "output_text", "text": "", "annotations": [], "logprobs": []})
        }
        ResponseContentPart::Refusal { .. } => json!({"type": "refusal", "refusal": ""}),
    };
    events.push(json!({
        "type": "response.content_part.added",
        "item_id": item_id,
        "output_index": output_index,
        "content_index": content_index,
        "part": empty_part,
    }));
    for delta in text_pieces(tokenizer, value) {
        events.push(json!({
            "type": format!("response.{event_name}.delta"),
            "item_id": item_id,
            "output_index": output_index,
            "content_index": content_index,
            "delta": delta,
            "logprobs": [],
        }));
    }
    events.push(json!({
        "type": format!("response.{event_name}.done"),
        "item_id": item_id,
        "output_index": output_index,
        "content_index": content_index,
        (value_name): value,
        "logprobs": [],
    }));
    events.push(json!({
        "type": "response.content_part.done",
        "item_id": item_id,
        "output_index": output_index,
        "content_index": content_index,
        "part": part,
    }));
}

fn text_pieces(tokenizer: &CoreBPE, text: &str) -> Vec<String> {
    token_pieces(tokenizer, &tokenizer.encode_with_special_tokens(text))
        .into_iter()
        .filter(|piece| !piece.is_empty())
        .collect()
}

fn terminal_event(status: ResponseStatus) -> &'static str {
    match status {
        ResponseStatus::Completed => "response.completed",
        ResponseStatus::Incomplete => "response.incomplete",
        ResponseStatus::Failed => "response.failed",
        ResponseStatus::Cancelled => "response.cancelled",
        ResponseStatus::InProgress => "response.in_progress",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::models::ModelCatalogue;
    use crate::responses::{CreateResponseRequest, render_response};
    use crate::sim::artifact::builtin_artifact;
    use crate::sim::plan::{SemanticCapabilities, compile};

    #[test]
    fn schedule_reconstructs_reasoning_and_text_before_exact_terminal_object() {
        let request: CreateResponseRequest = serde_json::from_value(json!({
            "model": "mock-reasoner",
            "input": "Which release should ship?",
            "reasoning": {"effort": "high", "summary": "auto"},
            "x_simulate": {"case": "reasoning-effort/release-decision"},
            "stream": true
        }))
        .unwrap();
        let artifact = builtin_artifact();
        let controls = artifact
            .selection_controls(Some("reasoning-effort/release-decision".to_string()), None);
        let models = ModelCatalogue::builtin();
        let plan = compile(
            &artifact.fixtures,
            &request.canonical_request(),
            &controls,
            &SemanticCapabilities::from(models.profile("mock-reasoner").unwrap()),
        )
        .unwrap();
        let tokenizer = tiktoken_rs::cl100k_base().unwrap();
        let response = render_response(&request, &plan, &tokenizer);
        let events = response_events(&response, &tokenizer);

        for (sequence, event) in events.iter().enumerate() {
            assert_eq!(event["sequence_number"], sequence);
        }
        assert_eq!(events[0]["type"], "response.created");
        assert_eq!(events[1]["type"], "response.in_progress");
        assert_eq!(events.last().unwrap()["type"], "response.completed");
        assert_eq!(
            events.last().unwrap()["response"],
            serde_json::to_value(&response).unwrap()
        );

        let mut reconstructed = BTreeMap::<(u64, u64), String>::new();
        let mut last_summary_delta = 0;
        let mut first_text_delta = usize::MAX;
        for (index, event) in events.iter().enumerate() {
            match event["type"].as_str() {
                Some("response.reasoning_summary_text.delta") => {
                    last_summary_delta = index;
                }
                Some("response.output_text.delta") => {
                    first_text_delta = first_text_delta.min(index);
                    let key = (
                        event["output_index"].as_u64().unwrap(),
                        event["content_index"].as_u64().unwrap(),
                    );
                    reconstructed
                        .entry(key)
                        .or_default()
                        .push_str(event["delta"].as_str().unwrap());
                }
                _ => {}
            }
        }
        assert!(last_summary_delta < first_text_delta);
        assert_eq!(reconstructed[&(1, 0)], "Ship release B.");

        let done_items: Vec<_> = events
            .iter()
            .filter(|event| event["type"] == "response.output_item.done")
            .collect();
        assert_eq!(done_items.len(), response.output.len());
        for event in done_items {
            let index = event["output_index"].as_u64().unwrap() as usize;
            assert_eq!(
                event["item"],
                serde_json::to_value(&response.output[index]).unwrap()
            );
        }
    }
}
