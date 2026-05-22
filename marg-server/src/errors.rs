use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use thiserror::Error;

use marg_providers::ProviderError;

#[derive(Debug, Error)]
pub enum ChatError {
    #[error("missing Authorization header")]
    MissingAuthHeader,

    #[error("invalid Authorization header")]
    InvalidAuthHeader,

    #[error("unauthorized")]
    Unauthorized,

    #[error("budget exceeded: spent {spent_usd} of {daily_usd} USD")]
    BudgetExceeded { spent_usd: f64, daily_usd: f64 },

    #[error("storage error: {0}")]
    Storage(String),

    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),

    #[error("invalid request: {0}")]
    BadRequest(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl IntoResponse for ChatError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            ChatError::MissingAuthHeader | ChatError::InvalidAuthHeader | ChatError::Unauthorized => {
                (StatusCode::UNAUTHORIZED, "unauthorized")
            }
            ChatError::BudgetExceeded { .. } => (StatusCode::TOO_MANY_REQUESTS, "budget_exceeded"),
            ChatError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            ChatError::Storage(_) | ChatError::Internal(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, "internal_error")
            }
            ChatError::Provider(err) => {
                if matches!(err, ProviderError::Timeout) {
                    (StatusCode::GATEWAY_TIMEOUT, "upstream_timeout")
                } else {
                    (StatusCode::BAD_GATEWAY, "upstream_error")
                }
            }
        };

        let mut headers = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(code) {
            headers.insert("x-marg-reason", v);
        }

        let body = Json(json!({
            "error": {
                "code": code,
                "message": self.to_string(),
            }
        }));

        (status, headers, body).into_response()
    }
}
