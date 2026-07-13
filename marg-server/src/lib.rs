//! # marg-server
//!
//! The HTTP server library behind Marg, the self-hosted AI gateway. It holds
//! the OpenAI-compatible proxy pipeline, the admin API, budget and rate-limit
//! enforcement, provider routing and failover, Prometheus metrics, and the
//! Kavach governance integration (default-deny gating plus a signed
//! post-quantum audit chain).
//!
//! ## Two ways to use it
//!
//! **As the standalone daemon.** [`run`] loads a config file, binds the
//! gateway and admin listeners, installs the reload signal handler, and serves
//! until shutdown. This is what the `marg` binary calls, and its behavior is
//! unchanged by anything below.
//!
//! **Embedded in your own binary.** [`GatewayBuilder`] assembles the same
//! gateway without binding any socket and returns a [`Gateway`] whose
//! [`Gateway::router`] you mount into your own axum app. You can inject a
//! shared audit chain so Marg's records land in your process's one chain, and
//! register content hooks ([`RequestContentHook`] / [`ResponseContentHook`])
//! that run inside the request pipeline and return a [`ContentDecision`]. The
//! embed API is purely additive: register nothing and the pipeline behaves
//! exactly as the standalone daemon does.
//!
//! ```ignore
//! use std::sync::Arc;
//! use marg_server::{GatewayBuilder, RequestContentHook, ResponseContentHook};
//! use kavach_pq::SignedAuditChain;
//!
//! let gateway = GatewayBuilder::from_config_path("marg.toml").await?
//!     .with_audit_chain(shared_chain)   // Marg appends to your one chain
//!     .with_pre_hook(pre_hook)          // runs before the gate and forwarding
//!     .with_post_hook(post_hook)        // runs on the provider response
//!     .build().await?;
//!
//! // Mount Marg as one plane of your own app, on your own listener.
//! let app: axum::Router = my_router().merge(gateway.router());
//! ```
//!
//! The injected chain must derive from the same keypair named in the config's
//! `[kavach].keypair_path`, so permits and audit entries share one trust
//! bundle. Because [`GatewayBuilder::with_audit_chain`] puts
//! `kavach_pq::SignedAuditChain` in this crate's public API, an embedding host
//! must resolve the same `kavach-pq` version this crate does. See the
//! [embedding guide] for the full hook contract, streaming behavior, and the
//! shared audit chain.
//!
//! [embedding guide]: https://github.com/SarthiAI/Marg/blob/main/docs/embedding.md

pub mod admin;
mod auth;
mod chat;
mod errors;
mod hooks;
pub mod kavach;
mod metered_storage;
pub mod metrics;
pub mod observability;
mod ops;
pub mod policy;
mod proxy;
mod quota;
mod shutdown;
mod state;
mod sse;
mod write_batcher;

