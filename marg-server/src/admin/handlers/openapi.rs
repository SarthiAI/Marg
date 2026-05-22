use axum::Json;
use serde_json::Value;

use crate::admin::error::AdminError;
use crate::admin::openapi;

pub async fn spec() -> Result<Json<Value>, AdminError> {
    Ok(Json(openapi::spec()))
}
