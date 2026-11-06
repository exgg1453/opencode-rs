//! HTTP API server (port of `packages/server` + `packages/protocol`). Owner: phase-4 agent.
//!
//! CONTRACT: oc-cli calls `oc_server::serve(agent, store, addr)`.

use oc_core::{Agent, SessionStore};

pub async fn serve(_agent: Agent, _store: SessionStore, _addr: std::net::SocketAddr) -> anyhow::Result<()> {
    todo!("phase 4")
}
