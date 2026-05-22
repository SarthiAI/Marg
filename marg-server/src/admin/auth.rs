use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::Response;

use marg_core::MargToken;

use crate::admin::error::AdminError;
use crate::state::{AppState, CachedAdmin};

/// Axum middleware that authenticates every admin request against the
/// `admin_tokens` table. A short-lived (5s) moka cache spares the database on
/// scrape-heavy tooling; revokes propagate within the cache window.
pub async fn require_admin_token(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, AdminError> {
    let header = req
        .headers()
        .get(AUTHORIZATION)
        .ok_or(AdminError::MissingAuth)?;
    let header_str = header.to_str().map_err(|_| AdminError::InvalidAuth)?;
    let token_str = header_str
        .strip_prefix("Bearer ")
        .ok_or(AdminError::InvalidAuth)?
        .trim();
    if token_str.is_empty() {
        return Err(AdminError::InvalidAuth);
    }

    let token = MargToken::from_str(token_str);
    let hash = token.hash();

    if let Some(_cached) = state.admin_cache.get(&hash).await {
        return Ok(next.run(req).await);
    }

    let admin = state
        .storage
        .get_admin_token_by_hash(&hash)
        .await
        .map_err(AdminError::from)?
        .ok_or(AdminError::Unauthorized)?;

    state
        .admin_cache
        .insert(hash, CachedAdmin { token: admin })
        .await;

    Ok(next.run(req).await)
}
