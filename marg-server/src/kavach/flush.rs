//! Background task that periodically flushes new signed audit entries to
//! disk.
//!
//! Kavach's `SignedAuditChain` keeps all entries in memory; the chain has no
//! native disk-backed mode in 0.1.0. Marg owns the persistence story: every
//! `[kavach].audit_flush_seconds` (default 60s), we slice the entries from
//! the last-flushed index to the chain head, encode them with
//! `kavach_pq::audit::export_jsonl`, and append the bytes to a per-process
//! JSONL file under `[kavach].audit_export_path`.
//!
//! One file per process lifetime keeps the implementation simple and the
//! verifier happy (one chain per file). Cross-restart chain continuity is a
//! v1.1 concern documented in `docs/cluster-deployment.md`; for now each
//! restart begins a fresh genesis chain.

use chrono::Utc;
use kavach_pq::audit::export_jsonl;
use kavach_pq::SignedAuditChain;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

pub struct AuditFlushTaskHandle {
    pub path: PathBuf,
}

pub fn spawn_audit_flush_task(
    chain: Arc<SignedAuditChain>,
    export_dir: String,
    flush_seconds: u64,
) -> AuditFlushTaskHandle {
    let path = chain_file_path(&export_dir);
    let task_path = path.clone();
    let last_flushed: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));
    let interval = Duration::from_secs(flush_seconds.max(1));

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
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // First tick fires immediately; skip it so we do not flush an empty
        // chain at boot.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(e) = flush_once(&chain, &task_path, &last_flushed).await {
                tracing::warn!(?e, path = %task_path.display(), "kavach audit flush failed");
            }
        }
    });

    AuditFlushTaskHandle { path }
}

async fn flush_once(
    chain: &Arc<SignedAuditChain>,
    path: &Path,
    last_flushed: &Arc<Mutex<u64>>,
) -> std::io::Result<()> {
    let entries = chain.entries();
    let mut guard = last_flushed.lock().await;
    let start = *guard as usize;
    if start >= entries.len() {
        return Ok(());
    }
    let slice = &entries[start..];
    let bytes = export_jsonl(slice).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::Other, format!("export_jsonl: {}", e))
    })?;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    file.write_all(&bytes).await?;
    file.flush().await?;
    let new_head = entries.len() as u64;
    *guard = new_head;
    tracing::debug!(
        path = %path.display(),
        appended = slice.len(),
        head_index = new_head,
        "kavach audit chain flushed"
    );
    Ok(())
}

fn chain_file_path(export_dir: &str) -> PathBuf {
    let stamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let mut p = PathBuf::from(export_dir);
    p.push(format!("audit-{}.jsonl", stamp));
    p
}
