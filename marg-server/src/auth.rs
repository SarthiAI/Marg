use axum::http::HeaderMap;
use marg_core::{KeyStatus, MargToken};

use crate::errors::ChatError;
use crate::state::{AppState, CachedKey};

pub async fn authenticate(state: &AppState, headers: &HeaderMap) -> Result<CachedKey, ChatError> {
    let header = headers.get(axum::http::header::AUTHORIZATION)
        .ok_or(ChatError::MissingAuthHeader)?;
    let header_str = header.to_str().map_err(|_| ChatError::InvalidAuthHeader)?;
    let token_str = header_str
        .strip_prefix("Bearer ")
        .ok_or(ChatError::InvalidAuthHeader)?
        .trim();
    if token_str.is_empty() {
        return Err(ChatError::InvalidAuthHeader);
    }

    let token = MargToken::from_str(token_str);
    let hash = token.hash();

    if let Some(cached) = state.key_cache.get(&hash).await {
        return validate_key(cached);
    }

    let key = state.storage.get_key_by_hash(&hash).await
        .map_err(|e| ChatError::Storage(e.to_string()))?
        .ok_or(ChatError::Unauthorized)?;
    let budget = state.storage.get_budget(&key.id).await
        .map_err(|e| ChatError::Storage(e.to_string()))?
        .unwrap_or_else(|| marg_core::BudgetSpec::unlimited(key.id.clone()));

    let cached = CachedKey { key, budget };
    state.key_cache.insert(hash, cached.clone()).await;
    validate_key(cached)
}

fn validate_key(cached: CachedKey) -> Result<CachedKey, ChatError> {
    if cached.key.status != KeyStatus::Active {
        return Err(ChatError::Unauthorized);
    }
    Ok(cached)
}
