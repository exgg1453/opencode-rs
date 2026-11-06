//! Abstractions over session storage and the agent so the HTTP layer is testable.

use std::path::PathBuf;

use async_trait::async_trait;
use oc_core::{Agent, AgentEvent, Session, SessionStore};
use tokio::sync::mpsc::UnboundedSender;

/// Session persistence.
pub trait SessionRepo: Send + Sync + 'static {
    fn save(&self, s: &Session) -> anyhow::Result<()>;
    /// `Ok(None)` when the session does not exist.
    fn load(&self, id: &str) -> anyhow::Result<Option<Session>>;
    fn list(&self) -> anyhow::Result<Vec<Session>>;
    /// Returns whether a session was removed.
    fn delete(&self, id: &str) -> anyhow::Result<bool>;
}

/// Runs one prompt turn against a session.
#[async_trait]
pub trait TurnRunner: Send + Sync + 'static {
    async fn run(
        &self,
        session: &mut Session,
        input: String,
        events: UnboundedSender<AgentEvent>,
    ) -> anyhow::Result<()>;
}

#[async_trait]
impl TurnRunner for Agent {
    async fn run(
        &self,
        session: &mut Session,
        input: String,
        events: UnboundedSender<AgentEvent>,
    ) -> anyhow::Result<()> {
        self.prompt(session, input, events).await
    }
}

fn sessions_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("opencode-rs").join("sessions")
}

impl SessionRepo for SessionStore {
    fn save(&self, s: &Session) -> anyhow::Result<()> {
        SessionStore::save(self, s)
    }
    fn load(&self, id: &str) -> anyhow::Result<Option<Session>> {
        if !valid_id(id) {
            return Ok(None);
        }
        // The store reports a missing session as an error; map an absent file to None.
        match SessionStore::load(self, id) {
            Ok(s) => Ok(Some(s)),
            Err(e) => {
                if sessions_dir().join(format!("{id}.json")).exists() {
                    Err(e)
                } else {
                    Ok(None)
                }
            }
        }
    }
    fn list(&self) -> anyhow::Result<Vec<Session>> {
        SessionStore::list(self)
    }
    fn delete(&self, id: &str) -> anyhow::Result<bool> {
        // SessionStore has no delete API; remove the documented on-disk file.
        if !valid_id(id) {
            return Ok(false);
        }
        match std::fs::remove_file(sessions_dir().join(format!("{id}.json"))) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}
