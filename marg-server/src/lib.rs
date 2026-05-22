mod auth;
mod chat;
mod errors;
mod ops;
mod proxy;
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

use marg_core::{Config, RoutingEngine};
use marg_providers::{AnthropicClient, BedrockClient, ChatCompletionsClient, GoogleClient, OpenAIClient};
use marg_storage::SqliteStorage;
use secrecy::SecretString;
use tower_http::cors::{Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

pub use ops::version_info;
pub use state::{AppState, ProviderRegistry};

pub async fn run(config_path: &str) -> anyhow::Result<()> {
    let cfg = Config::load(config_path)
        .with_context(|| format!("loading config from {}", config_path))?;

    let storage = build_storage(&cfg).await?;
    let providers = build_providers(&cfg)?;
    let registered: Vec<String> = providers.keys().cloned().collect();
    let routing = RoutingEngine::build(
        &cfg.routes,
        cfg.providers.default.clone().or_else(|| registered.first().cloned()),
        &registered,
    )
    .with_context(|| "compiling routing rules")?;
    let pricing = state::build_pricing(&cfg);

    let state = AppState::new(
        Arc::new(storage),
        providers,
        routing,
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

    tracing::info!(%addr, providers = ?registered, "marg listening");

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

fn build_providers(cfg: &Config) -> anyhow::Result<ProviderRegistry> {
    let mut registry: ProviderRegistry = ProviderRegistry::new();

    if let Some(openai) = &cfg.providers.openai {
        let client = OpenAIClient::new(
            openai.base_url.clone(),
            SecretString::new(openai.api_key.clone().into_boxed_str()),
            Duration::from_secs(openai.timeout_seconds),
        )
        .context("constructing OpenAIClient")?;
        registry.insert("openai".to_string(), Arc::new(client) as Arc<dyn ChatCompletionsClient>);
    }
    if let Some(anth) = &cfg.providers.anthropic {
        let client = AnthropicClient::new(
            anth.base_url.clone(),
            SecretString::new(anth.api_key.clone().into_boxed_str()),
            anth.api_version.clone(),
            anth.default_max_tokens,
            Duration::from_secs(anth.timeout_seconds),
        )
        .context("constructing AnthropicClient")?;
        registry.insert("anthropic".to_string(), Arc::new(client) as Arc<dyn ChatCompletionsClient>);
    }
    if let Some(google) = &cfg.providers.google {
        let client = GoogleClient::new(
            google.base_url.clone(),
            SecretString::new(google.api_key.clone().into_boxed_str()),
            google.api_version.clone(),
            Duration::from_secs(google.timeout_seconds),
        )
        .context("constructing GoogleClient")?;
        registry.insert("google".to_string(), Arc::new(client) as Arc<dyn ChatCompletionsClient>);
    }
    if let Some(bedrock) = &cfg.providers.bedrock {
        let access_key = resolve_secret(
            bedrock.access_key_id.clone(),
            "AWS_ACCESS_KEY_ID",
        )
        .context("Bedrock access_key_id missing (provide via config or AWS_ACCESS_KEY_ID env)")?;
        let secret_key = resolve_secret(
            bedrock.secret_access_key.clone(),
            "AWS_SECRET_ACCESS_KEY",
        )
        .context("Bedrock secret_access_key missing (provide via config or AWS_SECRET_ACCESS_KEY env)")?;
        let session_token = bedrock
            .session_token
            .clone()
            .or_else(|| std::env::var("AWS_SESSION_TOKEN").ok())
            .map(|s| SecretString::new(s.into_boxed_str()));
        let client = BedrockClient::new(
            bedrock.region.clone(),
            SecretString::new(access_key.into_boxed_str()),
            SecretString::new(secret_key.into_boxed_str()),
            session_token,
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

fn resolve_secret(from_config: Option<String>, env_var: &str) -> Option<String> {
    if let Some(v) = from_config.filter(|s| !s.is_empty()) {
        return Some(v);
    }
    std::env::var(env_var).ok().filter(|s| !s.is_empty())
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
