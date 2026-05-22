use async_trait::async_trait;
use chrono::NaiveDate;
use std::sync::Arc;
use std::time::Instant;

use marg_core::{
    AdminToken, BudgetSpec, MargKey, NewAdminToken, NewKey, PersistedRoute, RequestLogEntry,
};
use marg_storage::{
    BudgetReservation, HotStore, HotStoreError, RequestLogQuery, Storage, StorageError,
};

use crate::metrics::Metrics;

/// Wraps any `Storage` impl with Prometheus timing.
///
/// Every call records a sample on
/// `marg_storage_query_duration_seconds{operation, backend}`.
pub struct MeteredStorage {
    inner: Arc<dyn Storage>,
    metrics: Arc<Metrics>,
    backend: &'static str,
}

impl MeteredStorage {
    pub fn new(inner: Arc<dyn Storage>, metrics: Arc<Metrics>) -> Self {
        let backend = inner.backend_name();
        Self { inner, metrics, backend }
    }

    fn record(&self, op: &str, started: Instant) {
        let elapsed = started.elapsed().as_secs_f64();
        self.metrics.observe_storage(op, self.backend, elapsed);
    }
}

#[async_trait]
impl Storage for MeteredStorage {
    fn backend_name(&self) -> &'static str {
        self.backend
    }

    async fn ping(&self) -> Result<(), StorageError> {
        let started = Instant::now();
        let r = self.inner.ping().await;
        self.record("ping", started);
        r
    }

    async fn migrate(&self) -> Result<(), StorageError> {
        let started = Instant::now();
        let r = self.inner.migrate().await;
        self.record("migrate", started);
        r
    }

    async fn create_key(&self, new: NewKey) -> Result<MargKey, StorageError> {
        let started = Instant::now();
        let r = self.inner.create_key(new).await;
        self.record("create_key", started);
        r
    }

    async fn get_key_by_hash(&self, hash: &str) -> Result<Option<MargKey>, StorageError> {
        let started = Instant::now();
        let r = self.inner.get_key_by_hash(hash).await;
        self.record("get_key_by_hash", started);
        r
    }

    async fn get_key_by_id(&self, id: &str) -> Result<Option<MargKey>, StorageError> {
        let started = Instant::now();
        let r = self.inner.get_key_by_id(id).await;
        self.record("get_key_by_id", started);
        r
    }

    async fn list_keys(&self) -> Result<Vec<MargKey>, StorageError> {
        let started = Instant::now();
        let r = self.inner.list_keys().await;
        self.record("list_keys", started);
        r
    }

    async fn revoke_key(&self, id: &str) -> Result<(), StorageError> {
        let started = Instant::now();
        let r = self.inner.revoke_key(id).await;
        self.record("revoke_key", started);
        r
    }

    async fn upsert_budget(&self, spec: BudgetSpec) -> Result<(), StorageError> {
        let started = Instant::now();
        let r = self.inner.upsert_budget(spec).await;
        self.record("upsert_budget", started);
        r
    }

    async fn get_budget(&self, key_id: &str) -> Result<Option<BudgetSpec>, StorageError> {
        let started = Instant::now();
        let r = self.inner.get_budget(key_id).await;
        self.record("get_budget", started);
        r
    }

    async fn current_spend(&self, key_id: &str, day: NaiveDate) -> Result<f64, StorageError> {
        let started = Instant::now();
        let r = self.inner.current_spend(key_id, day).await;
        self.record("current_spend", started);
        r
    }

    async fn add_spend(
        &self,
        key_id: &str,
        day: NaiveDate,
        amount_usd: f64,
    ) -> Result<(), StorageError> {
        let started = Instant::now();
        let r = self.inner.add_spend(key_id, day, amount_usd).await;
        self.record("add_spend", started);
        r
    }

    async fn add_spend_batch(
        &self,
        items: &[(String, NaiveDate, f64)],
    ) -> Result<(), StorageError> {
        let started = Instant::now();
        let r = self.inner.add_spend_batch(items).await;
        self.record("add_spend_batch", started);
        r
    }

    async fn append_request_log(&self, entry: RequestLogEntry) -> Result<(), StorageError> {
        let started = Instant::now();
        let r = self.inner.append_request_log(entry).await;
        self.record("append_request_log", started);
        r
    }

    async fn append_request_logs(
        &self,
        entries: Vec<RequestLogEntry>,
    ) -> Result<(), StorageError> {
        let started = Instant::now();
        let r = self.inner.append_request_logs(entries).await;
        self.record("append_request_logs", started);
        r
    }

    async fn recent_request_logs(
        &self,
        key_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<RequestLogEntry>, StorageError> {
        let started = Instant::now();
        let r = self.inner.recent_request_logs(key_id, limit).await;
        self.record("recent_request_logs", started);
        r
    }

    async fn query_request_logs(
        &self,
        q: RequestLogQuery,
    ) -> Result<Vec<RequestLogEntry>, StorageError> {
        let started = Instant::now();
        let r = self.inner.query_request_logs(q).await;
        self.record("query_request_logs", started);
        r
    }

    async fn create_admin_token(&self, new: NewAdminToken) -> Result<AdminToken, StorageError> {
        let started = Instant::now();
        let r = self.inner.create_admin_token(new).await;
        self.record("create_admin_token", started);
        r
    }

    async fn list_admin_tokens(&self) -> Result<Vec<AdminToken>, StorageError> {
        let started = Instant::now();
        let r = self.inner.list_admin_tokens().await;
        self.record("list_admin_tokens", started);
        r
    }

    async fn get_admin_token_by_hash(
        &self,
        hash: &str,
    ) -> Result<Option<AdminToken>, StorageError> {
        let started = Instant::now();
        let r = self.inner.get_admin_token_by_hash(hash).await;
        self.record("get_admin_token_by_hash", started);
        r
    }

    async fn revoke_admin_token(&self, id: &str) -> Result<(), StorageError> {
        let started = Instant::now();
        let r = self.inner.revoke_admin_token(id).await;
        self.record("revoke_admin_token", started);
        r
    }

    async fn count_active_admin_tokens(&self) -> Result<u64, StorageError> {
        let started = Instant::now();
        let r = self.inner.count_active_admin_tokens().await;
        self.record("count_active_admin_tokens", started);
        r
    }

    async fn list_routes(&self) -> Result<Vec<PersistedRoute>, StorageError> {
        let started = Instant::now();
        let r = self.inner.list_routes().await;
        self.record("list_routes", started);
        r
    }

    async fn insert_route(&self, route: PersistedRoute) -> Result<(), StorageError> {
        let started = Instant::now();
        let r = self.inner.insert_route(route).await;
        self.record("insert_route", started);
        r
    }

    async fn delete_route(&self, id: &str) -> Result<(), StorageError> {
        let started = Instant::now();
        let r = self.inner.delete_route(id).await;
        self.record("delete_route", started);
        r
    }
}

