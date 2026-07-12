//! Signed cluster key-invalidation channel (ADR-027).
//!
//! When one Marg node kills a compromised key (a drift detector or an admin
//! action produces `Verdict::Invalidate`), every other node must drop that
//! key fast. The off-the-shelf `kavach-redis` broadcaster carries that
//! message over plaintext Redis Pub/Sub with nothing authenticating the
//! sender, so an attacker who can reach the operator's Redis could forge
//! "kill key X" messages and knock valid keys offline cluster-wide.
//!
//! ADR-027 closes that hole: Marg signs every invalidation with the same
//! post-quantum signer it already uses for the audit chain (ML-DSA-65,
//! hybrid with Ed25519), and every node verifies the signature before acting.
//! Anything unsigned, forged, or stale is dropped and counted. The transport
//! stays Redis Pub/Sub (the signature rides inside the payload), so no new
//! infrastructure is needed. Confidentiality is out of scope: the payload is
//! a key id, not a secret, so signing (authenticity + integrity + a bounded
//! replay window) is the right and sufficient control.
//!
//! Observe mode never broadcasts. The gate publishes on `Invalidate`
//! regardless of mode, but a cluster-wide key drop is an enforcement action,
//! so [`SignedRedisBroadcaster::publish`] short-circuits to a no-op unless the
//! runtime is in enforce mode (tracked by a shared flag the policy reload
//! flips).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use futures_util::StreamExt;
use kavach_core::invalidation::{BroadcastError, InvalidationBroadcaster};
use kavach_core::verdict::{InvalidationScope, InvalidationTarget};
use kavach_pq::{SignedPayload, Signer, Verifier};
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::metrics::Metrics;
use crate::state::AppState;

/// What gets signed and put on the wire: the scope plus the id of the node
/// that issued it, so receivers can skip the echo of their own broadcasts.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct InvalidationEnvelope {
    node_id: String,
    scope: InvalidationScope,
}

/// Construction-time failures for the signed broadcaster.
#[derive(Debug, Error)]
pub enum RedisBroadcasterError {
    #[error("redis client open: {0}")]
    Client(String),
    #[error("redis connect: {0}")]
    Connect(String),
}

/// Holds the background bridge task and aborts it when the broadcaster is
/// dropped, so a reload or shutdown does not leak a subscriber task.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Redis Pub/Sub-backed invalidation broadcaster that signs every message it
/// publishes and verifies every message it receives (ADR-027). Implements
/// `kavach_core::InvalidationBroadcaster`, so it plugs into
/// `Gate::with_broadcaster` exactly where the no-op broadcaster sat.
pub struct SignedRedisBroadcaster {
    // ConnectionManager (not a bare MultiplexedConnection) so the publish path
    // transparently reconnects after a Redis restart/blip. A plain multiplexed
    // connection, once broken, returns "broken pipe" forever and silently kills
    // cross-node invalidation until the process restarts.
    publish_conn: ConnectionManager,
    channel: String,
    node_id: String,
    signer: Arc<Signer>,
    local_sender: broadcast::Sender<InvalidationScope>,
    metrics: Arc<Metrics>,
    _bridge: AbortOnDrop,
}

impl SignedRedisBroadcaster {
    /// Connect to Redis, start the verify-and-forward bridge task, and return
    /// a ready broadcaster. Publishing is unconditional here; observe-mode
    /// suppression for the gate path lives in [`EnforceGatedBroadcaster`], so
    /// deliberate admin-initiated kills can still broadcast in any mode.
    #[allow(clippy::too_many_arguments)]
    pub async fn connect(
        redis_url: &str,
        channel: String,
        node_id: String,
        signer: Arc<Signer>,
        verifier: Arc<Verifier>,
        bridge_capacity: usize,
        max_message_age_seconds: i64,
        metrics: Arc<Metrics>,
    ) -> Result<Self, RedisBroadcasterError> {
        let client = redis::Client::open(redis_url)
            .map_err(|e| RedisBroadcasterError::Client(e.to_string()))?;
        let publish_conn = ConnectionManager::new(client.clone())
            .await
            .map_err(|e| RedisBroadcasterError::Connect(e.to_string()))?;

        let (local_sender, _) = broadcast::channel(bridge_capacity.max(1));

        let bridge = spawn_bridge(
            client,
            channel.clone(),
            node_id.clone(),
            verifier,
            max_message_age_seconds,
            local_sender.clone(),
            metrics.clone(),
        );

        Ok(Self {
            publish_conn,
            channel,
            node_id,
            signer,
            local_sender,
            metrics,
            _bridge: AbortOnDrop(bridge),
        })
    }
}

