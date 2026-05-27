use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use bytes::{Bytes, BytesMut};
use chrono::Utc;
use futures_util::StreamExt;
use kavach_core::{PermitToken, Verdict};
use serde_json::Value;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

use marg_core::{RequestLogEntry, RouteAttempt};
use marg_providers::{ChatRequest, ChatUsage};

use crate::auth;
use crate::errors::ChatError;
use crate::kavach::{
    self, action_context_from_request, audit_request_lifecycle, encode_permit_header,
    parse_caller_headers, verdict_kind_str, KavachMode, RequestLifecycle,
};
use crate::observability;
use crate::proxy;
use crate::quota;
use crate::sse;
use crate::state::AppState;
use crate::write_batcher::WriteJob;

const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;

pub async fn chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ChatError> {
    let decision_started = Instant::now();

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

    // Build the Kavach action context and call the gate. The gate's verdict
    // drives observe vs enforce branching below. The original request body is
    // kept for the audit snapshot (whether it lands in the chain depends on
    // [kavach].include_prompts).
    let pricing_for_estimate = state.pricing.load();
    let estimated_cost_for_gate = pricing_for_estimate.cost_usd(
        &req.model,
        req.estimated_input_tokens,
        req.max_output_tokens.unwrap_or(1024) as u64,
    );
    let caller_headers = parse_caller_headers(&headers);
    let ctx = action_context_from_request(
        &key,
        &req,
        estimated_cost_for_gate,
        caller_headers,
        &state.kavach.session_store,
    )
    .await;
    let real_verdict = state.kavach.gate.evaluate(&ctx).await;
    let mode = *state.kavach.mode.load_full();
    let effective_verdict = apply_mode(&ctx, &real_verdict, mode);

    let mut lifecycle = RequestLifecycle::new_from_request(&key, &req, estimated_cost_for_gate);
    lifecycle.prompt_redacted_or_omitted = !*state.kavach.include_prompts.load_full();
    let raw_request_value: Option<Value> = Some(req.raw.clone());

    // Short-circuit refusals + invalidations in enforce mode. We still emit
    // an audit entry so the operator can grep the chain for blocked attempts.
    // In observe mode the gate's would-refuse becomes a logged event and the
    // request continues to the upstream; `effective_verdict` is then Permit
    // and neither of the arms below matches.
    match &effective_verdict {
        Verdict::Refuse(reason) => {
            lifecycle.error_class = Some(format!("kavach_refuse:{}", reason.code));
            lifecycle.error_message = Some(reason.reason.clone());
            audit_request_lifecycle(
                &state.kavach.audit_chain,
                &ctx,
                &real_verdict,
                &effective_verdict,
                &lifecycle,
                mode.as_str(),
                *state.kavach.include_prompts.load_full(),
                raw_request_value.as_ref(),
            );
            return Err(ChatError::KavachRefuse {
                code: reason.code.to_string(),
                evaluator: reason.evaluator.clone(),
                reason: reason.reason.clone(),
            });
        }
        Verdict::Invalidate(scope) => {
            // Drift (or any other evaluator that produces Invalidate) flipped
            // the session. Three things follow before we return 403:
            //   1. Flip the persisted session row to `invalidated = true` so
            //      subsequent gate evaluations on this same Marg key refuse
            //      even if drift never triggers again.
            //   2. Drop the key from the local auth cache so the next request
            //      goes back to the DB and revalidates status.
            //   3. Append a `marg.key_event.v1` chain entry so the operator
            //      can grep the audit chain for invalidations.
            if let Err(e) = state
                .kavach
                .session_store
                .invalidate(ctx.session.session_id)
                .await
            {
                tracing::warn!(
                    ?e,
                    session_id = %ctx.session.session_id,
                    "failed to mark kavach session as invalidated"
                );
            }
            state.key_cache.invalidate_all();
            kavach::emit_key_event(
                &state.kavach.audit_chain,
                "drift",
                &key.id,
                kavach::KeyEventKind::Invalidated,
                Some(scope.reason.as_str()),
            );

            lifecycle.error_class = Some("kavach_invalidate".to_string());
            lifecycle.error_message = Some(scope.reason.clone());
            audit_request_lifecycle(
                &state.kavach.audit_chain,
                &ctx,
                &real_verdict,
                &effective_verdict,
                &lifecycle,
                mode.as_str(),
                *state.kavach.include_prompts.load_full(),
                raw_request_value.as_ref(),
            );
            return Err(ChatError::KavachInvalidate {
                evaluator: scope.evaluator.clone(),
                reason: scope.reason.clone(),
            });
        }
        Verdict::Permit(_) => {}
    }

    let pick_seed = uuid::Uuid::new_v4().as_u128() as u64;
    let routing_snapshot = state.routing.load();
    let resolution = routing_snapshot
        .resolve(&req.model, key.team.as_deref(), pick_seed)
        .map_err(|e| match e {
            marg_core::RoutingError::NoRouteMatched { model } => ChatError::NoRoute { model },
            marg_core::RoutingError::MisconfiguredRoute(msg) => ChatError::Internal(msg),
        })?;

    observability::record_target(&resolution.primary.provider, &resolution.primary.model);

    let quota_model = resolution.primary.model.clone();
    let reservation = quota::check(&state, &key.id, &budget, &req, &quota_model).await?;

    state
        .metrics
        .decision_duration_seconds
        .observe(decision_started.elapsed().as_secs_f64());

    let permit_token = match &effective_verdict {
        Verdict::Permit(token) => Some(token.clone()),
        _ => None,
    };

    let started = Instant::now();
    if req.stream {
        stream_response(
            state,
            key,
            budget,
            req,
            resolution,
            reservation,
            started,
            ctx,
            real_verdict,
            effective_verdict,
            permit_token,
            lifecycle,
            mode,
            raw_request_value,
        )
        .await
    } else {
        non_stream_response(
            state,
            key,
            budget,
            req,
            resolution,
            reservation,
            started,
            ctx,
            real_verdict,
            effective_verdict,
            permit_token,
            lifecycle,
            mode,
            raw_request_value,
        )
        .await
    }
}

