use std::path::PathBuf;

use anyhow::Context;
use oc_ai::ChatMessage;
use serde::{Deserialize, Serialize};

use crate::config::home_dir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub cwd: PathBuf,
    pub messages: Vec<ChatMessage>,
}

impl Session {
    pub fn new(cwd: PathBuf) -> Session {
        Session { id: uuid::Uuid::new_v4().to_string(), title: "New session".into(), cwd, messages: Vec::new() }
    }
}

/// Persists sessions on disk (~/.local/share/opencode-rs/sessions/<id>.json).
pub struct SessionStore {
    dir: PathBuf,
}

impl SessionStore {
    pub fn open_default() -> anyhow::Result<SessionStore> {
        let base = match std::env::var_os("XDG_DATA_HOME").filter(|x| !x.is_empty()) {
            Some(x) => PathBuf::from(x),
            None => home_dir().context("HOME is not set")?.join(".local/share"),
        };
        Self::open(base.join("opencode-rs/sessions"))
    }

    /// Open a store rooted at an explicit directory (created if missing).
    pub fn open(dir: PathBuf) -> anyhow::Result<SessionStore> {
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(SessionStore { dir })
    }

    fn path(&self, id: &str) -> anyhow::Result<PathBuf> {
        anyhow::ensure!(
            !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "invalid session id: {id}"
        );
        Ok(self.dir.join(format!("{id}.json")))
    }

    pub fn save(&self, s: &Session) -> anyhow::Result<()> {
        let path = self.path(&s.id)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(s)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn load(&self, id: &str) -> anyhow::Result<Session> {
        let path = self.path(id)?;
        let data = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        Ok(serde_json::from_slice(&data)?)
    }

    /// All sessions, most recently modified first.
    pub fn list(&self) -> anyhow::Result<Vec<Session>> {
        let mut items = vec![];
        for e in std::fs::read_dir(&self.dir)? {
            let e = e?;
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Ok(data) = std::fs::read(&p) else { continue };
            if let Ok(s) = serde_json::from_slice::<Session>(&data) {
                let m = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
                items.push((m, s));
            }
        }
        items.sort_by_key(|a| std::cmp::Reverse(a.0));
        Ok(items.into_iter().map(|(_, s)| s).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oc_ai::{ContentBlock, Role};

    #[test]
    fn roundtrip_and_list() {
        let d = tempfile::tempdir().unwrap();
        let st = SessionStore::open(d.path().join("s")).unwrap();
        let mut s = Session::new("/tmp".into());
        s.messages.push(ChatMessage { role: Role::User, content: vec![ContentBlock::Text { text: "hi".into() }] });
        st.save(&s).unwrap();
        let l = st.load(&s.id).unwrap();
        assert_eq!(l.messages, s.messages);
        assert_eq!(st.list().unwrap().len(), 1);
        assert!(st.load("../x").is_err());
    }
}