#[async_trait]
impl InvalidationBroadcaster for SignedRedisBroadcaster {
    async fn publish(&self, scope: InvalidationScope) -> Result<(), BroadcastError> {
        let envelope = InvalidationEnvelope {
            node_id: self.node_id.clone(),
            scope,
        };
        let data = serde_json::to_vec(&envelope)
            .map_err(|e| BroadcastError::Other(format!("encode envelope: {e}")))?;
        let signed = self
            .signer
            .sign(&data)
            .map_err(|e| BroadcastError::Other(format!("sign invalidation: {e}")))?;
        let wire = serde_json::to_vec(&signed)
            .map_err(|e| BroadcastError::Other(format!("encode signed payload: {e}")))?;

        // The ConnectionManager reconnects in the background after a Redis
        // blip, but the command that first meets the broken connection can
        // still fail while the reconnect is mid-flight. A missed publish here
        // silently downgrades a compromised-key kill to the slow cache-TTL
        // fallback, so retry a few times with a short backoff to ride out the
        // reconnect window before giving up. This path is admin/drift-rate, not
        // the request hot path, so the extra latency on failure is acceptable.
        let mut conn = self.publish_conn.clone();
        let mut last_err: Option<String> = None;
        for attempt in 0..4 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(150 * attempt as u64)).await;
            }
            match redis::cmd("PUBLISH")
                .arg(&self.channel)
                .arg(&wire)
                .query_async::<i64>(&mut conn)
                .await
            {
                Ok(_) => {
                    self.metrics.record_cluster_invalidation("published", "ok");
                    return Ok(());
                }
                Err(e) => last_err = Some(e.to_string()),
            }
        }
        self.metrics
            .record_cluster_invalidation("published", "failed");
        Err(BroadcastError::BackendUnavailable(
            last_err.unwrap_or_else(|| "publish failed".to_string()),
        ))
    }

    fn subscribe(&self) -> broadcast::Receiver<InvalidationScope> {
        self.local_sender.subscribe()
    }
}

/// Wraps a broadcaster so the gate path only fans out in enforce mode. The
/// gate calls `publish` on every `Invalidate` verdict regardless of mode, and
/// a cluster-wide key drop is an enforcement action, so in observe mode this
/// suppresses the publish (and counts it) instead of broadcasting. Deliberate
/// admin-initiated kills do NOT go through this wrapper; they call the inner
/// signed broadcaster directly so they fire in any mode.
pub struct EnforceGatedBroadcaster {
    inner: Arc<dyn InvalidationBroadcaster>,
    enforce: Arc<AtomicBool>,
    metrics: Arc<Metrics>,
}

impl EnforceGatedBroadcaster {
    pub fn new(
        inner: Arc<dyn InvalidationBroadcaster>,
        enforce: Arc<AtomicBool>,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self {
            inner,
            enforce,
            metrics,
        }
    }
}

#[async_trait]
impl InvalidationBroadcaster for EnforceGatedBroadcaster {
    async fn publish(&self, scope: InvalidationScope) -> Result<(), BroadcastError> {
        if !self.enforce.load(Ordering::Relaxed) {
            self.metrics
                .record_cluster_invalidation("published", "suppressed");
            return Ok(());
        }
        self.inner.publish(scope).await
    }

    fn subscribe(&self) -> broadcast::Receiver<InvalidationScope> {
        self.inner.subscribe()
    }
}

/// Spawn the bridge task: subscribe to the Redis channel, verify each signed
/// message, drop self-originated / forged / stale ones, and forward valid
/// scopes onto the local broadcast channel. Reconnects with backoff so a
/// transient Redis blip does not silently stop cluster invalidation.
fn spawn_bridge(
    client: redis::Client,
    channel: String,
    self_node_id: String,
    verifier: Arc<Verifier>,
    max_message_age_seconds: i64,
    sender: broadcast::Sender<InvalidationScope>,
    metrics: Arc<Metrics>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut backoff = Duration::from_millis(200);
        loop {
            match run_bridge_once(
                &client,
                &channel,
                &self_node_id,
                &verifier,
                max_message_age_seconds,
                &sender,
                &metrics,
            )
            .await
            {
                Ok(()) => {
                    backoff = Duration::from_millis(200);
                    tracing::debug!("cluster invalidation bridge stream ended, resubscribing");
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "cluster invalidation bridge dropped, reconnecting"
                    );
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(5));
        }
    })
}

