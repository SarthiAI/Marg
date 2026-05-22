use arc_swap::ArcSwap;
use moka::future::Cache;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use marg_core::{BudgetSpec, Config, MargKey, PricingTable, RoutingEngine, SecurityConfig};
use marg_providers::ChatCompletionsClient;
use marg_storage::{HotStore, Storage};

pub type ProviderRegistry = HashMap<String, Arc<dyn ChatCompletionsClient>>;

#[derive(Clone)]
pub struct AppState {
    pub storage: Arc<dyn Storage>,
    pub hot: Arc<dyn HotStore>,
    pub providers: Arc<ProviderRegistry>,
    pub routing: Arc<RoutingEngine>,
    pub pricing: Arc<ArcSwap<PricingTable>>,
    pub security: SecurityConfig,
    pub key_cache: Cache<String, CachedKey>,
}

#[derive(Clone, Debug)]
pub struct CachedKey {
    pub key: MargKey,
    pub budget: BudgetSpec,
}

impl AppState {
    pub fn new(
        storage: Arc<dyn Storage>,
        hot: Arc<dyn HotStore>,
        providers: ProviderRegistry,
        routing: RoutingEngine,
        pricing: PricingTable,
        security: SecurityConfig,
    ) -> Self {
        let key_cache = Cache::builder()
            .max_capacity(50_000)
            .time_to_live(Duration::from_secs(60))
            .build();
        Self {
            storage,
            hot,
            providers: Arc::new(providers),
            routing: Arc::new(routing),
            pricing: Arc::new(ArcSwap::from_pointee(pricing)),
            security,
            key_cache,
        }
    }
}

pub fn build_pricing(cfg: &Config) -> PricingTable {
    let mut table = PricingTable::defaults_all();
    for entry in &cfg.pricing {
        table.insert(&entry.model, entry.price());
    }
    table
}
