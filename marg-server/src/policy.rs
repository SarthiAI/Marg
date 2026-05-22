//! Live policy reload: rebuilds the [`RoutingEngine`] and the pricing table
//! from disk + storage and atomically swaps them into [`AppState`].

use std::sync::Arc;

use marg_core::{Config, PersistedRoute, PricingTable, RouteSpec, RoutingEngine};

use crate::state::{build_pricing, AppState};

pub struct ReloadOutcome {
    pub config_routes: usize,
    pub stored_routes: usize,
    pub pricing_entries: usize,
}

/// Re-read the config file from disk, fetch any routes persisted via
/// `POST /admin/routes`, rebuild the routing engine and pricing table, and
/// atomically swap them into the live `AppState`. No request is ever
/// evaluated against a half-loaded policy.
pub async fn reload(state: &AppState) -> Result<ReloadOutcome, ReloadError> {
    let cfg = Config::load(state.config_path.as_str())
        .map_err(|e| ReloadError::Config(e.to_string()))?;

    let stored = state
        .storage
        .list_routes()
        .await
        .map_err(|e| ReloadError::Storage(e.to_string()))?;

    let registered: Vec<String> = state.providers.keys().cloned().collect();

    let combined = combine_routes(&cfg.routes, &stored);
    let default_provider = cfg
        .providers
        .default
        .clone()
        .or_else(|| registered.first().cloned());

    let engine = RoutingEngine::build(&combined, default_provider, &registered)
        .map_err(|e| ReloadError::Config(e.to_string()))?;

    let pricing = build_pricing(&cfg);

    state.routing.store(Arc::new(engine));
    state.pricing.store(Arc::new(pricing));

    Ok(ReloadOutcome {
        config_routes: cfg.routes.len(),
        stored_routes: stored.len(),
        pricing_entries: cfg.pricing.len(),
    })
}

pub fn combine_routes(config_routes: &[RouteSpec], stored: &[PersistedRoute]) -> Vec<RouteSpec> {
    let mut out = Vec::with_capacity(config_routes.len() + stored.len());
    out.extend_from_slice(config_routes);
    for s in stored {
        out.push(s.to_route_spec());
    }
    out
}

pub fn build_initial_routing(
    cfg: &Config,
    stored: &[PersistedRoute],
    registered: &[String],
) -> Result<RoutingEngine, ReloadError> {
    let combined = combine_routes(&cfg.routes, stored);
    let default_provider = cfg
        .providers
        .default
        .clone()
        .or_else(|| registered.first().cloned());
    RoutingEngine::build(&combined, default_provider, registered)
        .map_err(|e| ReloadError::Config(e.to_string()))
}

pub fn build_initial_pricing(cfg: &Config) -> PricingTable {
    build_pricing(cfg)
}

#[derive(Debug, thiserror::Error)]
pub enum ReloadError {
    #[error("config error: {0}")]
    Config(String),

    #[error("storage error: {0}")]
    Storage(String),
}
