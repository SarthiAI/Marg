use async_trait::async_trait;
use chrono::NaiveDate;
use thiserror::Error;

use marg_core::{BudgetSpec, MargKey, NewKey, RequestLogEntry};

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

    async fn append_request_log(&self, entry: RequestLogEntry) -> Result<(), StorageError>;
    async fn recent_request_logs(&self, key_id: Option<&str>, limit: u32) -> Result<Vec<RequestLogEntry>, StorageError>;
}
