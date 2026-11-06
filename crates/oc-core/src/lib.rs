//! Agent loop, sessions, tools, config (port of `packages/core`).
//!
//! PUBLIC API CONTRACT — oc-cli, oc-tui, oc-server and oc-ext depend on these
//! signatures. Owner: phase-1 agent. Additions are fine; do not change
//! existing signatures. Bodies marked `todo!()` are for phase 1 to implement.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use oc_ai::{ChatMessage, Provider};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// "provider/model", e.g. "anthropic/claude-sonnet-5-5".
    pub model: Option<String>,
    #[serde(default)]
    pub mcp: serde_json::Map<String, serde_json::Value>,
}

impl Config {
    /// Load from global (~/.config/opencode/opencode.json) and project (./opencode.json) files.
    pub fn load(_cwd: &std::path::Path) -> anyhow::Result<Config> {
        todo!("phase 1")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub cwd: PathBuf,
    pub messages: Vec<ChatMessage>,
}

impl Session {
    pub fn new(cwd: PathBuf) -> Session {
        Session { id: uuid::Uuid::new_v4().to_string(), title: "New session".into(), cwd, messages: Vec::new() }
    }
}

/// Persists sessions on disk (~/.local/share/opencode-rs/sessions/<id>.json).
pub struct SessionStore;

impl SessionStore {
    pub fn open_default() -> anyhow::Result<SessionStore> {
        todo!("phase 1")
    }
    pub fn save(&self, _s: &Session) -> anyhow::Result<()> {
        todo!("phase 1")
    }
    pub fn load(&self, _id: &str) -> anyhow::Result<Session> {
        todo!("phase 1")
    }
    pub fn list(&self) -> anyhow::Result<Vec<Session>> {
        todo!("phase 1")
    }
}

/// Events emitted while the agent runs; consumed by CLI, TUI and server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    TextDelta { text: String },
    ToolStart { id: String, name: String, input: serde_json::Value },
    ToolEnd { id: String, output: String, is_error: bool },
    Done,
    Error { message: String },
}

pub struct ToolContext {
    pub cwd: PathBuf,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> serde_json::Value;
    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> anyhow::Result<String>;
}

#[derive(Clone, Default)]
pub struct ToolRegistry {
    pub tools: Vec<Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Built-in tools: read, write, edit, bash, grep, glob, list.
    pub fn builtin() -> ToolRegistry {
        todo!("phase 1")
    }
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.push(tool);
    }
}

pub struct Agent {
    pub provider: Arc<dyn Provider>,
    pub model: String,
    pub tools: ToolRegistry,
    pub system_prompt: String,
}

impl Agent {
    /// Runs the model/tool loop for one user prompt, appending to `session.messages`
    /// and streaming progress to `events`. Ends with `AgentEvent::Done` or `Error`.
    pub async fn prompt(
        &self,
        _session: &mut Session,
        _input: String,
        _events: UnboundedSender<AgentEvent>,
    ) -> anyhow::Result<()> {
        todo!("phase 1")
    }
}

/// Builds a provider from "provider/model" using env vars (ANTHROPIC_API_KEY, OPENAI_API_KEY).
pub fn provider_from_model(_model: &str) -> anyhow::Result<(Arc<dyn Provider>, String)> {
    todo!("phase 1")
}
