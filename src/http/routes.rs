use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::Sse;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, Visitor};
use tower_http::cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer, ExposeHeaders};
use uuid::Uuid;

use crate::config::{AuthSettings, ControlPlaneSettings, CorsSettings};
use crate::http::auth;
use crate::http::control::{self, RequestLog};
use crate::http::error::{ApiError, not_found_fallback};
use crate::http::metrics::Metrics;
use crate::model::{ChatCompletionList, ChatCompletionMessageList, ChatCompletionRequest};
use crate::models::ModelCatalogue;
use crate::request_types::ResponseFormat;
use crate::service::ChatService;
use crate::service::SemanticDiagnostics;
use crate::sim::artifact::{SemanticArtifact, builtin_artifact};
use crate::sim::canonical::CanonicalRequest;
use crate::sim::directive::Directive;
use crate::sim::plan::{PlanError, SemanticCapabilities, SemanticOutput, SemanticResponsePlan};
use crate::sim::scenario::{FaultKind, Scenario, Timing};
use crate::sim::stream::{CancelCounter, StreamPlan, sse_stream};
use crate::store::{ListFilters, SortOrder};

/// The page served at `/`, embedded so the binary works from any directory.
const INDEX_HTML: &str = include_str!("../../index.html");

const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");
const ACCEL_BUFFERING: HeaderName = HeaderName::from_static("x-accel-buffering");
/// Which rung of the matching ladder produced the reply; for test triage only.
const SIMULATE_MATCH: HeaderName = HeaderName::from_static("x-simulate-match");
const SIMULATE_DATASET: HeaderName = HeaderName::from_static("x-simulate-dataset-revision");
const SIMULATE_CASE: HeaderName = HeaderName::from_static("x-simulate-case");
const SIMULATE_VARIANT: HeaderName = HeaderName::from_static("x-simulate-variant");
const SIMULATE_PLAN: HeaderName = HeaderName::from_static("x-simulate-plan-digest");
/// Which build answered, so a CI job can assert it talked to the container it meant to.
const VERSION_HEADER: HeaderName = HeaderName::from_static("x-no-llm-api-version");

#[derive(Clone)]
pub struct AppState {
    pub service: Arc<ChatService>,
    pub cancels: Arc<CancelCounter>,
    pub models: ModelCatalogue,
    pub semantic: Arc<SemanticArtifact>,
    /// The live behaviour profile, swappable through the control plane.
    pub scenario: Arc<ArcSwap<Scenario>>,
    /// The profile the process started with, restored by `POST /_mock/reset`.
    pub boot_scenario: Arc<Scenario>,
    pub seed: u64,
    pub auth: AuthSettings,
    pub control_plane: ControlPlaneSettings,
    pub log: Arc<RequestLog>,
    pub metrics: Arc<Metrics>,
}

/// Everything the router needs beyond the service itself.
pub struct RouterOptions {
    pub models: ModelCatalogue,
    pub cors: CorsSettings,
    pub scenario: Scenario,
    pub seed: u64,
    pub auth: AuthSettings,
    pub control_plane: ControlPlaneSettings,
    pub metrics: bool,
}

impl Default for RouterOptions {
    fn default() -> Self {
        Self {
            models: ModelCatalogue::builtin(),
            cors: CorsSettings::default(),
            scenario: Scenario::default(),
            seed: 0,
            auth: AuthSettings::default(),
            control_plane: ControlPlaneSettings {
                enabled: true,
                token: None,
            },
            metrics: false,
        }
    }
}

pub fn build_router(service: Arc<ChatService>) -> Router {
    build_router_with_options(service, RouterOptions::default()).0
}

/// Builds the router and hands back the state tests need to observe.
pub fn build_router_with_state(service: Arc<ChatService>) -> (Router, Arc<CancelCounter>) {
    let (router, state) = build_router_with_options(service, RouterOptions::default());
    (router, state.cancels)
}

