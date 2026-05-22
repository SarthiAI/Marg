use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tracing::{field::Empty, Instrument, Span};
use uuid::Uuid;

use crate::state::AppState;

/// Header carried in and out of every Marg HTTP response so log lines and
/// client errors can be correlated.
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Axum middleware that assigns a request id (either honoured from the inbound
/// header or freshly minted), opens a span carrying it plus the eventual
/// `key_id` / `principal_id` / `provider` / `model` / `status` fields, and
/// echoes the id back to the client.
pub async fn request_context_layer(mut req: Request<Body>, next: Next) -> Response {
    let incoming = req
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let request_id = match incoming {
        Some(s) if !s.is_empty() && s.len() <= 128 => s,
        _ => Uuid::new_v4().to_string(),
    };

    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();

    if let Ok(v) = HeaderValue::from_str(&request_id) {
        req.headers_mut().insert(REQUEST_ID_HEADER, v);
    }

    let span = tracing::info_span!(
        target: "marg.request",
        "http.request",
        request_id = %request_id,
        http.method = %method,
        http.path = %path,
        key_id = Empty,
        principal_id = Empty,
        provider = Empty,
        model = Empty,
        status = Empty,
        latency_ms = Empty,
    );

    let id_for_response = request_id.clone();
    let mut response = next.run(req).instrument(span).await;
    if let Ok(v) = HeaderValue::from_str(&id_for_response) {
        response.headers_mut().insert(REQUEST_ID_HEADER, v);
    }
    response
}

/// Records `key_id` and `principal_id` on the current tracing span. Called by
/// the chat handler as soon as the request is authenticated.
pub fn record_principal(key_id: &str, principal_id: &str) {
    let span = Span::current();
    span.record("key_id", key_id);
    span.record("principal_id", principal_id);
}

/// Records the resolved upstream `provider` / `model` on the current span.
pub fn record_target(provider: &str, model: &str) {
    let span = Span::current();
    span.record("provider", provider);
    span.record("model", model);
}

/// Records final `status` / `latency_ms` on the current span and emits a
/// single structured access log line. Operators tail Marg's JSON logs and
/// filter on `target=marg.access` to get one row per finished request.
pub fn record_outcome(status: u16, latency_ms: u64) {
    let span = Span::current();
    span.record("status", status);
    span.record("latency_ms", latency_ms);
    tracing::info!(
        target: "marg.access",
        status,
        latency_ms,
        "chat_completion finished"
    );
}

pub async fn metrics_handler(State(state): State<AppState>) -> Response {
    match state.metrics.render() {
        Ok((content_type, body)) => {
            let mut headers = HeaderMap::new();
            if let Ok(ct) = HeaderValue::from_str(&content_type) {
                headers.insert(axum::http::header::CONTENT_TYPE, ct);
            }
            (StatusCode::OK, headers, body).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to encode prometheus metrics");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "metrics encoding failed",
            )
                .into_response()
        }
    }
}
