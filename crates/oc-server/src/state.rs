//! Shared application state: backends, event bus and per-session turn tracking.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use oc_core::{Agent, AgentEvent, SessionStore};
use serde::Serialize;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::backend::{SessionRepo, TurnRunner};
use crate::error::ApiError;

/// An agent event tagged with the session that produced it.
#[derive(Debug, Clone, Serialize)]
pub struct SessionEvent {
    pub session_id: String,
    pub event: AgentEvent,
}

#[derive(Clone)]
pub struct AppState {
    pub repo: Arc<dyn SessionRepo>,
    pub runner: Arc<dyn TurnRunner>,
    pub events: broadcast::Sender<SessionEvent>,
    running: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

impl AppState {
    pub fn new(repo: Arc<dyn SessionRepo>, runner: Arc<dyn TurnRunner>) -> Self {
        let (events, _) = broadcast::channel(1024);
        AppState { repo, runner, events, running: Default::default() }
    }

    /// Build state from a real agent (any `Provider`, including mocks) and store.
    pub fn from_agent(agent: Agent, store: SessionStore) -> Self {
        Self::new(Arc::new(store), Arc::new(agent))
    }

    /// Claim the single turn slot of a session; `Busy` if already taken.
    pub fn begin_turn(&self, id: &str) -> Result<TurnGuard, ApiError> {
        let mut map = self.running.lock().unwrap();
        if map.contains_key(id) {
            return Err(ApiError::Busy(format!("session {id} already has a running turn")));
        }
        let token = CancellationToken::new();
        map.insert(id.to_string(), token.clone());
        Ok(TurnGuard { id: id.to_string(), token, running: self.running.clone() })
    }

    /// Cancel a running turn; returns whether one was running.
    pub fn abort(&self, id: &str) -> bool {
        match self.running.lock().unwrap().get(id) {
            Some(t) => {
                t.cancel();
                true
            }
            None => false,
        }
    }

    pub fn is_running(&self, id: &str) -> bool {
        self.running.lock().unwrap().contains_key(id)
    }
}

/// Releases the session's turn slot on drop.
pub struct TurnGuard {
    id: String,
    pub token: CancellationToken,
    running: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        self.running.lock().unwrap().remove(&self.id);
    }
}