pub fn build_router_with_options(
    service: Arc<ChatService>,
    options: RouterOptions,
) -> (Router, AppState) {
    let boot_scenario = Arc::new(options.scenario.clone());
    let state = AppState {
        service,
        cancels: Arc::new(CancelCounter::default()),
        models: options.models,
        semantic: builtin_artifact(),
        scenario: Arc::new(ArcSwap::new(boot_scenario.clone())),
        boot_scenario,
        seed: options.seed,
        auth: options.auth,
        control_plane: options.control_plane,
        log: Arc::new(RequestLog::default()),
        metrics: Arc::new(Metrics::default()),
    };
    let routes = Router::new()
        .route(
            "/chat/completions",
            post(create_chat_completion).get(list_chat_completions),
        )
        .route(
            "/chat/completions/{completion_id}",
            get(get_chat_completion)
                .post(update_chat_completion)
                .delete(delete_chat_completion),
        )
        .route(
            "/chat/completions/{completion_id}/messages",
            get(get_chat_completion_messages),
        )
        .route("/models", get(list_models))
        .route("/models/{model_id}", get(get_model))
        .with_state(state.clone());

    let root = Router::new()
        .route("/", get(serve_index))
        .route("/health", get(health))
        .route("/ready", get(ready))
        .with_state(state.clone());

    let mut app = root.merge(routes.clone()).nest("/v1", routes);
    if options.metrics {
        app = app.merge(
            Router::new()
                .route("/metrics", get(metrics_endpoint))
                .with_state(state.clone()),
        );
    }
    if state.control_plane.enabled {
        app = app.merge(control::router(state.clone()));
    }

    let app = app
        .fallback(not_found_fallback)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ))
        .layer(middleware::from_fn_with_state(state.clone(), log_request))
        .layer(middleware::from_fn(request_id_layer))
        .layer(trace_layer())
        .layer(cors_layer(&options.cors));

    (app, state)
}

/// Records a redacted line per request for `GET /_mock/requests`.
async fn log_request(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .map(|(name, value)| control::redact(name.as_str(), value.to_str().unwrap_or("")))
        .collect();
    let response = next.run(request).await;
    state.metrics.record_request(response.status().as_u16());
    state.log.record(control::LoggedRequest {
        method,
        path,
        status: response.status().as_u16(),
        model: None,
        stream: false,
        headers,
    });
    response
}

/// Access logging at DEBUG. Only method, path, status and latency are recorded:
/// headers can carry credentials, so they never reach a span.
fn trace_layer() -> tower_http::trace::TraceLayer<
    tower_http::classify::SharedClassifier<tower_http::classify::ServerErrorsAsFailures>,
    fn(&axum::extract::Request) -> tracing::Span,
> {
    fn span(request: &axum::extract::Request) -> tracing::Span {
        tracing::debug_span!(
            "http",
            method = %request.method(),
            path = %request.uri().path(),
        )
    }

    tower_http::trace::TraceLayer::new_for_http().make_span_with(span as fn(&_) -> _)
}

/// Mirrors origin and requested headers so no SDK header list can go stale.
fn cors_layer(settings: &CorsSettings) -> CorsLayer {
    let origins = match settings {
        CorsSettings::MirrorAny => AllowOrigin::mirror_request(),
        CorsSettings::List(list) => AllowOrigin::list(
            list.iter()
                .filter_map(|origin| HeaderValue::from_str(origin).ok())
                .collect::<Vec<_>>(),
        ),
    };
    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods(AllowMethods::list([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::DELETE,
            axum::http::Method::OPTIONS,
        ]))
        .allow_headers(AllowHeaders::mirror_request())
        .expose_headers(ExposeHeaders::list([
            REQUEST_ID,
            VERSION_HEADER,
            header::RETRY_AFTER,
            HeaderName::from_static("x-ratelimit-limit-requests"),
            HeaderName::from_static("x-ratelimit-remaining-requests"),
            HeaderName::from_static("x-ratelimit-reset-requests"),
            SIMULATE_MATCH,
            SIMULATE_DATASET,
            SIMULATE_CASE,
            SIMULATE_VARIANT,
            SIMULATE_PLAN,
        ]))
        .max_age(Duration::from_secs(600))
}

