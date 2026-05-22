use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use bytes::{Bytes, BytesMut};
use chrono::Utc;
use futures_util::StreamExt;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

use marg_core::{RequestLogEntry, RouteAttempt};
use marg_providers::{ChatRequest, ChatUsage};

use crate::auth;
use crate::errors::ChatError;
use crate::observability;
use crate::proxy;
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
    observability::record_principal(&key.id, &key.principal.id);

    let req = ChatRequest::parse(&body).map_err(ChatError::Provider)?;

    let pick_seed = uuid::Uuid::new_v4().as_u128() as u64;
    let resolution = state
        .routing
        .resolve(&req.model, key.team.as_deref(), pick_seed)
        .map_err(|e| match e {
            marg_core::RoutingError::NoRouteMatched { model } => ChatError::NoRoute { model },
            marg_core::RoutingError::MisconfiguredRoute(msg) => ChatError::Internal(msg),
        })?;

    observability::record_target(&resolution.primary.provider, &resolution.primary.model);

    let quota_model = resolution.primary.model.clone();
    let reservation = quota::check(&state, &key.id, &budget, &req, &quota_model).await?;

    let started = Instant::now();
    if req.stream {
        stream_response(state, key, budget, req, resolution, reservation, started).await
    } else {
        non_stream_response(state, key, budget, req, resolution, reservation, started).await
    }
}

async fn non_stream_response(
    state: AppState,
    key: marg_core::MargKey,
    budget: marg_core::BudgetSpec,
    req: ChatRequest,
    resolution: marg_core::RouteResolution,
    reservation: quota::QuotaReservation,
    started: Instant,
) -> Result<Response, ChatError> {
    let outcome = match proxy::call_with_failover_non_stream(
        &state,
        resolution.primary.clone(),
        resolution.fallbacks.clone(),
        &req,
    )
    .await
    {
        Ok(o) => o,
        Err(e) => {
            settle_reservation(&state, &key.id, &reservation, 0.0).await;
            record_error_metric(&state, &resolution.primary, &e, started.elapsed());
            return Err(e);
        }
    };
    let provider_resp = outcome.value;
    let final_target = outcome.target;
    let mut attempts = outcome.previous_failures;
    attempts.push(outcome.log_entry);
    let latency = started.elapsed();
    let latency_ms = latency.as_millis().min(u64::MAX as u128) as u64;

    observability::record_target(&final_target.provider, &provider_resp.model);

    let pricing = state.pricing.load();
    let actual_cost = pricing.cost_usd(
        &provider_resp.model,
        provider_resp.usage.prompt_tokens,
        provider_resp.usage.completion_tokens,
    );

    settle_reservation(&state, &key.id, &reservation, actual_cost).await;
    update_budget_gauge(&state, &key.id, &budget);

    state.metrics.record_request(
        &final_target.provider,
        &provider_resp.model,
        provider_resp.status,
        latency.as_secs_f64(),
    );
    state.metrics.record_tokens(
        &provider_resp.model,
        provider_resp.usage.prompt_tokens,
        provider_resp.usage.completion_tokens,
    );
    observability::record_outcome(provider_resp.status, latency_ms);

    let log = RequestLogEntry {
        id: uuid::Uuid::new_v4().to_string(),
        timestamp: Utc::now(),
        key_id: key.id.clone(),
        principal_id: key.principal.id.clone(),
        provider: final_target.provider.clone(),
        model: provider_resp.model.clone(),
        input_tokens: provider_resp.usage.prompt_tokens,
        output_tokens: provider_resp.usage.completion_tokens,
        cost_usd: actual_cost,
        latency_ms,
        status: provider_resp.status,
        stream: false,
        error: None,
        attempts: attempts.clone(),
    };

    let storage = state.storage.clone();
    let key_id = key.id.clone();
    let day = reservation.day;
    if let Err(e) = storage.add_spend(&key_id, day, actual_cost).await {
        tracing::warn!(?e, key_id = %key_id, "failed to add spend after non-stream response");
    }
    if let Err(e) = storage.append_request_log(log).await {
        tracing::warn!(?e, "failed to append request log after non-stream response");
    }

    let status = StatusCode::from_u16(provider_resp.status).unwrap_or(StatusCode::OK);
    let mut response = Response::builder()
        .status(status)
        .header("content-type", "application/json");
    if let Some(builder_headers) = response.headers_mut() {
        attach_route_headers(builder_headers, &final_target, &attempts);
    }
    response
        .body(Body::from(provider_resp.body))
        .map_err(|e| ChatError::Internal(format!("build response: {}", e)))
}

