//! Startup: config, provider, tools, agent.

use std::path::PathBuf;

use anyhow::Result;
use oc_core::{Agent, AgentHooks, Config, ToolRegistry};

use crate::cli::{ThinkingArg, DEFAULT_MODEL};

pub const SYSTEM_PROMPT: &str = "You are OpenCode, an interactive AI coding agent running in the user's terminal. \
Help with software engineering tasks: read and search the codebase, edit files, run commands, and explain your work concisely. \
Prefer using the available tools over guessing, make minimal focused changes, and follow existing project conventions.";

/// Model precedence: CLI flag, then config, then default.
pub fn resolve_model(flag: Option<&str>, config: &Config) -> String {
    flag.map(str::to_string)
        .or_else(|| config.model.clone())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string())
}

/// Thinking precedence: CLI flag, then config (unset means off).
pub fn resolve_thinking(flag: Option<ThinkingArg>, config: &Config) -> Result<Option<oc_ai::ThinkingLevel>> {
    match flag {
        Some(f) => Ok(f.level()),
        None => config.thinking_level(),
    }
}

pub fn resolve_cwd(cwd: Option<PathBuf>) -> Result<PathBuf> {
    let p = match cwd {
        Some(p) => p,
        None => std::env::current_dir()?,
    };
    Ok(std::fs::canonicalize(&p).unwrap_or(p))
}

pub async fn build_agent(model_flag: Option<&str>, thinking_flag: Option<ThinkingArg>, cwd: &std::path::Path) -> Result<Agent> {
    // A project's own opencode.json / .opencode/plugins can define programs that would run
    // at startup. Only run them when the user has trusted this directory; otherwise ignore
    // those project-sourced, command-spawning settings (global config is always trusted).
    let trusted = oc_core::trust::is_trusted(cwd);
    let (config, project_wants_commands) = Config::load_gated(cwd, trusted)?;
    let project_plugin_dir = cwd.join(".opencode/plugins").is_dir();
    if !trusted && (project_wants_commands || project_plugin_dir) {
        eprintln!(
            "warning: this folder defines plugins/MCP/LSP commands that can run programs on your\n\
             machine. They are disabled because the folder is not trusted. If you trust it, run:\n\
             \n    opencode trust\n\n\
             (only do this for code you trust — it lets the project start programs automatically)"
        );
    }

    let thinking = resolve_thinking(thinking_flag, &config)?;
    let model = resolve_model(model_flag, &config);
    let (provider, model_id) = oc_core::provider_from_config(&model, &config)?;
    let mut tools = ToolRegistry::builtin();
    if let Err(e) = oc_ext::load_extensions(&config, &mut tools).await {
        tracing::warn!("failed to load extensions: {e:#}");
        eprintln!("warning: failed to load extensions: {e:#}");
    }
    let mut hooks = AgentHooks::default();
    match oc_ext::load_plugins_with_tools(&config, cwd, &mut tools, trusted).await {
        Ok(p) => hooks.tools = p,
        Err(e) => eprintln!("warning: failed to load plugins: {e:#}"),
    }
    hooks.diagnostics = oc_lsp::start(&config, cwd).await;
    // The permission handler depends on the front end (TUI, run, serve) and is set by the caller.
    Ok(Agent { provider, model: model_id, tools, system_prompt: SYSTEM_PROMPT.to_string(), hooks, thinking })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_precedence() {
        let mut c = Config::default();
        assert_eq!(resolve_model(None, &c), DEFAULT_MODEL);
        c.model = Some("openai/x".into());
        assert_eq!(resolve_model(None, &c), "openai/x");
        assert_eq!(resolve_model(Some("a/b"), &c), "a/b");
    }

    #[test]
    fn thinking_precedence() {
        use oc_ai::ThinkingLevel::*;
        let mut c = Config::default();
        assert_eq!(resolve_thinking(None, &c).unwrap(), None);
        c.thinking = Some("medium".into());
        assert_eq!(resolve_thinking(None, &c).unwrap(), Some(Medium));
        assert_eq!(resolve_thinking(Some(ThinkingArg::High), &c).unwrap(), Some(High));
        assert_eq!(resolve_thinking(Some(ThinkingArg::Off), &c).unwrap(), None);
        c.thinking = Some("nope".into());
        assert!(resolve_thinking(None, &c).is_err());
        assert_eq!(resolve_thinking(Some(ThinkingArg::Low), &c).unwrap(), Some(Low));
    }
}
