//! Optional bearer-token enforcement.
//!
//! Decision D4: `off` by default so nothing regresses, `any-bearer` because every
//! surveyed GUI insists on a non-empty key field, and `keys` with an explicit
//! forbidden list because key-rotation UIs branch on 401 versus 403.

use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::config::AuthMode;
use crate::http::error::ApiError;
use crate::http::routes::AppState;

/// Paths that authentication guards. Everything else stays open so a browser can
/// load the page and a probe can read liveness.
fn is_guarded(path: &str) -> bool {
    let path = path.strip_prefix("/v1").unwrap_or(path);
    path.starts_with("/chat/completions") || path.starts_with("/models")
}

pub async fn require_auth(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if state.auth.mode == AuthMode::Off || !is_guarded(request.uri().path()) {
        return next.run(request).await;
    }

    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|token| !token.is_empty());

    let Some(token) = presented else {
        return missing_key().into_response();
    };

    if state
        .auth
        .forbidden_keys
        .iter()
        .any(|forbidden| forbidden == token)
    {
        return forbidden_key(token).into_response();
    }

    let accepted = match state.auth.mode {
        AuthMode::Off => true,
        AuthMode::AnyBearer => true,
        AuthMode::Keys => state.auth.keys.iter().any(|key| key == token),
    };

    if accepted {
        next.run(request).await
    } else {
        incorrect_key(token).into_response()
    }
}

fn missing_key() -> ApiError {
    let mut error = ApiError::new(
        StatusCode::UNAUTHORIZED,
        "You didn't provide an API key. You need to provide your API key in an Authorization \
         header using Bearer auth (i.e. Authorization: Bearer YOUR_KEY).",
        "invalid_request_error",
    );
    error.www_authenticate = true;
    error
}

fn incorrect_key(token: &str) -> ApiError {
    let mut error = ApiError::new(
        StatusCode::UNAUTHORIZED,
        format!("Incorrect API key provided: {}.", mask(token)),
        "invalid_request_error",
    )
    .with_code("invalid_api_key");
    error.www_authenticate = true;
    error
}

fn forbidden_key(token: &str) -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        format!(
            "The API key {} is not allowed to access this resource.",
            mask(token)
        ),
        "invalid_request_error",
    )
    .with_code("access_denied")
}

/// Never echo a key: keep the first and last few characters only.
fn mask(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() <= 8 {
        return "*".repeat(chars.len().max(3));
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 2..].iter().collect();
    format!("{head}***{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_api_surface_is_guarded() {
        assert!(is_guarded("/chat/completions"));
        assert!(is_guarded("/v1/chat/completions"));
        assert!(is_guarded("/v1/models"));
        assert!(is_guarded("/models/mock-gpt-4o"));
        assert!(!is_guarded("/health"));
        assert!(!is_guarded("/"));
        assert!(!is_guarded("/_mock/scenario"));
    }

    #[test]
    fn masking_never_reveals_the_middle_of_a_key() {
        let masked = mask("sk-abcdefghijklmnop");
        assert!(masked.starts_with("sk-a"));
        assert!(masked.ends_with("op"));
        assert!(!masked.contains("defghij"));
        assert_eq!(mask("short"), "*****");
    }
}
