pub async fn signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(err) => {
                tracing::error!(?err, "failed to install SIGTERM handler, falling back to ctrl-c only");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = term.recv() => tracing::info!("received SIGTERM, beginning graceful shutdown"),
            res = tokio::signal::ctrl_c() => {
                if let Err(err) = res {
                    tracing::error!(?err, "ctrl-c handler error");
                }
                tracing::info!("received SIGINT, beginning graceful shutdown");
            }
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("received shutdown signal");
    }
}
