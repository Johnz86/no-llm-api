use std::convert::Infallible;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::model::{
    ChatCompletionList, ChatCompletionMessageList, ChatCompletionRequest, ChatCompletionResponse,
};
use crate::service::{ChatService, chunk_from_delta, delta_for_text, empty_delta, usage_chunk};
use crate::store::{ListFilters, SortOrder};

#[derive(Clone)]
pub struct AppState {
    pub service: Arc<ChatService>,
}

pub fn build_router(service: Arc<ChatService>) -> Router {
    let state = AppState { service };
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
        .with_state(state.clone());

    Router::new()
        .route("/", get(serve_index))
        .merge(routes.clone())
        .nest("/v1", routes)
}

async fn serve_index() -> impl IntoResponse {
    match tokio::fs::read_to_string("index.html").await {
        Ok(content) => Html(content).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "index.html not found").into_response(),
    }
}

async fn list_chat_completions(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(20).min(100);
    let fetch = limit.saturating_add(1);
    let order = match query.order.as_deref().map(SortOrder::from_str).transpose() {
        Ok(value) => value.unwrap_or(SortOrder::Ascending),
        Err(_) => return bad_request("invalid order"),
    };

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

    Json(list).into_response()
}

async fn create_chat_completion(
    State(state): State<AppState>,
    Json(request): Json<ChatCompletionRequest>,
) -> Response {
    let stream = request.stream;
    let prepared = state.service.create_completion(request).await;
    if !stream {
        return Json(prepared.response).into_response();
    }

    let (sender, receiver) = mpsc::channel::<Result<Event, Infallible>>(32);
    let response = prepared.response.clone();
    let tokens = prepared.tokens;
    let include_usage = prepared.include_usage_chunk;
    let rate = state.service.tokens_per_second();
    let tokenizer = state.service.tokenizer();
    tokio::spawn(async move {
        let delay = Duration::from_secs_f64(1.0 / rate.get() as f64);
        let mut rendered = String::new();
        let mut buffer: Vec<u32> = Vec::new();

        for (index, token) in tokens.into_iter().enumerate() {
            buffer.push(token);
            let decoded = match tokenizer.decode(buffer.clone()) {
                Ok(text) => text,
                Err(error) => {
                    tracing::error!(target: "no_llm_api", ?error, "failed to decode token stream");
                    return;
                }
            };
            let delta = &decoded[rendered.len()..];
            if !delta.is_empty() {
                let chunk = chunk_from_delta(
                    &response,
                    delta_for_text(delta.to_string(), index == 0),
                    None,
                );
                match Event::default().json_data(&chunk) {
                    Ok(event) => {
                        if sender.send(Ok(event)).await.is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        tracing::error!(target: "no_llm_api", ?error, "failed to serialize chunk");
                        return;
                    }
                }
            }
            rendered = decoded;
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
        }

        let finish = response
            .choices
            .first()
            .and_then(|choice| choice.finish_reason.clone());
        let final_chunk = chunk_from_delta(&response, empty_delta(), finish);
        if let Ok(event) = Event::default().json_data(&final_chunk) {
            let _ = sender.send(Ok(event)).await;
        }
        if include_usage {
            let usage = usage_chunk(&response);
            if let Ok(event) = Event::default().json_data(&usage) {
                let _ = sender.send(Ok(event)).await;
            }
        }
        let _ = sender.send(Ok(Event::default().data("[DONE]"))).await;
    });

    Sse::new(ReceiverStream::new(receiver))
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("ping"),
        )
        .into_response()
}

async fn get_chat_completion(
    State(state): State<AppState>,
    Path(completion_id): Path<String>,
) -> impl IntoResponse {
    match state.service.get(&completion_id).await {
        Some(record) => Json(record.completion).into_response(),
        None => not_found(&completion_id),
    }
}

#[derive(Deserialize)]
struct UpdateBody {
    metadata: serde_json::Map<String, serde_json::Value>,
}

async fn update_chat_completion(
    State(state): State<AppState>,
    Path(completion_id): Path<String>,
    Json(body): Json<UpdateBody>,
) -> impl IntoResponse {
    match state
        .service
        .update_metadata(&completion_id, body.metadata)
        .await
    {
        Some(record) => Json(record.completion).into_response(),
        None => not_found(&completion_id),
    }
}

async fn delete_chat_completion(
    State(state): State<AppState>,
    Path(completion_id): Path<String>,
) -> impl IntoResponse {
    match state.service.delete(&completion_id).await {
        Some(result) => Json(result).into_response(),
        None => not_found(&completion_id),
    }
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
    Query(query): Query<MessageQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(20).min(100);
    let fetch = limit.saturating_add(1);
    let order = match query.order.as_deref().map(SortOrder::from_str).transpose() {
        Ok(value) => value.unwrap_or(SortOrder::Ascending),
        Err(_) => return bad_request("invalid order"),
    };

    let mut messages = match state
        .service
        .messages(&completion_id, order, query.after.as_deref(), fetch)
        .await
    {
        Some(list) => list,
        None => return not_found(&completion_id),
    };

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

    Json(list).into_response()
}

#[derive(Serialize)]
struct ErrorResponse {
    error: ErrorBody,
}

#[derive(Serialize)]
struct ErrorBody {
    message: String,
    r#type: String,
}

fn not_found(resource: &str) -> Response {
    let body = ErrorResponse {
        error: ErrorBody {
            message: format!("resource '{}' was not found", resource),
            r#type: "invalid_request_error".to_string(),
        },
    };
    (StatusCode::NOT_FOUND, Json(body)).into_response()
}

fn bad_request(message: &str) -> Response {
    let body = ErrorResponse {
        error: ErrorBody {
            message: message.to_string(),
            r#type: "invalid_request_error".to_string(),
        },
    };
    (StatusCode::BAD_REQUEST, Json(body)).into_response()
}