/// Echoes a caller-supplied `x-request-id` or mints one, and stamps the build
/// version, on every response.
async fn request_id_layer(mut request: axum::extract::Request, next: Next) -> Response {
    let incoming = request
        .headers()
        .get(&REQUEST_ID)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let request_id = incoming.unwrap_or_else(|| format!("req_{}", Uuid::new_v4().as_simple()));
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        request.headers_mut().insert(&REQUEST_ID, value.clone());
        let mut response = next.run(request).await;
        let headers = response.headers_mut();
        headers.insert(&REQUEST_ID, value);
        headers.insert(
            &VERSION_HEADER,
            HeaderValue::from_static(env!("CARGO_PKG_VERSION")),
        );
        return response;
    }
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        &VERSION_HEADER,
        HeaderValue::from_static(env!("CARGO_PKG_VERSION")),
    );
    response
}

async fn method_not_allowed() -> Response {
    ApiError::method_not_allowed().into_response()
}

async fn serve_index() -> impl IntoResponse {
    Html(INDEX_HTML)
}

/// Liveness only: never touches the dataset or an upstream backend.
async fn health(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "models": state.models.ids().count(),
        "tokens_per_second": state.service.tokens_per_second().get(),
    }))
}

/// Readiness: everything a probe needs to know the mock can actually answer.
/// Never pings an upstream backend, and is exempt from auth and fault injection.
async fn ready(State(state): State<AppState>) -> Response {
    let scripts = state.service.script_count();
    let scenario = state.scenario.load_full();
    let body = serde_json::json!({
        "status": if scripts > 0 { "ready" } else { "not_ready" },
        "version": env!("CARGO_PKG_VERSION"),
        "scripts": scripts,
        "models": state.models.ids().count(),
        "scenario": scenario.name,
        "tokenizer": state.service.tokenizer_name(),
        "tokens_per_second": state.service.tokens_per_second().get(),
    });
    let status = if scripts > 0 {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(body)).into_response()
}

async fn list_models(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.models.list())
}

async fn get_model(
    State(state): State<AppState>,
    Path(model_id): Path<String>,
) -> Result<Json<crate::models::Model>, ApiError> {
    state
        .models
        .get(&model_id)
        .map(Json)
        .ok_or_else(|| ApiError::model_not_found(&model_id))
}

