use axum::{routing::get, Json, Router};
use serde_json::{json, Value};

pub fn router() -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/version", get(version))
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

async fn ready() -> Json<Value> {
    Json(json!({ "status": "ready" }))
}

async fn version() -> Json<Value> {
    Json(version_info())
}

pub fn version_info() -> Value {
    json!({
        "marg": env!("CARGO_PKG_VERSION"),
        "build_timestamp_unix": env!("MARG_BUILD_TIMESTAMP"),
    })
}
