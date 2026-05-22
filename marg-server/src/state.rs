use arc_swap::ArcSwap;
use moka::future::Cache;
use std::sync::Arc;
use std::time::Duration;

use marg_core::{BudgetSpec, Config, MargKey, PricingTable, SecurityConfig};
use marg_providers::ChatCompletionsClient;
use marg_storage::Storage;

#[derive(Clone)]
pub struct AppState {
    pub storage: Arc<dyn Storage>,
    pub provider: Arc<dyn ChatCompletionsClient>,
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
        provider: Arc<dyn ChatCompletionsClient>,
        pricing: PricingTable,
        security: SecurityConfig,
    ) -> Self {
        let key_cache = Cache::builder()
            .max_capacity(50_000)
            .time_to_live(Duration::from_secs(60))
            .build();
        Self {
            storage,
            provider,
            pricing: Arc::new(ArcSwap::from_pointee(pricing)),
            security,
            key_cache,
        }
    }
}

pub fn build_pricing(cfg: &Config) -> PricingTable {
    let mut table = PricingTable::defaults_openai();
    for entry in &cfg.pricing {
        table.insert(&entry.model, entry.price());
    }
    table
}
