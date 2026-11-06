//! Agent loop, sessions, tools, config (port of `packages/core`).
//!
//! PUBLIC API CONTRACT — oc-cli, oc-tui, oc-server and oc-ext depend on these
//! signatures. Owner: phase-1 agent. Additions are fine; do not change
//! existing signatures. Bodies marked `todo!()` are for phase 1 to implement.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use oc_ai::Provider;
use serde::{Deserialize, Serialize};

mod agent;
pub mod auth;
mod config;
pub mod hooks;
pub mod permission;
pub mod share;
mod prompt;
mod session;
pub mod tools;

pub use agent::{provider_from_config, provider_from_model};
pub use hooks::{AgentHooks, DiagnosticsProvider, PermissionDecision, PermissionHandler, PermissionRequest, ToolHook};
pub use config::Config;
pub use prompt::DEFAULT_SYSTEM_PROMPT;
pub use session::{Session, SessionStore};

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
        tools::builtin_tools()
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
    pub hooks: AgentHooks,
}

#[cfg(test)]
mod tests;