async fn run_bridge_once(
    client: &redis::Client,
    channel: &str,
    self_node_id: &str,
    verifier: &Verifier,
    max_message_age_seconds: i64,
    sender: &broadcast::Sender<InvalidationScope>,
    metrics: &Arc<Metrics>,
) -> Result<(), redis::RedisError> {
    let mut pubsub = client.get_async_pubsub().await?;
    pubsub.subscribe(channel).await?;
    tracing::info!(channel = %channel, "cluster invalidation bridge subscribed");
    let mut stream = pubsub.on_message();
    while let Some(msg) = stream.next().await {
        let payload: Vec<u8> = match msg.get_payload() {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "cluster invalidation: unreadable payload");
                metrics.record_cluster_invalidation("received", "rejected");
                continue;
            }
        };

        let signed: SignedPayload = match serde_json::from_slice(&payload) {
            Ok(s) => s,
            Err(_) => {
                metrics.record_cluster_invalidation("received", "rejected");
                continue;
            }
        };

        // Authenticity: drop anything not signed by the cluster key.
        if verifier.verify(&signed).is_err() {
            tracing::warn!("cluster invalidation: signature verification failed, dropping");
            metrics.record_cluster_invalidation("received", "rejected");
            continue;
        }

        // Replay bound: drop anything older than the configured window.
        let age = (Utc::now() - signed.signed_at).num_seconds();
        if age > max_message_age_seconds {
            tracing::warn!(age_seconds = age, "cluster invalidation: stale message dropped");
            metrics.record_cluster_invalidation("received", "rejected");
            continue;
        }

        let envelope: InvalidationEnvelope = match serde_json::from_slice(&signed.data) {
            Ok(e) => e,
            Err(_) => {
                metrics.record_cluster_invalidation("received", "rejected");
                continue;
            }
        };

        // Skip the echo of our own broadcast; the issuing node already
        // invalidated locally on the request path.
        if envelope.node_id == self_node_id {
            continue;
        }

        metrics.record_cluster_invalidation("received", "ok");
        // `send` errors only when there are zero local subscribers, which is
        // benign (the apply task may not be attached yet); ignore it.
        let _ = sender.send(envelope.scope);
    }
    Ok(())
}

/// Build the invalidation scope for an admin-initiated kill of a Marg key
/// (invalidate or revoke). Principal-targeted on the key id; the remote
/// listener responds by dropping the local auth cache so every node
/// revalidates the key against shared storage (for revoke it then sees the
/// revoked status; for invalidate it rebuilds fresh hot state).
pub fn cluster_invalidation_scope(key_id: &str, reason: &str) -> InvalidationScope {
    InvalidationScope {
        target: InvalidationTarget::Principal(key_id.to_string()),
        reason: reason.to_string(),
        evaluator: "admin".to_string(),
    }
}

/// Spawn the task that applies invalidations received from peer nodes. It
/// mirrors the local handling in `chat.rs`: flip the session row, drop the
/// local auth cache so the next request revalidates, and append a signed
/// `marg.key_event.v1` entry. Self-originated messages were already filtered
/// out in the bridge, so everything here came from another node.
///
/// On a single-node deployment the broadcaster is the no-op variant whose
/// subscription never yields, so this task idles harmlessly.
pub fn spawn_remote_invalidation_listener(state: AppState) -> JoinHandle<()> {
    let mut rx = state.kavach.invalidation_broadcaster.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(scope) => apply_remote_scope(&state, &scope).await,
                Err(broadcast::error::RecvError::Closed) => {
                    tracing::debug!("cluster invalidation listener: channel closed, exiting");
                    return;
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(
                        dropped = n,
                        "cluster invalidation listener lagged; some remote invalidations were not applied locally"
                    );
                }
            }
        }
    })
}

async fn apply_remote_scope(state: &AppState, scope: &InvalidationScope) {
    match &scope.target {
        InvalidationTarget::Session(uuid) => {
            if let Err(e) = state.kavach.session_store.invalidate(*uuid).await {
                tracing::warn!(
                    ?e,
                    session_id = %uuid,
                    "remote invalidation: failed to flip session row"
                );
            }
            state.key_cache.invalidate_all();
            crate::kavach::emit_key_event(
                &state.kavach.audit_chain,
                "cluster",
                &uuid.to_string(),
                crate::kavach::KeyEventKind::Invalidated,
                Some(scope.reason.as_str()),
            );
            tracing::info!(
                session_id = %uuid,
                evaluator = %scope.evaluator,
                reason = %scope.reason,
                "applied remote key invalidation"
            );
        }
        InvalidationTarget::Principal(id) => {
            // No principal -> session index in the hot path; drop the whole
            // auth cache so every key for this principal revalidates.
            state.key_cache.invalidate_all();
            tracing::info!(principal = %id, "remote principal invalidation: dropped local auth cache");
        }
        InvalidationTarget::Role(role) => {
            state.key_cache.invalidate_all();
            tracing::info!(role = %role, "remote role invalidation: dropped local auth cache");
        }
    }
}
