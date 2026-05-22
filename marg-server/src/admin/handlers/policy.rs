use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use marg_core::Config;

use crate::admin::error::AdminError;
use crate::policy;
use crate::state::AppState;

pub async fn view(State(state): State<AppState>) -> Result<Json<Value>, AdminError> {
    let cfg = Config::load(state.config_path.as_str())
        .map_err(|e| AdminError::Internal(format!("read config: {}", e)))?;
    let stored = state.storage.list_routes().await?;
    let providers: Vec<String> = state.providers.keys().cloned().collect();
    Ok(Json(json!({
        "config_path": state.config_path.as_str(),
        "providers": providers,
        "default_provider": cfg.providers.default,
        "config_routes": cfg.routes,
        "stored_routes": stored,
        "pricing": cfg.pricing,
    })))
}

pub async fn reload(State(state): State<AppState>) -> Result<Json<Value>, AdminError> {
    let outcome = policy::reload(&state)
        .await
        .map_err(|e| AdminError::BadRequest(e.to_string()))?;
    Ok(Json(json!({
        "reloaded": true,
        "config_routes": outcome.config_routes,
        "stored_routes": outcome.stored_routes,
        "pricing_entries": outcome.pricing_entries,
    })))
}
