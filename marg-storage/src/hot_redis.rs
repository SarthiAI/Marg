use async_trait::async_trait;
use chrono::NaiveDate;
use deadpool_redis::{Config as PoolConfig, Pool, Runtime};
use redis::{AsyncCommands, Script};

use crate::hot::{BudgetReservation, HotStore, HotStoreError};

const SPEND_TTL_SECONDS: u64 = 90_000; // ~25 hours, covers a UTC day boundary
// TTL for an idle token bucket: long enough that a key idle for a few minutes
// keeps its bucket warm, short enough that long-idle keys are reclaimed.
const RATE_BUCKET_TTL_SECONDS: u64 = 600;

pub struct RedisHotStore {
    pool: Pool,
    reserve_script: Script,
    rate_script: Script,
    key_prefix: String,
}

impl RedisHotStore {
    pub async fn connect(url: &str, key_prefix: Option<String>) -> Result<Self, HotStoreError> {
        let cfg = PoolConfig::from_url(url);
        let pool = cfg
            .create_pool(Some(Runtime::Tokio1))
            .map_err(|e| HotStoreError::Internal(format!("create redis pool: {}", e)))?;
        // Probe immediately so a misconfigured URL surfaces at startup, not on
        // the first request.
        let mut conn = pool
            .get()
            .await
            .map_err(|e| HotStoreError::Unreachable(format!("acquire connection: {}", e)))?;
        let _: String = redis::cmd("PING")
            .query_async(&mut conn)
            .await
            .map_err(|e| HotStoreError::Unreachable(format!("PING: {}", e)))?;

        let reserve_script = Script::new(
            r"
local current = tonumber(redis.call('GET', KEYS[1]) or '0')
local cost = tonumber(ARGV[1])
local limit = tonumber(ARGV[2])
if limit > 0 and (current + cost) > limit then
    return {0, tostring(current)}
end
local new_total = current + cost
redis.call('SET', KEYS[1], tostring(new_total), 'EX', ARGV[3])
return {1, tostring(new_total)}
",
        );
        // Token bucket.
        // KEYS[1] = bucket key
        // ARGV[1] = rpm, ARGV[2] = now_ms, ARGV[3] = ttl_seconds, ARGV[4] = strict (0 or 1)
        // strict=0: capacity = rpm (default token-bucket, burst up to rpm)
        // strict=1: capacity = 1 (no burst, sustained rate exactly rpm)
        let rate_script = Script::new(
            r"
local rpm = tonumber(ARGV[1])
local now_ms = tonumber(ARGV[2])
local ttl = tonumber(ARGV[3])
local strict = tonumber(ARGV[4])
local capacity
if strict == 1 then
    capacity = 1
else
    capacity = rpm
end
local refill_per_ms = rpm / 60000.0

local data = redis.call('HMGET', KEYS[1], 'tokens', 'last_ms')
local tokens = tonumber(data[1]) or capacity
local last = tonumber(data[2]) or now_ms

local elapsed = now_ms - last
if elapsed < 0 then elapsed = 0 end
tokens = tokens + elapsed * refill_per_ms
if tokens > capacity then tokens = capacity end

local allowed = 0
if tokens >= 1.0 then
    tokens = tokens - 1.0
    allowed = 1
end

redis.call('HMSET', KEYS[1], 'tokens', tokens, 'last_ms', now_ms)
redis.call('EXPIRE', KEYS[1], ttl)
return allowed
",
        );

        Ok(Self {
            pool,
            reserve_script,
            rate_script,
            key_prefix: key_prefix.unwrap_or_else(|| "marg".to_string()),
        })
    }

    fn spend_key(&self, key_id: &str, day: NaiveDate) -> String {
        format!("{}:spend:{}:{}", self.key_prefix, day, key_id)
    }

    fn rate_key(&self, key_id: &str) -> String {
        format!("{}:rate:{}", self.key_prefix, key_id)
    }
}

fn map_redis_err(prefix: &str, e: impl std::fmt::Display) -> HotStoreError {
    HotStoreError::Unreachable(format!("{}: {}", prefix, e))
}

