mod routes;
mod shutdown;

use anyhow::Context;
use std::net::SocketAddr;

pub use routes::version_info;

pub async fn run(config_path: &str) -> anyhow::Result<()> {
    let cfg = marg_core::Config::load(config_path)
        .with_context(|| format!("loading config from {}", config_path))?;

    let app = routes::router();

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
