//! Extension points the agent loop calls into: permissions, tool hooks
//! (plugins) and diagnostics (LSP). Implementations live in other modules
//! and crates; the loop only knows these traits.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;

use crate::AgentEvent;

/// A tool call waiting for the user's (or the rules') approval.
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub session_id: String,
    pub call_id: String,
    pub tool: String,
    pub input: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow,
    /// Allow now and remember it for the rest of the session.
    AllowAlways,
    Deny { reason: Option<String> },
}

#[async_trait]
pub trait PermissionHandler: Send + Sync {
    /// Called before every tool call. The handler applies configured rules
    /// itself and only asks the user when the rules say "ask".
    async fn check(&self, req: &PermissionRequest) -> PermissionDecision;
}

#[async_trait]
pub trait ToolHook: Send + Sync {
    /// May rewrite the input; returning Err blocks the call (the error text
    /// becomes the tool result).
    async fn before_tool(&self, _tool: &str, _input: &mut serde_json::Value) -> anyhow::Result<()> {
        Ok(())
    }
    /// May rewrite the output of a successful call.
    async fn after_tool(&self, _tool: &str, _input: &serde_json::Value, _output: &mut String) -> anyhow::Result<()> {
        Ok(())
    }
    /// Sees every event the agent emits.
    async fn on_event(&self, _session_id: &str, _event: &AgentEvent) {}
}

#[async_trait]
pub trait DiagnosticsProvider: Send + Sync {
    /// Diagnostics for a file the agent just wrote or edited, formatted for
    /// the model, or None when there is nothing to report.
    async fn diagnostics(&self, path: &Path) -> Option<String>;
}

#[derive(Clone, Default)]
pub struct AgentHooks {
    pub permission: Option<Arc<dyn PermissionHandler>>,
    pub tools: Vec<Arc<dyn ToolHook>>,
    pub diagnostics: Option<Arc<dyn DiagnosticsProvider>>,
}