#[allow(clippy::too_many_arguments)]
async fn non_stream_response(
    state: AppState,
    key: marg_core::MargKey,
    budget: marg_core::BudgetSpec,
    req: ChatRequest,
    resolution: marg_core::RouteResolution,
    reservation: quota::QuotaReservation,
    started: Instant,
    ctx: kavach_core::ActionContext,
    real_verdict: Verdict,
    effective_verdict: Verdict,
    permit_token: Option<PermitToken>,
    mut lifecycle: RequestLifecycle,
    mode: KavachMode,
    raw_request_value: Option<Value>,
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
            populate_lifecycle_error(&mut lifecycle, &e);
            audit_request_lifecycle(
                &state.kavach.audit_chain,
                &ctx,
                &real_verdict,
                &effective_verdict,
                &lifecycle,
                mode.as_str(),
                *state.kavach.include_prompts.load_full(),
                raw_request_value.as_ref(),
            );
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
        team: key.team.clone(),
        attempts: attempts.clone(),
    };

    if actual_cost > 0.0 {
        if state
            .write_batcher
            .enqueue(WriteJob::AddSpend {
                key_id: key.id.clone(),
                day: reservation.day,
                amount_usd: actual_cost,
            })
            .is_err()
        {
            return Err(ChatError::StorageOverloaded);
        }
    }
    if state
        .write_batcher
        .enqueue(WriteJob::RequestLog(log))
        .is_err()
    {
        return Err(ChatError::StorageOverloaded);
    }

    populate_lifecycle_success(
        &mut lifecycle,
        &final_target,
        &provider_resp.model,
        provider_resp.status,
        provider_resp.usage,
        actual_cost,
        latency_ms,
        &attempts,
        false,
    );
    audit_request_lifecycle(
        &state.kavach.audit_chain,
        &ctx,
        &real_verdict,
        &effective_verdict,
        &lifecycle,
        mode.as_str(),
        *state.kavach.include_prompts.load_full(),
        raw_request_value.as_ref(),
    );

    let status = StatusCode::from_u16(provider_resp.status).unwrap_or(StatusCode::OK);
    let mut response = Response::builder()
        .status(status)
        .header("content-type", "application/json");
    if let Some(builder_headers) = response.headers_mut() {
        attach_route_headers(builder_headers, &final_target, &attempts);
        attach_kavach_headers(builder_headers, mode, &real_verdict, &state, permit_token.as_ref());
    }
    response
        .body(Body::from(provider_resp.body))
        .map_err(|e| ChatError::Internal(format!("build response: {}", e)))
}

