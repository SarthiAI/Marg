//! Admin endpoints over the Kavach signed audit chain.
//!
//! After the flush task prunes flushed entries from memory (see `kavach/flush.rs`),
//! the full chain lives across two places: the on-disk JSONL segment holds the
//! older entries `[0, base_index)`, and the in-memory chain holds the resident
//! tail `[base_index, len)`. These endpoints reconstruct the full logical chain
//! from both, and do so **streaming**: they never load the whole (potentially
//! many-GB) segment file into memory.
//!
//! - `GET /admin/audit/entries?since=<index>&limit=<n>` returns a bounded,
//!   paginated JSON view (at most `limit` entries), reading disk only as far as
//!   needed.
//! - `GET /admin/audit/export?since=<index>` streams the chain as JSONL bytes
//!   (disk segment then in-memory tail) with O(1) server memory.
//! - `POST /admin/audit/verify` verifies the whole chain (or a supplied file) in
//!   fixed-size batches via `verify_chain_from`, so verification memory is
//!   bounded no matter how long the chain is.
//! - `GET /admin/audit/status` summarises head, length, mode, drift.

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::Response;
use axum::Json;
use bytes::Bytes;
use kavach_pq::audit::{export_jsonl, verify_chain_from, SignedAuditEntry};
use kavach_pq::Verifier;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_stream::wrappers::ReceiverStream;

use crate::admin::error::AdminError;
use crate::state::AppState;

/// Batch size for streamed verification. Bounds peak memory to one batch of
/// entries regardless of total chain length.
const VERIFY_BATCH: usize = 1000;

type AuditLines = tokio::io::Lines<BufReader<tokio::fs::File>>;

/// Open the on-disk audit segment for line-by-line streaming. `Ok(None)` when
/// nothing has been flushed yet (no file), which is not an error.
async fn open_audit_lines(path: &std::path::Path) -> Result<Option<AuditLines>, AdminError> {
    match tokio::fs::File::open(path).await {
        Ok(f) => Ok(Some(BufReader::new(f).lines())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(AdminError::Internal(format!("open audit file: {e}"))),
    }
}

/// Verify a stream of entries in fixed-size batches, chaining each batch from
/// the running `anchor` (the previous entry's `entry_hash`, or "genesis").
/// `cap`, when set, skips entries with logical index >= it (used to drop the
/// disk/memory overlap that a concurrent flush can create). Returns
/// `Ok((verified_count, ending_anchor))` or `Err((count_before_failure, msg))`.
async fn verify_stream(
    lines: &mut AuditLines,
    verifier: &Verifier,
    mut anchor: String,
    cap: Option<u64>,
) -> Result<(u64, String), (u64, String)> {
    let mut batch: Vec<SignedAuditEntry> = Vec::with_capacity(VERIFY_BATCH);
    let mut count = 0u64;
    loop {
        let line = match lines.next_line().await {
            Ok(Some(l)) => l,
            Ok(None) => break,
            Err(e) => return Err((count, format!("read audit file: {e}"))),
        };
        if line.trim().is_empty() {
            continue;
        }
        let entry: SignedAuditEntry = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(e) => return Err((count, format!("parse audit line: {e}"))),
        };
        if let Some(c) = cap {
            if entry.index >= c {
                continue;
            }
        }
        batch.push(entry);
        if batch.len() >= VERIFY_BATCH {
            if let Err(e) = verify_chain_from(&batch, verifier, &anchor) {
                return Err((count, e.to_string()));
            }
            anchor = batch.last().unwrap().entry_hash.clone();
            count += batch.len() as u64;
            batch.clear();
        }
    }
    if !batch.is_empty() {
        if let Err(e) = verify_chain_from(&batch, verifier, &anchor) {
            return Err((count, e.to_string()));
        }
        anchor = batch.last().unwrap().entry_hash.clone();
        count += batch.len() as u64;
    }
    Ok((count, anchor))
}

