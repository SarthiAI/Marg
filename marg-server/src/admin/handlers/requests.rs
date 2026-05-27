use axum::extract::{Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use data_encoding::BASE64URL_NOPAD;
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
    pub team: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    /// Opaque cursor produced by a previous page (see `next_cursor` in the
    /// response body). Base64-url of `"<rfc3339-timestamp>|<id>"`.
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_limit() -> u32 { 100 }

pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<ListRequestsParams>,
) -> Result<Json<Value>, AdminError> {
    let (before_timestamp, before_id) = match params.cursor.as_deref() {
        None => (None, None),
        Some(raw) => decode_cursor(raw)?,
    };
    let limit = params.limit.clamp(1, 10_000);
    let query = RequestLogQuery {
        since: params.since,
        key_id: params.key_id.filter(|s| !s.trim().is_empty()),
        team: params.team.filter(|s| !s.trim().is_empty()),
        model: params.model.filter(|s| !s.trim().is_empty()),
        provider: params.provider.filter(|s| !s.trim().is_empty()),
        before_timestamp,
        before_id,
        limit,
    };
    let entries = state.storage.query_request_logs(query).await?;
    let next_cursor = if entries.len() as u32 == limit {
        entries
            .last()
            .map(|e| encode_cursor(&e.timestamp, &e.id))
    } else {
        None
    };
    Ok(Json(json!({ "entries": entries, "next_cursor": next_cursor })))
}

fn encode_cursor(ts: &DateTime<Utc>, id: &str) -> String {
    BASE64URL_NOPAD.encode(format!("{}|{}", ts.to_rfc3339(), id).as_bytes())
}

fn decode_cursor(raw: &str) -> Result<(Option<DateTime<Utc>>, Option<String>), AdminError> {
    let bytes = BASE64URL_NOPAD
        .decode(raw.as_bytes())
        .map_err(|e| AdminError::BadRequest(format!("invalid cursor: {}", e)))?;
    let decoded = std::str::from_utf8(&bytes)
        .map_err(|e| AdminError::BadRequest(format!("invalid cursor: {}", e)))?;
    let (ts_str, id) = decoded
        .split_once('|')
        .ok_or_else(|| AdminError::BadRequest("cursor must be '<timestamp>|<id>'".into()))?;
    let ts = DateTime::parse_from_rfc3339(ts_str)
        .map_err(|e| AdminError::BadRequest(format!("invalid cursor timestamp: {}", e)))?
        .with_timezone(&Utc);
    Ok((Some(ts), Some(id.to_string())))
}