use anyhow::Context;
use axum::Router;
use axum::middleware;
use axum::routing::{get, post};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use marg_core::{secret, Config};
use marg_providers::{AnthropicClient, BedrockClient, ChatCompletionsClient, GoogleClient, OpenAIClient};
use marg_storage::{HotStore, LocalHotStore, RedisHotStore, PostgresStorage, SqliteStorage, Storage};
use secrecy::SecretString;
use tower_http::cors::{Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

pub use metrics::Metrics;
pub use ops::version_info;
pub use state::{AppState, ProviderRegistry};

// Embeddable gateway API (ADR-031). Additive: a host assembles the gateway
// without binding any socket, mounts the returned router in its own app,
// optionally injects a shared audit chain, and registers content hooks.
pub use hooks::{
    ContentDecision, RequestContentHook, RequestHookCtx, ResponseContentHook, ResponseHookCtx,
};
pub use kavach::KavachRuntime;

/// Resolved Kavach versions baked in at build time by `build.rs`. The header
/// `x-kavach-version` and the `/version` payload both use these constants so
/// what the runtime reports always matches what Cargo actually linked.
pub const KAVACH_CORE_VERSION: &str = env!("MARG_KAVACH_CORE_VERSION");
pub const KAVACH_PQ_VERSION: &str = env!("MARG_KAVACH_PQ_VERSION");
pub const KAVACH_REDIS_VERSION: &str = env!("MARG_KAVACH_REDIS_VERSION");

pub async fn run(config_path: &str) -> anyhow::Result<()> {
    check_fd_limit();

    // Single assembly path: the daemon is the embeddable gateway plus the
    // sockets, signal handler, and admin server the standalone process owns.
    // No hooks and no injected chain are registered, so the runtime is
    // identical to the pre-embed behaviour (internal chain from
    // `[kavach].keypair_path`, no content checks). See ADR-031.
    let gateway = GatewayBuilder::from_config_path(config_path)
        .await?
        .build()
        .await?;

    let state = gateway.state();
    let cfg = gateway.config().clone();

    // SIGHUP triggers a policy reload (Kavach side + routing side, single
    // transactional swap). Mirrors POST /admin/policy/reload.
    install_sighup_reload(state.clone());

    // Admin API on its own port (binds + spawns internally, as today). In
    // embed mode the host uses `Gateway::admin_router()` and binds it itself;
    // Marg does not auto-bind a second port there.
    admin::serve_admin(cfg.admin.clone(), state.clone())
        .await
        .context("starting admin api")?;

    let app = gateway.router();

    let addr: SocketAddr = cfg
        .server
        .bind
        .parse()
        .with_context(|| format!("parsing bind address '{}'", cfg.server.bind))?;

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding tcp listener to {}", addr))?;

    let registered: Vec<String> = state.providers.keys().cloned().collect();
    tracing::info!(
        %addr,
        storage = state.storage.backend_name(),
        hot_store = state.hot.backend_name(),
        providers = ?registered,
        "marg listening"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown::signal())
        .await
        .context("axum::serve returned an error")?;

    tracing::info!("marg shut down cleanly");
    Ok(())
}

/// Build the main gateway router: the five routes plus the standard
/// middleware layer. Shared by `run()` (standalone) and `Gateway::router()`
/// (embed) so both serve byte-for-byte the same surface.
fn build_router(state: AppState, cfg: &Config) -> Router {
    let max_body = cfg.server.max_body_bytes;

    let mut app = Router::new()
        .route("/health", get(ops::health))
        .route("/ready", get(ops::ready))
        .route("/version", get(ops::version))
        .route("/metrics", get(observability::metrics_handler))
        .route("/v1/chat/completions", post(chat::chat_completions))
        .with_state(state)
        .layer(middleware::from_fn(observability::request_context_layer))
        .layer(TraceLayer::new_for_http())
        .layer(RequestBodyLimitLayer::new(max_body));

    if cfg.cors.enabled {
        app = app.layer(build_cors(cfg));
    }
    app
}

/// Assembles the full gateway from config WITHOUT binding any socket or
/// installing any signal handler. The host owns the runtime, the listener(s),
/// and shutdown. Purely additive; see ADR-031.
pub struct GatewayBuilder {
    config: Config,
    config_path: Option<String>,
    audit_chain: Option<Arc<kavach_pq::SignedAuditChain>>,
    pre_hook: Option<Arc<dyn RequestContentHook>>,
    post_hook: Option<Arc<dyn ResponseContentHook>>,
}

impl GatewayBuilder {
    /// Load config from a TOML file path. Same format and validation as `run()`.
    pub async fn from_config_path(path: &str) -> anyhow::Result<Self> {
        let config =
            Config::load(path).with_context(|| format!("loading config from {}", path))?;
        Ok(Self {
            config,
            config_path: Some(path.to_string()),
            audit_chain: None,
            pre_hook: None,
            post_hook: None,
        })
    }

    /// Build from an already-parsed config (host constructed it in memory).
    /// Note: `reload()` needs a source path, so a gateway built this way
    /// cannot hot-reload from disk.
    pub fn from_config(config: Config) -> Self {
        Self {
            config,
            config_path: None,
            audit_chain: None,
            pre_hook: None,
            post_hook: None,
        }
    }

    /// Inject a host-owned shared audit chain. If set, Marg appends every audit
    /// entry it would normally write to THIS chain instead of creating its own
    /// from `[kavach].keypair_path`, and does not flush it to disk (the host
    /// owns export). The chain must derive from the same keypair named in
    /// `[kavach].keypair_path` so permits and audit entries share one trust
    /// bundle. See ADR-031 section 5.
    pub fn with_audit_chain(mut self, chain: Arc<kavach_pq::SignedAuditChain>) -> Self {
        self.audit_chain = Some(chain);
        self
    }

    /// Register a pre-request content hook. Invoked in the chat pipeline after
    /// parse and before the Kavach gate and forwarding.
    pub fn with_pre_hook(mut self, hook: Arc<dyn RequestContentHook>) -> Self {
        self.pre_hook = Some(hook);
        self
    }

    /// Register a post-response content hook. Invoked after a non-streaming
    /// provider response, before returning to the caller and before the final
    /// audit entry. Streaming behaviour is governed by
    /// `[kavach].buffer_streaming_for_post_hook`.
    pub fn with_post_hook(mut self, hook: Arc<dyn ResponseContentHook>) -> Self {
        self.post_hook = Some(hook);
        self
    }

    /// Assemble everything (storage, providers, routing, pricing, Kavach
    /// runtime, write batcher, hooks) and return a mounted `Gateway`. No
    /// sockets are bound. Background maintenance tasks (audit flush when the
    /// chain is not injected, and the cluster invalidation listener) are
    /// spawned and owned by the returned `Gateway`.
    pub async fn build(self) -> anyhow::Result<Gateway> {
        let cfg = self.config;
        let chain_injected = self.audit_chain.is_some();

        let metrics = Metrics::new();

        let storage = build_storage(&cfg).await?;
        let storage: Arc<dyn Storage> =
            Arc::new(metered_storage::MeteredStorage::new(storage, metrics.clone()));

        let hot = build_hot_store(&cfg).await?;
        let hot: Arc<dyn HotStore> =
            Arc::new(metered_storage::MeteredHotStore::new(hot, metrics.clone()));

        let providers = build_providers(&cfg)?;
        let stored_routes = storage
            .list_routes()
            .await
            .context("loading routes from storage")?;
        let registered: Vec<String> = providers.keys().cloned().collect();
        let routing = policy::build_initial_routing(&cfg, &stored_routes, &registered)
            .with_context(|| "compiling routing rules")?;
        let pricing = policy::build_initial_pricing(&cfg);

        let write_batcher = write_batcher::WriteBatcher::spawn(
            storage.clone(),
            metrics.clone(),
            cfg.storage.write_batcher.clone(),
        );

        // Build Kavach runtime from the policy file (or inline fallback).
        // Empty policy in enforce mode is a fatal startup error per ADR-014.
        let loaded_policy = marg_core::load_kavach_policy(
            &cfg.kavach,
            &cfg.inline_policies,
            &cfg.inline_invariants,
        )
        .with_context(|| "loading kavach policy source")?;
        // Cluster mode turns on when a Redis hot store is configured (ADR-027);
        // single-node passes None and keeps the in-memory shapes.
        let cluster_params = build_cluster_params(&cfg, metrics.clone())
            .context("resolving cluster mode configuration")?;
        let kavach_runtime =
            kavach::build_runtime(&cfg.kavach, &loaded_policy, cluster_params, self.audit_chain)
                .await
                .with_context(|| "building kavach runtime")?;

        // Periodic disk flush of the process-local chain. Skipped when the host
        // injected its own chain: the host then owns export and persistence, so
        // Marg must not also flush the shared chain to its own file.
        let flush_handle = if chain_injected {
            None
        } else {
            let handle = kavach::spawn_audit_flush_task(
                kavach_runtime.audit_chain.clone(),
                kavach_runtime.audit_export_file.clone(),
                cfg.kavach.audit_flush_seconds,
                cfg.kavach.audit_max_resident_bytes,
            );
            tracing::info!(
                path = %handle.path.display(),
                flush_seconds = cfg.kavach.audit_flush_seconds,
                "kavach audit chain export file"
            );
            Some(handle)
        };

        let config_path = self.config_path.clone().unwrap_or_default();
        let state = AppState::new(
            storage,
            hot,
            providers,
            routing,
            pricing,
            cfg.security.clone(),
            cfg.rate_limits.clone(),
            metrics,
            write_batcher,
            config_path,
            kavach_runtime,
        )
        .with_content_hooks(self.pre_hook, self.post_hook);

        // Cluster mode: apply key invalidations broadcast by peer nodes. On a
        // single-node deployment the broadcaster is the no-op variant, so this
        // task idles harmlessly. (ADR-027)
        let invalidation_listener = kavach::spawn_remote_invalidation_listener(state.clone());

        Ok(Gateway {
            state,
            config: cfg,
            _flush_handle: flush_handle,
            _invalidation_listener: invalidation_listener,
        })
    }
}

/// A fully assembled, mountable gateway. No sockets are bound; the host decides
/// how to serve it. Dropping the `Gateway` stops its background tasks.
pub struct Gateway {
    state: AppState,
    config: Config,
    _flush_handle: Option<kavach::AuditFlushTaskHandle>,
    _invalidation_listener: tokio::task::JoinHandle<()>,
}

impl Gateway {
    /// The main gateway router: `/v1/chat/completions`, `/health`, `/ready`,
    /// `/version`, `/metrics`. Mount or nest it in the host's own axum app and
    /// serve it on the host's own listener.
    pub fn router(&self) -> Router {
        build_router(self.state.clone(), &self.config)
    }

    /// The admin router, if `[admin].enabled` (else `None`). Carries the same
    /// request-context layer the standalone admin server applies; the host
    /// applies its own CORS / body limits and chooses whether and where to
    /// mount it. Marg does NOT bind a second port in embed mode.
    pub fn admin_router(&self) -> Option<Router> {
        if !self.config.admin.enabled {
            return None;
        }
        Some(
            admin::build_router(self.state.clone())
                .layer(middleware::from_fn(observability::request_context_layer)),
        )
    }

    /// The Kavach runtime, so the host can reach the shared audit chain and
    /// verifier for export/verification.
    pub fn kavach(&self) -> Arc<KavachRuntime> {
        self.state.kavach.clone()
    }

    /// Hot-reload policy/config from disk, equivalent to what SIGHUP triggers
    /// in `run()`. Requires the gateway to have been built from a config path
    /// (`from_config_path`); a gateway built via `from_config` has no source
    /// file and returns an error.
    pub async fn reload(&self) -> anyhow::Result<()> {
        policy::reload(&self.state)
            .await
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("policy reload failed: {}", e))
    }

    pub(crate) fn state(&self) -> AppState {
        self.state.clone()
    }

    pub(crate) fn config(&self) -> &Config {
        &self.config
    }
}

