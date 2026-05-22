use axum::extract::{Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use marg_storage::RequestLogQuery;

use crate::admin::error::AdminError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ListRequestsParams {
    #[serde(default)]
    pub since: Option<DateTime<Utc>>,
    #[serde(default)]
    pub key_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_limit() -> u32 { 100 }

pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<ListRequestsParams>,
) -> Result<Json<Value>, AdminError> {
    let query = RequestLogQuery {
        since: params.since,
        key_id: params.key_id.filter(|s| !s.trim().is_empty()),
        model: params.model.filter(|s| !s.trim().is_empty()),
        provider: params.provider.filter(|s| !s.trim().is_empty()),
        limit: params.limit.clamp(1, 10_000),
    };
    let entries = state.storage.query_request_logs(query).await?;
    Ok(Json(json!({ "entries": entries })))
}
