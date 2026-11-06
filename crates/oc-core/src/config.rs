use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// "provider/model", e.g. "anthropic/claude-sonnet-5-5".
    pub model: Option<String>,
    #[serde(default)]
    pub mcp: serde_json::Map<String, serde_json::Value>,
    /// Per-provider settings: `{ "<id>": { "options": { "baseURL": ..., ... } } }`.
    #[serde(default)]
    pub provider: serde_json::Map<String, serde_json::Value>,
    /// Permission rules, e.g. `{ "edit": "ask", "bash": { "git *": "allow", "*": "ask" } }`.
    #[serde(default)]
    pub permission: serde_json::Map<String, serde_json::Value>,
    /// TUI theme name.
    pub theme: Option<String>,
    /// Process plugins: `{ "<name>": { "command": [...], "enabled": true } }`.
    #[serde(default)]
    pub plugin: serde_json::Map<String, serde_json::Value>,
    /// Language servers: `{ "<name>": { "command": [...], "extensions": [...] } }`; `false` disables LSP.
    pub lsp: Option<serde_json::Value>,
    /// Base URL of the share server.
    pub share_url: Option<String>,
}

pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

fn global_config_path() -> Option<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME").filter(|x| !x.is_empty()) {
        return Some(PathBuf::from(x).join("opencode/opencode.json"));
    }
    home_dir().map(|h| h.join(".config/opencode/opencode.json"))
}

fn read_opt(path: &Path) -> anyhow::Result<Option<Config>> {
    match std::fs::read_to_string(path) {
        Ok(s) if s.trim().is_empty() => Ok(None),
        Ok(s) => Ok(Some(serde_json::from_str(&s).with_context(|| format!("parsing {}", path.display()))?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

impl Config {
    /// Load from global (~/.config/opencode/opencode.json) and project (./opencode.json) files.
    /// Project values override global ones; missing files are fine.
    pub fn load(cwd: &Path) -> anyhow::Result<Config> {
        let mut cfg = Config::default();
        if let Some(p) = global_config_path() {
            if let Some(g) = read_opt(&p)? {
                cfg.merge(g);
            }
        }
        if let Some(p) = read_opt(&cwd.join("opencode.json"))? {
            cfg.merge(p);
        }
        Ok(cfg)
    }

    fn merge(&mut self, other: Config) {
        if other.model.is_some() {
            self.model = other.model;
        }
        for (k, v) in other.mcp {
            self.mcp.insert(k, v);
        }
        for (k, v) in other.provider {
            self.provider.insert(k, v);
        }
        for (k, v) in other.permission {
            self.permission.insert(k, v);
        }
        for (k, v) in other.plugin {
            self.plugin.insert(k, v);
        }
        if other.theme.is_some() {
            self.theme = other.theme;
        }
        if other.lsp.is_some() {
            self.lsp = other.lsp;
        }
        if other.share_url.is_some() {
            self.share_url = other.share_url;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_overrides_and_missing_ok() {
        let d = tempfile::tempdir().unwrap();
        assert!(Config::load(d.path()).is_ok());
        std::fs::write(
            d.path().join("opencode.json"),
            r#"{"model":"openai/x","mcp":{"a":{"command":"y"}},"$schema":"zzz"}"#,
        )
        .unwrap();
        let c = Config::load(d.path()).unwrap();
        assert_eq!(c.model.as_deref(), Some("openai/x"));
        assert!(c.mcp.contains_key("a"));
    }

    #[test]
    fn merge_semantics() {
        let mut a = Config { model: Some("a/1".into()), ..Default::default() };
        a.mcp.insert("k".into(), serde_json::json!(1));
        let mut b = Config::default();
        b.mcp.insert("j".into(), serde_json::json!(2));
        a.merge(b);
        assert_eq!(a.model.as_deref(), Some("a/1"));
        assert_eq!(a.mcp.len(), 2);
    }
}
