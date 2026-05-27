use axum::extract::{Path, State};
use axum::Json;
use chrono::Utc;
use marg_storage::StorageError;
use serde_json::{json, Value};

use marg_core::{NewRouteRequest, PersistedRoute};

use crate::admin::error::AdminError;
use crate::policy;
use crate::state::AppState;

pub async fn list(State(state): State<AppState>) -> Result<Json<Value>, AdminError> {
    let stored = state.storage.list_routes().await?;
    Ok(Json(json!({ "routes": stored })))
}

pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<NewRouteRequest>,
) -> Result<Json<Value>, AdminError> {
    if req.primary.is_none() && req.split.is_empty() {
        return Err(AdminError::BadRequest(
            "route must declare either `primary` or `split`".into(),
        ));
    }
    if req.primary.is_some() && !req.split.is_empty() {
        return Err(AdminError::BadRequest(
            "route may not declare both `primary` and `split`".into(),
        ));
    }
    let existing = state.storage.list_routes().await?;
    let position = req.position.unwrap_or_else(|| {
        existing
            .iter()
            .map(|r| r.position)
            .max()
            .unwrap_or(-1)
            .saturating_add(1)
    });
    let route = PersistedRoute {
        id: uuid::Uuid::new_v4().to_string(),
        position,
        match_model: req.match_model.clone(),
        match_team: req.match_team.clone(),
        primary: req.primary.clone(),
        primary_model: req.primary_model.clone(),
        fallbacks: req.fallbacks.clone(),
        split: req.split.clone(),
        created_at: Utc::now(),
    };
    state.storage.insert_route(route.clone()).await?;

    if let Err(e) = policy::reload(&state).await {
        // Rolling back: drop the route we just inserted so the live policy
        // and the persisted set stay in sync.
        let _ = state.storage.delete_route(&route.id).await;
        return Err(AdminError::BadRequest(format!(
            "route stored but policy reload failed (route was rolled back): {}",
            e
        )));
    }
    Ok(Json(json!({ "route": route })))
}

/// PUT /admin/routes/:id - replace one stored route in place. Validates the
/// same primary-vs-split invariant as `create`. Reloads policy after the
/// swap; on reload failure the previous shape is restored so the live
/// engine and the database stay in sync. `position` may be omitted to keep
/// the current ordering.
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<NewRouteRequest>,
) -> Result<Json<Value>, AdminError> {
    if req.primary.is_none() && req.split.is_empty() {
        return Err(AdminError::BadRequest(
            "route must declare either `primary` or `split`".into(),
        ));
    }
    if req.primary.is_some() && !req.split.is_empty() {
        return Err(AdminError::BadRequest(
            "route may not declare both `primary` and `split`".into(),
        ));
    }
    let existing_all = state.storage.list_routes().await?;
    let previous = existing_all
        .into_iter()
        .find(|r| r.id == id)
        .ok_or(AdminError::NotFound)?;
    let next = PersistedRoute {
        id: previous.id.clone(),
        position: req.position.unwrap_or(previous.position),
        match_model: req.match_model.clone(),
        match_team: req.match_team.clone(),
        primary: req.primary.clone(),
        primary_model: req.primary_model.clone(),
        fallbacks: req.fallbacks.clone(),
        split: req.split.clone(),
        created_at: previous.created_at,
    };
    state.storage.update_route(next.clone()).await.map_err(|e| match e {
        StorageError::NotFound => AdminError::NotFound,
        other => AdminError::Storage(other.to_string()),
    })?;
    if let Err(e) = policy::reload(&state).await {
        // Restore the previous shape so the live engine and the persisted
        // set stay in sync, then surface the reload failure to the caller.
        let _ = state.storage.update_route(previous).await;
        return Err(AdminError::BadRequest(format!(
            "route updated but policy reload failed (route was rolled back): {}",
            e
        )));
    }
    Ok(Json(json!({ "route": next })))
}

/// DELETE /admin/routes/:id - drop one stored route. The live engine reloads
/// from the new set immediately. If the reload fails (which only happens if
/// the remaining route set is itself malformed, e.g. a config-side route
/// referencing a provider that is no longer registered), the deleted row is
/// reinserted so the engine and the database stay in sync.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AdminError> {
    let existing_all = state.storage.list_routes().await?;
    let previous = existing_all
        .into_iter()
        .find(|r| r.id == id)
        .ok_or(AdminError::NotFound)?;
    state.storage.delete_route(&id).await.map_err(|e| match e {
        StorageError::NotFound => AdminError::NotFound,
        other => AdminError::Storage(other.to_string()),
    })?;
    if let Err(e) = policy::reload(&state).await {
        let _ = state.storage.insert_route(previous).await;
        return Err(AdminError::BadRequest(format!(
            "route deleted but policy reload failed (route was restored): {}",
            e
        )));
    }
    Ok(Json(json!({ "id": id, "deleted": true })))
}