#[async_trait]
impl HotStore for RedisHotStore {
    fn backend_name(&self) -> &'static str {
        "redis"
    }

    async fn reserve_budget(
        &self,
        key_id: &str,
        day: NaiveDate,
        estimated_cost_usd: f64,
        limit_usd: f64,
    ) -> Result<BudgetReservation, HotStoreError> {
        let key = self.spend_key(key_id, day);
        let mut conn = self
            .pool
            .get()
            .await
            .map_err(|e| map_redis_err("acquire connection", e))?;
        let result: (i64, String) = self
            .reserve_script
            .key(key)
            .arg(estimated_cost_usd.to_string())
            .arg(limit_usd.to_string())
            .arg(SPEND_TTL_SECONDS)
            .invoke_async(&mut *conn)
            .await
            .map_err(|e| map_redis_err("reserve script", e))?;
        let spent_after: f64 = result.1.parse().unwrap_or(0.0);
        Ok(BudgetReservation {
            granted: result.0 == 1,
            spent_after,
        })
    }

    async fn settle_budget(
        &self,
        key_id: &str,
        day: NaiveDate,
        actual_minus_estimated_usd: f64,
    ) -> Result<(), HotStoreError> {
        if actual_minus_estimated_usd.abs() < f64::EPSILON {
            return Ok(());
        }
        let key = self.spend_key(key_id, day);
        let mut conn = self
            .pool
            .get()
            .await
            .map_err(|e| map_redis_err("acquire connection", e))?;
        let _: f64 = conn
            .incr(&key, actual_minus_estimated_usd)
            .await
            .map_err(|e| map_redis_err("incrbyfloat", e))?;
        let _: bool = conn
            .expire(&key, SPEND_TTL_SECONDS as i64)
            .await
            .map_err(|e| map_redis_err("expire", e))?;
        Ok(())
    }

    async fn current_spend(
        &self,
        key_id: &str,
        day: NaiveDate,
    ) -> Result<f64, HotStoreError> {
        let key = self.spend_key(key_id, day);
        let mut conn = self
            .pool
            .get()
            .await
            .map_err(|e| map_redis_err("acquire connection", e))?;
        let raw: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| map_redis_err("get", e))?;
        Ok(raw.and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0))
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
        let now_ms = chrono::Utc::now().timestamp_millis();
        let key = self.rate_key(key_id);
        let mut conn = self
            .pool
            .get()
            .await
            .map_err(|e| map_redis_err("acquire connection", e))?;
        let allowed: i64 = self
            .rate_script
            .key(key)
            .arg(rpm)
            .arg(now_ms)
            .arg(RATE_BUCKET_TTL_SECONDS)
            .arg(if strict { 1 } else { 0 })
            .invoke_async(&mut *conn)
            .await
            .map_err(|e| map_redis_err("rate script", e))?;
        Ok(allowed == 1)
    }

    async fn invalidate_key(&self, key_id: &str) -> Result<(), HotStoreError> {
        let mut conn = self
            .pool
            .get()
            .await
            .map_err(|e| map_redis_err("acquire connection", e))?;
        // Best-effort cleanup of all per-key hot state. We scan with a pattern
        // and delete; this is rarely called so the cost is acceptable.
        let pattern = format!("{}:*:*{}*", self.key_prefix, key_id);
        let mut cursor: u64 = 0;
        loop {
            let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(100)
                .query_async(&mut *conn)
                .await
                .map_err(|e| map_redis_err("scan", e))?;
            if !keys.is_empty() {
                let _: i64 = conn
                    .del(&keys)
                    .await
                    .map_err(|e| map_redis_err("del", e))?;
            }
            if next == 0 {
                break;
            }
            cursor = next;
        }
        Ok(())
    }

    async fn ping(&self) -> Result<(), HotStoreError> {
        let mut conn = self
            .pool
            .get()
            .await
            .map_err(|e| map_redis_err("acquire connection", e))?;
        let _: String = redis::cmd("PING")
            .query_async(&mut *conn)
            .await
            .map_err(|e| map_redis_err("PING", e))?;
        Ok(())
    }
}

