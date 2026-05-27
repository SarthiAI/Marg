use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use serde_json::json;
use thiserror::Error;

use marg_core::RouteAttempt;
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

    #[error("rate limit exceeded: rpm {rpm}")]
    RateLimited { rpm: u32 },

    #[error("storage error: {0}")]
    Storage(String),

    #[error("hot store error: {0}")]
    HotStore(String),

    #[error("storage overloaded: write batcher queue is full")]
    StorageOverloaded,

    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),

    #[error("provider {provider} error after {attempts} attempts: {source}", attempts = .attempts.len())]
    ProviderWithAttempts {
        provider: String,
        #[source]
        source: ProviderError,
        attempts: Vec<RouteAttempt>,
    },

    #[error("upstream provider {provider} returned status {status} (non-retriable)")]
    Upstream {
        status: u16,
        provider: String,
        body: Bytes,
        attempts: Vec<RouteAttempt>,
    },

    #[error("upstream stream from provider {provider} returned status {status}")]
    UpstreamStream {
        status: u16,
        provider: String,
        attempts: Vec<RouteAttempt>,
    },

    #[error("all upstream attempts failed ({count})", count = .attempts.len())]
    AllAttemptsFailed {
        attempts: Vec<RouteAttempt>,
        last_error: Option<String>,
    },

    #[error("no route matched for model '{model}' and no default provider configured")]
    NoRoute { model: String },

    #[error("provider '{0}' is not configured")]
    UnknownProvider(String),

    #[error("invalid request: {0}")]
    BadRequest(String),

    #[error("internal error: {0}")]
    Internal(String),

    /// Kavach refused the action in enforce mode. Surfaces as 403 with
    /// `x-marg-reason: kavach_refuse` and a machine-readable Kavach refuse
    /// code in `x-kavach-refuse-code`.
    #[error("kavach refused: [{code}] {evaluator}: {reason}")]
    KavachRefuse {
        code: String,
        evaluator: String,
        reason: String,
    },

    /// Kavach revoked broader authority for this principal/session. Same 403
    /// shape as `KavachRefuse`, but `x-marg-reason: kavach_invalidate`.
    #[error("kavach invalidated session: {evaluator}: {reason}")]
    KavachInvalidate {
        evaluator: String,
        reason: String,
    },
}

impl ChatError {
    pub fn attempts(&self) -> Vec<RouteAttempt> {
        match self {
            ChatError::ProviderWithAttempts { attempts, .. } => attempts.clone(),
            ChatError::Upstream { attempts, .. } => attempts.clone(),
            ChatError::UpstreamStream { attempts, .. } => attempts.clone(),
            ChatError::AllAttemptsFailed { attempts, .. } => attempts.clone(),
            _ => Vec::new(),
        }
    }
}

impl IntoResponse for ChatError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            ChatError::MissingAuthHeader | ChatError::InvalidAuthHeader | ChatError::Unauthorized => {
                (StatusCode::UNAUTHORIZED, "unauthorized")
            }
            ChatError::BudgetExceeded { .. } => (StatusCode::TOO_MANY_REQUESTS, "budget_exceeded"),
            ChatError::RateLimited { .. } => (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
            ChatError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            ChatError::NoRoute { .. } | ChatError::UnknownProvider(_) => {
                (StatusCode::BAD_REQUEST, "no_route")
            }
            ChatError::Storage(_) | ChatError::Internal(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, "internal_error")
            }
            ChatError::HotStore(_) => (StatusCode::SERVICE_UNAVAILABLE, "hot_store_unreachable"),
            ChatError::StorageOverloaded => {
                (StatusCode::SERVICE_UNAVAILABLE, "storage_overloaded")
            }
            ChatError::Provider(err) | ChatError::ProviderWithAttempts { source: err, .. } => {
                if matches!(err, ProviderError::Timeout) {
                    (StatusCode::GATEWAY_TIMEOUT, "upstream_timeout")
                } else {
                    (StatusCode::BAD_GATEWAY, "upstream_error")
                }
            }
            ChatError::Upstream { status, .. } | ChatError::UpstreamStream { status, .. } => {
                let s = StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY);
                (s, "upstream_error")
            }
            ChatError::AllAttemptsFailed { .. } => (StatusCode::BAD_GATEWAY, "all_attempts_failed"),
            ChatError::KavachRefuse { .. } => (StatusCode::FORBIDDEN, "kavach_refuse"),
            ChatError::KavachInvalidate { .. } => (StatusCode::FORBIDDEN, "kavach_invalidate"),
        };

        let mut headers = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(code) {
            headers.insert("x-marg-reason", v);
        }
        if let ChatError::KavachRefuse { code, evaluator, .. } = &self {
            if let Ok(v) = HeaderValue::from_str(code) {
                headers.insert("x-kavach-refuse-code", v);
            }
            if let Ok(v) = HeaderValue::from_str(evaluator) {
                headers.insert("x-kavach-evaluator", v);
            }
        }
        if let ChatError::KavachInvalidate { evaluator, .. } = &self {
            if let Ok(v) = HeaderValue::from_str(evaluator) {
                headers.insert("x-kavach-evaluator", v);
            }
        }
        let attempts = self.attempts();
        if !attempts.is_empty() {
            if let Ok(v) = HeaderValue::from_str(&attempts.len().to_string()) {
                headers.insert("x-marg-attempts", v);
            }
        }

        let body = Json(json!({
            "error": {
                "code": code,
                "message": self.to_string(),
                "attempts": attempts,
            }
        }));

        (status, headers, body).into_response()
    }
}
