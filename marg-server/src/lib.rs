pub mod admin;
mod auth;
mod chat;
mod errors;
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

/// Resolved Kavach versions baked in at build time by `build.rs`. The header
/// `x-kavach-version` and the `/version` payload both use these constants so
/// what the runtime reports always matches what Cargo actually linked.
pub const KAVACH_CORE_VERSION: &str = env!("MARG_KAVACH_CORE_VERSION");
pub const KAVACH_PQ_VERSION: &str = env!("MARG_KAVACH_PQ_VERSION");
pub const KAVACH_REDIS_VERSION: &str = env!("MARG_KAVACH_REDIS_VERSION");

pub async fn run(config_path: &str) -> anyhow::Result<()> {
    let cfg = Config::load(config_path)
        .with_context(|| format!("loading config from {}", config_path))?;

    check_fd_limit();

    let metrics = Metrics::new();

    let storage = build_storage(&cfg).await?;
    let storage: Arc<dyn Storage> =
        Arc::new(metered_storage::MeteredStorage::new(storage, metrics.clone()));

    let hot = build_hot_store(&cfg).await?;
    let hot: Arc<dyn HotStore> =
        Arc::new(metered_storage::MeteredHotStore::new(hot, metrics.clone()));

    let providers = build_providers(&cfg)?;
    let registered: Vec<String> = providers.keys().cloned().collect();
    let stored_routes = storage
        .list_routes()
        .await
        .context("loading routes from storage")?;
    let routing = policy::build_initial_routing(&cfg, &stored_routes, &registered)
        .with_context(|| "compiling routing rules")?;
    let pricing = policy::build_initial_pricing(&cfg);

    let write_batcher = write_batcher::WriteBatcher::spawn(
        storage.clone(),
        metrics.clone(),
        cfg.storage.write_batcher.clone(),
    );

    // Build Kavach runtime from the policy file (or inline fallback).
    // Empty policy in enforce mode is a fatal startup error per ADR-014;
    // the build_runtime call enforces that contract.
    let loaded_policy = marg_core::load_kavach_policy(
        &cfg.kavach,
        &cfg.inline_policies,
        &cfg.inline_invariants,
    )
    .with_context(|| "loading kavach policy source")?;
    // Cluster mode turns on when a Redis hot store is configured: the gate
    // gets the signed cross-node invalidation broadcaster (ADR-027) and the
    // shared Redis session store. Single-node passes None and keeps the
    // in-memory shapes.
    let cluster_params = build_cluster_params(&cfg, metrics.clone())
        .context("resolving cluster mode configuration")?;
    let kavach_runtime = kavach::build_runtime(&cfg.kavach, &loaded_policy, cluster_params)
        .await
        .with_context(|| "building kavach runtime")?;

    // Periodic disk flush. One JSONL file per process lifetime; cross-restart
    // chain merging is a v1.1 concern documented in docs/cluster-deployment.md.
    let flush_handle = kavach::spawn_audit_flush_task(
        kavach_runtime.audit_chain.clone(),
        kavach_runtime.audit_export_file.clone(),
        cfg.kavach.audit_flush_seconds,
        cfg.kavach.audit_max_resident_bytes,
    );
    tracing::info!(
        path = %flush_handle.path.display(),
        flush_seconds = cfg.kavach.audit_flush_seconds,
        "kavach audit chain export file"
    );

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
        config_path.to_string(),
        kavach_runtime.clone(),
    );

    // SIGHUP triggers a policy reload (Kavach side + routing side, single
    // transactional swap). Mirrors POST /admin/policy/reload.
    install_sighup_reload(state.clone());

    // Cluster mode: apply key invalidations broadcast by peer nodes. On a
    // single-node deployment the broadcaster is the no-op variant whose
    // subscription never yields, so this task idles harmlessly. (ADR-027)
    let _invalidation_listener = kavach::spawn_remote_invalidation_listener(state.clone());

    admin::serve_admin(cfg.admin.clone(), state.clone())
        .await
        .context("starting admin api")?;

    let max_body = cfg.server.max_body_bytes;

    let mut app = Router::new()
        .route("/health", get(ops::health))
        .route("/ready", get(ops::ready))
        .route("/version", get(ops::version))
        .route("/metrics", get(observability::metrics_handler))
        .route("/v1/chat/completions", post(chat::chat_completions))
        .with_state(state.clone())
        .layer(middleware::from_fn(observability::request_context_layer))
        .layer(TraceLayer::new_for_http())
        .layer(RequestBodyLimitLayer::new(max_body));

    if cfg.cors.enabled {
        let cors = build_cors(&cfg);
        app = app.layer(cors);
    }

    let addr: SocketAddr = cfg
        .server
        .bind
        .parse()
        .with_context(|| format!("parsing bind address '{}'", cfg.server.bind))?;

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding tcp listener to {}", addr))?;

    tracing::info!(
        %addr,
        storage = state.storage.backend_name(),
        hot_store = state.hot.backend_name(),
        providers = ?registered,
        stored_routes = stored_routes.len(),
        "marg listening"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown::signal())
        .await
        .context("axum::serve returned an error")?;

    tracing::info!("marg shut down cleanly");
    Ok(())
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
