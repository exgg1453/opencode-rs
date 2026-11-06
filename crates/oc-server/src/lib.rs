//! HTTP API server (port of `packages/server` + `packages/protocol`).
//!
//! CONTRACT: oc-cli calls `oc_server::serve(agent, store, addr)`.
//!
//! Endpoints: `GET /health`, `GET|POST /session`, `GET|DELETE /session/{id}`,
//! `GET|POST /session/{id}/message`, `POST /session/{id}/abort`, `GET /event` (SSE).

pub mod backend;
pub mod error;
pub mod handlers;
pub mod routes;
pub mod share_html;
pub mod share_store;
pub mod state;

pub use routes::router;
pub use state::AppState;

use oc_core::{Agent, SessionStore};

pub async fn serve(mut agent: Agent, store: SessionStore, addr: std::net::SocketAddr) -> anyhow::Result<()> {
    // Tool calls whose rules say "ask" are published as `permission.asked` events and wait
    // for `POST /session/{id}/permission/{permission_id}`.
    let cwd = std::env::current_dir().unwrap_or_default();
    let config = oc_core::Config::load(&cwd).unwrap_or_default();
    let broker = handlers::permission::PermissionBroker::default();
    agent.hooks.permission =
        Some(std::sync::Arc::new(oc_core::permission::RulePermission::new(&config, std::sync::Arc::new(broker.clone())).with_cwd(cwd)));
    let app = router(AppState::from_agent(agent, store).with_permissions(broker));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("oc-server listening on {}", listener.local_addr()?);
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod share_tests;
