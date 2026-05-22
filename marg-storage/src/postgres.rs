use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::{PgPool, Row};
use std::str::FromStr;
use std::time::Duration;

use marg_core::{
    BudgetSpec, KeyStatus, MargKey, NewKey, Principal, PrincipalKind, RequestLogEntry,
};

use crate::{Storage, StorageError};

#[derive(Clone)]
pub struct PostgresStorage {
    pool: PgPool,
}

impl PostgresStorage {
    pub async fn connect(dsn: &str) -> Result<Self, StorageError> {
        let pool = PgPoolOptions::new()
            .max_connections(32)
            .min_connections(2)
            .acquire_timeout(Duration::from_secs(5))
            .connect(dsn)
            .await
            .map_err(map_err)?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

fn map_err<E: std::fmt::Display>(e: E) -> StorageError {
    StorageError::Backend(e.to_string())
}

fn map_sqlx(e: sqlx::Error) -> StorageError {
    use sqlx::Error;
    if matches!(&e, Error::Database(db) if db.is_unique_violation()) {
        return StorageError::Duplicate(e.to_string());
    }
    if let Error::RowNotFound = &e {
        return StorageError::NotFound;
    }
    StorageError::Backend(e.to_string())
}

fn row_to_key(row: &PgRow) -> Result<MargKey, StorageError> {
    let principal_kind: String = row.try_get("principal_kind").map_err(map_sqlx)?;
    let status: String = row.try_get("status").map_err(map_sqlx)?;
    let created_at: DateTime<Utc> = row.try_get("created_at").map_err(map_sqlx)?;
    let revoked_at: Option<DateTime<Utc>> = row.try_get("revoked_at").map_err(map_sqlx)?;
    let team: Option<String> = row.try_get("team").map_err(map_sqlx)?;
    Ok(MargKey {
        id: row.try_get("id").map_err(map_sqlx)?,
        token_hash: row.try_get("token_hash").map_err(map_sqlx)?,
        token_prefix: row.try_get("token_prefix").map_err(map_sqlx)?,
        principal: Principal {
            id: row.try_get("principal_id").map_err(map_sqlx)?,
            kind: PrincipalKind::from_str(&principal_kind).map_err(StorageError::Backend)?,
        },
        team,
        status: KeyStatus::from_str(&status).map_err(StorageError::Backend)?,
        created_at,
        revoked_at,
    })
}

#[async_trait]
impl Storage for PostgresStorage {
    fn backend_name(&self) -> &'static str {
        "postgres"
    }

    async fn ping(&self) -> Result<(), StorageError> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(map_sqlx)
    }

    async fn migrate(&self) -> Result<(), StorageError> {
        sqlx::migrate!("./migrations/postgres")
            .run(&self.pool)
            .await
            .map_err(|e| StorageError::Backend(format!("migrate: {}", e)))?;
        Ok(())
    }

    async fn create_key(&self, new: NewKey) -> Result<MargKey, StorageError> {
        sqlx::query(
            "INSERT INTO keys (id, token_hash, token_prefix, principal_id, principal_kind, status, created_at, team) \
             VALUES ($1, $2, $3, $4, $5, 'active', $6, $7)",
        )
        .bind(&new.id)
        .bind(&new.token_hash)
        .bind(&new.token_prefix)
        .bind(&new.principal_id)
        .bind(new.principal_kind.to_string())
        .bind(new.created_at)
        .bind(&new.team)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;

        Ok(MargKey {
            id: new.id,
            token_hash: new.token_hash,
            token_prefix: new.token_prefix,
            principal: Principal {
                id: new.principal_id,
                kind: new.principal_kind,
            },
            team: new.team,
            status: KeyStatus::Active,
            created_at: new.created_at,
            revoked_at: None,
        })
    }

