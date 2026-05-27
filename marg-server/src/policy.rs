//! Live policy reload: rebuilds the [`RoutingEngine`] and the pricing table
//! from disk + storage, re-reads the Kavach policy file, and atomically swaps
//! all three into [`AppState`] under a single transaction.

use std::sync::Arc;

use marg_core::{load_kavach_policy, Config, PersistedRoute, PricingTable, RouteSpec, RoutingEngine};

use crate::kavach;
use crate::state::{build_pricing, AppState};

pub struct ReloadOutcome {
    pub config_routes: usize,
    pub stored_routes: usize,
    pub pricing_entries: usize,
    pub kavach_rules: usize,
    pub kavach_invariants: usize,
    pub kavach_previous_hash: String,
    pub kavach_new_hash: String,
    pub kavach_source_path: Option<String>,
    pub kavach_mode: String,
}

/// Re-read the config file from disk, the Kavach policy file or directory,
/// and the persisted routes; rebuild routing, pricing, the Kavach policy
/// engine, and the invariant set; then atomically swap them in. No request is
/// ever evaluated against a half-loaded policy. On any failure the previous
/// state keeps serving and the error propagates to the caller.
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

    // Re-read the Kavach policy source before swapping any state. If parsing
    // fails the routing/pricing swap is also skipped, the operator sees the
    // failure, and traffic continues against the previous good policy.
    let loaded_kavach = load_kavach_policy(
        &cfg.kavach,
        &cfg.inline_policies,
        &cfg.inline_invariants,
    )
    .map_err(|e| ReloadError::Config(format!("kavach policy load: {}", e)))?;
    let kavach_outcome = kavach::reload_policy(&state.kavach, &cfg.kavach, &loaded_kavach)
        .map_err(|e| ReloadError::Config(format!("kavach reload: {}", e)))?;

    state.routing.store(Arc::new(engine));
    state.pricing.store(Arc::new(pricing));

    let mode = state.kavach.mode.load().as_str().to_string();

    Ok(ReloadOutcome {
        config_routes: cfg.routes.len(),
        stored_routes: stored.len(),
        pricing_entries: cfg.pricing.len(),
        kavach_rules: kavach_outcome.policy_rule_count,
        kavach_invariants: kavach_outcome.invariant_count,
        kavach_previous_hash: kavach_outcome.previous_hash,
        kavach_new_hash: kavach_outcome.new_hash,
        kavach_source_path: kavach_outcome
            .source_path
            .map(|p| p.display().to_string()),
        kavach_mode: mode,
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
