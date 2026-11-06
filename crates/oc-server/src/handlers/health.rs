use axum::Json;
use serde_json::{json, Value};

use crate::error::ApiError;

pub async fn health() -> Json<Value> {
    Json(json!({ "healthy": true, "version": env!("CARGO_PKG_VERSION") }))
}

pub async fn not_found() -> ApiError {
    ApiError::NotFound("route not found".into())
}
