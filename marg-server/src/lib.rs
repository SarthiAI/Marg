mod auth;
mod chat;
mod errors;
mod ops;
mod quota;
mod shutdown;
mod state;
mod sse;

use anyhow::Context;
use axum::Router;
use axum::routing::{get, post};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use marg_core::Config;
use marg_providers::OpenAIClient;
use marg_storage::SqliteStorage;
use secrecy::SecretString;
use tower_http::cors::{Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

pub use ops::version_info;
pub use state::AppState;

pub async fn run(config_path: &str) -> anyhow::Result<()> {
    let cfg = Config::load(config_path)
        .with_context(|| format!("loading config from {}", config_path))?;

    let storage = build_storage(&cfg).await?;
    let provider = build_provider(&cfg)?;
    let pricing = state::build_pricing(&cfg);

    let state = AppState::new(
        Arc::new(storage),
        provider,
        pricing,
        cfg.security.clone(),
    );

    let max_body = cfg.server.max_body_bytes;

    let mut app = Router::new()
        .route("/health", get(ops::health))
        .route("/ready", get(ops::ready))
        .route("/version", get(ops::version))
        .route("/v1/chat/completions", post(chat::chat_completions))
        .with_state(state)
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

    tracing::info!(%addr, "marg listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown::signal())
        .await
        .context("axum::serve returned an error")?;

    tracing::info!("marg shut down cleanly");
    Ok(())
}

async fn build_storage(cfg: &Config) -> anyhow::Result<SqliteStorage> {
    let storage = SqliteStorage::open(&cfg.storage.path)
        .await
        .with_context(|| format!("opening sqlite at {}", cfg.storage.path))?;
    storage.migrate().await.context("running database migrations")?;
    Ok(storage)
}

fn build_provider(cfg: &Config) -> anyhow::Result<Arc<dyn marg_providers::ChatCompletionsClient>> {
    let Some(openai) = &cfg.providers.openai else {
        anyhow::bail!("no provider configured: add [providers.openai] api_key = '...' to marg.toml");
    };
    let timeout = Duration::from_secs(openai.timeout_seconds);
    let client = OpenAIClient::new(
        openai.base_url.clone(),
        SecretString::new(openai.api_key.clone().into_boxed_str()),
        timeout,
    )
    .context("constructing OpenAIClient")?;
    Ok(Arc::new(client))
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
