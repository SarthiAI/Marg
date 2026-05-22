use arc_swap::ArcSwap;
use moka::future::Cache;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use marg_core::{
    AdminToken, BudgetSpec, Config, MargKey, PricingTable, RoutingEngine, SecurityConfig,
};
use marg_providers::ChatCompletionsClient;
use marg_storage::{HotStore, Storage};

use crate::metrics::Metrics;

pub type ProviderRegistry = HashMap<String, Arc<dyn ChatCompletionsClient>>;

#[derive(Clone)]
pub struct AppState {
    pub storage: Arc<dyn Storage>,
    pub hot: Arc<dyn HotStore>,
    pub providers: Arc<ProviderRegistry>,
    pub routing: Arc<ArcSwap<RoutingEngine>>,
    pub pricing: Arc<ArcSwap<PricingTable>>,
    pub security: SecurityConfig,
    pub key_cache: Cache<String, CachedKey>,
    pub admin_cache: Cache<String, CachedAdmin>,
    pub metrics: Arc<Metrics>,
    pub config_path: Arc<String>,
}

#[derive(Clone, Debug)]
pub struct CachedKey {
    pub key: MargKey,
    pub budget: BudgetSpec,
}

#[derive(Clone, Debug)]
pub struct CachedAdmin {
    pub token: AdminToken,
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        storage: Arc<dyn Storage>,
        hot: Arc<dyn HotStore>,
        providers: ProviderRegistry,
        routing: RoutingEngine,
        pricing: PricingTable,
        security: SecurityConfig,
        metrics: Arc<Metrics>,
        config_path: String,
    ) -> Self {
        let key_cache = Cache::builder()
            .max_capacity(50_000)
            .time_to_live(Duration::from_secs(60))
            .build();
        let admin_cache = Cache::builder()
            .max_capacity(1_000)
            .time_to_live(Duration::from_secs(5))
            .build();
        Self {
            storage,
            hot,
            providers: Arc::new(providers),
            routing: Arc::new(ArcSwap::from_pointee(routing)),
            pricing: Arc::new(ArcSwap::from_pointee(pricing)),
            security,
            key_cache,
            admin_cache,
            metrics,
            config_path: Arc::new(config_path),
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
