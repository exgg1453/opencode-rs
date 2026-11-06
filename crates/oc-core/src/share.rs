//! Client side of session sharing (uploads to our own oc-server share endpoints).
//! Owner: share/auth agent. Keep these signatures.

use crate::{Config, Session};

/// Uploads `session` to the share server and returns its public URL.
pub async fn share_session(_config: &Config, _session: &Session) -> anyhow::Result<String> {
    anyhow::bail!("sharing is not implemented yet")
}

/// Removes a previously shared session from the share server.
pub async fn unshare_session(_config: &Config, _session_id: &str) -> anyhow::Result<()> {
    anyhow::bail!("sharing is not implemented yet")
}
