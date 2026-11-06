//! Language Server Protocol client: after the agent writes or edits a file,
//! ask the matching language server for diagnostics.

mod client;
pub mod framing;
mod servers;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use oc_core::{Config, DiagnosticsProvider};
use tokio::sync::Mutex;

pub use client::Diag;
use client::Client;
use servers::ServerDef;

const MAX_DIAGNOSTICS: usize = 20;

/// Returns None when LSP is disabled (`"lsp": false`) or no server is available.
/// Servers start lazily on first use.
pub async fn start(config: &Config, cwd: &Path) -> Option<Arc<dyn DiagnosticsProvider>> {
    let defs = servers::table(config.lsp.as_ref())?;
    let timeout = std::env::var("OC_LSP_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_secs(3));
    Some(Arc::new(Lsp::new(defs, cwd.to_path_buf(), timeout)))
}

/// Format errors for the model:
/// `<file_diagnostics>\nERROR [line:col] message\n</file_diagnostics>` (1-based).
pub fn format_diagnostics(diags: &[Diag]) -> Option<String> {
    let errors: Vec<&Diag> = diags.iter().filter(|d| d.severity == 1).collect();
    if errors.is_empty() {
        return None;
    }
    let mut out = String::from("<file_diagnostics>\n");
    for d in errors.iter().take(MAX_DIAGNOSTICS) {
        out.push_str(&format!("ERROR [{}:{}] {}\n", d.line + 1, d.col + 1, d.message));
    }
    if errors.len() > MAX_DIAGNOSTICS {
        out.push_str(&format!("... and {} more\n", errors.len() - MAX_DIAGNOSTICS));
    }
    out.push_str("</file_diagnostics>");
    Some(out)
}

enum Slot {
    Ready(Arc<Client>),
    Broken,
}

struct Lsp {
    defs: Vec<ServerDef>,
    cwd: PathBuf,
    timeout: Duration,
    /// (server name, root) -> client
    clients: Mutex<HashMap<(String, PathBuf), Slot>>,
}

impl Lsp {
    fn new(defs: Vec<ServerDef>, cwd: PathBuf, timeout: Duration) -> Self {
        Lsp { defs, cwd, timeout, clients: Mutex::new(HashMap::new()) }
    }

    async fn client_for(&self, def: &ServerDef, file: &Path) -> Option<Arc<Client>> {
        let root = servers::find_root(file, &def.root_markers, &self.cwd);
        let key = (def.name.clone(), root.clone());
        let mut map = self.clients.lock().await;
        match map.get(&key) {
            Some(Slot::Ready(c)) if c.is_alive() => return Some(c.clone()),
            Some(Slot::Ready(_)) => {
                tracing::warn!("language server {} exited; disabling", def.name);
                map.insert(key, Slot::Broken);
                return None;
            }
            Some(Slot::Broken) => return None,
            None => {}
        }
        match Client::start(&def.command, &root).await {
            Ok(c) => {
                map.insert(key, Slot::Ready(c.clone()));
                Some(c)
            }
            Err(e) => {
                tracing::warn!("language server {} failed to start: {e}", def.name);
                map.insert(key, Slot::Broken);
                None
            }
        }
    }
}

#[async_trait]
impl DiagnosticsProvider for Lsp {
    async fn diagnostics(&self, path: &Path) -> Option<String> {
        let path = if path.is_absolute() { path.to_path_buf() } else { self.cwd.join(path) };
        let ext = path.extension()?.to_string_lossy().to_lowercase();
        let text = tokio::fs::read_to_string(&path).await.ok()?;
        // First matching definition whose binary exists (so pylsp can back up pyright).
        for def in self.defs.iter().filter(|d| d.extensions.contains(&ext)) {
            if def.command.first().and_then(|c| servers::find_binary(c)).is_none() {
                continue;
            }
            let Some(client) = self.client_for(def, &path).await else { continue };
            return match client.sync_and_wait(&path, servers::language_id(&ext), &text, self.timeout).await {
                Ok(d) => format_diagnostics(&d),
                Err(e) => {
                    tracing::warn!("language server {} failed: {e}", def.name);
                    None
                }
            };
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn d(line: u32, sev: u8, m: &str) -> Diag {
        Diag { line, col: 2, severity: sev, message: m.into() }
    }

    #[test]
    fn formatting() {
        assert_eq!(format_diagnostics(&[d(0, 2, "w")]), None);
        assert_eq!(
            format_diagnostics(&[d(4, 1, "oops"), d(0, 2, "w")]).unwrap(),
            "<file_diagnostics>\nERROR [5:3] oops\n</file_diagnostics>"
        );
        let many: Vec<Diag> = (0..25).map(|i| d(i, 1, "x")).collect();
        assert!(format_diagnostics(&many).unwrap().contains("... and 5 more"));
    }

    #[tokio::test]
    async fn disabled() {
        let cfg = Config { lsp: Some(json!(false)), ..Default::default() };
        assert!(start(&cfg, Path::new("/tmp")).await.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn real_rust_analyzer() {
        if servers::find_binary("rust-analyzer").is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"t\"\nversion=\"0.1.0\"\nedition=\"2021\"\n").unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        let f = dir.path().join("src/lib.rs");
        std::fs::write(&f, "pub fn f() -> i32 { \"s\" }\n").unwrap();
        std::env::set_var("OC_LSP_TIMEOUT_MS", "30000");
        let p = start(&Config::default(), dir.path()).await.unwrap();
        let out = p.diagnostics(&f).await;
        assert!(out.unwrap().contains("ERROR"));
    }
}