fn entry_json(e: &SignedAuditEntry) -> Value {
    json!({
        "index": e.index,
        "previous_hash": e.previous_hash,
        "entry_hash": e.entry_hash,
        "mode": e.mode().to_string(),
        "signed_payload_key_id": e.signed_payload.key_id,
        "signed_payload_signed_at": e.signed_payload.signed_at,
        "data": parse_inner_entry(&e.signed_payload.data),
    })
}

#[derive(Debug, Deserialize)]
pub struct ListAuditParams {
    #[serde(default)]
    pub since: Option<u64>,
    #[serde(default = "default_audit_limit")]
    pub limit: u32,
}

fn default_audit_limit() -> u32 {
    100
}

/// Paginated view of the full chain starting at logical index `since`, at most
/// `limit` entries. Reads the disk segment only as far as needed (early-stops
/// once `limit` is reached or the in-memory portion begins), then fills the
/// remainder from the resident tail. Peak memory is bounded by `limit`.
pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<ListAuditParams>,
) -> Result<Json<Value>, AdminError> {
    let since = params.since.unwrap_or(0);
    let limit = params.limit.clamp(1, 5_000) as usize;
    let head_hash = state.kavach.audit_chain.head_hash();
    let total = state.kavach.audit_chain.len();
    let base = state.kavach.audit_chain.base_index();

    let mut collected: Vec<SignedAuditEntry> = Vec::new();

    // Disk portion: entries in [since, base). Stream and early-stop.
    if since < base {
        if let Some(mut lines) = open_audit_lines(&state.kavach.audit_export_file).await? {
            while collected.len() < limit {
                match lines
                    .next_line()
                    .await
                    .map_err(|e| AdminError::Internal(format!("read audit file: {e}")))?
                {
                    Some(line) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        let e: SignedAuditEntry = serde_json::from_str(&line)
                            .map_err(|e| AdminError::Internal(format!("parse audit line: {e}")))?;
                        if e.index < since {
                            continue;
                        }
                        if e.index >= base {
                            break; // remainder is in memory
                        }
                        collected.push(e);
                    }
                    None => break,
                }
            }
        }
    }

    // Memory portion: fill the rest from the resident tail.
    if collected.len() < limit {
        let mem_from = since.max(base);
        if let Ok(tail) = state.kavach.audit_chain.entries_since(mem_from) {
            for e in tail {
                if collected.len() >= limit {
                    break;
                }
                collected.push(e);
            }
        }
    }

    let view: Vec<Value> = collected.iter().map(entry_json).collect();
    Ok(Json(json!({
        "head_hash": head_hash,
        "total": total,
        "from": since,
        "count": view.len(),
        "entries": view,
    })))
}

/// Stream the full chain from logical index `since` as JSONL, disk segment then
/// in-memory tail, with O(1) server memory. A background task feeds line chunks
/// through a channel into the response body.
pub async fn export(
    State(state): State<AppState>,
    Query(params): Query<ListAuditParams>,
) -> Result<Response, AdminError> {
    let since = params.since.unwrap_or(0);
    let path = state.kavach.audit_export_file.clone();
    // Snapshot the tail first; `cap` drops any disk/memory overlap a concurrent
    // flush may have created, so the export never duplicates an index.
    let tail = state.kavach.audit_chain.entries();
    let cap = tail.first().map(|e| e.index);

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(16);
    tokio::spawn(async move {
        if let Ok(f) = tokio::fs::File::open(&path).await {
            let mut lines = BufReader::new(f).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        let idx = match serde_json::from_str::<SignedAuditEntry>(&line) {
                            Ok(e) => e.index,
                            Err(_) => continue,
                        };
                        if idx < since {
                            continue;
                        }
                        if let Some(c) = cap {
                            if idx >= c {
                                continue;
                            }
                        }
                        let mut b = line.into_bytes();
                        b.push(b'\n');
                        if tx.send(Ok(Bytes::from(b))).await.is_err() {
                            return; // client hung up
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                        return;
                    }
                }
            }
        }
        // In-memory tail (index >= since).
        let filtered: Vec<SignedAuditEntry> =
            tail.into_iter().filter(|e| e.index >= since).collect();
        if !filtered.is_empty() {
            match export_jsonl(&filtered) {
                Ok(bytes) => {
                    let _ = tx.send(Ok(Bytes::from(bytes))).await;
                }
                Err(e) => {
                    let _ = tx
                        .send(Err(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            e.to_string(),
                        )))
                        .await;
                }
            }
        }
    });

    let body = Body::from_stream(ReceiverStream::new(rx));
    Response::builder()
        .status(200)
        .header(header::CONTENT_TYPE, "application/jsonl; charset=utf-8")
        .header("x-marg-audit-from", since.to_string())
        .body(body)
        .map_err(|e| AdminError::Internal(e.to_string()))
}