async fn list_chat_completions(
    State(state): State<AppState>,
    query: Result<Query<ListQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, ApiError> {
    let Query(query) = query?;
    let limit = query.limit.unwrap_or(20).min(100);
    let fetch = limit.saturating_add(1);
    let order = parse_order(query.order.as_deref())?;

    let mut results = state
        .service
        .list(order, query.after.as_deref(), fetch, query.to_filters())
        .await;
    let has_more = results.len() > limit;
    if has_more {
        results.truncate(limit);
    }

    let first_id = results
        .first()
        .map(|completion| completion.completion.id.clone());
    let last_id = results
        .last()
        .map(|completion| completion.completion.id.clone());
    let data: Vec<crate::model::StoredChatCompletionView> = results
        .into_iter()
        .map(|item| item.to_list_item())
        .collect();

    let list = ChatCompletionList {
        object: "list".to_string(),
        data,
        first_id,
        last_id,
        has_more,
    };

    Ok(Json(list).into_response())
}

async fn create_chat_completion(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: Result<Json<ChatCompletionRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(request) = body?;
    crate::http::validate::validate(&request)?;

    let scenario = state.scenario.load_full();
    let directive = Directive::from_headers(
        headers
            .iter()
            .filter_map(|(name, value)| value.to_str().ok().map(|value| (name.as_str(), value))),
    )
    .merge(
        request
            .x_simulate
            .as_ref()
            .map(Directive::from_value)
            .unwrap_or_default(),
    );
    // Per-model latency fills in for a scenario that expresses no opinion, so a
    // catalogue can make "the reasoning model is slower" true without a restart.
    let base = model_timing(&state, &request.model).unwrap_or_else(|| scenario.timing.clone());
    let effective_scenario = Scenario {
        timing: if scenario.timing == Timing::default() {
            base
        } else {
            scenario.timing.clone()
        },
        ..scenario.as_ref().clone()
    };
    let (timing, fault) = directive.apply(&effective_scenario);

    let stream = request.stream;
    let prepared = if directive.case.is_some() || directive.variant.is_some() {
        let profile = state
            .models
            .profile(&request.model)
            .ok_or_else(|| ApiError::model_not_found(&request.model))?;
        let controls = state
            .semantic
            .selection_controls(directive.case.clone(), directive.variant.clone());
        let plan = crate::sim::plan::compile(
            &state.semantic.fixtures,
            &CanonicalRequest::from_chat(&request),
            &controls,
            &SemanticCapabilities::from(profile),
        )
        .map_err(semantic_plan_error)?;
        validate_semantic_schema(&request, &plan)?;
        state
            .service
            .create_semantic_completion(request, &plan)
            .await?
    } else {
        state.service.create_completion(request).await?
    };
    let match_kind = HeaderValue::from_str(&prepared.match_kind)
        .expect("simulation match kinds are valid header values");
    let plan_seed = state.seed ^ plan_seed_of(&prepared.response.id);

    // An http_error fault answers before any streaming starts, the way a real
    // rate limit or outage does.
    if fault.kind == FaultKind::HttpError && fault_fires(&fault, plan_seed) {
        state.metrics.record_fault();
        return Err(http_fault_error(&fault));
    }
    state.metrics.record_completion(stream);

    if !stream {
        let mut response = Json(prepared.response.lean()).into_response();
        response.headers_mut().insert(&SIMULATE_MATCH, match_kind);
        insert_semantic_headers(response.headers_mut(), prepared.semantic.as_ref());
        return Ok(response);
    }

    let semantic = prepared.semantic.clone();
    let plan = StreamPlan::with_profile(
        prepared,
        state.service.tokenizer().as_ref(),
        state.service.tokens_per_second(),
        &timing,
        &fault,
        plan_seed,
    );
    let keep_alive = plan.keep_alive();

    let sse = Sse::new(sse_stream(plan, state.cancels.clone()));
    let mut response = if keep_alive {
        sse.keep_alive(
            axum::response::sse::KeepAlive::new()
                .interval(Duration::from_secs(5))
                .text("ping"),
        )
        .into_response()
    } else {
        sse.into_response()
    };
    let response_headers = response.headers_mut();
    response_headers.insert(&ACCEL_BUFFERING, HeaderValue::from_static("no"));
    response_headers.insert(&SIMULATE_MATCH, match_kind);
    insert_semantic_headers(response_headers, semantic.as_ref());
    Ok(response)
}

fn insert_semantic_headers(
    headers: &mut axum::http::HeaderMap,
    diagnostics: Option<&SemanticDiagnostics>,
) {
    let Some(diagnostics) = diagnostics else {
        return;
    };
    for (name, value) in [
        (&SIMULATE_DATASET, diagnostics.dataset_revision.as_str()),
        (&SIMULATE_CASE, diagnostics.case_id.as_str()),
        (&SIMULATE_VARIANT, diagnostics.variant_id.as_str()),
        (&SIMULATE_PLAN, diagnostics.plan_digest.as_str()),
    ] {
        if let Ok(value) = HeaderValue::from_str(value) {
            headers.insert(name, value);
        }
    }
}

fn semantic_plan_error(error: PlanError) -> ApiError {
    let param = match error {
        PlanError::UnknownCase(_) | PlanError::IncompatibleCase(_) => "x_simulate.case",
        PlanError::UnknownVariant { .. } | PlanError::IncompatibleVariant { .. } => {
            "x_simulate.variant"
        }
        PlanError::UnsupportedEffort { .. } | PlanError::MissingEffortVariant { .. } => {
            "reasoning_effort"
        }
        PlanError::UnsupportedStructuredOutput(_) => "response_format",
        PlanError::NoMatch
        | PlanError::AmbiguousMatch(_)
        | PlanError::InvalidStructuredOutput { .. } => "x_simulate.case",
    };
    ApiError::invalid_request(error.to_string())
        .with_param(param)
        .with_code("semantic_selection_error")
}

fn validate_semantic_schema(
    request: &ChatCompletionRequest,
    plan: &SemanticResponsePlan,
) -> Result<(), ApiError> {
    let structured = plan.output.iter().find_map(|output| match output {
        SemanticOutput::Structured { value, .. } => Some(value),
        _ => None,
    });
    match &request.response_format {
        Some(ResponseFormat::JsonSchema { json_schema }) => {
            let schema = json_schema.schema.as_ref().ok_or_else(|| {
                semantic_schema_error("Invalid response_format: json_schema.schema is required.")
            })?;
            let value = structured.ok_or_else(|| {
                semantic_schema_error(
                    "The selected semantic variant does not contain structured output.",
                )
            })?;
            let validator = jsonschema::validator_for(schema).map_err(|_| {
                semantic_schema_error("Invalid response_format: json_schema.schema cannot compile.")
            })?;
            if validator.validate(value).is_err() {
                return Err(semantic_schema_error(
                    "The selected semantic output does not satisfy response_format.json_schema.",
                ));
            }
        }
        Some(ResponseFormat::JsonObject) => {
            if !structured.is_some_and(serde_json::Value::is_object) {
                return Err(semantic_schema_error(
                    "The selected semantic variant does not contain a JSON object.",
                ));
            }
        }
        Some(ResponseFormat::Text) | None => {}
    }
    Ok(())
}

fn semantic_schema_error(message: &str) -> ApiError {
    ApiError::invalid_request(message)
        .with_param("response_format")
        .with_code("semantic_schema_error")
}

/// Prometheus text exposition; only mounted when --metrics is set.
async fn metrics_endpoint(State(state): State<AppState>) -> Response {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        state.metrics.render(&state.cancels),
    )
        .into_response()
}

