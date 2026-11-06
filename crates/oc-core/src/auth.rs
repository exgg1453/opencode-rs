//! Stored provider credentials (`opencode auth login`).
//!
//! Stored in `$XDG_DATA_HOME/opencode-rs/auth.json` (or `~/.local/share/opencode-rs/auth.json`)
//! with mode 0600, as `{ "<provider>": {"type":"api","key":"..."} }`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// A stored credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Credential {
    Api { key: String },
}

/// Data directory for opencode-rs state (`$XDG_DATA_HOME/opencode-rs`).
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|x| !x.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join(".local/share"))
        })
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("opencode-rs")
}

/// Credential file store; the directory is injectable for tests.
#[derive(Debug, Clone)]
pub struct AuthStore {
    path: PathBuf,
}

impl AuthStore {
    pub fn open_default() -> AuthStore {
        AuthStore::in_dir(&data_dir())
    }

    pub fn in_dir(dir: &Path) -> AuthStore {
        AuthStore { path: dir.join("auth.json") }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read(&self) -> BTreeMap<String, Credential> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn write(&self, map: &BTreeMap<String, Credential>) -> anyhow::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let tmp = self.path.with_extension("json.tmp");
        write_private(&tmp, serde_json::to_string_pretty(map)?.as_bytes())?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn get_api_key(&self, provider: &str) -> Option<String> {
        match self.read().get(provider) {
            Some(Credential::Api { key }) if !key.is_empty() => Some(key.clone()),
            _ => None,
        }
    }

    pub fn set_api_key(&self, provider: &str, key: &str) -> anyhow::Result<()> {
        anyhow::ensure!(!provider.trim().is_empty(), "provider must not be empty");
        anyhow::ensure!(!key.trim().is_empty(), "key must not be empty");
        let mut map = self.read();
        map.insert(provider.to_string(), Credential::Api { key: key.trim().to_string() });
        self.write(&map)
    }

    /// Returns whether a credential was removed.
    pub fn remove(&self, provider: &str) -> anyhow::Result<bool> {
        let mut map = self.read();
        let removed = map.remove(provider).is_some();
        if removed {
            self.write(&map)?;
        }
        Ok(removed)
    }

    /// Provider ids that have stored credentials, sorted.
    pub fn list(&self) -> Vec<String> {
        self.read().into_keys().collect()
    }
}

#[cfg(unix)]
fn write_private(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.write_all(data)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    std::fs::write(path, data)?;
    Ok(())
}

/// API key saved for `provider` (e.g. "anthropic"), if any.
pub fn get_api_key(provider: &str) -> Option<String> {
    AuthStore::open_default().get_api_key(provider)
}

pub fn set_api_key(provider: &str, key: &str) -> anyhow::Result<()> {
    AuthStore::open_default().set_api_key(provider, key)
}

pub fn remove_api_key(provider: &str) -> anyhow::Result<bool> {
    AuthStore::open_default().remove(provider)
}

pub fn list_providers() -> Vec<String> {
    AuthStore::open_default().list()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_list_remove() {
        let d = tempfile::tempdir().unwrap();
        let s = AuthStore::in_dir(&d.path().join("nested"));
        assert_eq!(s.get_api_key("anthropic"), None);
        s.set_api_key("openai", "sk-1").unwrap();
        s.set_api_key("anthropic", " sk-2 ").unwrap();
        assert_eq!(s.get_api_key("anthropic").as_deref(), Some("sk-2"));
        assert_eq!(s.list(), vec!["anthropic", "openai"]);
        assert!(s.remove("openai").unwrap());
        assert!(!s.remove("openai").unwrap());
        assert_eq!(s.list(), vec!["anthropic"]);
    }

    #[test]
    fn file_format_and_mode() {
        let d = tempfile::tempdir().unwrap();
        let s = AuthStore::in_dir(d.path());
        s.set_api_key("x", "k").unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(s.path()).unwrap()).unwrap();
        assert_eq!(v, serde_json::json!({"x": {"type": "api", "key": "k"}}));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(s.path()).unwrap().permissions().mode();
            assert_eq!(m & 0o777, 0o600);
        }
    }

    #[test]
    fn rejects_empty_and_tolerates_garbage() {
        let d = tempfile::tempdir().unwrap();
        let s = AuthStore::in_dir(d.path());
        assert!(s.set_api_key("x", "  ").is_err());
        std::fs::write(s.path(), "not json").unwrap();
        assert_eq!(s.get_api_key("x"), None);
        s.set_api_key("x", "k").unwrap();
        assert_eq!(s.get_api_key("x").as_deref(), Some("k"));
    }
}
