//! Router assembly.

use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderValue, Method};
use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;

use crate::handlers::{events, health, sessions, share};
use crate::state::AppState;

const SHARE_BODY_LIMIT: usize = 64 * 1024 * 1024;

/// CORS only for explicitly allowed origins; with none, browsers get no CORS headers
/// and cross-site pages cannot read responses.
fn cors(state: &AppState) -> CorsLayer {
    let origins: Vec<HeaderValue> =
        state.security.allowed_origins().iter().filter_map(|o| HeaderValue::from_str(o).ok()).collect();
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
}

pub fn router(state: AppState) -> Router {
    let cors = cors(&state);
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
        .layer(axum::middleware::from_fn_with_state(state.clone(), crate::security::enforce))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
