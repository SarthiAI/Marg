use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;
use marg_core::{RequestLogEntry, WriteBatcherConfig};
use marg_storage::Storage;
use tokio::sync::mpsc;

use crate::metrics::Metrics;

/// One unit of asynchronous storage work. Both kinds are accumulated by the
/// background task into per-kind batches and flushed together.
#[derive(Debug)]
pub enum WriteJob {
    AddSpend {
        key_id: String,
        day: NaiveDate,
        amount_usd: f64,
    },
    RequestLog(RequestLogEntry),
}

/// Fail-closed asynchronous write batcher.
///
/// `WriteBatcher::enqueue` is non-blocking: it returns `Err(Overflow)` when the
/// bounded channel is full. The chat pipeline surfaces this as a 503 with
/// `x-marg-reason: storage_overloaded` rather than silently dropping the write.
///
/// The background task drains the channel, splits jobs by kind, and flushes
/// each kind either when its batch reaches `max_batch_size` or when
/// `max_batch_age_ms` has elapsed since the first item in the batch landed.
pub struct WriteBatcher {
    tx: mpsc::Sender<WriteJob>,
    metrics: Arc<Metrics>,
}

#[derive(Debug, Clone, Copy)]
pub struct Overflow;

impl WriteBatcher {
    pub fn spawn(
        storage: Arc<dyn Storage>,
        metrics: Arc<Metrics>,
        cfg: WriteBatcherConfig,
    ) -> Arc<Self> {
        let depth = cfg.channel_depth.max(1);
        let (tx, rx) = mpsc::channel::<WriteJob>(depth);
        let metrics_for_loop = metrics.clone();
        tokio::spawn(run_loop(rx, storage, metrics_for_loop, cfg));
        Arc::new(Self {
            tx,
            metrics,
        })
    }

    /// Try to enqueue a single write. Returns `Err(Overflow)` if the queue is
    /// full so the caller can refuse the request with `storage_overloaded`.
    pub fn enqueue(&self, job: WriteJob) -> Result<(), Overflow> {
        match self.tx.try_send(job) {
            Ok(_) => {
                self.update_depth_gauge();
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.metrics.write_batcher_overflow_total.inc();
                Err(Overflow)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                // Background task is gone, which only happens during shutdown.
                // Treat as overflow so the caller fails closed.
                self.metrics.write_batcher_overflow_total.inc();
                Err(Overflow)
            }
        }
    }

    fn update_depth_gauge(&self) {
        // tokio mpsc has no direct depth probe, but capacity() returns the
        // remaining capacity. We compute depth from that.
        let remaining = self.tx.capacity();
        let max = self.tx.max_capacity();
        let depth = max.saturating_sub(remaining) as i64;
        self.metrics.write_batcher_queue_depth.set(depth);
    }
}

async fn run_loop(
    mut rx: mpsc::Receiver<WriteJob>,
    storage: Arc<dyn Storage>,
    metrics: Arc<Metrics>,
    cfg: WriteBatcherConfig,
) {
    let max_batch = cfg.max_batch_size.max(1);
    let max_age = Duration::from_millis(cfg.max_batch_age_ms.max(1));

    let mut spend_batch: Vec<(String, NaiveDate, f64)> = Vec::with_capacity(max_batch);
    let mut log_batch: Vec<RequestLogEntry> = Vec::with_capacity(max_batch);

    loop {
        // Block until at least one job arrives, then drain whatever else is
        // already queued, then optionally wait up to max_age for the batch to
        // grow before flushing.
        let first = match rx.recv().await {
            Some(j) => j,
            None => break,
        };
        ingest(&mut spend_batch, &mut log_batch, first);

        // Greedy drain without waiting.
        while spend_batch.len() < max_batch && log_batch.len() < max_batch {
            match rx.try_recv() {
                Ok(j) => ingest(&mut spend_batch, &mut log_batch, j),
                Err(_) => break,
            }
        }

        // If we're already over the batch threshold, flush immediately.
        // Otherwise wait up to max_age for more jobs.
        if spend_batch.len() < max_batch && log_batch.len() < max_batch {
            let deadline = tokio::time::sleep(max_age);
            tokio::pin!(deadline);
            loop {
                if spend_batch.len() >= max_batch || log_batch.len() >= max_batch {
                    break;
                }
                tokio::select! {
                    biased;
                    _ = &mut deadline => break,
                    maybe = rx.recv() => {
                        match maybe {
                            Some(j) => ingest(&mut spend_batch, &mut log_batch, j),
                            None => {
                                flush_all(&storage, &metrics, &mut spend_batch, &mut log_batch).await;
                                return;
                            }
                        }
                    }
                }
            }
        }

        flush_all(&storage, &metrics, &mut spend_batch, &mut log_batch).await;
        // Refresh gauge after every flush so it doesn't drift past zero.
        metrics.write_batcher_queue_depth.set(0);
    }
}

fn ingest(
    spend_batch: &mut Vec<(String, NaiveDate, f64)>,
    log_batch: &mut Vec<RequestLogEntry>,
    job: WriteJob,
) {
    match job {
        WriteJob::AddSpend { key_id, day, amount_usd } => {
            if amount_usd > 0.0 {
                spend_batch.push((key_id, day, amount_usd));
            }
        }
        WriteJob::RequestLog(entry) => {
            log_batch.push(entry);
        }
    }
}

async fn flush_all(
    storage: &Arc<dyn Storage>,
    metrics: &Arc<Metrics>,
    spend_batch: &mut Vec<(String, NaiveDate, f64)>,
    log_batch: &mut Vec<RequestLogEntry>,
) {
    if !spend_batch.is_empty() {
        let rows = spend_batch.len() as u64;
        match storage.add_spend_batch(spend_batch.as_slice()).await {
            Ok(()) => {
                metrics
                    .write_batcher_flushes_total
                    .with_label_values(&["ok", "spend"])
                    .inc();
                metrics
                    .write_batcher_rows_total
                    .with_label_values(&["spend"])
                    .inc_by(rows);
            }
            Err(e) => {
                metrics
                    .write_batcher_flushes_total
                    .with_label_values(&["err", "spend"])
                    .inc();
                tracing::warn!(
                    error = %e,
                    rows = rows,
                    "write batcher add_spend_batch flush failed"
                );
            }
        }
        spend_batch.clear();
    }
    if !log_batch.is_empty() {
        let rows = log_batch.len() as u64;
        let entries = std::mem::take(log_batch);
        match storage.append_request_logs(entries).await {
            Ok(()) => {
                metrics
                    .write_batcher_flushes_total
                    .with_label_values(&["ok", "request_log"])
                    .inc();
                metrics
                    .write_batcher_rows_total
                    .with_label_values(&["request_log"])
                    .inc_by(rows);
            }
            Err(e) => {
                metrics
                    .write_batcher_flushes_total
                    .with_label_values(&["err", "request_log"])
                    .inc();
                tracing::warn!(
                    error = %e,
                    rows = rows,
                    "write batcher append_request_logs flush failed"
                );
            }
        }
    }
}
