use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Row, SqlitePool};
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use marg_core::{
    BudgetSpec, KeyStatus, MargKey, NewKey, Principal, PrincipalKind, RequestLogEntry,
};

use crate::{Storage, StorageError};

#[derive(Clone)]
pub struct SqliteStorage {
    pool: SqlitePool,
}

impl SqliteStorage {
    pub async fn open(path: &str) -> Result<Self, StorageError> {
        let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path))
            .map_err(map_err)?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(16)
            .min_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect_with(opts)
            .await
            .map_err(map_err)?;

        Ok(Self { pool })
    }

    pub async fn open_default() -> Result<Self, StorageError> {
        Self::open("./marg.db").await
    }

    pub async fn migrate(&self) -> Result<(), StorageError> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(|e| StorageError::Backend(format!("migrate: {}", e)))?;
        Ok(())
    }

    pub fn pool(&self) -> &SqlitePool {
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

fn datetime_from_str(s: &str) -> Result<DateTime<Utc>, StorageError> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| StorageError::Backend(format!("invalid datetime '{}': {}", s, e)))
}

fn row_to_key(row: &sqlx::sqlite::SqliteRow) -> Result<MargKey, StorageError> {
    let principal_kind: String = row.try_get("principal_kind").map_err(map_sqlx)?;
    let status: String = row.try_get("status").map_err(map_sqlx)?;
    let created_at: String = row.try_get("created_at").map_err(map_sqlx)?;
    let revoked_at: Option<String> = row.try_get("revoked_at").map_err(map_sqlx)?;
    let team: Option<String> = row.try_get("team").map_err(map_sqlx)?;
    Ok(MargKey {
        id: row.try_get("id").map_err(map_sqlx)?,
        token_hash: row.try_get("token_hash").map_err(map_sqlx)?,
        token_prefix: row.try_get("token_prefix").map_err(map_sqlx)?,
        principal: Principal {
            id: row.try_get("principal_id").map_err(map_sqlx)?,
            kind: PrincipalKind::from_str(&principal_kind)
                .map_err(StorageError::Backend)?,
        },
        team,
        status: KeyStatus::from_str(&status).map_err(StorageError::Backend)?,
        created_at: datetime_from_str(&created_at)?,
        revoked_at: match revoked_at {
            Some(s) => Some(datetime_from_str(&s)?),
            None => None,
        },
    })
}

