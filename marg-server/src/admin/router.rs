use axum::middleware;
use axum::routing::{delete, get, post};
use axum::Router;

use crate::admin::auth::require_admin_token;
use crate::admin::console;
use crate::admin::handlers;
use crate::state::AppState;

/// Builds the admin Router. Auth middleware is applied to every route except
/// `/admin/openapi.json` so tooling can discover the surface before it has a
/// token to negotiate with.
pub fn build_router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/admin/keys", post(handlers::keys::create).get(handlers::keys::list))
        .route("/admin/keys/:id", get(handlers::keys::get).delete(handlers::keys::revoke))
        .route("/admin/keys/:id/invalidate", post(handlers::keys::invalidate))
        .route("/admin/budgets", post(handlers::budgets::upsert))
        .route("/admin/budgets/:key_id", get(handlers::budgets::get))
        .route("/admin/routes", get(handlers::routes::list).post(handlers::routes::create))
        .route("/admin/policy", get(handlers::policy::view))
        .route("/admin/policy/reload", post(handlers::policy::reload))
        .route("/admin/providers/health", get(handlers::providers::health))
        .route("/admin/requests", get(handlers::requests::list))
        .route(
            "/admin/auth/tokens",
            post(handlers::auth_tokens::create).get(handlers::auth_tokens::list),
        )
        .route("/admin/auth/tokens/:id", delete(handlers::auth_tokens::revoke))
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(state.clone(), require_admin_token));

    let public = Router::new()
        .route("/admin/openapi.json", get(handlers::openapi::spec))
        .route("/metrics", get(crate::observability::metrics_handler))
        .route("/", get(console::root_redirect))
        .route("/console", get(console::console_redirect))
        .route("/console/", get(console::index))
        .route("/console/*rest", get(console::asset))
        .with_state(state);

    public.merge(protected)
}
