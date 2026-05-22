use async_trait::async_trait;
use chrono::NaiveDate;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HotStoreError {
    #[error("hot store backend unreachable: {0}")]
    Unreachable(String),

    #[error("hot store internal error: {0}")]
    Internal(String),
}

#[derive(Debug, Clone, Copy)]
pub struct BudgetReservation {
    pub granted: bool,
    pub spent_after: f64,
}

#[async_trait]
pub trait HotStore: Send + Sync {
    fn backend_name(&self) -> &'static str;

    /// Atomic check-and-increment for budget enforcement. Returns `granted=false`
    /// when the reservation would push current spend over `limit_usd`.
    async fn reserve_budget(
        &self,
        key_id: &str,
        day: NaiveDate,
        estimated_cost_usd: f64,
        limit_usd: f64,
    ) -> Result<BudgetReservation, HotStoreError>;

    /// Apply the delta between estimated and actual cost after the request
    /// completed. `actual_minus_estimated` may be negative.
    async fn settle_budget(
        &self,
        key_id: &str,
        day: NaiveDate,
        actual_minus_estimated_usd: f64,
    ) -> Result<(), HotStoreError>;

    /// Snapshot current spend without modifying it. Used to seed reporting.
    async fn current_spend(
        &self,
        key_id: &str,
        day: NaiveDate,
    ) -> Result<f64, HotStoreError>;

    /// Returns true if the request fits inside the per-key rpm budget. An
    /// rpm of zero disables the limit. When `strict` is true the bucket is
    /// configured as capacity 1 with refill = rpm/60 per second: the sustained
    /// rate is exactly rpm with zero burst tolerance. When `strict` is false
    /// (default token-bucket convention) capacity = rpm and refill = rpm per
    /// 60 000 ms: a fresh bucket starts full and a steady stream sustains rpm.
    async fn allow_request(
        &self,
        key_id: &str,
        rpm: u32,
        strict: bool,
    ) -> Result<bool, HotStoreError>;

    /// Invalidate any cached hot state for this key (e.g. after revocation).
    async fn invalidate_key(&self, key_id: &str) -> Result<(), HotStoreError>;

    /// Cheap reachability probe used by /ready.
    async fn ping(&self) -> Result<(), HotStoreError>;
}