#[allow(clippy::too_many_arguments)]
async fn stream_response(
    state: AppState,
    key: marg_core::MargKey,
    budget: marg_core::BudgetSpec,
    req: ChatRequest,
    resolution: marg_core::RouteResolution,
    reservation: quota::QuotaReservation,
    started: Instant,
    ctx: kavach_core::ActionContext,
    real_verdict: Verdict,
    effective_verdict: Verdict,
    permit_token: Option<PermitToken>,
    mut lifecycle: RequestLifecycle,
    mode: KavachMode,
    raw_request_value: Option<Value>,
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
            populate_lifecycle_error(&mut lifecycle, &e);
            audit_request_lifecycle(
                &state.kavach.audit_chain,
                &ctx,
                &real_verdict,
                &effective_verdict,
                &lifecycle,
                mode.as_str(),
                *state.kavach.include_prompts.load_full(),
                raw_request_value.as_ref(),
            );
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

    let hot = state.hot.clone();
    let pricing = state.pricing.clone();
    let metrics = state.metrics.clone();
    let write_batcher = state.write_batcher.clone();
    let kavach_runtime = state.kavach.clone();
    let key_id = key.id.clone();
    let principal_id = key.principal.id.clone();
    let team_for_log = key.team.clone();
    let attempts_for_log = attempts.clone();
    let reservation_day = reservation.day;
    let reservation_cost = reservation.estimated_cost_usd;
    let reservation_enforced = reservation.enforced;
    let budget_for_gauge = budget.clone();
    let final_target_for_audit = final_target.clone();
    let ctx_for_audit = ctx.clone();
    let real_verdict_for_audit = real_verdict.clone();
    let effective_verdict_for_audit = effective_verdict.clone();
    let raw_request_value_for_audit = raw_request_value.clone();
    let mode_str = mode.as_str();

    metrics.stream_started(&provider_name);
    let stream_provider = provider_name.clone();
    let stream_model = route_model.clone();

    tokio::spawn(async move {
        let mut byte_stream = provider_stream.byte_stream;
        let mut buffer = BytesMut::new();
        let mut usage = ChatUsage::default();
        let mut stream_error: Option<String> = None;
        let mut client_disconnected = false;

        while let Some(chunk) = byte_stream.next().await {
            match chunk {
                Ok(bytes) => {
                    if tx.send(Ok(bytes.clone())).is_err() {
                        client_disconnected = true;
                        metrics.record_provider_error(&stream_provider, "client_disconnect");
                        break;
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
                    let _ = tx.send(Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        msg.clone(),
                    )));
                    stream_error = Some(msg);
                    break;
                }
            }
        }
        drop(byte_stream);
        drop(tx);

        let latency = started.elapsed();
        let latency_ms = latency.as_millis().min(u64::MAX as u128) as u64;
        let cost = pricing
            .load()
            .cost_usd(&stream_model, usage.prompt_tokens, usage.completion_tokens);

        let final_status = if client_disconnected {
            499
        } else {
            provider_status
        };
        metrics.record_request(
            &stream_provider,
            &stream_model,
            final_status,
            latency.as_secs_f64(),
        );
        metrics.record_tokens(&stream_model, usage.prompt_tokens, usage.completion_tokens);
        metrics.stream_finished(&stream_provider);

        let logged_error = stream_error.clone().or_else(|| {
            if client_disconnected {
                Some("client disconnected, upstream cancelled".to_string())
            } else {
                None
            }
        });

        let entry = RequestLogEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            key_id: key_id.clone(),
            principal_id,
            provider: stream_provider.clone(),
            model: stream_model.clone(),
            input_tokens: usage.prompt_tokens,
            output_tokens: usage.completion_tokens,
            cost_usd: cost,
            latency_ms,
            status: final_status,
            stream: true,
            error: logged_error.clone(),
            team: team_for_log.clone(),
            attempts: attempts_for_log.clone(),
        };

        if reservation_enforced {
            let delta = cost - reservation_cost;
            if let Err(e) = hot.settle_budget(&key_id, reservation_day, delta).await {
                tracing::warn!(?e, key_id = %key_id, "failed to settle hot budget after stream");
            }
        }
        if cost > 0.0 {
            if let Err(_) = write_batcher.enqueue(WriteJob::AddSpend {
                key_id: key_id.clone(),
                day: reservation_day,
                amount_usd: cost,
            }) {
                tracing::warn!(key_id = %key_id, "write batcher overflow: dropped streaming spend");
            }
        }
        if let Err(_) = write_batcher.enqueue(WriteJob::RequestLog(entry)) {
            tracing::warn!(key_id = %key_id, "write batcher overflow: dropped streaming request log");
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

        let mut lifecycle = lifecycle.clone();
        populate_lifecycle_success(
            &mut lifecycle,
            &final_target_for_audit,
            &stream_model,
            final_status,
            usage,
            cost,
            latency_ms,
            &attempts_for_log,
            client_disconnected,
        );
        if let Some(err_msg) = logged_error {
            lifecycle.error_class = Some(if client_disconnected {
                "client_disconnect".to_string()
            } else {
                "upstream_stream_error".to_string()
            });
            lifecycle.error_message = Some(err_msg);
        }
        audit_request_lifecycle(
            &kavach_runtime.audit_chain,
            &ctx_for_audit,
            &real_verdict_for_audit,
            &effective_verdict_for_audit,
            &lifecycle,
            mode_str,
            *kavach_runtime.include_prompts.load_full(),
            raw_request_value_for_audit.as_ref(),
        );
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
        attach_kavach_headers(builder_headers, mode, &real_verdict, &state, permit_token.as_ref());
    }
    response
        .body(body)
        .map_err(|e| ChatError::Internal(format!("build streaming response: {}", e)))
}

