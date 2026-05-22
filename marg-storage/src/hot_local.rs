use async_trait::async_trait;
use chrono::{NaiveDate, Utc};
use std::collections::HashMap;
use std::sync::Mutex;

use crate::hot::{BudgetReservation, HotStore, HotStoreError};

#[derive(Default)]
struct Counters {
    spend: HashMap<(String, NaiveDate), f64>,
    rate: HashMap<String, RateBucket>,
}

#[derive(Clone, Copy)]
struct RateBucket {
    tokens: f64,
    last_refill_unix_ms: i64,
}

pub struct LocalHotStore {
    state: Mutex<Counters>,
}

impl Default for LocalHotStore {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalHotStore {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(Counters::default()),
        }
    }
}

#[async_trait]
impl HotStore for LocalHotStore {
    fn backend_name(&self) -> &'static str {
        "local"
    }

    async fn reserve_budget(
        &self,
        key_id: &str,
        day: NaiveDate,
        estimated_cost_usd: f64,
        limit_usd: f64,
    ) -> Result<BudgetReservation, HotStoreError> {
        let mut g = self
            .state
            .lock()
            .map_err(|e| HotStoreError::Internal(format!("poisoned mutex: {}", e)))?;
        let key = (key_id.to_string(), day);
        let current = *g.spend.get(&key).unwrap_or(&0.0);
        if limit_usd > 0.0 && current + estimated_cost_usd > limit_usd {
            return Ok(BudgetReservation {
                granted: false,
                spent_after: current,
            });
        }
        let next = current + estimated_cost_usd;
        g.spend.insert(key, next);
        Ok(BudgetReservation {
            granted: true,
            spent_after: next,
        })
    }

    async fn settle_budget(
        &self,
        key_id: &str,
        day: NaiveDate,
        actual_minus_estimated_usd: f64,
    ) -> Result<(), HotStoreError> {
        let mut g = self
            .state
            .lock()
            .map_err(|e| HotStoreError::Internal(format!("poisoned mutex: {}", e)))?;
        let key = (key_id.to_string(), day);
        let current = *g.spend.get(&key).unwrap_or(&0.0);
        let next = (current + actual_minus_estimated_usd).max(0.0);
        g.spend.insert(key, next);
        Ok(())
    }

    async fn current_spend(
        &self,
        key_id: &str,
        day: NaiveDate,
    ) -> Result<f64, HotStoreError> {
        let g = self
            .state
            .lock()
            .map_err(|e| HotStoreError::Internal(format!("poisoned mutex: {}", e)))?;
        Ok(*g.spend.get(&(key_id.to_string(), day)).unwrap_or(&0.0))
    }

    async fn allow_request(
        &self,
        key_id: &str,
        rpm: u32,
        strict: bool,
    ) -> Result<bool, HotStoreError> {
        if rpm == 0 {
            return Ok(true);
        }
        // Token bucket. In default mode capacity = rpm and refill = rpm per
        // 60 000 ms (burst tolerance up to rpm, sustained rate rpm). In strict
        // mode capacity = 1 and refill = rpm/60 per second (no burst, sustained
        // rate exactly rpm).
        let mut g = self
            .state
            .lock()
            .map_err(|e| HotStoreError::Internal(format!("poisoned mutex: {}", e)))?;
        let now_ms = Utc::now().timestamp_millis();
        let capacity: f64 = if strict { 1.0 } else { rpm as f64 };
        let refill_per_ms = (rpm as f64) / 60_000.0;
        let bucket = g.rate.entry(key_id.to_string()).or_insert(RateBucket {
            tokens: capacity,
            last_refill_unix_ms: now_ms,
        });
        let elapsed_ms = (now_ms - bucket.last_refill_unix_ms).max(0) as f64;
        bucket.tokens = (bucket.tokens + elapsed_ms * refill_per_ms).min(capacity);
        bucket.last_refill_unix_ms = now_ms;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn invalidate_key(&self, key_id: &str) -> Result<(), HotStoreError> {
        let mut g = self
            .state
            .lock()
            .map_err(|e| HotStoreError::Internal(format!("poisoned mutex: {}", e)))?;
        g.rate.remove(key_id);
        g.spend.retain(|(k, _), _| k != key_id);
        Ok(())
    }

    async fn ping(&self) -> Result<(), HotStoreError> {
        Ok(())
    }
}
