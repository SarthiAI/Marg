//! Per-request and lifecycle audit emission.
//!
//! ADR-015 mandates that Marg writes **one signed chain entry per Marg
//! request** with the full lifecycle packed into the `context_snapshot` JSON,
//! plus separate dedicated entries for non-request events (policy reload, key
//! events). This module is the single place those entries are constructed
//! and appended to `SignedAuditChain`.
//!
//! Encoding the response permit token for the caller header is also here
//! since it pairs naturally with the lifecycle emit.

use chrono::Utc;
use data_encoding::BASE64URL_NOPAD;
use kavach_core::audit::AuditEntry;
use kavach_core::{ActionContext, PermitToken, Verdict};
use kavach_pq::SignedAuditChain;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

use crate::kavach::context::RequestLifecycle;

#[derive(Debug, Clone, Copy)]
pub enum KeyEventKind {
    Created,
    Revoked,
    Invalidated,
    Expired,
}

impl KeyEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            KeyEventKind::Created => "created",
            KeyEventKind::Revoked => "revoked",
            KeyEventKind::Invalidated => "invalidated",
            KeyEventKind::Expired => "expired",
        }
    }
}

/// Build and append the per-request audit entry. The real verdict (what the
/// gate actually decided) is captured under `verdict.real_kind`; the effective
/// verdict (what Marg applied to the request after observe-mode synthesis) is
/// captured under `verdict.effective_kind`. The two diverge in observe mode:
/// `real_kind = "refuse"` and `effective_kind = "permit"` is the "would have
/// refused" signal `marg policy audit` surfaces.
pub fn audit_request_lifecycle(
    chain: &Arc<SignedAuditChain>,
    ctx: &ActionContext,
    real_verdict: &Verdict,
    effective_verdict: &Verdict,
    lifecycle: &RequestLifecycle,
    mode: &str,
    include_prompts: bool,
    raw_request_body: Option<&serde_json::Value>,
) {
    let mut entry = AuditEntry::from_verdict(ctx, real_verdict);
    let snapshot = build_snapshot(
        ctx,
        real_verdict,
        effective_verdict,
        lifecycle,
        mode,
        include_prompts,
        raw_request_body,
    );
    entry.context_snapshot = Some(snapshot);

    let chain = chain.clone();
    // Spawn off the request runtime: signing is fast but still O(ms); the
    // request path does not need to wait for the chain append to return.
    tokio::task::spawn_blocking(move || {
        if let Err(e) = chain.append(&entry) {
            tracing::warn!(?e, "signed audit chain append failed for marg.request.v1");
        }
    });
}

/// Append a `marg.policy_reload.v1` entry. Called from the admin reload path
/// (and from SIGHUP). The principal is `"admin:<token_id>"` when an admin
/// triggered the reload, `"system"` for SIGHUP, recorded by the caller.
pub fn emit_policy_reload(
    chain: &Arc<SignedAuditChain>,
    principal: &str,
    previous_hash: &str,
    new_hash: &str,
    source_path: Option<&PathBuf>,
    policy_rule_count: usize,
    invariant_count: usize,
    success: bool,
    error: Option<String>,
) {
    let now = Utc::now();
    let evaluation_id = Uuid::new_v4();
    let snapshot = json!({
        "schema": "marg.policy_reload.v1",
        "timestamp": now.to_rfc3339(),
        "principal": principal,
        "previous_hash": previous_hash,
        "new_hash": new_hash,
        "source_path": source_path.map(|p| p.display().to_string()),
        "policy_rule_count": policy_rule_count,
        "invariant_count": invariant_count,
        "success": success,
        "error": error,
    });
    let entry = AuditEntry {
        id: Uuid::new_v4(),
        evaluation_id,
        timestamp: now,
        principal_id: principal.to_string(),
        action_name: "marg.policy_reload.v1".to_string(),
        resource: source_path.map(|p| p.display().to_string()),
        verdict: if success { "permit" } else { "refuse" }.to_string(),
        verdict_detail: if success {
            format!("reloaded {} -> {}", previous_hash, new_hash)
        } else {
            error.clone().unwrap_or_else(|| "reload failed".to_string())
        },
        decided_by: Some("marg".to_string()),
        session_id: Uuid::nil(),
        ip: None,
        context_snapshot: Some(snapshot),
    };
    let chain = chain.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(e) = chain.append(&entry) {
            tracing::warn!(?e, "signed audit chain append failed for marg.policy_reload.v1");
        }
    });
}

