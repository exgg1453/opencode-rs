//! Terminal UI (port of `packages/tui`). Owner: phase-3 agent.
//!
//! CONTRACT: oc-cli calls `oc_tui::run(agent, session, store)`.

use oc_core::{Agent, Session, SessionStore};

pub async fn run(_agent: Agent, _session: Session, _store: SessionStore) -> anyhow::Result<()> {
    todo!("phase 3")
}