#[async_trait]
impl Storage for SqliteStorage {
    async fn create_key(&self, new: NewKey) -> Result<MargKey, StorageError> {
        let created_at_str = new.created_at.to_rfc3339();
        sqlx::query(
            "INSERT INTO keys (id, token_hash, token_prefix, principal_id, principal_kind, status, created_at, team) \
             VALUES (?, ?, ?, ?, ?, 'active', ?, ?)",
        )
        .bind(&new.id)
        .bind(&new.token_hash)
        .bind(&new.token_prefix)
        .bind(&new.principal_id)
        .bind(new.principal_kind.to_string())
        .bind(&created_at_str)
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
        let row = sqlx::query("SELECT * FROM keys WHERE token_hash = ?")
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
        let row = sqlx::query("SELECT * FROM keys WHERE id = ?")
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
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE keys SET status = 'revoked', revoked_at = ? WHERE id = ? AND status = 'active'",
        )
        .bind(&now)
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
            "INSERT INTO budgets (key_id, daily_usd, rpm) VALUES (?, ?, ?) \
             ON CONFLICT(key_id) DO UPDATE SET daily_usd = excluded.daily_usd, rpm = excluded.rpm",
        )
        .bind(&spec.key_id)
        .bind(spec.daily_usd)
        .bind(spec.rpm as i64)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    async fn get_budget(&self, key_id: &str) -> Result<Option<BudgetSpec>, StorageError> {
        let row = sqlx::query("SELECT key_id, daily_usd, rpm FROM budgets WHERE key_id = ?")
            .bind(key_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx)?;
        Ok(row.map(|r| BudgetSpec {
            key_id: r.get("key_id"),
            daily_usd: r.get("daily_usd"),
            rpm: r.get::<i64, _>("rpm") as u32,
        }))
    }

    async fn current_spend(&self, key_id: &str, day: NaiveDate) -> Result<f64, StorageError> {
        let day_str = day.to_string();
        let row = sqlx::query("SELECT spent_usd FROM budget_counters WHERE key_id = ? AND day = ?")
            .bind(key_id)
            .bind(&day_str)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx)?;
        Ok(row.map(|r| r.get::<f64, _>("spent_usd")).unwrap_or(0.0))
    }

    async fn add_spend(&self, key_id: &str, day: NaiveDate, amount_usd: f64) -> Result<(), StorageError> {
        if amount_usd <= 0.0 {
            return Ok(());
        }
        let day_str = day.to_string();
        sqlx::query(
            "INSERT INTO budget_counters (key_id, day, spent_usd) VALUES (?, ?, ?) \
             ON CONFLICT(key_id, day) DO UPDATE SET spent_usd = spent_usd + excluded.spent_usd",
        )
        .bind(key_id)
        .bind(&day_str)
        .bind(amount_usd)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    async fn append_request_log(&self, entry: RequestLogEntry) -> Result<(), StorageError> {
        let attempts_json: Option<String> = if entry.attempts.is_empty() {
            None
        } else {
            Some(
                serde_json::to_string(&entry.attempts)
                    .map_err(|e| StorageError::Backend(format!("serialize attempts: {}", e)))?,
            )
        };
        sqlx::query(
            "INSERT INTO request_log (id, timestamp, key_id, principal_id, provider, model, \
             input_tokens, output_tokens, cost_usd, latency_ms, status, stream, error, attempts) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&entry.id)
        .bind(entry.timestamp.to_rfc3339())
        .bind(&entry.key_id)
        .bind(&entry.principal_id)
        .bind(&entry.provider)
        .bind(&entry.model)
        .bind(entry.input_tokens as i64)
        .bind(entry.output_tokens as i64)
        .bind(entry.cost_usd)
        .bind(entry.latency_ms as i64)
        .bind(entry.status as i64)
        .bind(if entry.stream { 1i64 } else { 0i64 })
        .bind(&entry.error)
        .bind(&attempts_json)
        .execute(&self.pool)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    async fn recent_request_logs(&self, key_id: Option<&str>, limit: u32) -> Result<Vec<RequestLogEntry>, StorageError> {
        let limit = limit.min(10_000) as i64;
        let rows = match key_id {
            Some(k) => {
                sqlx::query("SELECT * FROM request_log WHERE key_id = ? ORDER BY timestamp DESC LIMIT ?")
                    .bind(k)
                    .bind(limit)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(map_sqlx)?
            }
            None => {
                sqlx::query("SELECT * FROM request_log ORDER BY timestamp DESC LIMIT ?")
                    .bind(limit)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(map_sqlx)?
            }
        };
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let ts_str: String = r.try_get("timestamp").map_err(map_sqlx)?;
            let timestamp = datetime_from_str(&ts_str)?;
            let stream_int: i64 = r.try_get("stream").map_err(map_sqlx)?;
            let attempts_raw: Option<String> = r.try_get("attempts").map_err(map_sqlx)?;
            let attempts = match attempts_raw.as_deref() {
                Some(s) if !s.is_empty() => serde_json::from_str(s)
                    .map_err(|e| StorageError::Backend(format!("decode attempts: {}", e)))?,
                _ => Vec::new(),
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
                status: r.try_get::<i64, _>("status").map_err(map_sqlx)? as u16,
                stream: stream_int != 0,
                error: r.try_get("error").map_err(map_sqlx)?,
                attempts,
            });
        }
        Ok(out)
    }
}

#[allow(dead_code)]
fn _path_compat<P: AsRef<Path>>(_p: P) {}
