use axum::extract::{Path, State};
use axum::Json;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};

use marg_core::BudgetSpec;

use crate::admin::error::AdminError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsertBudgetRequest {
    pub key_id: String,
    pub daily_usd: f64,
    #[serde(default)]
    pub rpm: u32,
}

pub async fn upsert(
    State(state): State<AppState>,
    Json(req): Json<UpsertBudgetRequest>,
) -> Result<Json<Value>, AdminError> {
    if req.key_id.trim().is_empty() {
        return Err(AdminError::BadRequest("key_id is required".into()));
    }
    if req.daily_usd < 0.0 {
        return Err(AdminError::BadRequest("daily_usd must be >= 0".into()));
    }
    let spec = BudgetSpec {
        key_id: req.key_id.clone(),
        daily_usd: req.daily_usd,
        rpm: req.rpm,
    };
    state.storage.upsert_budget(spec.clone()).await?;
    state.key_cache.invalidate_all();
    Ok(Json(json!({ "budget": spec })))
}

pub async fn get(
    State(state): State<AppState>,
    Path(key_id): Path<String>,
) -> Result<Json<Value>, AdminError> {
    let budget = state.storage.get_budget(&key_id).await?;
    let day = Utc::now().date_naive();
    let spent = state.storage.current_spend(&key_id, day).await?;
    Ok(Json(json!({
        "budget": budget,
        "day": day.to_string(),
        "spent_usd": spent,
        "remaining_usd": budget
            .as_ref()
            .map(|b| if b.daily_usd > 0.0 { (b.daily_usd - spent).max(0.0) } else { f64::INFINITY })
            .unwrap_or(f64::INFINITY),
    })))
}
