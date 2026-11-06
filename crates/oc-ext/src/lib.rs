//! MCP client + plugin system (port of MCP/plugin parts of `packages/core` and `packages/plugin`).
//!
//! `load_extensions` connects to every enabled server in `config.mcp` and registers each
//! MCP tool as an `oc_core::Tool` named `<server>_<tool>`.

pub mod client;
pub mod config;
pub mod plugin;
pub mod process;
pub mod rpc;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use oc_core::{Config, Tool, ToolContext, ToolHook, ToolRegistry};

pub use client::{McpClient, McpToolInfo};
pub use config::McpServerConfig;
pub use plugin::{Plugin, PluginRegistry};

/// Replace anything outside [a-zA-Z0-9_-] with '_'.
pub fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

pub struct McpTool {
    full_name: String,
    tool_name: String,
    description: String,
    schema: serde_json::Value,
    client: Arc<McpClient>,
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.full_name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn input_schema(&self) -> serde_json::Value {
        self.schema.clone()
    }
    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> anyhow::Result<String> {
        let (text, is_error) = self.client.call_tool(&self.tool_name, input).await?;
        if is_error {
            anyhow::bail!("{}", if text.is_empty() { "MCP tool returned an error".to_string() } else { text });
        }
        Ok(text)
    }
}

/// Connect to one server, handshake, and wrap its tools.
pub async fn connect_server(name: &str, cfg: &McpServerConfig) -> anyhow::Result<Vec<Arc<dyn Tool>>> {
    let timeout = cfg.timeout_ms().map(Duration::from_millis).unwrap_or(client::DEFAULT_TIMEOUT);
    let client = match cfg {
        McpServerConfig::Local { command, environment, .. } => {
            McpClient::connect_stdio(name, command, environment, timeout).await?
        }
        McpServerConfig::Remote { url, headers, .. } => McpClient::connect_http(name, url, headers, timeout)?,
    };
    client.initialize().await?;
    let tools = client.list_tools().await?;
    let client = Arc::new(client);
    Ok(tools
        .into_iter()
        .map(|t| {
            Arc::new(McpTool {
                full_name: format!("{}_{}", sanitize(name), sanitize(&t.name)),
                tool_name: t.name,
                description: t.description,
                schema: t.input_schema,
                client: client.clone(),
            }) as Arc<dyn Tool>
        })
        .collect())
}

pub async fn load_extensions(config: &Config, registry: &mut ToolRegistry) -> anyhow::Result<()> {
    let servers: Vec<_> = config::parse_mcp(&config.mcp)
        .into_iter()
        .filter_map(|(name, r)| match r {
            Ok(c) if c.enabled() => Some((name, c)),
            Ok(_) => None,
            Err(e) => {
                tracing::warn!("invalid MCP config '{name}': {e:#}");
                None
            }
        })
        .collect();
    let results = futures_join(servers).await;
    for (name, r) in results {
        match r {
            Ok(tools) => tools.into_iter().for_each(|t| registry.register(t)),
            Err(e) => tracing::warn!("MCP server '{name}' failed to load: {e:#}"),
        }
    }
    Ok(())
}

async fn futures_join(
    servers: Vec<(String, McpServerConfig)>,
) -> Vec<(String, anyhow::Result<Vec<Arc<dyn Tool>>>)> {
    let handles: Vec<_> = servers
        .into_iter()
        .map(|(name, cfg)| {
            let n = name.clone();
            (name, tokio::spawn(async move { connect_server(&n, &cfg).await }))
        })
        .collect();
    let mut out = Vec::new();
    for (name, h) in handles {
        let r = h.await.unwrap_or_else(|e| Err(anyhow::anyhow!("task failed: {e}")));
        out.push((name, r));
    }
    out
}

/// Starts the process plugins (see `PLUGINS.md`) and returns them as agent tool hooks.
/// Custom tools defined by plugins are dropped; use [`load_plugins_with_tools`] to get them.
pub async fn load_plugins(config: &Config, cwd: &std::path::Path) -> anyhow::Result<Vec<Arc<dyn ToolHook>>> {
    let mut scratch = ToolRegistry::default();
    load_plugins_with_tools(config, cwd, &mut scratch).await
}

/// Like [`load_plugins`], but also registers plugin-defined tools into `registry`.
/// A plugin that fails to start is logged and skipped.
pub async fn load_plugins_with_tools(
    config: &Config,
    cwd: &std::path::Path,
    registry: &mut ToolRegistry,
) -> anyhow::Result<Vec<Arc<dyn ToolHook>>> {
    let specs = process::discover(config, cwd);
    let mut hooks: Vec<Arc<dyn ToolHook>> = Vec::new();
    for spec in specs {
        match process::ProcessPlugin::start(&spec, cwd).await {
            Ok((plugin, tools)) => {
                tools.into_iter().for_each(|t| registry.register(t));
                hooks.push(plugin);
            }
            Err(e) => tracing::warn!("plugin '{}' failed to start: {e:#}", spec.name),
        }
    }
    Ok(hooks)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sanitizes() {
        assert_eq!(sanitize("my.server/x y"), "my_server_x_y");
        assert_eq!(sanitize("ok-1_a"), "ok-1_a");
    }
}
