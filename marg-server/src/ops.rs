use axum::{Json, extract::State};
use serde_json::{json, Value};

use crate::state::AppState;

pub async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

pub async fn ready(State(state): State<AppState>) -> Json<Value> {
    // Try a cheap storage round-trip so /ready actually reflects readiness.
    match state.storage.list_keys().await {
        Ok(_) => Json(json!({ "status": "ready" })),
        Err(e) => Json(json!({ "status": "degraded", "error": e.to_string() })),
    }
}

pub async fn version() -> Json<Value> {
    Json(version_info())
}

pub fn version_info() -> Value {
    json!({
        "marg": env!("CARGO_PKG_VERSION"),
        "build_timestamp_unix": env!("MARG_BUILD_TIMESTAMP"),
    })
}
