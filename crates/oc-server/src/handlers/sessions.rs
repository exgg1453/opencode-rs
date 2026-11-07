//! Session CRUD, prompting and abort.

use std::path::PathBuf;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use oc_ai::{ChatMessage, Role};
use oc_core::{AgentEvent, Session};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::state::{AppState, SessionEvent};

#[derive(Serialize)]
pub struct SessionSummary {
    id: String,
    title: String,
    cwd: PathBuf,
    message_count: usize,
    running: bool,
}

fn summary(state: &AppState, s: &Session) -> SessionSummary {
    SessionSummary {
        id: s.id.clone(),
        title: s.title.clone(),
        cwd: s.cwd.clone(),
        message_count: s.messages.len(),
        running: state.is_running(&s.id),
    }
}

fn load(state: &AppState, id: &str) -> Result<Session, ApiError> {
    state.repo.load(id)?.ok_or_else(|| ApiError::NotFound(format!("session {id} not found")))
}

pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<SessionSummary>>, ApiError> {
    let sessions = state.repo.list()?;
    Ok(Json(sessions.iter().map(|s| summary(&state, s)).collect()))
}

#[derive(Deserialize, Default)]
pub struct CreateBody {
    cwd: Option<PathBuf>,
}

pub async fn create(
    State(state): State<AppState>,
    body: Option<Json<CreateBody>>,
) -> Result<(StatusCode, Json<Session>), ApiError> {
    let requested = body.and_then(|Json(b)| b.cwd);
    let cwd = state.security.session_dir(requested.as_deref())?;
    let session = Session::new(cwd);
    state.repo.save(&session)?;
    Ok((StatusCode::CREATED, Json(session)))
}

pub async fn get(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Session>, ApiError> {
    Ok(Json(load(&state, &id)?))
}

pub async fn delete(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    if state.is_running(&id) {
        return Err(ApiError::Busy(format!("session {id} has a running turn")));
    }
    if !state.repo.delete(&id)? {
        return Err(ApiError::NotFound(format!("session {id} not found")));
    }
    Ok(Json(json!({ "deleted": true })))
}

pub async fn messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<ChatMessage>>, ApiError> {
    Ok(Json(load(&state, &id)?.messages))
}

#[derive(Deserialize)]
pub struct PromptBody {
    text: String,
}

pub async fn prompt(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Result<Json<PromptBody>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<ChatMessage>, ApiError> {
    let Json(body) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    if body.text.trim().is_empty() {
        return Err(ApiError::BadRequest("text must not be empty".into()));
    }
    let mut session = load(&state, &id)?;
    // Sessions created elsewhere (e.g. by the TUI) may live outside the server root.
    state.security.session_dir(Some(&session.cwd))?;
    let guard = state.begin_turn(&id)?;

    // Forward agent events, tagged with the session id, onto the broadcast bus.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
    let bus = state.events.clone();
    let sid = id.clone();
    let forward = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let _ = bus.send(SessionEvent { session_id: sid.clone(), event });
        }
    });

    // Run in a separate task so a dropped client connection does not cancel the turn.
    let runner = state.runner.clone();
    let repo = state.repo.clone();
    let token = guard.token.clone();
    let text = body.text;
    let task = tokio::spawn(async move {
        let _guard = guard;
        let outcome = tokio::select! {
            r = runner.run(&mut session, text, tx) => Some(r),
            _ = token.cancelled() => None,
        };
        let saved = repo.save(&session);
        (session, outcome, saved)
    });
    let (session, outcome, saved) =
        task.await.map_err(|e| ApiError::Internal(format!("turn task failed: {e}")))?;
    let _ = forward.await;

    match outcome {
        None => return Err(ApiError::Aborted(format!("turn for session {id} was aborted"))),
        Some(r) => r?,
    }
    saved?;
    session
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::Assistant)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::Internal("turn produced no assistant message".into()))
}

pub async fn abort(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    if state.abort(&id) {
        return Ok(Json(json!({ "aborted": true })));
    }
    // Distinguish unknown session from idle one.
    load(&state, &id)?;
    Ok(Json(json!({ "aborted": false })))
}