/// In observe mode a real Refuse/Invalidate becomes an effective Permit so
/// the request proceeds and the operator sees the would-refuse event in the
/// audit chain. In enforce mode the real verdict is the effective verdict.
fn apply_mode(
    ctx: &kavach_core::ActionContext,
    real_verdict: &Verdict,
    mode: KavachMode,
) -> Verdict {
    match mode {
        KavachMode::Enforce => real_verdict.clone(),
        KavachMode::Observe => match real_verdict {
            Verdict::Permit(_) => real_verdict.clone(),
            Verdict::Refuse(_) | Verdict::Invalidate(_) => {
                tracing::info!(
                    evaluation_id = %ctx.evaluation_id,
                    action = %ctx.action.name,
                    verdict = %verdict_kind_str(real_verdict),
                    "observe-only: would have blocked this action"
                );
                Verdict::Permit(PermitToken::new(ctx.evaluation_id, ctx.action.name.clone()))
            }
        },
    }
}

fn populate_lifecycle_success(
    lifecycle: &mut RequestLifecycle,
    target: &marg_core::ResolvedTarget,
    response_model: &str,
    status: u16,
    usage: ChatUsage,
    actual_cost_usd: f64,
    latency_ms: u64,
    attempts: &[RouteAttempt],
    client_disconnected: bool,
) {
    lifecycle.provider = Some(target.provider.clone());
    lifecycle.route_model = Some(response_model.to_string());
    lifecycle.provider_status = Some(status);
    lifecycle.failovers = attempts
        .iter()
        .filter(|a| !matches!(a.outcome, marg_core::AttemptOutcome::Success))
        .count() as u32;
    lifecycle.attempts = attempts
        .iter()
        .map(|a| serde_json::to_value(a).unwrap_or(Value::Null))
        .collect();
    lifecycle.response_status = Some(status);
    lifecycle.response_input_tokens = usage.prompt_tokens;
    lifecycle.response_output_tokens = usage.completion_tokens;
    lifecycle.response_actual_cost_usd = actual_cost_usd;
    lifecycle.response_latency_ms = latency_ms;
    lifecycle.response_client_disconnect = client_disconnected;
}