/// Wraps any `HotStore` impl with Prometheus timing.
pub struct MeteredHotStore {
    inner: Arc<dyn HotStore>,
    metrics: Arc<Metrics>,
    backend: &'static str,
}

impl MeteredHotStore {
    pub fn new(inner: Arc<dyn HotStore>, metrics: Arc<Metrics>) -> Self {
        let backend = inner.backend_name();
        Self { inner, metrics, backend }
    }

    fn record(&self, op: &str, started: Instant) {
        let elapsed = started.elapsed().as_secs_f64();
        self.metrics.observe_hot_store(op, self.backend, elapsed);
    }
}

#[async_trait]
impl HotStore for MeteredHotStore {
    fn backend_name(&self) -> &'static str {
        self.backend
    }

    async fn reserve_budget(
        &self,
        key_id: &str,
        day: NaiveDate,
        estimated_cost_usd: f64,
        limit_usd: f64,
    ) -> Result<BudgetReservation, HotStoreError> {
        let started = Instant::now();
        let r = self
            .inner
            .reserve_budget(key_id, day, estimated_cost_usd, limit_usd)
            .await;
        self.record("reserve_budget", started);
        r
    }

    async fn settle_budget(
        &self,
        key_id: &str,
        day: NaiveDate,
        actual_minus_estimated_usd: f64,
    ) -> Result<(), HotStoreError> {
        let started = Instant::now();
        let r = self
            .inner
            .settle_budget(key_id, day, actual_minus_estimated_usd)
            .await;
        self.record("settle_budget", started);
        r
    }

    async fn current_spend(
        &self,
        key_id: &str,
        day: NaiveDate,
    ) -> Result<f64, HotStoreError> {
        let started = Instant::now();
        let r = self.inner.current_spend(key_id, day).await;
        self.record("current_spend", started);
        r
    }

    async fn allow_request(
        &self,
        key_id: &str,
        rpm: u32,
        strict: bool,
    ) -> Result<bool, HotStoreError> {
        let started = Instant::now();
        let r = self.inner.allow_request(key_id, rpm, strict).await;
        self.record("allow_request", started);
        r
    }

    async fn invalidate_key(&self, key_id: &str) -> Result<(), HotStoreError> {
        let started = Instant::now();
        let r = self.inner.invalidate_key(key_id).await;
        self.record("invalidate_key", started);
        r
    }

    async fn ping(&self) -> Result<(), HotStoreError> {
        let started = Instant::now();
        let r = self.inner.ping().await;
        self.record("ping", started);
        r
    }
}