async fn build_storage(cfg: &Config) -> anyhow::Result<Arc<dyn Storage>> {
    match cfg.storage.backend.as_str() {
        "sqlite" => {
            let storage = SqliteStorage::open(&cfg.storage.path)
                .await
                .with_context(|| format!("opening sqlite at {}", cfg.storage.path))?;
            storage.migrate().await.context("running sqlite migrations")?;
            Ok(Arc::new(storage) as Arc<dyn Storage>)
        }
        "postgres" => {
            let dsn_ref = cfg
                .storage
                .dsn
                .as_deref()
                .context("storage.dsn must be set for postgres backend")?;
            let dsn = secret::resolve(dsn_ref)
                .with_context(|| "resolving storage.dsn secret reference")?;
            let storage = PostgresStorage::connect(
                &dsn,
                cfg.storage.max_connections,
                cfg.storage.min_connections,
            )
            .await
            .with_context(|| "connecting to postgres")?;
            storage.migrate().await.context("running postgres migrations")?;
            Ok(Arc::new(storage) as Arc<dyn Storage>)
        }
        other => {
            anyhow::bail!(
                "storage.backend '{}' is not supported: choose 'sqlite' or 'postgres'",
                other
            );
        }
    }
}

async fn build_hot_store(cfg: &Config) -> anyhow::Result<Arc<dyn HotStore>> {
    match &cfg.storage.hot {
        None => Ok(Arc::new(LocalHotStore::new()) as Arc<dyn HotStore>),
        Some(hot_cfg) => match hot_cfg.backend.as_str() {
            "redis" => {
                let url_ref = hot_cfg
                    .url
                    .as_deref()
                    .context("storage.hot.url must be set for redis hot store")?;
                let url = secret::resolve(url_ref)
                    .with_context(|| "resolving storage.hot.url secret reference")?;
                let store = RedisHotStore::connect(&url, hot_cfg.key_prefix.clone())
                    .await
                    .with_context(|| "connecting to redis hot store")?;
                Ok(Arc::new(store) as Arc<dyn HotStore>)
            }
            other => anyhow::bail!(
                "storage.hot.backend '{}' is not supported: choose 'redis' or omit the [storage.hot] block",
                other
            ),
        },
    }
}

