//! The `/_mock` control plane: reshape behaviour at runtime, without a restart.
//!
//! Decision D1: one namespace on the API listener, enabled by default only when
//! the bind address is loopback. On a public bind it must be switched on
//! deliberately, and a token is expected.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use serde_json::Value;

use crate::http::error::ApiError;
use crate::http::routes::AppState;
use crate::sim::scenario::Scenario;

/// How many recent requests the log keeps.
const LOG_CAPACITY: usize = 100;

/// Header names whose values are never recorded.
const REDACTED: [&str; 4] = ["authorization", "api-key", "cookie", "proxy-authorization"];

#[derive(Debug, Clone, Serialize)]
pub struct LoggedRequest {
    pub method: String,
    pub path: String,
    pub status: u16,
    pub model: Option<String>,
    pub stream: bool,
    pub headers: Vec<(String, String)>,
}

/// A bounded, redacted log of recent requests.
#[derive(Debug, Default)]
pub struct RequestLog {
    entries: Mutex<VecDeque<LoggedRequest>>,
}

impl RequestLog {
    pub fn record(&self, entry: LoggedRequest) {
        let mut entries = self.entries.lock().expect("request log poisoned");
        if entries.len() == LOG_CAPACITY {
            entries.pop_front();
        }
        entries.push_back(entry);
    }

    pub fn snapshot(&self) -> Vec<LoggedRequest> {
        self.entries
            .lock()
            .expect("request log poisoned")
            .iter()
            .cloned()
            .collect()
    }

    pub fn clear(&self) {
        self.entries.lock().expect("request log poisoned").clear();
    }
}

/// Redacts a header pair for the log.
pub fn redact(name: &str, value: &str) -> (String, String) {
    let lowered = name.to_ascii_lowercase();
    if REDACTED.contains(&lowered.as_str()) {
        return (lowered, "[redacted]".to_string());
    }
    (lowered, value.to_string())
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/_mock/scenario",
            get(get_scenario).put(put_scenario).patch(patch_scenario),
        )
        .route("/_mock/reset", post(reset))
        .route("/_mock/models", get(models))
        .route("/_mock/requests", get(requests))
        .route("/_mock/responses", get(responses))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_control_token,
        ))
        .with_state(state)
}

/// A token is required whenever one is configured.
async fn require_control_token(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let Some(expected) = state.control_plane.token.as_deref() else {
        return next.run(request).await;
    };
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim);
    if presented == Some(expected) {
        next.run(request).await
    } else {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "The control plane requires a bearer token.",
            "invalid_request_error",
        )
        .with_code("invalid_api_key")
        .into_response()
    }
}

async fn get_scenario(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.scenario.load_full().as_ref().clone())
}

async fn put_scenario(
    State(state): State<AppState>,
    body: Result<Json<Scenario>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(scenario) = body?;
    state.scenario.store(Arc::new(scenario.clone()));
    Ok(Json(scenario).into_response())
}

/// Merges a partial scenario into the live one, member by member.
async fn patch_scenario(
    State(state): State<AppState>,
    body: Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(patch) = body?;
    let current = state.scenario.load_full();
    let mut merged = serde_json::to_value(current.as_ref())
        .map_err(|error| ApiError::server_error(error.to_string()))?;
    merge(&mut merged, &patch);
    let scenario: Scenario = serde_json::from_value(merged)
        .map_err(|error| ApiError::invalid_request(error.to_string()))?;
    state.scenario.store(Arc::new(scenario.clone()));
    Ok(Json(scenario).into_response())
}

async fn reset(State(state): State<AppState>) -> impl IntoResponse {
    state.scenario.store(state.boot_scenario.clone());
    state.log.clear();
    state.responses.clear().await;
    Json(state.boot_scenario.as_ref().clone())
}

async fn models(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.models.debug_view())
}

async fn requests(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({ "object": "list", "data": state.log.snapshot() }))
}

async fn responses(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({
        "object": "list",
        "data": state.responses.snapshot().await,
    }))
}

/// Recursive object merge; scalars and arrays replace wholesale.
fn merge(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(target), Value::Object(patch)) => {
            for (key, value) in patch {
                merge(target.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
        (target, patch) => *target = patch.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_are_never_logged() {
        assert_eq!(
            redact("Authorization", "Bearer sk-secret"),
            ("authorization".to_string(), "[redacted]".to_string())
        );
        assert_eq!(
            redact("X-Stainless-Lang", "js"),
            ("x-stainless-lang".to_string(), "js".to_string())
        );
    }

    #[test]
    fn the_log_is_bounded() {
        let log = RequestLog::default();
        for index in 0..(LOG_CAPACITY + 10) {
            log.record(LoggedRequest {
                method: "POST".into(),
                path: format!("/{index}"),
                status: 200,
                model: None,
                stream: false,
                headers: Vec::new(),
            });
        }
        let snapshot = log.snapshot();
        assert_eq!(snapshot.len(), LOG_CAPACITY);
        assert_eq!(snapshot[0].path, "/10", "oldest entries are dropped");
    }

    #[test]
    fn merge_only_replaces_named_members() {
        let mut target = serde_json::json!({"timing":{"ttft_ms":0,"jitter_ms":5},"name":"a"});
        merge(&mut target, &serde_json::json!({"timing":{"ttft_ms":250}}));
        assert_eq!(target["timing"]["ttft_ms"], 250);
        assert_eq!(target["timing"]["jitter_ms"], 5);
        assert_eq!(target["name"], "a");
    }
}
