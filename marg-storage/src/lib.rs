use async_trait::async_trait;
use chrono::NaiveDate;
use thiserror::Error;

use marg_core::{
    AdminToken, BudgetSpec, MargKey, NewAdminToken, NewKey, PersistedRoute, RequestLogEntry,
};

pub mod hot;
pub mod hot_local;

#[cfg(feature = "sqlite")]
pub mod sqlite;
#[cfg(feature = "postgres")]
pub mod postgres;
#[cfg(feature = "redis-hot")]
pub mod hot_redis;

#[cfg(feature = "sqlite")]
pub use sqlite::SqliteStorage;
#[cfg(feature = "postgres")]
pub use postgres::PostgresStorage;
#[cfg(feature = "redis-hot")]
pub use hot_redis::RedisHotStore;

pub use hot::{BudgetReservation, HotStore, HotStoreError};
pub use hot_local::LocalHotStore;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("not found")]
    NotFound,

    #[error("duplicate: {0}")]
    Duplicate(String),

    #[error("backend error: {0}")]
    Backend(String),
}

#[async_trait]
pub trait Storage: Send + Sync {
    fn backend_name(&self) -> &'static str;
    async fn ping(&self) -> Result<(), StorageError>;
    async fn migrate(&self) -> Result<(), StorageError>;

    async fn create_key(&self, new: NewKey) -> Result<MargKey, StorageError>;
    async fn get_key_by_hash(&self, hash: &str) -> Result<Option<MargKey>, StorageError>;
    async fn get_key_by_id(&self, id: &str) -> Result<Option<MargKey>, StorageError>;
    async fn list_keys(&self) -> Result<Vec<MargKey>, StorageError>;
    async fn revoke_key(&self, id: &str) -> Result<(), StorageError>;

    async fn upsert_budget(&self, spec: BudgetSpec) -> Result<(), StorageError>;
    async fn get_budget(&self, key_id: &str) -> Result<Option<BudgetSpec>, StorageError>;

    async fn current_spend(&self, key_id: &str, day: NaiveDate) -> Result<f64, StorageError>;
    async fn add_spend(&self, key_id: &str, day: NaiveDate, amount_usd: f64) -> Result<(), StorageError>;

    /// Apply many spend deltas in one round-trip. Backends should fold same
    /// `(key_id, day)` entries together and execute a single multi-row upsert.
    /// Default impl loops so older backends keep working; production backends
    /// override for throughput.
    async fn add_spend_batch(
        &self,
        items: &[(String, NaiveDate, f64)],
    ) -> Result<(), StorageError> {
        for (key_id, day, amount) in items {
            self.add_spend(key_id, *day, *amount).await?;
        }
        Ok(())
    }

    async fn append_request_log(&self, entry: RequestLogEntry) -> Result<(), StorageError>;

    /// Append many request log rows in one round-trip. Default impl loops.
    /// Production backends override with a multi-row INSERT.
    async fn append_request_logs(
        &self,
        entries: Vec<RequestLogEntry>,
    ) -> Result<(), StorageError> {
        for e in entries {
            self.append_request_log(e).await?;
        }
        Ok(())
    }

    async fn recent_request_logs(&self, key_id: Option<&str>, limit: u32) -> Result<Vec<RequestLogEntry>, StorageError>;
    async fn query_request_logs(&self, q: RequestLogQuery) -> Result<Vec<RequestLogEntry>, StorageError>;

    // P05 admin surface
    async fn create_admin_token(&self, new: NewAdminToken) -> Result<AdminToken, StorageError>;
    async fn list_admin_tokens(&self) -> Result<Vec<AdminToken>, StorageError>;
    async fn get_admin_token_by_hash(&self, hash: &str) -> Result<Option<AdminToken>, StorageError>;
    async fn revoke_admin_token(&self, id: &str) -> Result<(), StorageError>;
    async fn count_active_admin_tokens(&self) -> Result<u64, StorageError>;

    async fn list_routes(&self) -> Result<Vec<PersistedRoute>, StorageError>;
    async fn insert_route(&self, route: PersistedRoute) -> Result<(), StorageError>;
    async fn update_route(&self, route: PersistedRoute) -> Result<(), StorageError>;
    async fn delete_route(&self, id: &str) -> Result<(), StorageError>;
}

/// Filter set accepted by [`Storage::query_request_logs`]. Mirrors the
/// `GET /admin/requests?since=...&key_id=...&model=...` admin endpoint.
#[derive(Debug, Clone, Default)]
pub struct RequestLogQuery {
    pub since: Option<chrono::DateTime<chrono::Utc>>,
    pub key_id: Option<String>,
    pub team: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    /// Cursor: timestamp of the last row from the previous page. Rows with
    /// the exact same timestamp are disambiguated by [`Self::before_id`].
    pub before_timestamp: Option<chrono::DateTime<chrono::Utc>>,
    pub before_id: Option<String>,
    pub limit: u32,
}