#[derive(Debug, Deserialize, Default)]
pub struct VerifyRequest {
    /// Optional path to a JSONL file on disk. When omitted, verifies the live
    /// full chain (on-disk segment + in-memory tail).
    #[serde(default)]
    pub path: Option<String>,
}

pub async fn verify(
    State(state): State<AppState>,
    Json(req): Json<VerifyRequest>,
) -> Result<Json<Value>, AdminError> {
    let verifier = &state.kavach.verifier;

    // A caller-supplied file: verify it whole, from genesis, streamed.
    if let Some(path) = req.path.as_ref() {
        let mut lines = match tokio::fs::File::open(path).await {
            Ok(f) => BufReader::new(f).lines(),
            Err(e) => return Err(AdminError::BadRequest(format!("open {path}: {e}"))),
        };
        let source = format!("file:{path}");
        return Ok(
            match verify_stream(&mut lines, verifier, "genesis".to_string(), None).await {
                Ok((count, _)) => Json(json!({"verified": true, "source": source, "count": count})),
                Err((count, err)) => {
                    Json(json!({"verified": false, "source": source, "count": count, "error": err}))
                }
            },
        );
    }

    // Live full chain: disk [0, cap) streamed, then the in-memory tail, so a
    // multi-GB chain verifies without ever being held whole in memory.
    let tail = state.kavach.audit_chain.entries();
    let cap = tail.first().map(|e| e.index);
    let mut anchor = "genesis".to_string();
    let mut total = 0u64;

    if let Some(mut lines) = open_audit_lines(&state.kavach.audit_export_file).await? {
        match verify_stream(&mut lines, verifier, anchor.clone(), cap).await {
            Ok((count, ending)) => {
                anchor = ending;
                total += count;
            }
            Err((count, err)) => {
                return Ok(Json(json!({
                    "verified": false, "source": "full_chain",
                    "count": total + count, "error": err,
                })));
            }
        }
    }

    if !tail.is_empty() {
        if let Err(e) = verify_chain_from(&tail, verifier, &anchor) {
            return Ok(Json(json!({
                "verified": false, "source": "full_chain",
                "count": total, "error": e.to_string(),
            })));
        }
        total += tail.len() as u64;
    }

    Ok(Json(json!({
        "verified": true,
        "source": "full_chain",
        "count": total,
    })))
}

pub async fn status(State(state): State<AppState>) -> Result<Json<Value>, AdminError> {
    let head_hash = state.kavach.audit_chain.head_hash();
    let total = state.kavach.audit_chain.len();
    let (resident_entries, resident_bytes) = state.kavach.audit_chain.resident_stats();
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
            "resident_entries": resident_entries,
            "resident_bytes": resident_bytes,
            "export_file": state.kavach.audit_export_file.display().to_string(),
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
        "session_tracking": *state.kavach.session_tracking_needed.load_full(),
    })))
}

fn parse_inner_entry(payload: &[u8]) -> Value {
    serde_json::from_slice::<Value>(payload).unwrap_or(Value::Null)
}
