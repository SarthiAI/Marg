//! Admin endpoints over the Kavach signed audit chain.
//!
//! Three endpoints in P09:
//!
//! - `GET /admin/audit/entries?since=<index>&limit=<n>` walks the in-memory
//!   chain and returns a paginated JSON view, useful for ad-hoc inspection.
//! - `GET /admin/audit/export?since=<index>` streams the chain as JSONL bytes
//!   (each line is a `SignedAuditEntry`). Use this for offline verification
//!   or batch shipping to a SIEM.
//! - `POST /admin/audit/verify` verifies the live in-memory chain (or a file
//!   path the caller supplies) with `kavach_pq::audit::verify_chain`.
//! - `GET /admin/audit/status` summarises chain head, length, mode, last
//!   flushed file path, drift counters.

use axum::extract::{Query, State};
use axum::http::header;
use axum::response::Response;
use axum::Json;
use kavach_pq::audit::{export_jsonl, parse_jsonl, verify_chain};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::admin::error::AdminError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ListAuditParams {
    #[serde(default)]
    pub since: Option<u64>,
    #[serde(default = "default_audit_limit")]
    pub limit: u32,
}

fn default_audit_limit() -> u32 { 100 }

pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<ListAuditParams>,
) -> Result<Json<Value>, AdminError> {
    let entries = state.kavach.audit_chain.entries();
    let start = params.since.unwrap_or(0) as usize;
    let limit = params.limit.clamp(1, 5_000) as usize;
    let slice = if start >= entries.len() {
        &[][..]
    } else {
        let end = (start + limit).min(entries.len());
        &entries[start..end]
    };
    let head_hash = state.kavach.audit_chain.head_hash();
    let total = entries.len() as u64;
    let view: Vec<Value> = slice
        .iter()
        .map(|e| {
            json!({
                "index": e.index,
                "previous_hash": e.previous_hash,
                "entry_hash": e.entry_hash,
                "mode": e.mode().to_string(),
                "signed_payload_key_id": e.signed_payload.key_id,
                "signed_payload_signed_at": e.signed_payload.signed_at,
                "data": parse_inner_entry(&e.signed_payload.data),
            })
        })
        .collect();
    Ok(Json(json!({
        "head_hash": head_hash,
        "total": total,
        "from": start,
        "count": slice.len(),
        "entries": view,
    })))
}

pub async fn export(
    State(state): State<AppState>,
    Query(params): Query<ListAuditParams>,
) -> Result<Response, AdminError> {
    let entries = state.kavach.audit_chain.entries();
    let start = params.since.unwrap_or(0) as usize;
    let slice = if start >= entries.len() {
        &[][..]
    } else {
        &entries[start..]
    };
    let bytes = export_jsonl(slice).map_err(|e| AdminError::Internal(e.to_string()))?;
    let resp = Response::builder()
        .status(200)
        .header(header::CONTENT_TYPE, "application/jsonl; charset=utf-8")
        .header("x-marg-audit-from", start.to_string())
        .header("x-marg-audit-count", slice.len().to_string())
        .body(axum::body::Body::from(bytes))
        .map_err(|e| AdminError::Internal(e.to_string()))?;
    Ok(resp)
}

#[derive(Debug, Deserialize, Default)]
pub struct VerifyRequest {
    /// Optional path to a JSONL file on disk. When omitted, verifies the live
    /// in-memory chain.
    #[serde(default)]
    pub path: Option<String>,
}

pub async fn verify(
    State(state): State<AppState>,
    Json(req): Json<VerifyRequest>,
) -> Result<Json<Value>, AdminError> {
    let (entries, source) = if let Some(path) = req.path.as_ref() {
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| AdminError::BadRequest(format!("read {}: {}", path, e)))?;
        let parsed = parse_jsonl(&bytes)
            .map_err(|e| AdminError::BadRequest(format!("parse {}: {}", path, e)))?;
        (parsed, format!("file:{}", path))
    } else {
        (state.kavach.audit_chain.entries(), "live_chain".to_string())
    };
    match verify_chain(&entries, &state.kavach.verifier) {
        Ok(()) => Ok(Json(json!({
            "verified": true,
            "source": source,
            "count": entries.len(),
        }))),
        Err(e) => Ok(Json(json!({
            "verified": false,
            "source": source,
            "count": entries.len(),
            "error": e.to_string(),
        }))),
    }
}

pub async fn status(State(state): State<AppState>) -> Result<Json<Value>, AdminError> {
    let head_hash = state.kavach.audit_chain.head_hash();
    let total = state.kavach.audit_chain.len();
    let mode_arc = state.kavach.mode.load_full();
    let policy_hash = state.kavach.policy_source_hash.load_full();
    let policy_path = state
        .kavach
        .policy_source_path
        .as_ref()
        .map(|p| p.display().to_string());
    let loaded_at = state.kavach.policy_loaded_at.load_full();
    let rule_count = *state.kavach.policy_rule_count.load_full();
    let invariant_count = *state.kavach.invariant_count.load_full();
    let expose_permit = *state.kavach.expose_permit_to_caller.load_full();
    let forward_permit = *state.kavach.forward_permit_to_provider.load_full();
    let permit_ttl = *state.kavach.permit_ttl_seconds.load_full();
    let signer = &state.kavach.permit_signer;
    let drift = state.kavach.drift_state.load_full();
    let drift_detectors_json: Vec<Value> = drift
        .detectors
        .iter()
        .map(|d| json!({ "name": d.name, "parameters": d.parameters }))
        .collect();
    Ok(Json(json!({
        "mode": mode_arc.as_str(),
        "kavach_core_version": crate::KAVACH_CORE_VERSION,
        "kavach_pq_version": crate::KAVACH_PQ_VERSION,
        "audit_chain": {
            "head_hash": head_hash,
            "length": total,
        },
        "policy": {
            "source_path": policy_path,
            "source_hash": policy_hash.as_str(),
            "loaded_at": loaded_at.as_str(),
            "rule_count": rule_count,
            "invariant_count": invariant_count,
        },
        "permits": {
            "expose_to_caller": expose_permit,
            "forward_to_provider": forward_permit,
            "ttl_seconds": permit_ttl,
            "signer": {
                "enabled": signer.enabled,
                "algorithm": signer.algorithm,
                "key_id": signer.key_id,
            },
        },
        "drift": {
            "enabled": drift.enabled,
            "warning_threshold": drift.warning_threshold,
            "detectors": drift_detectors_json,
        },
    })))
}

fn parse_inner_entry(payload: &[u8]) -> Value {
    serde_json::from_slice::<Value>(payload).unwrap_or(Value::Null)
}
