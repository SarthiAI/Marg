use std::sync::Arc;
use std::time::Instant;

use marg_core::{AttemptOutcome, ResolvedTarget, RouteAttempt};
use marg_providers::{ChatCompletionsClient, ChatRequest, ChatResponse, ChatStream, ProviderError};

use crate::errors::ChatError;
use crate::state::AppState;

pub struct AttemptResult<T> {
    pub target: ResolvedTarget,
    pub value: T,
    pub log_entry: RouteAttempt,
    pub previous_failures: Vec<RouteAttempt>,
}

pub async fn call_with_failover_non_stream(
    state: &AppState,
    primary: ResolvedTarget,
    fallbacks: Vec<ResolvedTarget>,
    req: &ChatRequest,
) -> Result<AttemptResult<ChatResponse>, ChatError> {
    let mut attempts: Vec<RouteAttempt> = Vec::new();
    let mut last_error: Option<ChatError> = None;
    let mut last_provider: Option<String> = None;
    let chain: Vec<ResolvedTarget> = std::iter::once(primary).chain(fallbacks).collect();
    for target in chain {
        if let Some(prev) = &last_provider {
            state.metrics.record_failover(prev, &target.provider);
        }
        let provider = resolve_client(state, &target.provider)?;
        let mut attempt_req = req.clone();
        attempt_req.set_target_model(&target.model);
        let started = Instant::now();
        match provider.chat_completion(attempt_req).await {
            Ok(resp) => {
                let latency_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                if (200..300).contains(&resp.status) {
                    let log = RouteAttempt {
                        provider: target.provider.clone(),
                        model: target.model.clone(),
                        status: resp.status,
                        latency_ms,
                        outcome: AttemptOutcome::Success,
                        error: None,
                    };
                    return Ok(AttemptResult {
                        target,
                        value: resp,
                        log_entry: log,
                        previous_failures: attempts,
                    });
                }
                let outcome = classify_status(resp.status);
                state
                    .metrics
                    .record_provider_error(&target.provider, error_kind(outcome));
                let error_msg = decode_body_excerpt(&resp.body);
                attempts.push(RouteAttempt {
                    provider: target.provider.clone(),
                    model: target.model.clone(),
                    status: resp.status,
                    latency_ms,
                    outcome,
                    error: Some(error_msg.clone()),
                });
                if !outcome.is_retriable() {
                    return Err(ChatError::Upstream {
                        status: resp.status,
                        provider: target.provider,
                        body: resp.body,
                        attempts,
                    });
                }
                last_error = Some(ChatError::Upstream {
                    status: resp.status,
                    provider: target.provider.clone(),
                    body: resp.body,
                    attempts: Vec::new(),
                });
                last_provider = Some(target.provider.clone());
            }
            Err(err) => {
                let latency_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                let outcome = classify_provider_error(&err);
                state
                    .metrics
                    .record_provider_error(&target.provider, error_kind(outcome));
                let status = match &err {
                    ProviderError::Upstream { status, .. } => *status,
                    ProviderError::Timeout => 504,
                    _ => 502,
                };
                attempts.push(RouteAttempt {
                    provider: target.provider.clone(),
                    model: target.model.clone(),
                    status,
                    latency_ms,
                    outcome,
                    error: Some(err.to_string()),
                });
                if !outcome.is_retriable() {
                    return Err(ChatError::ProviderWithAttempts {
                        provider: target.provider,
                        source: err,
                        attempts,
                    });
                }
                last_error = Some(ChatError::Provider(err));
                last_provider = Some(target.provider.clone());
            }
        }
    }
    Err(ChatError::AllAttemptsFailed {
        attempts,
        last_error: last_error.map(|e| e.to_string()),
    })
}

