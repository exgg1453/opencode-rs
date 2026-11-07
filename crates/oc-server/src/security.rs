//! Request authentication for the HTTP API.
//!
//! The server can run tools (including `bash`) on the host, so every request must prove
//! it comes from someone who was given the server's token. On top of that:
//! - `Origin` must be on the allowlist, so a web page in the user's browser cannot
//!   drive the API (CSRF), even with a guessed or leaked token;
//! - `Host` must be on the allowlist when the server listens on loopback, which
//!   defeats DNS rebinding;
//! - session working directories must stay inside the server's root.
//!
//! Only shared-session pages are public; their writes are protected by the per-share secret.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, Method};
use axum::middleware::Next;
use axum::response::Response;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Clone)]
pub struct Security {
    /// Bearer token required on protected routes; `None` disables the check (tests only).
    token: Option<String>,
    /// Accepted `Host` names (without port); `None` accepts any.
    allowed_hosts: Option<Vec<String>>,
    /// Accepted `Origin` values, e.g. `https://app.example.com`.
    allowed_origins: Vec<String>,
    /// Session working directories must be inside this directory.
    root: Option<PathBuf>,
}

impl Security {
    /// Require `token`; accept only loopback host names when `loopback` is set.
    pub fn new(token: impl Into<String>, loopback: bool) -> Security {
        let allowed_hosts = loopback.then(|| vec!["localhost".into(), "127.0.0.1".into(), "::1".into()]);
        Security { token: Some(token.into()), allowed_hosts, allowed_origins: Vec::new(), root: None }
    }

    /// No authentication at all. Only for tests and embedding behind another auth layer.
    pub fn disabled() -> Security {
        Security { token: None, allowed_hosts: None, allowed_origins: Vec::new(), root: None }
    }

    pub fn with_root(mut self, root: PathBuf) -> Security {
        self.root = Some(std::fs::canonicalize(&root).unwrap_or(root));
        self
    }

    pub fn allow_host(mut self, host: &str) -> Security {
        if let Some(h) = self.allowed_hosts.as_mut() {
            h.push(host.trim_matches(['[', ']']).to_ascii_lowercase());
        }
        self
    }

    pub fn allow_origin(mut self, origin: &str) -> Security {
        self.allowed_origins.push(origin.trim_end_matches('/').to_ascii_lowercase());
        self
    }

    pub fn allowed_origins(&self) -> &[String] {
        &self.allowed_origins
    }

    /// Random token with ~244 bits of OS randomness.
    pub fn generate_token() -> String {
        format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
    }

    /// Resolve a requested session directory; it must exist and lie inside the root.
    pub fn session_dir(&self, requested: Option<&Path>) -> Result<PathBuf, ApiError> {
        let root = match &self.root {
            Some(r) => r.clone(),
            None => std::env::current_dir().map_err(|e| ApiError::Internal(e.to_string()))?,
        };
        let Some(req) = requested else { return Ok(root) };
        let joined = if req.is_absolute() { req.to_path_buf() } else { root.join(req) };
        // canonicalize resolves `..` and symlinks, so neither can step outside the root.
        let real = std::fs::canonicalize(&joined)
            .map_err(|_| ApiError::BadRequest(format!("cwd {} does not exist", joined.display())))?;
        if self.root.is_some() && !real.starts_with(&root) {
            return Err(ApiError::Forbidden(format!("cwd must be inside {}", root.display())));
        }
        Ok(real)
    }

    fn host_ok(&self, req: &Request) -> bool {
        let Some(allowed) = &self.allowed_hosts else { return true };
        let Some(host) = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()) else { return false };
        let name = strip_port(host).to_ascii_lowercase();
        allowed.contains(&name)
    }

    fn origin_ok(&self, req: &Request) -> bool {
        match req.headers().get(header::ORIGIN).and_then(|o| o.to_str().ok()) {
            None => true,
            Some(o) => self.allowed_origins.iter().any(|a| a.eq_ignore_ascii_case(o.trim_end_matches('/'))),
        }
    }

    fn token_ok(&self, req: &Request) -> bool {
        let Some(want) = &self.token else { return true };
        let bearer = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim);
        // EventSource cannot set headers, so `GET /event?token=...` is accepted as well.
        let query = (req.uri().path() == "/event")
            .then(|| req.uri().query())
            .flatten()
            .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("token=")));
        bearer.or(query).is_some_and(|got| constant_time_eq(got.as_bytes(), want.as_bytes()))
    }
}

/// `host:port`, `[v6]:port` or a bare host → host.
fn strip_port(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    match host.rsplit_once(':') {
        Some((h, port)) if !h.contains(':') && port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => host,
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Routes anyone may call: health and reading/updating a shared session (writes need
/// the per-share secret). Creating a share needs the server token.
fn is_public(method: &Method, path: &str) -> bool {
    if path == "/health" {
        return true;
    }
    match path.strip_prefix("/share/") {
        Some(rest) if !rest.is_empty() => {
            let ok_path = rest.split('/').count() == 1 || rest.ends_with("/data");
            ok_path && matches!(*method, Method::GET | Method::HEAD | Method::PUT | Method::DELETE)
        }
        _ => false,
    }
}

pub async fn enforce(State(state): State<AppState>, req: Request, next: Next) -> Result<Response, ApiError> {
    let sec: &Arc<Security> = &state.security;
    if !sec.host_ok(&req) {
        return Err(ApiError::Forbidden("host not allowed".into()));
    }
    let public = is_public(req.method(), req.uri().path());
    // Public share pages are plain links and may be opened from anywhere.
    if !public && !sec.origin_ok(&req) {
        return Err(ApiError::Forbidden("origin not allowed".into()));
    }
    if !public && req.method() != Method::OPTIONS && !sec.token_ok(&req) {
        return Err(ApiError::Unauthorized("missing or invalid bearer token".into()));
    }
    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ports() {
        assert_eq!(strip_port("localhost:4096"), "localhost");
        assert_eq!(strip_port("127.0.0.1"), "127.0.0.1");
        assert_eq!(strip_port("[::1]:4096"), "::1");
        assert_eq!(strip_port("evil.test:80"), "evil.test");
    }

    #[test]
    fn public_routes() {
        assert!(is_public(&Method::GET, "/health"));
        assert!(is_public(&Method::GET, "/share/abc"));
        assert!(is_public(&Method::GET, "/share/abc/data"));
        assert!(is_public(&Method::DELETE, "/share/abc"));
        assert!(!is_public(&Method::POST, "/share"));
        assert!(!is_public(&Method::POST, "/session"));
        assert!(!is_public(&Method::GET, "/event"));
        assert!(!is_public(&Method::GET, "/share/abc/../../session"));
    }

    #[test]
    fn session_dir_stays_in_root() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("proj");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let sec = Security::new("t", true).with_root(root.clone());
        let real_root = std::fs::canonicalize(&root).unwrap();
        assert_eq!(sec.session_dir(None).unwrap(), real_root);
        assert_eq!(sec.session_dir(Some(Path::new("sub"))).unwrap(), real_root.join("sub"));
        assert!(matches!(sec.session_dir(Some(Path::new("/"))), Err(ApiError::Forbidden(_))));
        assert!(matches!(sec.session_dir(Some(Path::new("../"))), Err(ApiError::Forbidden(_))));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/", root.join("escape")).unwrap();
            assert!(matches!(sec.session_dir(Some(Path::new("escape"))), Err(ApiError::Forbidden(_))));
        }
    }
}
