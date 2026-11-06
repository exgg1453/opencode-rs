//! Router assembly.

use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::handlers::{events, health, sessions};
use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health::health))
        .route("/session", get(sessions::list).post(sessions::create))
        .route("/session/{id}", get(sessions::get).delete(sessions::delete))
        .route("/session/{id}/message", get(sessions::messages).post(sessions::prompt))
        .route("/session/{id}/abort", post(sessions::abort))
        .route("/event", get(events::stream))
        .fallback(health::not_found)
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}
