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
pub mod state;

pub use routes::router;
pub use state::AppState;

use oc_core::{Agent, SessionStore};

pub async fn serve(agent: Agent, store: SessionStore, addr: std::net::SocketAddr) -> anyhow::Result<()> {
    let app = router(AppState::from_agent(agent, store));
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