    async fn get_key_by_hash(&self, hash: &str) -> Result<Option<MargKey>, StorageError> {
        let row = sqlx::query("SELECT * FROM keys WHERE token_hash = $1")
            .bind(hash)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx)?;
        match row {
            Some(r) => Ok(Some(row_to_key(&r)?)),
            None => Ok(None),
        }
    }

    async fn get_key_by_id(&self, id: &str) -> Result<Option<MargKey>, StorageError> {
        let row = sqlx::query("SELECT * FROM keys WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx)?;
        match row {
            Some(r) => Ok(Some(row_to_key(&r)?)),
            None => Ok(None),
        }
    }

    async fn list_keys(&self) -> Result<Vec<MargKey>, StorageError> {
        let rows = sqlx::query("SELECT * FROM keys ORDER BY created_at DESC")
            .fetch_all(&self.pool)
            .await
            .map_err(map_sqlx)?;
        let mut keys = Vec::with_capacity(rows.len());
        for row in &rows {
            keys.push(row_to_key(row)?);
        }
        Ok(keys)
    }

    async fn revoke_key(&self, id: &str) -> Result<(), StorageError> {
        let result = sqlx::query(
            "UPDATE keys SET status = 'revoked', revoked_at = NOW() WHERE id = $1 AND status = 'active'",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;
        if result.rows_affected() == 0 {
            return Err(StorageError::NotFound);
        }
        Ok(())
    }

    async fn upsert_budget(&self, spec: BudgetSpec) -> Result<(), StorageError> {
        sqlx::query(
            "INSERT INTO budgets (key_id, daily_usd, rpm) VALUES ($1, $2, $3) \
             ON CONFLICT (key_id) DO UPDATE SET daily_usd = excluded.daily_usd, rpm = excluded.rpm",
        )
        .bind(&spec.key_id)
        .bind(spec.daily_usd)
        .bind(spec.rpm as i32)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    async fn get_budget(&self, key_id: &str) -> Result<Option<BudgetSpec>, StorageError> {
        let row = sqlx::query("SELECT key_id, daily_usd, rpm FROM budgets WHERE key_id = $1")
            .bind(key_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx)?;
        Ok(row.map(|r| BudgetSpec {
            key_id: r.get("key_id"),
            daily_usd: r.get("daily_usd"),
            rpm: r.get::<i32, _>("rpm") as u32,
        }))
    }

    async fn current_spend(&self, key_id: &str, day: NaiveDate) -> Result<f64, StorageError> {
        let row = sqlx::query("SELECT spent_usd FROM budget_counters WHERE key_id = $1 AND day = $2")
            .bind(key_id)
            .bind(day)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx)?;
        Ok(row.map(|r| r.get::<f64, _>("spent_usd")).unwrap_or(0.0))
    }

    async fn add_spend(&self, key_id: &str, day: NaiveDate, amount_usd: f64) -> Result<(), StorageError> {
        if amount_usd <= 0.0 {
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO budget_counters (key_id, day, spent_usd) VALUES ($1, $2, $3) \
             ON CONFLICT (key_id, day) DO UPDATE SET spent_usd = budget_counters.spent_usd + excluded.spent_usd",
        )
        .bind(key_id)
        .bind(day)
        .bind(amount_usd)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    async fn append_request_log(&self, entry: RequestLogEntry) -> Result<(), StorageError> {
        let attempts_json: Option<serde_json::Value> = if entry.attempts.is_empty() {
            None
        } else {
            Some(
                serde_json::to_value(&entry.attempts)
                    .map_err(|e| StorageError::Backend(format!("serialize attempts: {}", e)))?,
            )
        };
        sqlx::query(
            "INSERT INTO request_log (id, timestamp, key_id, principal_id, provider, model, \
             input_tokens, output_tokens, cost_usd, latency_ms, status, stream, error, attempts) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
        )
        .bind(&entry.id)
        .bind(entry.timestamp)
        .bind(&entry.key_id)
        .bind(&entry.principal_id)
        .bind(&entry.provider)
        .bind(&entry.model)
        .bind(entry.input_tokens as i64)
        .bind(entry.output_tokens as i64)
        .bind(entry.cost_usd)
        .bind(entry.latency_ms as i64)
        .bind(entry.status as i32)
        .bind(entry.stream)
        .bind(&entry.error)
        .bind(&attempts_json)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    async fn recent_request_logs(
        &self,
        key_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<RequestLogEntry>, StorageError> {
        let limit = limit.min(10_000) as i64;
        let rows = match key_id {
            Some(k) => {
                sqlx::query(
                    "SELECT * FROM request_log WHERE key_id = $1 ORDER BY timestamp DESC LIMIT $2",
                )
                .bind(k)
                .bind(limit)
                .fetch_all(&self.pool)
                .await
                .map_err(map_sqlx)?
            }
            None => {
                sqlx::query("SELECT * FROM request_log ORDER BY timestamp DESC LIMIT $1")
                    .bind(limit)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(map_sqlx)?
            }
        };
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let timestamp: DateTime<Utc> = r.try_get("timestamp").map_err(map_sqlx)?;
            let attempts_raw: Option<serde_json::Value> = r.try_get("attempts").map_err(map_sqlx)?;
            let attempts = match attempts_raw {
                Some(v) => serde_json::from_value(v)
                    .map_err(|e| StorageError::Backend(format!("decode attempts: {}", e)))?,
                None => Vec::new(),
            };
            out.push(RequestLogEntry {
                id: r.try_get("id").map_err(map_sqlx)?,
                timestamp,
                key_id: r.try_get("key_id").map_err(map_sqlx)?,
                principal_id: r.try_get("principal_id").map_err(map_sqlx)?,
                provider: r.try_get("provider").map_err(map_sqlx)?,
                model: r.try_get("model").map_err(map_sqlx)?,
                input_tokens: r.try_get::<i64, _>("input_tokens").map_err(map_sqlx)? as u64,
                output_tokens: r.try_get::<i64, _>("output_tokens").map_err(map_sqlx)? as u64,
                cost_usd: r.try_get("cost_usd").map_err(map_sqlx)?,
                latency_ms: r.try_get::<i64, _>("latency_ms").map_err(map_sqlx)? as u64,
                status: r.try_get::<i32, _>("status").map_err(map_sqlx)? as u16,
                stream: r.try_get::<bool, _>("stream").map_err(map_sqlx)?,
                error: r.try_get("error").map_err(map_sqlx)?,
                attempts,
            });
        }
        Ok(out)
    }
}
