use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use bytes::{Bytes, BytesMut};
use chrono::Utc;
use futures_util::StreamExt;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

use marg_core::RequestLogEntry;
use marg_providers::{ChatRequest, ChatUsage};

use crate::auth;
use crate::errors::ChatError;
use crate::quota;
use crate::sse;
use crate::state::AppState;

const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;

pub async fn chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ChatError> {
    if body.len() > MAX_REQUEST_BYTES {
        return Err(ChatError::BadRequest(format!(
            "request body too large: {} bytes",
            body.len()
        )));
    }

    let cached = auth::authenticate(&state, &headers).await?;
    let key = cached.key.clone();
    let budget = cached.budget.clone();

    let req = ChatRequest::parse(&body)
        .map_err(ChatError::Provider)?;

    quota::check(&state, &key.id, &budget, &req).await?;

    let started = Instant::now();
    if req.stream {
        stream_response(state, key, req, started).await
    } else {
        non_stream_response(state, key, req, started).await
    }
}

async fn non_stream_response(
    state: AppState,
    key: marg_core::MargKey,
    req: ChatRequest,
    started: Instant,
) -> Result<Response, ChatError> {
    let model_requested = req.model.clone();
    let provider_resp = state.provider.chat_completion(req).await?;
    let latency_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;

    let pricing = state.pricing.load();
    let actual_cost = pricing.cost_usd(
        &provider_resp.model,
        provider_resp.usage.prompt_tokens,
        provider_resp.usage.completion_tokens,
    );

    let provider_name = state.provider.provider_name().to_string();
    let log = RequestLogEntry {
        id: uuid::Uuid::new_v4().to_string(),
        timestamp: Utc::now(),
        key_id: key.id.clone(),
        principal_id: key.principal.id.clone(),
        provider: provider_name,
        model: provider_resp.model.clone(),
        input_tokens: provider_resp.usage.prompt_tokens,
        output_tokens: provider_resp.usage.completion_tokens,
        cost_usd: actual_cost,
        latency_ms,
        status: provider_resp.status,
        stream: false,
        error: None,
    };

    let storage = state.storage.clone();
    let key_id = key.id.clone();
    let day = Utc::now().date_naive();
    if let Err(e) = storage.add_spend(&key_id, day, actual_cost).await {
        tracing::warn!(?e, key_id = %key_id, "failed to add spend after non-stream response");
    }
    if let Err(e) = storage.append_request_log(log).await {
        tracing::warn!(?e, "failed to append request log after non-stream response");
    }
    let _ = model_requested;

    let status = StatusCode::from_u16(provider_resp.status)
        .unwrap_or(StatusCode::OK);
    let response = Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(provider_resp.body))
        .map_err(|e| ChatError::Internal(format!("build response: {}", e)))?;
    Ok(response)
}

async fn stream_response(
    state: AppState,
    key: marg_core::MargKey,
    req: ChatRequest,
    started: Instant,
) -> Result<Response, ChatError> {
    let model = req.model.clone();
    let provider_name = state.provider.provider_name().to_string();
    let upstream = state.provider.chat_completion_stream(req).await?;
    let provider_status = upstream.status;

    let (tx, rx) = mpsc::unbounded_channel::<Result<Bytes, std::io::Error>>();

    let storage = state.storage.clone();
    let pricing = state.pricing.clone();
    let key_id = key.id.clone();
    let principal_id = key.principal.id.clone();

    tokio::spawn(async move {
        let mut byte_stream = upstream.byte_stream;
        let mut buffer = BytesMut::new();
        let mut usage = ChatUsage::default();
        let mut stream_error: Option<String> = None;
        let mut client_alive = true;

        while let Some(chunk) = byte_stream.next().await {
            match chunk {
                Ok(bytes) => {
                    if client_alive {
                        if tx.send(Ok(bytes.clone())).is_err() {
                            client_alive = false;
                        }
                    }
                    buffer.extend_from_slice(&bytes);
                    while let Some(event) = sse::take_event(&mut buffer) {
                        if let Some(found) = sse::parse_usage(&event) {
                            usage = found;
                        }
                    }
                }
                Err(e) => {
                    let msg = format!("upstream stream error: {}", e);
                    if client_alive {
                        let _ = tx.send(Err(std::io::Error::new(std::io::ErrorKind::Other, msg.clone())));
                    }
                    stream_error = Some(msg);
                    break;
                }
            }
        }
        drop(tx);

        let latency_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let cost = pricing
            .load()
            .cost_usd(&model, usage.prompt_tokens, usage.completion_tokens);

        let entry = RequestLogEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            key_id: key_id.clone(),
            principal_id,
            provider: provider_name,
            model: model.clone(),
            input_tokens: usage.prompt_tokens,
            output_tokens: usage.completion_tokens,
            cost_usd: cost,
            latency_ms,
            status: provider_status,
            stream: true,
            error: stream_error,
        };

        let day = Utc::now().date_naive();
        if let Err(e) = storage.add_spend(&key_id, day, cost).await {
            tracing::warn!(?e, key_id = %key_id, "failed to add spend after stream");
        }
        if let Err(e) = storage.append_request_log(entry).await {
            tracing::warn!(?e, "failed to append request log after stream");
        }
    });

    let status = StatusCode::from_u16(provider_status).unwrap_or(StatusCode::OK);
    let body = Body::from_stream(UnboundedReceiverStream::new(rx));
    let response = Response::builder()
        .status(status)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .header("connection", "keep-alive")
        .body(body)
        .map_err(|e| ChatError::Internal(format!("build streaming response: {}", e)))?;
    Ok(response)
}