/// Append a `marg.key_event.v1` entry. Called from admin handlers
/// (create / revoke / invalidate) and from the gate's `Verdict::Invalidate`
/// branch. The principal is the admin token id for admin-driven events,
/// `"system"` for system-driven (e.g. expiry, drift).
pub fn emit_key_event(
    chain: &Arc<SignedAuditChain>,
    principal: &str,
    key_id: &str,
    kind: KeyEventKind,
    reason: Option<&str>,
) {
    let now = Utc::now();
    let snapshot = json!({
        "schema": "marg.key_event.v1",
        "timestamp": now.to_rfc3339(),
        "principal": principal,
        "marg_key_id": key_id,
        "kind": kind.as_str(),
        "reason": reason,
    });
    let entry = AuditEntry {
        id: Uuid::new_v4(),
        evaluation_id: Uuid::new_v4(),
        timestamp: now,
        principal_id: principal.to_string(),
        action_name: format!("marg.key_event.{}", kind.as_str()),
        resource: Some(key_id.to_string()),
        verdict: "permit".to_string(),
        verdict_detail: format!("key {} {}", key_id, kind.as_str()),
        decided_by: Some("marg".to_string()),
        session_id: Uuid::nil(),
        ip: None,
        context_snapshot: Some(snapshot),
    };
    let chain = chain.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(e) = chain.append(&entry) {
            tracing::warn!(?e, "signed audit chain append failed for marg.key_event.v1");
        }
    });
}

/// Encode a `PermitToken` as a base64-url-no-pad string suitable for the
/// `X-Kavach-Permit` response or outbound header. The JSON form is
/// deterministic per Kavach's serde impl.
pub fn encode_permit_header(token: &PermitToken) -> Option<String> {
    let json = serde_json::to_vec(token).ok()?;
    Some(BASE64URL_NOPAD.encode(&json))
}

pub fn verdict_kind_str(v: &Verdict) -> &'static str {
    match v {
        Verdict::Permit(_) => "permit",
        Verdict::Refuse(_) => "refuse",
        Verdict::Invalidate(_) => "invalidate",
    }
}

fn build_snapshot(
    ctx: &ActionContext,
    real_verdict: &Verdict,
    effective_verdict: &Verdict,
    lifecycle: &RequestLifecycle,
    mode: &str,
    include_prompts: bool,
    raw_request_body: Option<&serde_json::Value>,
) -> Value {
    let verdict_detail = match real_verdict {
        Verdict::Permit(token) => json!({
            "real_kind": "permit",
            "effective_kind": verdict_kind_str(effective_verdict),
            "evaluator": null,
            "reason_code": null,
            "reason_text": null,
            "permit_token_id": token.token_id.to_string(),
        }),
        Verdict::Refuse(reason) => json!({
            "real_kind": "refuse",
            "effective_kind": verdict_kind_str(effective_verdict),
            "evaluator": reason.evaluator,
            "reason_code": reason.code.to_string(),
            "reason_text": reason.reason,
            "permit_token_id": null,
        }),
        Verdict::Invalidate(scope) => json!({
            "real_kind": "invalidate",
            "effective_kind": verdict_kind_str(effective_verdict),
            "evaluator": scope.evaluator,
            "reason_code": "INVALIDATE",
            "reason_text": scope.reason,
            "permit_token_id": null,
        }),
    };

    let provider_call = json!({
        "provider": lifecycle.provider,
        "model": lifecycle.route_model,
        "status": lifecycle.provider_status,
        "failovers": lifecycle.failovers,
        "attempts": lifecycle.attempts,
    });
    let response = json!({
        "status": lifecycle.response_status,
        "input_tokens": lifecycle.response_input_tokens,
        "output_tokens": lifecycle.response_output_tokens,
        "actual_cost_usd": lifecycle.response_actual_cost_usd,
        "latency_ms": lifecycle.response_latency_ms,
        "client_disconnect": lifecycle.response_client_disconnect,
    });
    let error = if lifecycle.error_class.is_some() || lifecycle.error_message.is_some() {
        json!({
            "class": lifecycle.error_class,
            "message": lifecycle.error_message,
        })
    } else {
        Value::Null
    };

    let prompt = if include_prompts {
        raw_request_body.cloned().unwrap_or(Value::Null)
    } else {
        Value::Null
    };

    json!({
        "schema": "marg.request.v1",
        "timestamp": Utc::now().to_rfc3339(),
        "mode": mode,
        "request_id": lifecycle.request_id,
        "evaluation_id": ctx.evaluation_id.to_string(),
        "principal_id": lifecycle.principal_id,
        "principal_kind": lifecycle.principal_kind,
        "action_name": lifecycle.action_name,
        "model": lifecycle.model,
        "input_token_count": lifecycle.input_token_count,
        "max_tokens": lifecycle.max_tokens,
        "estimated_cost_usd": lifecycle.estimated_cost_usd,
        "streaming": lifecycle.streaming,
        "verdict": verdict_detail,
        "provider_call": provider_call,
        "response": response,
        "error": error,
        "prompt_included": include_prompts,
        "prompt": prompt,
    })
}