async fn stream_response(
    state: AppState,
    key: marg_core::MargKey,
    budget: marg_core::BudgetSpec,
    req: ChatRequest,
    resolution: marg_core::RouteResolution,
    reservation: quota::QuotaReservation,
    started: Instant,
) -> Result<Response, ChatError> {
    let outcome = match proxy::call_with_failover_stream(
        &state,
        resolution.primary.clone(),
        resolution.fallbacks.clone(),
        &req,
    )
    .await
    {
        Ok(o) => o,
        Err(e) => {
            settle_reservation(&state, &key.id, &reservation, 0.0).await;
            record_error_metric(&state, &resolution.primary, &e, started.elapsed());
            return Err(e);
        }
    };
    let provider_stream = outcome.value;
    let final_target = outcome.target;
    let mut attempts = outcome.previous_failures;
    attempts.push(outcome.log_entry.clone());
    let provider_status = provider_stream.status;
    let route_model = final_target.model.clone();
    let provider_name = final_target.provider.clone();

    observability::record_target(&provider_name, &route_model);

    let (tx, rx) = mpsc::unbounded_channel::<Result<Bytes, std::io::Error>>();

    let storage = state.storage.clone();
    let hot = state.hot.clone();
    let pricing = state.pricing.clone();
    let metrics = state.metrics.clone();
    let key_id = key.id.clone();
    let principal_id = key.principal.id.clone();
    let attempts_for_log = attempts.clone();
    let reservation_day = reservation.day;
    let reservation_cost = reservation.estimated_cost_usd;
    let reservation_enforced = reservation.enforced;
    let budget_for_gauge = budget.clone();

    metrics.stream_started(&provider_name);
    let stream_provider = provider_name.clone();
    let stream_model = route_model.clone();

    tokio::spawn(async move {
        let mut byte_stream = provider_stream.byte_stream;
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
                        let _ = tx.send(Err(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            msg.clone(),
                        )));
                    }
                    stream_error = Some(msg);
                    break;
                }
            }
        }
        drop(tx);

        let latency = started.elapsed();
        let latency_ms = latency.as_millis().min(u64::MAX as u128) as u64;
        let cost = pricing
            .load()
            .cost_usd(&stream_model, usage.prompt_tokens, usage.completion_tokens);

        metrics.record_request(
            &stream_provider,
            &stream_model,
            provider_status,
            latency.as_secs_f64(),
        );
        metrics.record_tokens(&stream_model, usage.prompt_tokens, usage.completion_tokens);
        metrics.stream_finished(&stream_provider);

        let entry = RequestLogEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            key_id: key_id.clone(),
            principal_id,
            provider: stream_provider,
            model: stream_model,
            input_tokens: usage.prompt_tokens,
            output_tokens: usage.completion_tokens,
            cost_usd: cost,
            latency_ms,
            status: provider_status,
            stream: true,
            error: stream_error,
            attempts: attempts_for_log,
        };

        if reservation_enforced {
            let delta = cost - reservation_cost;
            if let Err(e) = hot.settle_budget(&key_id, reservation_day, delta).await {
                tracing::warn!(?e, key_id = %key_id, "failed to settle hot budget after stream");
            }
        }
        if let Err(e) = storage.add_spend(&key_id, reservation_day, cost).await {
            tracing::warn!(?e, key_id = %key_id, "failed to add spend after stream");
        }
        if let Err(e) = storage.append_request_log(entry).await {
            tracing::warn!(?e, "failed to append request log after stream");
        }

        if budget_for_gauge.daily_usd > 0.0 {
            match hot.current_spend(&key_id, reservation_day).await {
                Ok(spent) => {
                    let remaining = (budget_for_gauge.daily_usd - spent).max(0.0);
                    metrics.set_budget_remaining(&key_id, remaining);
                }
                Err(e) => tracing::warn!(?e, key_id = %key_id, "could not refresh budget gauge"),
            }
        }
    });

    observability::record_outcome(provider_status, started.elapsed().as_millis().min(u64::MAX as u128) as u64);

    let status = StatusCode::from_u16(provider_status).unwrap_or(StatusCode::OK);
    let body = Body::from_stream(UnboundedReceiverStream::new(rx));
    let mut response = Response::builder()
        .status(status)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .header("connection", "keep-alive");
    if let Some(builder_headers) = response.headers_mut() {
        attach_route_headers(builder_headers, &final_target, &attempts);
    }
    response
        .body(body)
        .map_err(|e| ChatError::Internal(format!("build streaming response: {}", e)))
}

