//! Session sharing endpoints: `POST /share`, `PUT|DELETE /share/{id}`,
//! `GET /share/{id}` (HTML) and `GET /share/{id}/data` (JSON).

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::Json;
use oc_core::Session;
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::share_store::{secrets_match, StoredShare};
use crate::state::AppState;

fn parse_session(body: Value) -> Result<Value, ApiError> {
    let s: Session = serde_json::from_value(body).map_err(|e| ApiError::BadRequest(format!("invalid session: {e}")))?;
    serde_json::to_value(&s).map_err(|e| ApiError::Internal(e.to_string()))
}

fn public_base(headers: &HeaderMap) -> String {
    if let Ok(v) = std::env::var("OPENCODE_PUBLIC_URL") {
        let v = v.trim().trim_end_matches('/').to_string();
        if !v.is_empty() {
            return v;
        }
    }
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .filter(|h| h.chars().all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c)))
        .unwrap_or("127.0.0.1:4096");
    format!("http://{host}")
}

fn supplied_secret(headers: &HeaderMap) -> Option<String> {
    if let Some(v) = headers.get("x-share-secret").and_then(|v| v.to_str().ok()) {
        return Some(v.to_string());
    }
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
}

fn load_authorized(state: &AppState, id: &str, headers: &HeaderMap) -> Result<StoredShare, ApiError> {
    let share = state.shares.load(id)?.ok_or_else(|| ApiError::NotFound(format!("share {id} not found")))?;
    match supplied_secret(headers) {
        Some(s) if secrets_match(&s, &share.secret) => Ok(share),
        _ => Err(ApiError::Forbidden("missing or invalid share secret".into())),
    }
}

pub async fn create(State(state): State<AppState>, headers: HeaderMap, Json(body): Json<Value>) -> Result<Response, ApiError> {
    let session = parse_session(body)?;
    let share = state.shares.create(session)?;
    let url = format!("{}/share/{}", public_base(&headers), share.id);
    let out = json!({ "id": share.id, "url": url, "secret": share.secret });
    Ok((StatusCode::CREATED, Json(out)).into_response())
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut share = load_authorized(&state, &id, &headers)?;
    share.session = parse_session(body)?;
    state.shares.save(&share)?;
    Ok(Json(json!({ "id": share.id, "url": format!("{}/share/{}", public_base(&headers), share.id) })))
}

pub async fn remove(State(state): State<AppState>, Path(id): Path<String>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    load_authorized(&state, &id, &headers)?;
    state.shares.delete(&id)?;
    Ok(Json(json!({ "deleted": true })))
}

pub async fn data(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    let share = state.shares.load(&id)?.ok_or_else(|| ApiError::NotFound(format!("share {id} not found")))?;
    Ok(Json(share.session))
}

pub async fn page(State(state): State<AppState>, Path(id): Path<String>) -> Result<Html<String>, ApiError> {
    let share = state.shares.load(&id)?.ok_or_else(|| ApiError::NotFound(format!("share {id} not found")))?;
    let session: Session =
        serde_json::from_value(share.session).map_err(|e| ApiError::Internal(format!("stored session invalid: {e}")))?;
    Ok(Html(crate::share_html::render(&session)))
}
