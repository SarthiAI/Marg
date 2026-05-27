use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde_json::{json, Value};

use crate::state::AppState;

pub async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

pub async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    let mut status_code = StatusCode::OK;
    let mut storage = json!({"backend": state.storage.backend_name(), "ok": true});
    if let Err(e) = state.storage.ping().await {
        status_code = StatusCode::SERVICE_UNAVAILABLE;
        storage["ok"] = json!(false);
        storage["error"] = json!(e.to_string());
    }
    let mut hot = json!({"backend": state.hot.backend_name(), "ok": true});
    if let Err(e) = state.hot.ping().await {
        status_code = StatusCode::SERVICE_UNAVAILABLE;
        hot["ok"] = json!(false);
        hot["error"] = json!(e.to_string());
    }
    let body = Json(json!({
        "status": if status_code == StatusCode::OK { "ready" } else { "degraded" },
        "storage": storage,
        "hot": hot,
    }));
    (status_code, body)
}

pub async fn version() -> Json<Value> {
    Json(version_info())
}

pub fn version_info() -> Value {
    json!({
        "marg": env!("CARGO_PKG_VERSION"),
        "build_timestamp_unix": env!("MARG_BUILD_TIMESTAMP"),
        "kavach_core": crate::KAVACH_CORE_VERSION,
        "kavach_pq": crate::KAVACH_PQ_VERSION,
        "kavach_redis": crate::KAVACH_REDIS_VERSION,
    })
}