/// The pacing a catalogue entry declares, when it declares any.
fn model_timing(state: &AppState, model: &str) -> Option<Timing> {
    let latency = &state.models.profile(model)?.latency;
    if latency.ttft_ms.is_none()
        && latency.tokens_per_second.is_none()
        && latency.jitter_ms.is_none()
    {
        return None;
    }
    Some(Timing {
        ttft_ms: latency.ttft_ms.unwrap_or(0),
        tokens_per_second: latency.tokens_per_second,
        jitter_ms: latency.jitter_ms.unwrap_or(0),
        chunk_tokens: None,
        burst_frames: 0,
    })
}

/// A stable per-request seed, derived from the already-derived completion id.
fn plan_seed_of(id: &str) -> u64 {
    let mut digest = crate::sim::digest::Digest::new();
    digest.field(id.as_bytes());
    digest.finish()
}

fn fault_fires(fault: &crate::sim::scenario::Fault, seed: u64) -> bool {
    if fault.rate >= 1.0 {
        return true;
    }
    if fault.rate <= 0.0 {
        return false;
    }
    use rand::{RngExt, SeedableRng};
    rand::rngs::StdRng::seed_from_u64(seed).random::<f64>() < fault.rate
}

fn http_fault_error(fault: &crate::sim::scenario::Fault) -> ApiError {
    let status = fault
        .status
        .and_then(|status| StatusCode::from_u16(status).ok())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let (message, kind, code) = match status {
        StatusCode::TOO_MANY_REQUESTS => (
            "Rate limit reached for requests. Please try again later.".to_string(),
            "rate_limit_error",
            Some("rate_limit_exceeded"),
        ),
        StatusCode::SERVICE_UNAVAILABLE => (
            "The engine is currently overloaded, please try again later.".to_string(),
            "server_error",
            Some("service_unavailable"),
        ),
        other => (
            format!("The server had an error processing your request ({other})."),
            "server_error",
            None,
        ),
    };
    let mut error = ApiError::new(status, message, kind);
    if let Some(code) = code {
        error = error.with_code(code);
    }
    error.retry_after = fault.retry_after;
    error
}