pub async fn call_with_failover_stream(
    state: &AppState,
    primary: ResolvedTarget,
    fallbacks: Vec<ResolvedTarget>,
    req: &ChatRequest,
) -> Result<AttemptResult<ChatStream>, ChatError> {
    let mut attempts: Vec<RouteAttempt> = Vec::new();
    let mut last_error: Option<ChatError> = None;
    let mut last_provider: Option<String> = None;
    let chain: Vec<ResolvedTarget> = std::iter::once(primary).chain(fallbacks).collect();
    for target in chain {
        if let Some(prev) = &last_provider {
            state.metrics.record_failover(prev, &target.provider);
        }
        let provider = resolve_client(state, &target.provider)?;
        let mut attempt_req = req.clone();
        attempt_req.set_target_model(&target.model);
        let started = Instant::now();
        match provider.chat_completion_stream(attempt_req).await {
            Ok(stream) => {
                let latency_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                if (200..300).contains(&stream.status) {
                    let log = RouteAttempt {
                        provider: target.provider.clone(),
                        model: target.model.clone(),
                        status: stream.status,
                        latency_ms,
                        outcome: AttemptOutcome::Success,
                        error: None,
                    };
                    return Ok(AttemptResult {
                        target,
                        value: stream,
                        log_entry: log,
                        previous_failures: attempts,
                    });
                }
                let outcome = classify_status(stream.status);
                state
                    .metrics
                    .record_provider_error(&target.provider, error_kind(outcome));
                attempts.push(RouteAttempt {
                    provider: target.provider.clone(),
                    model: target.model.clone(),
                    status: stream.status,
                    latency_ms,
                    outcome,
                    error: Some(format!("upstream returned status {}", stream.status)),
                });
                if !outcome.is_retriable() {
                    return Err(ChatError::UpstreamStream {
                        status: stream.status,
                        provider: target.provider,
                        attempts,
                    });
                }
                last_provider = Some(target.provider.clone());
            }
            Err(err) => {
                let latency_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                let outcome = classify_provider_error(&err);
                state
                    .metrics
                    .record_provider_error(&target.provider, error_kind(outcome));
                let status = match &err {
                    ProviderError::Upstream { status, .. } => *status,
                    ProviderError::Timeout => 504,
                    _ => 502,
                };
                attempts.push(RouteAttempt {
                    provider: target.provider.clone(),
                    model: target.model.clone(),
                    status,
                    latency_ms,
                    outcome,
                    error: Some(err.to_string()),
                });
                if !outcome.is_retriable() {
                    return Err(ChatError::ProviderWithAttempts {
                        provider: target.provider,
                        source: err,
                        attempts,
                    });
                }
                last_error = Some(ChatError::Provider(err));
                last_provider = Some(target.provider.clone());
            }
        }
    }
    Err(ChatError::AllAttemptsFailed {
        attempts,
        last_error: last_error.map(|e| e.to_string()),
    })
}

fn resolve_client(
    state: &AppState,
    provider: &str,
) -> Result<Arc<dyn ChatCompletionsClient>, ChatError> {
    state
        .providers
        .get(provider)
        .cloned()
        .ok_or_else(|| ChatError::UnknownProvider(provider.to_string()))
}

fn classify_status(status: u16) -> AttemptOutcome {
    if status >= 500 {
        AttemptOutcome::Upstream5xx
    } else if status >= 400 {
        AttemptOutcome::Upstream4xx
    } else {
        AttemptOutcome::Success
    }
}

fn classify_provider_error(err: &ProviderError) -> AttemptOutcome {
    match err {
        ProviderError::Timeout => AttemptOutcome::Timeout,
        ProviderError::Network(_) => AttemptOutcome::Network,
        ProviderError::Upstream { status, .. } if *status >= 500 => AttemptOutcome::Upstream5xx,
        ProviderError::Upstream { status, .. } if *status >= 400 => AttemptOutcome::Upstream4xx,
        ProviderError::Upstream { .. } => AttemptOutcome::Internal,
        ProviderError::InvalidRequest(_) | ProviderError::MissingField(_) => {
            AttemptOutcome::Upstream4xx
        }
        ProviderError::Internal(_) => AttemptOutcome::Internal,
    }
}

fn error_kind(outcome: AttemptOutcome) -> &'static str {
    match outcome {
        AttemptOutcome::Success => "success",
        AttemptOutcome::Upstream5xx => "upstream_5xx",
        AttemptOutcome::Upstream4xx => "upstream_4xx",
        AttemptOutcome::Timeout => "timeout",
        AttemptOutcome::Network => "network",
        AttemptOutcome::Cancelled => "cancelled",
        AttemptOutcome::Internal => "internal",
    }
}

fn decode_body_excerpt(body: &bytes::Bytes) -> String {
    let s = String::from_utf8_lossy(body);
    if s.len() > 512 {
        format!("{}...", &s[..512])
    } else {
        s.to_string()
    }
}
