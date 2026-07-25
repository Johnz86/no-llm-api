//! The single error shape every response uses.
//!
//! All four members of the spec's `Error` schema (`openapi.yaml:37735`, schema
//! `Error`) are always serialised, including explicit nulls: openai-node reads
//! `code` and `param` straight off the body, so omitting them surfaces
//! `undefined` in the GUI instead of a usable message.

use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::service::ServiceError;

#[derive(Debug, Serialize)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub message: String,
    pub r#type: String,
    pub param: Option<String>,
    pub code: Option<String>,
}

/// An error paired with the status and headers it must be delivered with.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub body: ErrorBody,
    pub www_authenticate: bool,
    /// Seconds for a `Retry-After` header, which is what client backoff reads.
    pub retry_after: Option<u64>,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>, kind: &str) -> Self {
        Self {
            status,
            body: ErrorBody {
                message: message.into(),
                r#type: kind.to_string(),
                param: None,
                code: None,
            },
            www_authenticate: false,
            retry_after: None,
        }
    }

    pub fn with_code(mut self, code: &str) -> Self {
        self.body.code = Some(code.to_string());
        self
    }

    pub fn with_param(mut self, param: &str) -> Self {
        self.body.param = Some(param.to_string());
        self
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message, "invalid_request_error")
    }

    pub fn not_found(resource: &str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            format!("No such resource: '{resource}'"),
            "invalid_request_error",
        )
    }

    pub fn model_not_found(model: &str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            format!("The model '{model}' does not exist or you do not have access to it."),
            "invalid_request_error",
        )
        .with_code("model_not_found")
        .with_param("model")
    }

    pub fn unknown_route(path: &str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            format!("Unknown request URL: {path}. Please check the URL for typos."),
            "invalid_request_error",
        )
        .with_code("unknown_url")
    }

    pub fn method_not_allowed() -> Self {
        Self::new(
            StatusCode::METHOD_NOT_ALLOWED,
            "Not allowed. Please check the HTTP method for this route.",
            "invalid_request_error",
        )
        .with_code("method_not_allowed")
    }

    pub fn server_error(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message, "server_error")
    }
}

impl From<ServiceError> for ApiError {
    fn from(error: ServiceError) -> Self {
        ApiError::server_error(error.to_string())
    }
}

/// Maps axum's body-extraction failures onto the spec's error shape.
impl From<JsonRejection> for ApiError {
    fn from(rejection: JsonRejection) -> Self {
        let status = match &rejection {
            JsonRejection::JsonDataError(_) | JsonRejection::JsonSyntaxError(_) => {
                StatusCode::BAD_REQUEST
            }
            JsonRejection::MissingJsonContentType(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            _ => StatusCode::BAD_REQUEST,
        };
        let message = match &rejection {
            JsonRejection::MissingJsonContentType(_) => {
                "Expected a request body with 'Content-Type: application/json'.".to_string()
            }
            other => other.body_text(),
        };
        ApiError::new(status, message, "invalid_request_error")
    }
}

impl From<QueryRejection> for ApiError {
    fn from(rejection: QueryRejection) -> Self {
        ApiError::invalid_request(rejection.body_text())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let retry_after = self.retry_after;
        let www_authenticate = self.www_authenticate;
        let mut response = (self.status, Json(ErrorEnvelope { error: self.body })).into_response();
        if www_authenticate {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        if let Some(seconds) = retry_after
            && let Ok(value) = HeaderValue::from_str(&seconds.to_string())
        {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        response
    }
}

/// Router fallback: an unmatched path must still answer with JSON.
pub async fn not_found_fallback(uri: axum::http::Uri) -> Response {
    ApiError::unknown_route(uri.path()).into_response()
}