async fn get_chat_completion(
    State(state): State<AppState>,
    Path(completion_id): Path<String>,
) -> Result<Response, ApiError> {
    state
        .service
        .get(&completion_id)
        .await
        .map(|record| Json(record.completion.stored_view()).into_response())
        .ok_or_else(|| ApiError::not_found(&completion_id))
}

#[derive(Deserialize)]
struct UpdateBody {
    metadata: serde_json::Map<String, serde_json::Value>,
}

async fn update_chat_completion(
    State(state): State<AppState>,
    Path(completion_id): Path<String>,
    body: Result<Json<UpdateBody>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(body) = body?;
    state
        .service
        .update_metadata(&completion_id, body.metadata)
        .await
        .map(|record| Json(record.completion.stored_view()).into_response())
        .ok_or_else(|| ApiError::not_found(&completion_id))
}

async fn delete_chat_completion(
    State(state): State<AppState>,
    Path(completion_id): Path<String>,
) -> Result<Response, ApiError> {
    state
        .service
        .delete(&completion_id)
        .await
        .map(|result| Json(result).into_response())
        .ok_or_else(|| ApiError::not_found(&completion_id))
}

#[derive(Default, Deserialize)]
struct ListQuery {
    after: Option<String>,
    limit: Option<usize>,
    order: Option<String>,
    model: Option<String>,
    #[serde(default)]
    #[serde(flatten)]
    metadata: MetadataFilters,
}

#[derive(Default, Deserialize)]
struct MessageQuery {
    after: Option<String>,
    limit: Option<usize>,
    order: Option<String>,
}

#[derive(Default)]
struct MetadataFilters {
    pairs: Vec<(String, String)>,
}

impl MetadataFilters {
    fn entries(&self) -> &[(String, String)] {
        &self.pairs
    }
}

impl ListQuery {
    fn to_filters(&self) -> ListFilters {
        ListFilters {
            model: self.model.clone(),
            metadata: self.metadata.entries().to_vec(),
        }
    }
}

impl<'de> Deserialize<'de> for MetadataFilters {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct MetadataVisitor;

        impl<'de> Visitor<'de> for MetadataVisitor {
            type Value = MetadataFilters;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("metadata query parameters")
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(MetadataFilters::default())
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut pairs = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, String>()? {
                    if let Some(inner) = key
                        .strip_prefix("metadata[")
                        .and_then(|rest| rest.strip_suffix(']'))
                    {
                        pairs.push((inner.to_string(), value));
                    }
                }
                Ok(MetadataFilters { pairs })
            }
        }

        deserializer.deserialize_any(MetadataVisitor)
    }
}

async fn get_chat_completion_messages(
    State(state): State<AppState>,
    Path(completion_id): Path<String>,
    query: Result<Query<MessageQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, ApiError> {
    let Query(query) = query?;
    let limit = query.limit.unwrap_or(20).min(100);
    let fetch = limit.saturating_add(1);
    let order = parse_order(query.order.as_deref())?;

    let mut messages = state
        .service
        .messages(&completion_id, order, query.after.as_deref(), fetch)
        .await
        .ok_or_else(|| ApiError::not_found(&completion_id))?;

    let has_more = messages.len() > limit;
    if has_more {
        messages.truncate(limit);
    }

    let first_id = messages.first().map(|message| message.id.clone());
    let last_id = messages.last().map(|message| message.id.clone());
    let list = ChatCompletionMessageList {
        object: "list".to_string(),
        data: messages,
        first_id,
        last_id,
        has_more,
    };

    Ok(Json(list).into_response())
}

/// Shared parsing for the `order` query parameter used by both list routes.
fn parse_order(raw: Option<&str>) -> Result<SortOrder, ApiError> {
    match raw.map(SortOrder::from_str).transpose() {
        Ok(value) => Ok(value.unwrap_or(SortOrder::Ascending)),
        Err(_) => Err(ApiError::invalid_request(
            "Invalid value for 'order': expected 'asc' or 'desc'.",
        )
        .with_param("order")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_page_is_embedded_in_the_binary() {
        assert!(INDEX_HTML.contains("<html") || INDEX_HTML.contains("<!DOCTYPE"));
    }
}
