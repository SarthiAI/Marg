pub mod admin;
mod auth;
mod chat;
mod errors;
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
    );

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
            Duration::from_secs(bedrock.timeout_seconds),
        )
        .context("constructing BedrockClient")?;
        registry.insert("bedrock".to_string(), Arc::new(client) as Arc<dyn ChatCompletionsClient>);
    }

    if registry.is_empty() {
        anyhow::bail!(
            "no providers configured: add at least one of [providers.openai], [providers.anthropic], [providers.google], [providers.bedrock] to marg.toml"
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