/// Resolve cluster-mode runtime parameters. Returns `Some` only when a Redis
/// hot store is configured (`[storage.hot].backend = "redis"`), which is the
/// signal that this Marg is part of a cluster. The node id comes from
/// `[kavach.cluster].node_id` when set, otherwise a fresh random id is minted
/// at boot (sufficient for correct self-skip on the invalidation channel).
fn build_cluster_params(
    cfg: &Config,
    metrics: Arc<Metrics>,
) -> anyhow::Result<Option<kavach::ClusterRuntimeParams>> {
    let Some(hot) = &cfg.storage.hot else {
        return Ok(None);
    };
    if hot.backend.as_str() != "redis" {
        return Ok(None);
    }
    let url_ref = hot
        .url
        .as_deref()
        .context("storage.hot.url must be set for the redis hot store (cluster mode)")?;
    let redis_url = secret::resolve(url_ref)
        .with_context(|| "resolving storage.hot.url secret reference for cluster mode")?;
    let cc = &cfg.kavach.cluster;
    let node_id = cc
        .node_id
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    Ok(Some(kavach::ClusterRuntimeParams {
        redis_url,
        channel: cc.invalidation_channel.clone(),
        node_id,
        bridge_capacity: cc.bridge_capacity,
        max_message_age_seconds: cc.max_message_age_seconds,
        session_ttl_seconds: cc.session_ttl_seconds,
        metrics,
    }))
}