fn populate_lifecycle_error(lifecycle: &mut RequestLifecycle, err: &ChatError) {
    lifecycle.error_class = Some(error_class_for(err).to_string());
    lifecycle.error_message = Some(err.to_string());
    let attempts = err.attempts();
    lifecycle.failovers = attempts
        .iter()
        .filter(|a| !matches!(a.outcome, marg_core::AttemptOutcome::Success))
        .count() as u32;
    lifecycle.attempts = attempts
        .iter()
        .map(|a| serde_json::to_value(a).unwrap_or(Value::Null))
        .collect();
}

fn error_class_for(err: &ChatError) -> &'static str {
    match err {
        ChatError::Upstream { .. } | ChatError::UpstreamStream { .. } => "upstream_status",
        ChatError::Provider(_) | ChatError::ProviderWithAttempts { .. } => "provider_error",
        ChatError::AllAttemptsFailed { .. } => "all_attempts_failed",
        ChatError::Storage(_) | ChatError::StorageOverloaded => "storage",
        ChatError::HotStore(_) => "hot_store",
        ChatError::RateLimited { .. } => "rate_limited",
        ChatError::BudgetExceeded { .. } => "budget_exceeded",
        ChatError::KavachRefuse { .. } => "kavach_refuse",
        ChatError::KavachInvalidate { .. } => "kavach_invalidate",
        _ => "other",
    }
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
        ChatError::KavachRefuse { .. } | ChatError::KavachInvalidate { .. } => 403,
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
    let failovers = attempts
        .iter()
        .filter(|a| !matches!(a.outcome, marg_core::AttemptOutcome::Success))
        .count();
    if let Ok(v) = HeaderValue::from_str(&failovers.to_string()) {
        headers.insert("x-marg-failovers", v);
    }
    if let Ok(v) = HeaderValue::from_str(&attempts.len().to_string()) {
        headers.insert("x-marg-attempts", v);
    }
}

fn attach_kavach_headers(
    headers: &mut HeaderMap,
    mode: KavachMode,
    real_verdict: &Verdict,
    state: &AppState,
    permit_token: Option<&PermitToken>,
) {
    if let Ok(v) = HeaderValue::from_str(mode.as_str()) {
        headers.insert("x-kavach-mode", v);
    }
    if let Ok(v) = HeaderValue::from_str(verdict_kind_str(real_verdict)) {
        headers.insert("x-kavach-verdict", v);
    }
    if let Ok(v) = HeaderValue::from_str(crate::KAVACH_CORE_VERSION) {
        headers.insert("x-kavach-version", v);
    }
    if *state.kavach.expose_permit_to_caller.load_full() {
        if let Some(token) = permit_token {
            if let Some(encoded) = encode_permit_header(token) {
                if let Ok(v) = HeaderValue::from_str(&encoded) {
                    headers.insert("x-kavach-permit", v);
                }
            }
        }
    }
    let _ = kavach::verdict_kind_str; // keep import alive
}

