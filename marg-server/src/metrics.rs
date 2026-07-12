use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, IntGaugeVec, Opts,
    Registry, TextEncoder,
};
use std::sync::Arc;

/// Process-wide Prometheus registry plus the handles to every metric Marg emits.
///
/// Constructed once at server boot, shared via `Arc<Metrics>` across the
/// request pipeline.
pub struct Metrics {
    pub registry: Registry,
    pub requests_total: IntCounterVec,
    pub request_duration_seconds: HistogramVec,
    pub decision_duration_seconds: prometheus::Histogram,
    pub tokens_total: IntCounterVec,
    pub budget_remaining_usd: prometheus::GaugeVec,
    pub provider_errors_total: IntCounterVec,
    pub failover_total: IntCounterVec,
    pub storage_query_duration_seconds: HistogramVec,
    pub hot_store_query_duration_seconds: HistogramVec,
    pub active_streams: IntGaugeVec,
    pub write_batcher_queue_depth: IntGauge,
    pub write_batcher_flushes_total: IntCounterVec,
    pub write_batcher_rows_total: IntCounterVec,
    pub write_batcher_overflow_total: IntCounter,
    pub cluster_invalidations_total: IntCounterVec,
}

impl Metrics {
    pub fn new() -> Arc<Self> {
        let registry = Registry::new();

        let requests_total = IntCounterVec::new(
            Opts::new(
                "marg_requests_total",
                "Total chat completion requests handled, labelled by upstream provider, resolved model and HTTP status code.",
            ),
            &["provider", "model", "status"],
        )
        .expect("valid metric definition: marg_requests_total");

        let request_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "marg_request_duration_seconds",
                "End-to-end request duration in seconds, from arrival to last byte forwarded.",
            )
            .buckets(vec![
                0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0,
                10.0, 30.0, 60.0,
            ]),
            &["provider", "model"],
        )
        .expect("valid metric definition: marg_request_duration_seconds");

        let decision_duration_seconds = prometheus::Histogram::with_opts(
            HistogramOpts::new(
                "marg_decision_duration_seconds",
                "Marg-internal decision duration in seconds: auth + budget reserve + rate-limit check + route selection, BEFORE the upstream call. Measured per accepted request.",
            )
            .buckets(vec![
                0.00001, 0.00002, 0.00005, 0.0001, 0.00025, 0.0005, 0.001, 0.0025, 0.005, 0.01,
                0.025, 0.05, 0.1,
            ]),
        )
        .expect("valid metric definition: marg_decision_duration_seconds");

        let tokens_total = IntCounterVec::new(
            Opts::new(
                "marg_tokens_total",
                "Total tokens that flowed through Marg, labelled by direction (input or output) and resolved model.",
            ),
            &["direction", "model"],
        )
        .expect("valid metric definition: marg_tokens_total");

        let budget_remaining_usd = prometheus::GaugeVec::new(
            Opts::new(
                "marg_budget_remaining_usd",
                "Most recently observed remaining daily budget in USD for the labelled Marg key.",
            ),
            &["key_id"],
        )
        .expect("valid metric definition: marg_budget_remaining_usd");

        let provider_errors_total = IntCounterVec::new(
            Opts::new(
                "marg_provider_errors_total",
                "Total upstream provider error events, labelled by provider and error kind (5xx, 4xx, timeout, network, internal).",
            ),
            &["provider", "kind"],
        )
        .expect("valid metric definition: marg_provider_errors_total");

        let failover_total = IntCounterVec::new(
            Opts::new(
                "marg_failover_total",
                "Total times Marg failed over from one upstream provider to another, labelled by source and destination provider.",
            ),
            &["from_provider", "to_provider"],
        )
        .expect("valid metric definition: marg_failover_total");

        let storage_query_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "marg_storage_query_duration_seconds",
                "Durable storage query duration in seconds, labelled by logical operation and backend.",
            )
            .buckets(vec![
                0.0001, 0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5,
                5.0,
            ]),
            &["operation", "backend"],
        )
        .expect("valid metric definition: marg_storage_query_duration_seconds");

        let hot_store_query_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "marg_hot_store_query_duration_seconds",
                "Hot store query duration in seconds, labelled by logical operation and backend.",
            )
            .buckets(vec![
                0.00005, 0.0001, 0.00025, 0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1,
                0.25, 0.5,
            ]),
            &["operation", "backend"],
        )
        .expect("valid metric definition: marg_hot_store_query_duration_seconds");

        let active_streams = IntGaugeVec::new(
            Opts::new(
                "marg_active_streams",
                "Currently open streaming responses, labelled by provider.",
            ),
            &["provider"],
        )
        .expect("valid metric definition: marg_active_streams");

        let write_batcher_queue_depth = IntGauge::with_opts(Opts::new(
            "marg_write_batcher_queue_depth",
            "Pending work items in the asynchronous storage-write batcher queue.",
        ))
        .expect("valid metric definition: marg_write_batcher_queue_depth");

        let write_batcher_flushes_total = IntCounterVec::new(
            Opts::new(
                "marg_write_batcher_flushes_total",
                "Batches flushed by the asynchronous write batcher, labelled by outcome (ok or err) and kind (spend or request_log).",
            ),
            &["outcome", "kind"],
        )
        .expect("valid metric definition: marg_write_batcher_flushes_total");

        let write_batcher_rows_total = IntCounterVec::new(
            Opts::new(
                "marg_write_batcher_rows_total",
                "Rows flushed by the asynchronous write batcher, labelled by kind (spend or request_log).",
            ),
            &["kind"],
        )
        .expect("valid metric definition: marg_write_batcher_rows_total");

        let write_batcher_overflow_total = IntCounter::with_opts(Opts::new(
            "marg_write_batcher_overflow_total",
            "Total requests refused with 503 storage_overloaded because the write batcher queue was full.",
        ))
        .expect("valid metric definition: marg_write_batcher_overflow_total");

        let cluster_invalidations_total = IntCounterVec::new(
            Opts::new(
                "marg_cluster_invalidations_total",
                "Signed cluster key-invalidation messages by direction (published|received) and result (ok|suppressed|rejected).",
            ),
            &["direction", "result"],
        )
        .expect("valid metric definition: marg_cluster_invalidations_total");

        registry
            .register(Box::new(requests_total.clone()))
            .expect("register requests_total");
        registry
            .register(Box::new(request_duration_seconds.clone()))
            .expect("register request_duration_seconds");
        registry
            .register(Box::new(decision_duration_seconds.clone()))
            .expect("register decision_duration_seconds");
        registry
            .register(Box::new(tokens_total.clone()))
            .expect("register tokens_total");
        registry
            .register(Box::new(budget_remaining_usd.clone()))
            .expect("register budget_remaining_usd");
        registry
            .register(Box::new(provider_errors_total.clone()))
            .expect("register provider_errors_total");
        registry
            .register(Box::new(failover_total.clone()))
            .expect("register failover_total");
        registry
            .register(Box::new(storage_query_duration_seconds.clone()))
            .expect("register storage_query_duration_seconds");
        registry
            .register(Box::new(hot_store_query_duration_seconds.clone()))
            .expect("register hot_store_query_duration_seconds");
        registry
            .register(Box::new(active_streams.clone()))
            .expect("register active_streams");
        registry
            .register(Box::new(write_batcher_queue_depth.clone()))
            .expect("register write_batcher_queue_depth");
        registry
            .register(Box::new(write_batcher_flushes_total.clone()))
            .expect("register write_batcher_flushes_total");
        registry
            .register(Box::new(write_batcher_rows_total.clone()))
            .expect("register write_batcher_rows_total");
        registry
            .register(Box::new(write_batcher_overflow_total.clone()))
            .expect("register write_batcher_overflow_total");
        registry
            .register(Box::new(cluster_invalidations_total.clone()))
            .expect("register cluster_invalidations_total");

        #[cfg(target_os = "linux")]
        {
            let process_collector =
                prometheus::process_collector::ProcessCollector::for_self();
            let _ = registry.register(Box::new(process_collector));
        }

        Arc::new(Self {
            registry,
            requests_total,
            request_duration_seconds,
            decision_duration_seconds,
            tokens_total,
            budget_remaining_usd,
            provider_errors_total,
            failover_total,
            storage_query_duration_seconds,
            hot_store_query_duration_seconds,
            active_streams,
            write_batcher_queue_depth,
            write_batcher_flushes_total,
            write_batcher_rows_total,
            write_batcher_overflow_total,
            cluster_invalidations_total,
        })
    }

    /// Count a cluster key-invalidation event. `direction` is `published` or
    /// `received`; `result` is `ok`, `suppressed` (observe mode), or
    /// `rejected` (failed verification / stale / undecodable).
    pub fn record_cluster_invalidation(&self, direction: &str, result: &str) {
        self.cluster_invalidations_total
            .with_label_values(&[direction, result])
            .inc();
    }

    pub fn record_request(&self, provider: &str, model: &str, status: u16, duration_seconds: f64) {
        let status_label = status.to_string();
        self.requests_total
            .with_label_values(&[provider, model, &status_label])
            .inc();
        self.request_duration_seconds
            .with_label_values(&[provider, model])
            .observe(duration_seconds);
    }

    pub fn record_tokens(&self, model: &str, input: u64, output: u64) {
        if input > 0 {
            self.tokens_total
                .with_label_values(&["input", model])
                .inc_by(input);
        }
        if output > 0 {
            self.tokens_total
                .with_label_values(&["output", model])
                .inc_by(output);
        }
    }

    pub fn set_budget_remaining(&self, key_id: &str, remaining_usd: f64) {
        self.budget_remaining_usd
            .with_label_values(&[key_id])
            .set(remaining_usd);
    }

    pub fn clear_budget_remaining(&self, key_id: &str) {
        let _ = self.budget_remaining_usd.remove_label_values(&[key_id]);
    }

    pub fn record_provider_error(&self, provider: &str, kind: &str) {
        self.provider_errors_total
            .with_label_values(&[provider, kind])
            .inc();
    }

    pub fn record_failover(&self, from_provider: &str, to_provider: &str) {
        self.failover_total
            .with_label_values(&[from_provider, to_provider])
            .inc();
    }

    pub fn observe_storage(&self, operation: &str, backend: &str, duration_seconds: f64) {
        self.storage_query_duration_seconds
            .with_label_values(&[operation, backend])
            .observe(duration_seconds);
    }

    pub fn observe_hot_store(&self, operation: &str, backend: &str, duration_seconds: f64) {
        self.hot_store_query_duration_seconds
            .with_label_values(&[operation, backend])
            .observe(duration_seconds);
    }

    pub fn stream_started(&self, provider: &str) {
        self.active_streams.with_label_values(&[provider]).inc();
    }

    pub fn stream_finished(&self, provider: &str) {
        self.active_streams.with_label_values(&[provider]).dec();
    }

    pub fn render(&self) -> Result<(String, Vec<u8>), prometheus::Error> {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        let mut buffer = Vec::with_capacity(8 * 1024);
        encoder.encode(&metric_families, &mut buffer)?;
        Ok((encoder.format_type().to_string(), buffer))
    }
}