fn build_providers(cfg: &Config) -> anyhow::Result<ProviderRegistry> {
    let mut registry: ProviderRegistry = ProviderRegistry::new();

    if let Some(openai) = &cfg.providers.openai {
        let api_key = secret::resolve(&openai.api_key)
            .context("resolving providers.openai.api_key secret reference")?;
        let client = OpenAIClient::new(
            openai.base_url.clone(),
            SecretString::new(api_key.into_boxed_str()),
            Duration::from_secs(openai.timeout_seconds),
        )
        .context("constructing OpenAIClient")?;
        registry.insert("openai".to_string(), Arc::new(client) as Arc<dyn ChatCompletionsClient>);
    }
    if let Some(anth) = &cfg.providers.anthropic {
        let api_key = secret::resolve(&anth.api_key)
            .context("resolving providers.anthropic.api_key secret reference")?;
        let client = AnthropicClient::new(
            anth.base_url.clone(),
            SecretString::new(api_key.into_boxed_str()),
            anth.api_version.clone(),
            anth.default_max_tokens,
            Duration::from_secs(anth.timeout_seconds),
        )
        .context("constructing AnthropicClient")?;
        registry.insert("anthropic".to_string(), Arc::new(client) as Arc<dyn ChatCompletionsClient>);
    }
    if let Some(google) = &cfg.providers.google {
        let api_key = secret::resolve(&google.api_key)
            .context("resolving providers.google.api_key secret reference")?;
        let client = GoogleClient::new(
            google.base_url.clone(),
            SecretString::new(api_key.into_boxed_str()),
            google.api_version.clone(),
            Duration::from_secs(google.timeout_seconds),
        )
        .context("constructing GoogleClient")?;
        registry.insert("google".to_string(), Arc::new(client) as Arc<dyn ChatCompletionsClient>);
    }
    if let Some(bedrock) = &cfg.providers.bedrock {
        let access_key = resolve_aws_secret(
            bedrock.access_key_id.as_deref(),
            "AWS_ACCESS_KEY_ID",
        )
        .context("Bedrock access_key_id missing (provide via config or AWS_ACCESS_KEY_ID env)")?;
        let secret_key = resolve_aws_secret(
            bedrock.secret_access_key.as_deref(),
            "AWS_SECRET_ACCESS_KEY",
        )
        .context("Bedrock secret_access_key missing (provide via config or AWS_SECRET_ACCESS_KEY env)")?;
        let session_token = match bedrock.session_token.as_deref() {
            Some(s) if !s.trim().is_empty() => Some(
                secret::resolve(s)
                    .context("resolving providers.bedrock.session_token")?,
            ),
            _ => std::env::var("AWS_SESSION_TOKEN").ok().filter(|s| !s.is_empty()),
        };
        let session_secret = session_token.map(|s| SecretString::new(s.into_boxed_str()));
        let client = BedrockClient::new(
            bedrock.region.clone(),
            SecretString::new(access_key.into_boxed_str()),
            SecretString::new(secret_key.into_boxed_str()),
            session_secret,
            bedrock.default_max_tokens,
            bedrock.anthropic_version.clone(),
            bedrock.base_url.clone(),
            Duration::from_secs(bedrock.timeout_seconds),
        )
        .context("constructing BedrockClient")?;
        registry.insert("bedrock".to_string(), Arc::new(client) as Arc<dyn ChatCompletionsClient>);
    }

    let mut compat: Vec<(&String, &marg_core::config::OpenAiProviderConfig)> =
        cfg.providers.openai_compatible.iter().collect();
    compat.sort_by(|a, b| a.0.cmp(b.0));
    for (name, entry) in compat {
        let api_key = secret::resolve(&entry.api_key).with_context(|| {
            format!(
                "resolving providers.openai_compatible.{}.api_key secret reference",
                name
            )
        })?;
        let client = OpenAIClient::new(
            entry.base_url.clone(),
            SecretString::new(api_key.into_boxed_str()),
            Duration::from_secs(entry.timeout_seconds),
        )
        .with_context(|| {
            format!(
                "constructing OpenAI-compatible client for providers.openai_compatible.{}",
                name
            )
        })?;
        registry.insert(name.clone(), Arc::new(client) as Arc<dyn ChatCompletionsClient>);
    }

    if registry.is_empty() {
        // Boot anyway so the admin console comes up. The first chat request
        // gets a clean error from the routing layer; the operator's job is
        // to log into the console and configure a provider. This is the
        // single-command-install story from P13: `curl | sh` lands a
        // working gateway, the operator wires up the upstream after.
        tracing::warn!(
            "no providers configured: chat endpoints will refuse until at least one of [providers.openai], [providers.anthropic], [providers.google], [providers.bedrock], or [providers.openai_compatible.<name>] is set in marg.toml. The admin console is still available."
        );
    }
    Ok(registry)
}

