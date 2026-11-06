//! Router assembly.

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::handlers::{events, health, sessions, share};
use crate::state::AppState;

const SHARE_BODY_LIMIT: usize = 64 * 1024 * 1024;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health::health))
        .route("/session", get(sessions::list).post(sessions::create))
        .route("/session/{id}", get(sessions::get).delete(sessions::delete))
        .route("/session/{id}/message", get(sessions::messages).post(sessions::prompt))
        .route("/session/{id}/abort", post(sessions::abort))
        .route("/event", get(events::stream))
        .route("/session/{id}/permission/{permission_id}", post(crate::handlers::permission::respond))
        .route("/share", post(share::create).layer(DefaultBodyLimit::max(SHARE_BODY_LIMIT)))
        .route(
            "/share/{id}",
            get(share::page).put(share::update).delete(share::remove).layer(DefaultBodyLimit::max(SHARE_BODY_LIMIT)),
        )
        .route("/share/{id}/data", get(share::data))
        .fallback(health::not_found)
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}
