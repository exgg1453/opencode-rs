//! On-disk store of shared session snapshots (`<dir>/<id>.json`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredShare {
    pub id: String,
    pub secret: String,
    pub session: Value,
}

#[derive(Debug, Clone)]
pub struct ShareStore {
    dir: PathBuf,
}

pub fn valid_share_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Constant-time-ish string comparison.
pub fn secrets_match(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl ShareStore {
    pub fn new(dir: PathBuf) -> ShareStore {
        ShareStore { dir }
    }

    /// Default location: `$XDG_DATA_HOME/opencode-rs/server-shares`.
    pub fn default_dir() -> ShareStore {
        ShareStore::new(oc_core::auth::data_dir().join("server-shares"))
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    pub fn create(&self, session: Value) -> anyhow::Result<StoredShare> {
        let s = StoredShare {
            id: uuid::Uuid::new_v4().simple().to_string(),
            secret: format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple()),
            session,
        };
        self.save(&s)?;
        Ok(s)
    }

    pub fn save(&self, s: &StoredShare) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.path(&s.id);
        let tmp = path.with_extension("tmp");
        write_private(&tmp, &serde_json::to_vec(s)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn load(&self, id: &str) -> anyhow::Result<Option<StoredShare>> {
        if !valid_share_id(id) {
            return Ok(None);
        }
        match std::fs::read(self.path(id)) {
            Ok(b) => Ok(Some(serde_json::from_slice(&b)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn delete(&self, id: &str) -> anyhow::Result<bool> {
        if !valid_share_id(id) {
            return Ok(false);
        }
        match std::fs::remove_file(self.path(id)) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, data: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.write_all(data)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, data: &[u8]) -> anyhow::Result<()> {
    std::fs::write(path, data)?;
    Ok(())
}
