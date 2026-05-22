use anyhow::Context;
use axum::middleware;
use axum::Router;
use chrono::Utc;
use std::net::SocketAddr;
use tower_http::cors::{Any, CorsLayer};

use marg_core::{AdminConfig, CorsConfig, MargToken, NewAdminToken};

use crate::admin::router::build_router;
use crate::observability::request_context_layer;
use crate::shutdown;
use crate::state::AppState;

/// Launch the admin API server in a background task. Returns once the
/// listener is bound; the task lives until process shutdown.
pub async fn serve_admin(admin_cfg: AdminConfig, state: AppState) -> anyhow::Result<()> {
    if !admin_cfg.enabled {
        tracing::info!("admin api disabled (set [admin].enabled = true to enable)");
        return Ok(());
    }

    bootstrap_admin_token(&state, &admin_cfg).await?;

    let addr: SocketAddr = admin_cfg
        .bind
        .parse()
        .with_context(|| format!("parsing admin bind '{}'", admin_cfg.bind))?;

    let mut app: Router = build_router(state.clone())
        .layer(middleware::from_fn(request_context_layer));

    if admin_cfg.cors.enabled {
        app = app.layer(build_cors(&admin_cfg.cors));
    }

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding admin listener to {}", addr))?;

    tracing::info!(%addr, "marg admin api listening");

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(shutdown::signal())
            .await
        {
            tracing::error!(?e, "admin api server exited with error");
        } else {
            tracing::info!("admin api server stopped");
        }
    });

    Ok(())
}

/// If no active admin tokens are present, mint one and either write it to the
/// configured bootstrap path or log a clear instruction line. Idempotent.
async fn bootstrap_admin_token(state: &AppState, admin_cfg: &AdminConfig) -> anyhow::Result<()> {
    let count = state
        .storage
        .count_active_admin_tokens()
        .await
        .context("counting admin tokens")?;
    if count > 0 {
        return Ok(());
    }

    let token = MargToken::generate();
    let plain = token.expose().to_string();
    let new = NewAdminToken {
        id: uuid::Uuid::new_v4().to_string(),
        token_hash: token.hash(),
        token_prefix: token.display_prefix(),
        label: "bootstrap".to_string(),
        created_at: Utc::now(),
    };
    state
        .storage
        .create_admin_token(new)
        .await
        .context("inserting bootstrap admin token")?;

    let path = admin_cfg.bootstrap_token_path.trim();
    if path.is_empty() {
        tracing::warn!(
            token = %plain,
            "no active admin tokens and admin.bootstrap_token_path is empty: minted token printed once below (rotate via POST /admin/auth/tokens)"
        );
    } else {
        match write_bootstrap_file(path, &plain) {
            Ok(()) => tracing::info!(
                path = %path,
                "wrote bootstrap admin token (chmod 0600). Rotate via POST /admin/auth/tokens"
            ),
            Err(e) => tracing::warn!(
                ?e,
                token = %plain,
                "could not write bootstrap admin token to disk: token logged once above, rotate via POST /admin/auth/tokens"
            ),
        }
    }
    Ok(())
}

fn write_bootstrap_file(path: &str, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    file.write_all(contents.as_bytes())?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}

fn build_cors(cors_cfg: &CorsConfig) -> CorsLayer {
    if cors_cfg.allowed_origins.is_empty() {
        return CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);
    }
    let mut layer = CorsLayer::new().allow_methods(Any).allow_headers(Any);
    for origin in &cors_cfg.allowed_origins {
        if let Ok(parsed) = origin.parse::<http::HeaderValue>() {
            layer = layer.allow_origin(parsed);
        }
    }
    layer
}

