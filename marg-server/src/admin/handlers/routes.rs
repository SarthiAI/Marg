use axum::extract::State;
use axum::Json;
use chrono::Utc;
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
