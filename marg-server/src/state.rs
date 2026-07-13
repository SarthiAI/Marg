use arc_swap::ArcSwap;
use moka::future::Cache;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use marg_core::{
    AdminToken, BudgetSpec, Config, MargKey, PricingTable, RateLimitsConfig, RoutingEngine,
    SecurityConfig,
};
use marg_providers::ChatCompletionsClient;
use marg_storage::{HotStore, Storage};

use crate::hooks::{RequestContentHook, ResponseContentHook};
use crate::kavach::KavachRuntime;
use crate::metrics::Metrics;
use crate::write_batcher::WriteBatcher;

pub type ProviderRegistry = HashMap<String, Arc<dyn ChatCompletionsClient>>;

#[derive(Clone)]
pub struct AppState {
    pub storage: Arc<dyn Storage>,
    pub hot: Arc<dyn HotStore>,
    pub providers: Arc<ProviderRegistry>,
    pub routing: Arc<ArcSwap<RoutingEngine>>,
    pub pricing: Arc<ArcSwap<PricingTable>>,
    pub security: SecurityConfig,
    pub rate_limits: RateLimitsConfig,
    pub key_cache: Cache<String, CachedKey>,
    pub admin_cache: Cache<String, CachedAdmin>,
    pub metrics: Arc<Metrics>,
    pub write_batcher: Arc<WriteBatcher>,
    pub config_path: Arc<String>,
    pub kavach: Arc<KavachRuntime>,
    /// Optional host-registered content hooks (embeddable gateway API,
    /// ADR-031). `None` on the standalone `run()` path and any library use
    /// that does not register them, in which case the chat pipeline behaves
    /// exactly as it did before this API existed.
    pub pre_hook: Option<Arc<dyn RequestContentHook>>,
    pub post_hook: Option<Arc<dyn ResponseContentHook>>,
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
        rate_limits: RateLimitsConfig,
        metrics: Arc<Metrics>,
        write_batcher: Arc<WriteBatcher>,
        config_path: String,
        kavach: Arc<KavachRuntime>,
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
            rate_limits,
            key_cache,
            admin_cache,
            metrics,
            write_batcher,
            config_path: Arc::new(config_path),
            kavach,
            pre_hook: None,
            post_hook: None,
        }
    }

    /// Attach host-registered content hooks. Used by `GatewayBuilder`; the
    /// standalone `run()` path never calls this, so its hooks stay `None`.
    pub(crate) fn with_content_hooks(
        mut self,
        pre_hook: Option<Arc<dyn RequestContentHook>>,
        post_hook: Option<Arc<dyn ResponseContentHook>>,
    ) -> Self {
        self.pre_hook = pre_hook;
        self.post_hook = post_hook;
        self
    }
}

pub fn build_pricing(cfg: &Config) -> PricingTable {
    let mut table = PricingTable::defaults_all();
    for entry in &cfg.pricing {
        table.insert(&entry.model, entry.price());
    }
    table
}
