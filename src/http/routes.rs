use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderName, HeaderValue, header};
use axum::middleware::{self, Next};
use axum::response::sse::Sse;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, Visitor};
use tower_http::cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer, ExposeHeaders};
use uuid::Uuid;

use crate::config::CorsSettings;
use crate::http::error::{ApiError, not_found_fallback};
use crate::model::{
    ChatCompletionList, ChatCompletionMessageList, ChatCompletionRequest, ChatCompletionResponse,
};
use crate::models::ModelCatalogue;
use crate::service::ChatService;
use crate::sim::stream::{CancelCounter, StreamPlan, sse_stream};
use crate::store::{ListFilters, SortOrder};

/// The page served at `/`, embedded so the binary works from any directory.
const INDEX_HTML: &str = include_str!("../../index.html");

const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");
const ACCEL_BUFFERING: HeaderName = HeaderName::from_static("x-accel-buffering");
/// Which rung of the matching ladder produced the reply; for test triage only.
const SIMULATE_MATCH: HeaderName = HeaderName::from_static("x-simulate-match");

#[derive(Clone)]
pub struct AppState {
    pub service: Arc<ChatService>,
    pub cancels: Arc<CancelCounter>,
    pub models: ModelCatalogue,
}

pub fn build_router(service: Arc<ChatService>) -> Router {
    build_router_with_state(service).0
}

/// Builds the router and hands back the shared counters tests need to observe.
pub fn build_router_with_state(service: Arc<ChatService>) -> (Router, Arc<CancelCounter>) {
    build_router_with_options(service, ModelCatalogue::builtin(), &CorsSettings::default())
}

pub fn build_router_with_options(
    service: Arc<ChatService>,
    models: ModelCatalogue,
    cors: &CorsSettings,
) -> (Router, Arc<CancelCounter>) {
    let cancels = Arc::new(CancelCounter::default());
    let state = AppState {
        service,
        cancels: cancels.clone(),
        models,
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
        .with_state(state);

    let app = root
        .merge(routes.clone())
        .nest("/v1", routes)
        .fallback(not_found_fallback)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(middleware::from_fn(request_id_layer))
        .layer(cors_layer(cors));

    (app, cancels)
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
            header::RETRY_AFTER,
            HeaderName::from_static("x-ratelimit-limit-requests"),
            HeaderName::from_static("x-ratelimit-remaining-requests"),
            HeaderName::from_static("x-ratelimit-reset-requests"),
        ]))
        .max_age(Duration::from_secs(600))
}

/// Echoes a caller-supplied `x-request-id` or mints one, on every response.
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
        response.headers_mut().insert(&REQUEST_ID, value);
        return response;
    }
    next.run(request).await
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
    let data: Vec<ChatCompletionResponse> = results
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
    body: Result<Json<ChatCompletionRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(request) = body?;
    if request.messages.is_empty() {
        return Err(ApiError::invalid_request(
            "Invalid value for 'messages': expected a non-empty array.",
        )
        .with_param("messages"));
    }
    let stream = request.stream;
    let prepared = state.service.create_completion(request).await?;
    let match_kind = HeaderValue::from_static(prepared.match_kind.as_str());
    if !stream {
        let mut response = Json(prepared.response).into_response();
        response.headers_mut().insert(&SIMULATE_MATCH, match_kind);
        return Ok(response);
    }

    let plan = StreamPlan::new(
        prepared,
        state.service.tokenizer().as_ref(),
        state.service.tokens_per_second(),
    );

    let mut response = Sse::new(sse_stream(plan, state.cancels.clone())).into_response();
    let headers = response.headers_mut();
    headers.insert(&ACCEL_BUFFERING, HeaderValue::from_static("no"));
    headers.insert(&SIMULATE_MATCH, match_kind);
    Ok(response)
}

async fn get_chat_completion(
    State(state): State<AppState>,
    Path(completion_id): Path<String>,
) -> Result<Response, ApiError> {
    state
        .service
        .get(&completion_id)
        .await
        .map(|record| Json(record.completion).into_response())
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
        .map(|record| Json(record.completion).into_response())
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
