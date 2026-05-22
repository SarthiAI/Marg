use axum::extract::{Path, State};
use axum::Json;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};

use marg_core::{MargToken, NewAdminToken};

use crate::admin::error::AdminError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateTokenRequest {
    #[serde(default)]
    pub label: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateTokenRequest>,
) -> Result<Json<Value>, AdminError> {
    let token = MargToken::generate();
    let plain = token.expose().to_string();
    let new = NewAdminToken {
        id: uuid::Uuid::new_v4().to_string(),
        token_hash: token.hash(),
        token_prefix: token.display_prefix(),
        label: req.label.unwrap_or_default(),
        created_at: Utc::now(),
    };
    let saved = state.storage.create_admin_token(new).await?;
    Ok(Json(json!({
        "token_record": saved,
        "token": plain,
    })))
}

pub async fn list(State(state): State<AppState>) -> Result<Json<Value>, AdminError> {
    let tokens = state.storage.list_admin_tokens().await?;
    Ok(Json(json!({ "tokens": tokens })))
}

pub async fn revoke(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AdminError> {
    state.storage.revoke_admin_token(&id).await?;
    state.admin_cache.invalidate_all();
    Ok(Json(json!({ "id": id, "revoked": true })))
}
