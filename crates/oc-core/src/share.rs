//! Client side of session sharing (uploads to our own oc-server share endpoints).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};

use crate::{Config, Session};

/// Locally remembered share: lets `unshare` authenticate against the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareRecord {
    pub id: String,
    pub secret: String,
    pub url: String,
    /// Server base URL the share was created on.
    pub base: String,
}

/// JSON file `shares.json` mapping session id to its share record.
#[derive(Debug, Clone)]
pub struct ShareStore {
    path: PathBuf,
}

impl ShareStore {
    pub fn open_default() -> ShareStore {
        ShareStore::in_dir(&crate::auth::data_dir())
    }
    pub fn in_dir(dir: &Path) -> ShareStore {
        ShareStore { path: dir.join("shares.json") }
    }
    fn read(&self) -> BTreeMap<String, ShareRecord> {
        std::fs::read_to_string(&self.path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    fn write(&self, m: &BTreeMap<String, ShareRecord>) -> anyhow::Result<()> {
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(&self.path, serde_json::to_string_pretty(m)?)?;
        Ok(())
    }
    pub fn get(&self, session_id: &str) -> Option<ShareRecord> {
        self.read().get(session_id).cloned()
    }
    pub fn put(&self, session_id: &str, rec: ShareRecord) -> anyhow::Result<()> {
        let mut m = self.read();
        m.insert(session_id.to_string(), rec);
        self.write(&m)
    }
    pub fn remove(&self, session_id: &str) -> anyhow::Result<Option<ShareRecord>> {
        let mut m = self.read();
        let r = m.remove(session_id);
        if r.is_some() {
            self.write(&m)?;
        }
        Ok(r)
    }
}

/// Base URL of the share server: config, then `OPENCODE_SHARE_URL`, then localhost.
pub fn share_base(config: &Config) -> String {
    let raw = config
        .share_url
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("OPENCODE_SHARE_URL").ok().filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| "http://127.0.0.1:4096".to_string());
    raw.trim().trim_end_matches('/').to_string()
}

/// Share-server token: `OPENCODE_SHARE_TOKEN`, then `share_token` in the config.
pub fn share_token(config: &Config) -> Option<String> {
    std::env::var("OPENCODE_SHARE_TOKEN")
        .ok()
        .or_else(|| config.share_token.clone())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

#[derive(Debug, Deserialize)]
struct ShareResponse {
    id: String,
    url: String,
    secret: String,
}

async fn error_text(resp: reqwest::Response) -> String {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    format!("{status}: {}", body.chars().take(300).collect::<String>())
}

/// Uploads `session` to the share server and returns its public URL.
/// Re-sharing an already shared session updates the existing share.
pub async fn share_session(config: &Config, session: &Session) -> anyhow::Result<String> {
    share_with_store(config, session, &ShareStore::open_default()).await
}

pub async fn share_with_store(config: &Config, session: &Session, store: &ShareStore) -> anyhow::Result<String> {
    let base = share_base(config);
    let client = reqwest::Client::new();
    if let Some(rec) = store.get(&session.id).filter(|r| r.base == base) {
        let resp = client
            .put(format!("{base}/share/{}", rec.id))
            .header("x-share-secret", &rec.secret)
            .json(session)
            .send()
            .await
            .with_context(|| format!("contacting share server at {base}"))?;
        if resp.status().is_success() {
            return Ok(rec.url);
        }
        // Share vanished on the server: fall through and create a new one.
    }
    let mut post = client.post(format!("{base}/share")).json(session);
    if let Some(t) = share_token(config) {
        post = post.bearer_auth(t);
    }
    let resp = post
        .send()
        .await
        .with_context(|| format!("contacting share server at {base}"))?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        bail!("share server at {base} needs its API token: set OPENCODE_SHARE_TOKEN (printed by `opencode serve`)");
    }
    if !resp.status().is_success() {
        bail!("share server rejected the session ({})", error_text(resp).await);
    }
    let r: ShareResponse = resp.json().await.context("invalid share server response")?;
    store.put(&session.id, ShareRecord { id: r.id, secret: r.secret, url: r.url.clone(), base })?;
    Ok(r.url)
}

/// Removes a previously shared session from the share server.
pub async fn unshare_session(config: &Config, session_id: &str) -> anyhow::Result<()> {
    unshare_with_store(config, session_id, &ShareStore::open_default()).await
}

pub async fn unshare_with_store(config: &Config, session_id: &str, store: &ShareStore) -> anyhow::Result<()> {
    let Some(rec) = store.get(session_id) else {
        bail!("session {session_id} is not shared");
    };
    let base = if rec.base.is_empty() { share_base(config) } else { rec.base.clone() };
    let resp = reqwest::Client::new()
        .delete(format!("{base}/share/{}", rec.id))
        .header("x-share-secret", &rec.secret)
        .send()
        .await
        .with_context(|| format!("contacting share server at {base}"))?;
    // 404 means it is already gone; treat as success.
    if !resp.status().is_success() && resp.status().as_u16() != 404 {
        bail!("share server refused to unshare ({})", error_text(resp).await);
    }
    store.remove(session_id)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_precedence_and_trim() {
        let c = Config { share_url: Some("http://h:1/".into()), ..Default::default() };
        assert_eq!(share_base(&c), "http://h:1");
    }

    #[test]
    fn store_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let s = ShareStore::in_dir(d.path());
        assert!(s.get("a").is_none());
        let r = ShareRecord { id: "i".into(), secret: "s".into(), url: "u".into(), base: "b".into() };
        s.put("a", r.clone()).unwrap();
        assert_eq!(s.get("a"), Some(r.clone()));
        assert_eq!(s.remove("a").unwrap(), Some(r));
        assert!(s.get("a").is_none());
    }

    #[tokio::test]
    async fn unshare_unknown_errors() {
        let d = tempfile::tempdir().unwrap();
        let s = ShareStore::in_dir(d.path());
        assert!(unshare_with_store(&Config::default(), "nope", &s).await.is_err());
    }
}
