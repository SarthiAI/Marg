use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use thiserror::Error;

/// Single error type for every admin endpoint. Each variant maps to one HTTP
/// status and one stable `code` value so external tooling can program against
/// them.
#[derive(Debug, Error)]
pub enum AdminError {
    #[error("missing Authorization header")]
    MissingAuth,

    #[error("invalid Authorization header")]
    InvalidAuth,

    #[error("unauthorized")]
    Unauthorized,

    #[error("not found")]
    NotFound,

    #[error("invalid request: {0}")]
    BadRequest(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl AdminError {
    fn code(&self) -> &'static str {
        match self {
            AdminError::MissingAuth | AdminError::InvalidAuth | AdminError::Unauthorized => {
                "unauthorized"
            }
            AdminError::NotFound => "not_found",
            AdminError::BadRequest(_) => "bad_request",
            AdminError::Conflict(_) => "conflict",
            AdminError::Storage(_) => "storage_error",
            AdminError::Internal(_) => "internal_error",
        }
    }

    fn http_status(&self) -> StatusCode {
        match self {
            AdminError::MissingAuth | AdminError::InvalidAuth | AdminError::Unauthorized => {
                StatusCode::UNAUTHORIZED
            }
            AdminError::NotFound => StatusCode::NOT_FOUND,
            AdminError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AdminError::Conflict(_) => StatusCode::CONFLICT,
            AdminError::Storage(_) => StatusCode::SERVICE_UNAVAILABLE,
            AdminError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for AdminError {
    fn into_response(self) -> Response {
        let status = self.http_status();
        let body = Json(json!({
            "error": {
                "code": self.code(),
                "message": self.to_string(),
            }
        }));
        (status, body).into_response()
    }
}

impl From<marg_storage::StorageError> for AdminError {
    fn from(e: marg_storage::StorageError) -> Self {
        use marg_storage::StorageError;
        match e {
            StorageError::NotFound => AdminError::NotFound,
            StorageError::Duplicate(msg) => AdminError::Conflict(msg),
            StorageError::Backend(msg) => AdminError::Storage(msg),
        }
    }
}
