//! MCP client + plugin system (port of MCP/plugin parts of `packages/core` and `packages/plugin`).
//! Owner: phase-5 agent.
//!
//! CONTRACT: oc-cli calls `oc_ext::load_extensions(&config, &mut registry)` to add
//! MCP-server tools into the agent's ToolRegistry.

use oc_core::{Config, ToolRegistry};

pub async fn load_extensions(_config: &Config, _registry: &mut ToolRegistry) -> anyhow::Result<()> {
    todo!("phase 5")
}
