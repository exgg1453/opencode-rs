//! Workspace trust: which project directories may run programs defined in their own
//! `opencode.json` (`mcp`/`lsp`/`plugin` commands) or `.opencode/plugins/`.
//!
//! A freshly cloned, untrusted repository must not be able to run code just because the
//! user opened it, so these project-sourced, process-spawning settings are ignored until
//! the user runs `opencode trust` in that directory (like an editor's "restricted mode").
//! Global settings under `~/.config/opencode` are always trusted — they are the user's own.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn store_path() -> PathBuf {
    crate::auth::data_dir().join("trust.json")
}

/// Canonical form of a directory, so `.`/`..`/symlinks compare equal.
pub fn canonical(dir: &Path) -> PathBuf {
    std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
}

fn read() -> BTreeSet<String> {
    std::fs::read_to_string(store_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn write(set: &BTreeSet<String>) -> anyhow::Result<()> {
    let path = store_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(set)?)?;
    Ok(())
}

/// Whether `dir` has been trusted to run its own project-defined commands.
pub fn is_trusted(dir: &Path) -> bool {
    let key = canonical(dir);
    read().iter().any(|t| Path::new(t) == key)
}

/// Trust `dir`. Returns false if it was already trusted.
pub fn trust(dir: &Path) -> anyhow::Result<bool> {
    let mut set = read();
    let added = set.insert(canonical(dir).to_string_lossy().into_owned());
    if added {
        write(&set)?;
    }
    Ok(added)
}

/// Remove trust for `dir`. Returns whether it was trusted.
pub fn untrust(dir: &Path) -> anyhow::Result<bool> {
    let mut set = read();
    let removed = set.remove(&canonical(dir).to_string_lossy().into_owned());
    if removed {
        write(&set)?;
    }
    Ok(removed)
}

/// All trusted directories, sorted.
pub fn list() -> Vec<String> {
    read().into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_is_stable() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        let real = std::fs::canonicalize(d.path()).unwrap();
        // `.../sub/..` collapses to the real dir once the path exists.
        assert_eq!(canonical(&d.path().join("sub/..")), real);
    }
}