async fn settle_reservation(
    state: &AppState,
    key_id: &str,
    reservation: &quota::QuotaReservation,
    actual_cost: f64,
) {
    if !reservation.enforced {
        return;
    }
    let delta = actual_cost - reservation.estimated_cost_usd;
    if let Err(e) = state.hot.settle_budget(key_id, reservation.day, delta).await {
        tracing::warn!(?e, %key_id, "hot store settle_budget failed");
    }
}

fn update_budget_gauge(state: &AppState, key_id: &str, budget: &marg_core::BudgetSpec) {
    if budget.daily_usd <= 0.0 {
        return;
    }
    let day = Utc::now().date_naive();
    let hot = state.hot.clone();
    let metrics = state.metrics.clone();
    let key_id = key_id.to_string();
    let limit = budget.daily_usd;
    tokio::spawn(async move {
        match hot.current_spend(&key_id, day).await {
            Ok(spent) => {
                let remaining = (limit - spent).max(0.0);
                metrics.set_budget_remaining(&key_id, remaining);
            }
            Err(e) => tracing::warn!(?e, key_id = %key_id, "could not refresh budget gauge"),
        }
    });
}

fn record_error_metric(
    state: &AppState,
    primary: &marg_core::ResolvedTarget,
    err: &ChatError,
    elapsed: std::time::Duration,
) {
    let status = match err {
        ChatError::Upstream { status, .. } | ChatError::UpstreamStream { status, .. } => *status,
        ChatError::Provider(ref e) | ChatError::ProviderWithAttempts { source: ref e, .. } => {
            match e {
                marg_providers::ProviderError::Upstream { status, .. } => *status,
                marg_providers::ProviderError::Timeout => 504,
                _ => 502,
            }
        }
        ChatError::AllAttemptsFailed { .. } => 502,
        _ => 500,
    };
    state.metrics.record_request(
        &primary.provider,
        &primary.model,
        status,
        elapsed.as_secs_f64(),
    );
    observability::record_outcome(status, elapsed.as_millis().min(u64::MAX as u128) as u64);
}

fn attach_route_headers(
    headers: &mut HeaderMap,
    target: &marg_core::ResolvedTarget,
    attempts: &[RouteAttempt],
) {
    if let Ok(v) = HeaderValue::from_str(&target.provider) {
        headers.insert("x-marg-provider", v);
    }
    if let Ok(v) = HeaderValue::from_str(&target.model) {
        headers.insert("x-marg-model", v);
    }
    let failovers = attempts.iter().filter(|a| !matches!(a.outcome, marg_core::AttemptOutcome::Success)).count();
    if let Ok(v) = HeaderValue::from_str(&failovers.to_string()) {
        headers.insert("x-marg-failovers", v);
    }
    if let Ok(v) = HeaderValue::from_str(&attempts.len().to_string()) {
        headers.insert("x-marg-attempts", v);
    }
}
