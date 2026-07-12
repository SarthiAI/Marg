//! Background task that persists new signed audit entries to disk and then
//! prunes them from memory, keeping the chain's resident footprint bounded.
//!
//! Kavach's `SignedAuditChain` retains entries in memory until a consumer
//! prunes them (kavach-pq >= 0.1.3 adds `entries_since` / `prune_before`). Marg
//! owns the persistence + prune loop: it appends the resident tail to a
//! per-process JSONL file under `[kavach].audit_export_path`, fsyncs, then calls
//! `prune_before` so the flushed entries are released from RAM. The loop fires
//! on `[kavach].audit_flush_seconds` OR as soon as the resident window exceeds
//! `[kavach].audit_max_resident_bytes`, whichever comes first, so peak memory
//! stays bounded regardless of request rate.
//!
//! One file per process lifetime keeps the verifier happy (one continuous chain
//! per file, anchored at genesis). The admin audit handlers read this same file
//! back and stitch it to the in-memory tail to serve / verify the full chain.
//! Cross-restart continuity remains a later concern; each restart begins a
//! fresh genesis chain and a fresh file.

use chrono::Utc;
use kavach_pq::audit::export_jsonl;
use kavach_pq::SignedAuditChain;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tokio::time::Instant;

pub struct AuditFlushTaskHandle {
    pub path: PathBuf,
}

/// Per-process audit chain export file path. Computed once and shared by the
/// flush task and the admin audit handlers, so both read/write the same file.
pub fn chain_file_path(export_dir: &str) -> PathBuf {
    let stamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let mut p = PathBuf::from(export_dir);
    p.push(format!("audit-{}.jsonl", stamp));
    p
}

pub fn spawn_audit_flush_task(
    chain: Arc<SignedAuditChain>,
    path: PathBuf,
    flush_seconds: u64,
    max_resident_bytes: u64,
) -> AuditFlushTaskHandle {
    let task_path = path.clone();
    // last_flushed tracks the logical index up to which entries are durably on
    // disk; it equals the chain's base_index after each prune.
    let last_flushed: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));
    let flush_interval = Duration::from_secs(flush_seconds.max(1));

    if let Some(parent) = task_path.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!(
                    ?e,
                    path = %parent.display(),
                    "could not create kavach audit export directory; flushes will fail until it exists"
                );
            }
        }
    }

    tokio::spawn(async move {
        // Poll frequently so the size trigger is responsive; the time trigger
        // still gates the steady-state flush cadence.
        let poll = flush_interval.min(Duration::from_secs(1));
        let mut ticker = tokio::time::interval(poll);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await; // consume the immediate first tick
        let mut last_flush_at = Instant::now();
        loop {
            ticker.tick().await;
            let (resident_entries, resident_bytes) = chain.resident_stats();
            if resident_entries == 0 {
                continue;
            }
            let time_due = last_flush_at.elapsed() >= flush_interval;
            let size_due = max_resident_bytes > 0 && resident_bytes >= max_resident_bytes;
            if !time_due && !size_due {
                continue;
            }
            match flush_once(&chain, &task_path, &last_flushed).await {
                Ok(_) => last_flush_at = Instant::now(),
                Err(e) => {
                    tracing::warn!(?e, path = %task_path.display(), "kavach audit flush failed");
                }
            }
        }
    });

    AuditFlushTaskHandle { path }
}

/// Flush the resident tail to disk, fsync, then prune it from memory. Prune only
/// what was durably written, so a crash between write and prune loses nothing:
/// the entries are on disk and the next boot starts a fresh chain from there.
async fn flush_once(
    chain: &Arc<SignedAuditChain>,
    path: &Path,
    last_flushed: &Arc<Mutex<u64>>,
) -> std::io::Result<u64> {
    let mut guard = last_flushed.lock().await;
    let from = *guard;
    let tail = chain.entries_since(from).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("entries_since({from}): {e}"),
        )
    })?;
    if tail.is_empty() {
        return Ok(0);
    }
    let new_head = from + tail.len() as u64;
    let bytes = export_jsonl(&tail)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("export_jsonl: {e}")))?;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    file.write_all(&bytes).await?;
    file.flush().await?;
    file.sync_data().await?;
    // Only now that the bytes are durable do we release them from memory.
    chain.prune_before(new_head);
    *guard = new_head;
    tracing::debug!(
        path = %path.display(),
        appended = tail.len(),
        head_index = new_head,
        "kavach audit chain flushed and pruned"
    );
    Ok(tail.len() as u64)
}