fn resolve_aws_secret(from_config: Option<&str>, env_var: &str) -> Option<String> {
    if let Some(v) = from_config.filter(|s| !s.trim().is_empty()) {
        return secret::resolve(v).ok().filter(|s| !s.is_empty());
    }
    std::env::var(env_var).ok().filter(|s| !s.is_empty())
}

/// Reads the current process RLIMIT_NOFILE soft limit on Linux and logs a
/// warning if it is below the recommended production floor.
///
/// On non-Linux hosts (e.g. macOS during dev) the check is a no-op; the
/// systemd-managed production path is Linux-only.
fn check_fd_limit() {
    const RECOMMENDED_SOFT_LIMIT: u64 = 65_536;
    let limits = match std::fs::read_to_string("/proc/self/limits") {
        Ok(text) => text,
        Err(_) => return,
    };
    for line in limits.lines() {
        if !line.starts_with("Max open files") {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 5 {
            return;
        }
        let soft: u64 = match parts[3].parse() {
            Ok(v) => v,
            Err(_) => return,
        };
        let hard: u64 = parts[4].parse().unwrap_or(soft);
        if soft < RECOMMENDED_SOFT_LIMIT {
            tracing::warn!(
                soft_limit = soft,
                hard_limit = hard,
                recommended = RECOMMENDED_SOFT_LIMIT,
                "RLIMIT_NOFILE soft limit is below the recommended production floor; \
                 saturating throughput may surface as 'accept error: Too many open files'. \
                 See marg/docs/install.md and marg/docs/cluster-deployment.md for the systemd \
                 unit and limits.d snippets."
            );
        } else {
            tracing::info!(
                soft_limit = soft,
                hard_limit = hard,
                "RLIMIT_NOFILE check passed"
            );
        }
        return;
    }
}

/// Install a SIGHUP handler that re-reads `marg.toml` plus the Kavach policy
/// file and atomically swaps both routing/pricing and Kavach policy. On
/// failure the previous good state keeps serving and the failure is logged +
/// recorded in the signed audit chain. Best-effort on non-unix platforms
/// (Windows: no-op).
fn install_sighup_reload(state: AppState) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        tokio::spawn(async move {
            let mut hup = match signal(SignalKind::hangup()) {
                Ok(s) => s,
                Err(err) => {
                    tracing::error!(?err, "failed to install SIGHUP handler; policy reload will only run via /admin/policy/reload");
                    return;
                }
            };
            loop {
                if hup.recv().await.is_none() {
                    break;
                }
                tracing::info!("received SIGHUP, triggering policy reload");
                match policy::reload(&state).await {
                    Ok(outcome) => tracing::info!(
                        marg_routes = outcome.config_routes + outcome.stored_routes,
                        pricing_entries = outcome.pricing_entries,
                        kavach_rules = outcome.kavach_rules,
                        kavach_invariants = outcome.kavach_invariants,
                        "policy reloaded on SIGHUP"
                    ),
                    Err(e) => tracing::warn!(?e, "policy reload on SIGHUP failed; previous policy still in effect"),
                }
            }
        });
    }
    #[cfg(not(unix))]
    {
        let _ = state;
    }
}

fn build_cors(cfg: &Config) -> CorsLayer {
    if cfg.cors.allowed_origins.is_empty() {
        return CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any);
    }
    let mut layer = CorsLayer::new().allow_methods(Any).allow_headers(Any);
    for origin in &cfg.cors.allowed_origins {
        if let Ok(parsed) = origin.parse::<http::HeaderValue>() {
            layer = layer.allow_origin(parsed);
        }
    }
    layer
}
