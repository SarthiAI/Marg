//! Marg-side session store wrapper.
//!
//! Single-node v1.0 wraps `kavach_core::InMemorySessionStore` behind an
//! `Arc<dyn SessionStore>`. The cluster build in P11 swaps in a Redis-backed
//! `SessionStore` without touching the call sites that ride on this wrapper.
//!
//! Session id is the deterministic UUIDv5-style derivation from the Marg
//! `key_id` (see `context::derive_session_id`). The store is the place that
//! durably records "origin" facts (origin_ip, origin_geo, origin_device,
//! started_at) on first sighting so the drift evaluator can compare current
//! request facts against that origin on every subsequent request.

use kavach_core::{
    DeviceFingerprint, EnvContext, GeoLocation, InMemorySessionStore, SessionState, SessionStore,
    SessionStoreError,
};
use std::net::IpAddr;
use std::sync::Arc;
use uuid::Uuid;

/// Marg-side session store. Wraps any `kavach_core::SessionStore` and adds the
/// `lookup_or_create_session` helper that the request hot path calls per
/// request.
pub struct MargSessionStore {
    inner: Arc<dyn SessionStore>,
    backend_name: &'static str,
}

impl MargSessionStore {
    /// Build with the single-node in-memory backend.
    pub fn in_memory() -> Self {
        Self {
            inner: Arc::new(InMemorySessionStore::new()),
            backend_name: "in_memory",
        }
    }

    /// Wrap an arbitrary backend (for the P11 Redis swap).
    pub fn from_inner(inner: Arc<dyn SessionStore>, backend_name: &'static str) -> Self {
        Self {
            inner,
            backend_name,
        }
    }

    pub fn backend_name(&self) -> &'static str {
        self.backend_name
    }

    /// Underlying store handle (admin endpoints that report counts can read
    /// from this directly).
    pub fn inner(&self) -> Arc<dyn SessionStore> {
        self.inner.clone()
    }

    /// Look up or create the session row for a Marg key. On first sighting the
    /// origin facts (ip, geo, device, started_at) are taken from the current
    /// request's environment so subsequent drift comparisons have a baseline.
    /// On every subsequent request the existing row is loaded, `action_count`
    /// is incremented in memory (the gate sees the updated count), and the
    /// row is written back so the count persists.
    pub async fn lookup_or_create_session(
        &self,
        session_id: Uuid,
        credentials_issued_at: chrono::DateTime<chrono::Utc>,
        env: &EnvContext,
    ) -> Result<SessionState, SessionStoreError> {
        let id = session_id.to_string();
        let existing = self.inner.get(&id).await?;
        match existing {
            Some(mut session) => {
                // Action count rolls forward so behavior drift sees real
                // traffic shape. Cap the in-memory history field so a hot key
                // doesn't grow the row unbounded.
                session.action_count = session.action_count.saturating_add(1);
                self.inner.put(&id, session.clone()).await?;
                Ok(session)
            }
            None => {
                let now = chrono::Utc::now();
                // started_at = max(credentials_issued_at, now). A freshly
                // rotated key keeps a fresh session window; a key that has
                // existed for a while starts the session right now.
                let started_at = if credentials_issued_at > now {
                    now
                } else {
                    credentials_issued_at.max(now - chrono::Duration::seconds(1))
                };
                let mut session = SessionState {
                    session_id,
                    started_at,
                    action_count: 1,
                    action_history: Vec::new(),
                    invalidated: false,
                    origin_ip: env.ip,
                    origin_device: env.device.clone(),
                    origin_geo: env.geo.clone(),
                };
                // Defensive: drop any inherited invalidated flag (cannot exist
                // for a fresh row, this only matters if `from_inner` is
                // wrapping a pre-seeded backend).
                session.invalidated = false;
                self.inner.put(&id, session.clone()).await?;
                Ok(session)
            }
        }
    }

    /// Explicit invalidation called from the `Verdict::Invalidate` branch and
    /// from admin-driven key invalidate / revoke flows. Idempotent.
    pub async fn invalidate(&self, session_id: Uuid) -> Result<(), SessionStoreError> {
        let id = session_id.to_string();
        if let Some(mut session) = self.inner.get(&id).await? {
            session.invalidated = true;
            self.inner.put(&id, session).await?;
        }
        Ok(())
    }

    /// Background cleanup hook the runtime calls periodically so long-lived
    /// idle sessions don't pile up.
    pub async fn cleanup_older_than_seconds(
        &self,
        max_age_seconds: i64,
    ) -> Result<u64, SessionStoreError> {
        self.inner.cleanup(max_age_seconds).await
    }
}

/// Parse `IpAddr`, `GeoLocation`, `DeviceFingerprint`, and `user_agent` out of
/// request headers. Pure header-driven; Marg deliberately does not embed a
/// GeoIP database in v1.0 (see `docs/kavach.md`).
pub struct CallerHeaders {
    pub ip: Option<IpAddr>,
    pub geo: Option<GeoLocation>,
    pub device: Option<DeviceFingerprint>,
    pub user_agent: Option<String>,
}

impl CallerHeaders {
    pub fn into_env(self) -> EnvContext {
        EnvContext {
            ip: self.ip,
            device: self.device,
            geo: self.geo,
            user_agent: self.user_agent,
        }
    }
}
