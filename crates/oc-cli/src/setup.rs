//! Startup: config, provider, tools, agent.

use std::path::PathBuf;

use anyhow::Result;
use oc_core::{Agent, Config, ToolRegistry};

use crate::cli::DEFAULT_MODEL;

pub const SYSTEM_PROMPT: &str = "You are OpenCode, an interactive AI coding agent running in the user's terminal. \
Help with software engineering tasks: read and search the codebase, edit files, run commands, and explain your work concisely. \
Prefer using the available tools over guessing, make minimal focused changes, and follow existing project conventions.";

/// Model precedence: CLI flag, then config, then default.
pub fn resolve_model(flag: Option<&str>, config: &Config) -> String {
    flag.map(str::to_string)
        .or_else(|| config.model.clone())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string())
}

pub fn resolve_cwd(cwd: Option<PathBuf>) -> Result<PathBuf> {
    let p = match cwd {
        Some(p) => p,
        None => std::env::current_dir()?,
    };
    Ok(std::fs::canonicalize(&p).unwrap_or(p))
}

pub async fn build_agent(model_flag: Option<&str>, cwd: &std::path::Path) -> Result<Agent> {
    let config = Config::load(cwd)?;
    let model = resolve_model(model_flag, &config);
    let (provider, model_id) = oc_core::provider_from_model(&model)?;
    let mut tools = ToolRegistry::builtin();
    if let Err(e) = oc_ext::load_extensions(&config, &mut tools).await {
        tracing::warn!("failed to load extensions: {e:#}");
        eprintln!("warning: failed to load extensions: {e:#}");
    }
    Ok(Agent { provider, model: model_id, tools, system_prompt: SYSTEM_PROMPT.to_string() })
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
}
