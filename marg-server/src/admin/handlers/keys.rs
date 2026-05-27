use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::str::FromStr;

use marg_core::{BudgetSpec, MargKey, MargToken, NewKey, PrincipalKind};

use crate::admin::error::AdminError;
use crate::kavach::{emit_key_event, KeyEventKind};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateKeyRequest {
    pub principal_id: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub team: Option<String>,
    #[serde(default)]
    pub daily_budget_usd: f64,
    #[serde(default)]
    pub rpm: u32,
}

fn default_kind() -> String { "user".to_string() }

#[derive(Debug, Serialize)]
pub struct CreateKeyResponse {
    pub key: MargKey,
    pub token: String,
    pub budget: BudgetSpec,
}

pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateKeyRequest>,
) -> Result<Json<CreateKeyResponse>, AdminError> {
    if req.principal_id.trim().is_empty() {
        return Err(AdminError::BadRequest("principal_id is required".into()));
    }
    let kind = PrincipalKind::from_str(&req.kind)
        .map_err(AdminError::BadRequest)?;
    if req.daily_budget_usd < 0.0 {
        return Err(AdminError::BadRequest(
            "daily_budget_usd must be >= 0".into(),
        ));
    }

    let token = MargToken::generate();
    let plain = token.expose().to_string();
    let new = NewKey::build(req.principal_id.clone(), kind, &token).with_team(req.team.clone());
    let key_id = new.id.clone();
    let saved = state.storage.create_key(new).await?;
    let budget = BudgetSpec {
        key_id: key_id.clone(),
        daily_usd: req.daily_budget_usd,
        rpm: req.rpm,
    };
    state.storage.upsert_budget(budget.clone()).await?;

    emit_key_event(
        &state.kavach.audit_chain,
        "admin",
        &saved.id,
        KeyEventKind::Created,
        Some(&format!("principal={} kind={}", req.principal_id, req.kind)),
    );

    Ok(Json(CreateKeyResponse {
        key: saved,
        token: plain,
        budget,
    }))
}

#[derive(Debug, Deserialize)]
pub struct ListKeysParams {
    #[serde(default)]
    pub principal: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<ListKeysParams>,
) -> Result<Json<Value>, AdminError> {
    let mut keys = state.storage.list_keys().await?;
    if let Some(p) = &params.principal {
        keys.retain(|k| k.principal.id == *p);
    }
    if let Some(k_str) = &params.kind {
        let want = PrincipalKind::from_str(k_str).map_err(AdminError::BadRequest)?;
        keys.retain(|k| k.principal.kind == want);
    }
    if let Some(s) = &params.status {
        keys.retain(|k| k.status.to_string() == *s);
    }
    Ok(Json(json!({ "keys": keys })))
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AdminError> {
    let key = state
        .storage
        .get_key_by_id(&id)
        .await?
        .ok_or(AdminError::NotFound)?;
    let budget = state.storage.get_budget(&id).await?;
    Ok(Json(json!({ "key": key, "budget": budget })))
}

pub async fn revoke(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AdminError> {
    state.storage.revoke_key(&id).await?;
    if let Err(e) = state.hot.invalidate_key(&id).await {
        tracing::warn!(?e, key_id = %id, "failed to invalidate hot store entry on revoke");
    }
    state.metrics.clear_budget_remaining(&id);
    state.key_cache.invalidate_all();
    emit_key_event(
        &state.kavach.audit_chain,
        "admin",
        &id,
        KeyEventKind::Revoked,
        Some("admin revoke"),
    );
    Ok(Json(json!({ "id": id, "revoked": true })))
}

pub async fn invalidate(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AdminError> {
    state
        .hot
        .invalidate_key(&id)
        .await
        .map_err(|e| AdminError::Storage(e.to_string()))?;
    state.key_cache.invalidate_all();
    // P09 ships single-node invalidation only; cluster broadcast lands in P10
    // when kavach-redis joins the workspace and the broadcaster swaps from
    // Noop to RedisInvalidationBroadcaster.
    emit_key_event(
        &state.kavach.audit_chain,
        "admin",
        &id,
        KeyEventKind::Invalidated,
        Some("admin invalidate"),
    );
    Ok(Json(json!({ "id": id, "invalidated": true })))
}
