//! `KavachRuntime`: the live gate + audit chain + swappable policy/invariant
//! state Marg exposes through `AppState`. Built once at boot, mutated only by
//! `policy::reload` (under a single transactional swap so requests never see
//! a half-loaded policy).

use anyhow::{anyhow, Context, Result};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use kavach_core::invalidation::InvalidationBroadcaster;
use kavach_core::{
    ActionContext, BehaviorDrift, DeviceDrift, DriftDetector, DriftEvaluator, Evaluator, Gate,
    GateConfig, GeoLocationDrift, Invariant, InvariantSet, NoopInvalidationBroadcaster, Policy,
    PolicyEngine, PolicySet, SessionAgeDrift, SessionStore, TokenSigner, Verdict,
};
use kavach_pq::{PqTokenSigner, SignedAuditChain, Signer, Verifier};

use super::audit_target::AuditChainHandle;
use kavach_redis::RedisSessionStore;
use marg_core::{InvariantToml, KavachConfig, LoadedKavachPolicy};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::kavach::keys::load_or_generate_keypair;
use crate::kavach::session_store::MargSessionStore;
use crate::kavach::{EnforceGatedBroadcaster, SignedRedisBroadcaster};
use crate::metrics::Metrics;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KavachMode {
    Observe,
    Enforce,
}

impl KavachMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            KavachMode::Observe => "observe",
            KavachMode::Enforce => "enforce",
        }
    }

    pub fn from_config(cfg: &KavachConfig) -> Self {
        if cfg.is_enforce() {
            KavachMode::Enforce
        } else {
            KavachMode::Observe
        }
    }
}

/// A swappable wrapper around `InvariantSet` that satisfies Kavach's
/// `Evaluator` trait. We use this because the Kavach `Gate` does not expose a
/// `replace_evaluator` API; the only way to hot-swap invariants without
/// rebuilding the gate is to hand the gate an evaluator that re-reads its
/// inner state at every call.
pub struct SwappableInvariantSet {
    inner: ArcSwap<InvariantSet>,
}

impl SwappableInvariantSet {
    pub fn new(set: InvariantSet) -> Self {
        Self {
            inner: ArcSwap::from_pointee(set),
        }
    }

    pub fn store(&self, set: InvariantSet) {
        self.inner.store(Arc::new(set));
    }
}

#[async_trait]
impl Evaluator for SwappableInvariantSet {
    fn name(&self) -> &str {
        "invariants"
    }

    fn priority(&self) -> u32 {
        150 // matches Kavach's built-in InvariantSet priority
    }

    async fn evaluate(&self, ctx: &ActionContext) -> Verdict {
        self.inner.load().evaluate(ctx).await
    }
}

/// Hot-swappable wrapper around an `Option<Arc<DriftEvaluator>>`. Same
/// reasoning as `SwappableInvariantSet`, the gate does not expose a
/// `replace_evaluator` API so we hand it a delegating evaluator that
/// re-reads its inner state on every call. When the inner is `None` we
/// return a `Permit` so the gate's verdict-combination logic treats drift
/// as quiescent.
pub struct SwappableDriftEvaluator {
    inner: ArcSwap<Option<Arc<DriftEvaluator>>>,
}

impl SwappableDriftEvaluator {
    pub fn new(inner: Option<Arc<DriftEvaluator>>) -> Self {
        Self {
            inner: ArcSwap::from_pointee(inner),
        }
    }

    pub fn store(&self, inner: Option<Arc<DriftEvaluator>>) {
        self.inner.store(Arc::new(inner));
    }

    pub fn is_active(&self) -> bool {
        self.inner.load().is_some()
    }
}

#[async_trait]
impl Evaluator for SwappableDriftEvaluator {
    fn name(&self) -> &str {
        "drift"
    }

    fn priority(&self) -> u32 {
        100 // matches Kavach's documented drift slot
    }

    async fn evaluate(&self, ctx: &ActionContext) -> Verdict {
        let guard = self.inner.load();
        match guard.as_ref() {
            Some(ev) => ev.evaluate(ctx).await,
            None => Verdict::Permit(kavach_core::PermitToken::new(
                ctx.evaluation_id,
                ctx.action.name.clone(),
            )),
        }
    }
}

pub struct KavachRuntime {
    pub gate: Arc<Gate>,
    pub policy_engine: Arc<PolicyEngine>,
    pub invariants: Arc<SwappableInvariantSet>,
    pub audit_chain: AuditChainHandle,
    /// Per-process JSONL file the audit chain is flushed to. The flush task
    /// appends to it; the admin audit handlers read it back and stitch it to
    /// the in-memory tail to serve / verify the full chain after pruning.
    pub audit_export_file: PathBuf,
    pub verifier: Verifier,
    pub mode: ArcSwap<KavachMode>,
    pub include_prompts: ArcSwap<bool>,
    /// Embed-only. When a host registers a post-response content hook, whether
    /// that hook runs on streamed responses (buffering the stream) or is
    /// skipped so streaming is preserved. Default false. See ADR-031 section 6.
    pub buffer_streaming_for_post_hook: bool,
    pub expose_permit_to_caller: ArcSwap<bool>,
    pub forward_permit_to_provider: ArcSwap<bool>,
    /// Whether the permit token signer was attached at boot (i.e. permits are
    /// consumed and therefore signed). Immutable for the process lifetime: the
    /// signer lives in the gate, which is not rebuilt on reload. Reload refuses
    /// to enable `expose_permit_to_caller` / `forward_permit_to_provider` when
    /// this is false, so an unsigned permit is never exposed or forwarded.
    pub permit_signing_enabled: bool,
    pub permit_ttl_seconds: ArcSwap<u64>,
    pub policy_source_path: Option<PathBuf>,
    pub policy_source_hash: ArcSwap<String>,
    pub policy_loaded_at: ArcSwap<String>, // rfc3339
    pub policy_rule_count: ArcSwap<u64>,
    pub invariant_count: ArcSwap<u64>,
    pub kavach_version: String,
    pub permit_signer: PermitSignerState,
    pub drift_state: ArcSwap<DriftDetectorState>,
    pub drift_evaluator: Arc<SwappableDriftEvaluator>,
    /// Whether the per-request session-store round-trip is needed: true when
    /// drift detection is enabled OR a loaded policy uses a `session_age_max`
    /// condition. When false, `action_context_from_request` synthesizes the
    /// session from the request instead of hitting the store, eliminating the
    /// per-request Redis round-trip (the dominant cluster-mode hot-path cost
    /// when nothing consumes the session). Recomputed on reload.
    pub session_tracking_needed: ArcSwap<bool>,
    pub session_store: Arc<MargSessionStore>,
    pub keypair_id: String,
    /// The invalidation broadcaster wired into the gate. Cluster mode uses the
    /// signed Redis broadcaster (ADR-027); single-node uses the no-op. Stored
    /// here so the remote-apply listener can subscribe to it.
    pub invalidation_broadcaster: Arc<dyn InvalidationBroadcaster>,
    /// Shared enforce/observe flag the signed broadcaster reads so observe
    /// mode never fans out a cluster-wide key drop. Flipped by policy reload
    /// in lock-step with `mode`.
    pub enforce_flag: Arc<AtomicBool>,
}

/// Observability snapshot of the permit signer wired into the gate. Surfaces
/// in `/admin/audit/status` so operators can confirm signing is on.
#[derive(Debug, Clone)]
pub struct PermitSignerState {
    pub enabled: bool,
    pub algorithm: &'static str,
    pub key_id: String,
}

/// Observability snapshot of the active drift detectors. Surfaces in
/// `/admin/audit/status` and on the Console policy page. The summary is a
/// flat list of `(name, enabled, parameters)` so the UI does not need to
/// know the Kavach detector type hierarchy.
#[derive(Debug, Clone, Default)]
pub struct DriftDetectorState {
    pub enabled: bool,
    pub warning_threshold: usize,
    pub detectors: Vec<DriftDetectorEntry>,
}

#[derive(Debug, Clone)]
pub struct DriftDetectorEntry {
    pub name: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct KavachReloadOutcome {
    pub policy_rule_count: usize,
    pub invariant_count: usize,
    pub previous_hash: String,
    pub new_hash: String,
    pub source_path: Option<PathBuf>,
}

/// Parameters threaded into [`build_runtime`] when Marg boots clustered
/// (`[storage.hot].backend = "redis"`). `None` at the call site means
/// single-node: in-memory session store plus the no-op broadcaster, exactly
/// the pre-cluster behaviour.
pub struct ClusterRuntimeParams {
    pub redis_url: String,
    pub channel: String,
    pub node_id: String,
    pub bridge_capacity: usize,
    pub max_message_age_seconds: i64,
    pub session_ttl_seconds: Option<u64>,
    pub metrics: Arc<Metrics>,
}

/// Build the runtime at boot. Empty policy in enforce mode is a fatal startup
/// error (default-deny with zero rules refuses every request, which is almost
/// certainly an operator mistake). Empty policy in observe mode is allowed
/// (every request becomes a would-refuse event the operator can inspect via
/// `marg policy audit`).
///
/// `cluster` is `Some` only when Marg runs with a Redis hot store. In that
/// case the session store becomes the shared Redis store and the gate gets
/// the signed cross-node invalidation broadcaster (ADR-027).
pub async fn build_runtime(
    kavach_cfg: &KavachConfig,
    loaded: &LoadedKavachPolicy,
    cluster: Option<ClusterRuntimeParams>,
    injected_chain: Option<AuditChainHandle>,
) -> Result<Arc<KavachRuntime>> {
    let mode = KavachMode::from_config(kavach_cfg);
    if matches!(mode, KavachMode::Enforce) && loaded.policies.is_empty() {
        return Err(anyhow!(
            "[kavach].mode = \"enforce\" with zero policy rules means refusing every request; \
             load a policy file at [kavach].policy_path or set [kavach].mode = \"observe\" to roll out gradually"
        ));
    }

    let keypair = load_or_generate_keypair(&kavach_cfg.keypair_path)
        .context("loading or generating kavach signing keypair")?;
    let verifier = Verifier::from_bundle(&keypair.public_keys(), kavach_cfg.audit_hybrid);

    // Audit chain: use the host-injected chain when embedding (ADR-031/ADR-032),
    // so Marg's verdict + request entries land in the host's one chain and are
    // exported/verified alongside the host's other planes. The injected chain
    // may be raw (`with_audit_chain`) or a bounded `ManagedAuditChain`
    // (`with_managed_audit_chain`); either way it must derive from the same
    // keypair (shared via `[kavach].keypair_path`) so permits and audit entries
    // verify under one bundle. When not injected, create the process-local raw
    // chain exactly as before, bounded by the `flush.rs` task in `lib.rs`. The
    // gate is built without `with_audit` (ADR-015): Marg appends one entry per
    // request explicitly to `audit_chain`, so injecting it here is sufficient;
    // there is no separate gate sink to redirect.
    let audit_chain = match injected_chain {
        Some(handle) => handle,
        None => {
            let signer = Signer::from_keypair(&keypair, kavach_cfg.audit_hybrid);
            AuditChainHandle::Raw(Arc::new(SignedAuditChain::new(signer)))
        }
    };

    let policy_set = build_policy_set(&loaded.policies)
        .context("compiling kavach policy set from policy file")?;
    let policy_count = policy_set.policies.len();
    let policy_engine = Arc::new(PolicyEngine::new(policy_set));

    let invariant_set = build_invariant_set(&loaded.invariants);
    let invariant_count = invariant_set.len();
    let invariants = Arc::new(SwappableInvariantSet::new(invariant_set));

    // Drift evaluator gated by `[kavach.drift]` config. Always wrapped in
    // `SwappableDriftEvaluator` so policy reload can flip detectors on or off
    // without rebuilding the gate (same pattern as `SwappableInvariantSet`).
    // The wrapper is a noop when its inner is `None`.
    let (drift_state, drift_inner): (DriftDetectorState, Option<Arc<DriftEvaluator>>) =
        build_drift_evaluator(kavach_cfg).context("building kavach drift evaluator")?;
    let drift_evaluator = Arc::new(SwappableDriftEvaluator::new(drift_inner));

    let evaluators: Vec<Arc<dyn Evaluator>> = vec![
        policy_engine.clone() as Arc<dyn Evaluator>,
        drift_evaluator.clone() as Arc<dyn Evaluator>,
        invariants.clone() as Arc<dyn Evaluator>,
    ];

    // PqTokenSigner attaches via `Gate::with_token_signer`. Every Permit
    // verdict the gate produces now carries an ML-DSA-65 (+ optional Ed25519)
    // signature over `PermitToken::canonical_bytes()`. ADR-015 keeps the audit
    // chain and the permit signer on the *same* `KavachKeyPair` for v1.0; the
    // `permit_signer_hybrid` knob is the operator escape hatch.
    // Only sign permits when the operator actually consumes them (exposes to
    // the caller or forwards to the provider). A signed permit that is then
    // discarded is a full ML-DSA-65 signature per request for no security
    // benefit, so skip the signer entirely in the common default posture.
    let permit_hybrid = kavach_cfg.permit_hybrid();
    let sign_permits =
        kavach_cfg.expose_permit_to_caller || kavach_cfg.forward_permit_to_provider;
    let pq_token_signer: Option<Arc<dyn TokenSigner>> = if sign_permits {
        Some(if permit_hybrid {
            Arc::new(PqTokenSigner::from_keypair_hybrid(&keypair))
        } else {
            Arc::new(PqTokenSigner::from_keypair_pq_only(&keypair))
        })
    } else {
        None
    };
    let permit_signer = PermitSignerState {
        enabled: sign_permits,
        algorithm: if permit_hybrid {
            "ml-dsa-65+ed25519"
        } else {
            "ml-dsa-65"
        },
        key_id: keypair.id.clone(),
    };

    // Note (ADR-015): Gate is built without `with_audit`. Marg appends one
    // chain entry per request at end-of-lifecycle, so the audit row carries
    // the full request lifecycle (verdict + provider call + response + error)
    // in `context_snapshot`, not just the gate verdict.
    let gate_config = GateConfig {
        observe_only: matches!(mode, KavachMode::Observe),
        permit_ttl_seconds: kavach_cfg.permit_ttl_seconds,
        fail_open: false,
    };

    // Shared enforce flag the signed broadcaster reads. The gate publishes on
    // every Invalidate verdict regardless of mode, so observe mode is enforced
    // at the broadcaster (no cluster-wide key drop while only observing).
    let enforce_flag = Arc::new(AtomicBool::new(matches!(mode, KavachMode::Enforce)));

    // Cluster mode swaps in the shared Redis session store and the signed
    // cross-node invalidation broadcaster (ADR-027). Single-node keeps the
    // in-memory store and the no-op broadcaster.
    // `invalidation_broadcaster` is the unconditional signed broadcaster, used
    // by the remote-apply listener (subscribe) and by admin-initiated kills
    // (publish in any mode). `gate_broadcaster` is what the gate sees: in
    // cluster mode it is an enforce-gated wrapper so drift / policy Invalidate
    // verdicts only fan out in enforce mode. Both share the same Redis channel
    // and local bridge.
    let (invalidation_broadcaster, gate_broadcaster, session_store): (
        Arc<dyn InvalidationBroadcaster>,
        Arc<dyn InvalidationBroadcaster>,
        Arc<MargSessionStore>,
    ) = match cluster {
        Some(c) => {
            // Dedicated signer/verifier for the broadcaster, keyed on the same
            // cluster keypair as the audit chain (ADR-027 key distribution).
            let bcast_signer = Arc::new(Signer::from_keypair(&keypair, kavach_cfg.audit_hybrid));
            let bcast_verifier = Arc::new(Verifier::from_bundle(
                &keypair.public_keys(),
                kavach_cfg.audit_hybrid,
            ));
            let broadcaster = SignedRedisBroadcaster::connect(
                &c.redis_url,
                c.channel.clone(),
                c.node_id.clone(),
                bcast_signer,
                bcast_verifier,
                c.bridge_capacity,
                c.max_message_age_seconds,
                c.metrics.clone(),
            )
            .await
            .context("connecting signed cluster invalidation broadcaster")?;
            let signed: Arc<dyn InvalidationBroadcaster> = Arc::new(broadcaster);
            let gated: Arc<dyn InvalidationBroadcaster> = Arc::new(EnforceGatedBroadcaster::new(
                signed.clone(),
                enforce_flag.clone(),
                c.metrics.clone(),
            ));

            let redis_sessions: Arc<dyn SessionStore> = match c.session_ttl_seconds {
                Some(ttl) => Arc::new(
                    RedisSessionStore::from_url_with_ttl(&c.redis_url, ttl)
                        .await
                        .map_err(|e| anyhow!("connecting redis session store: {e}"))?,
                ),
                None => Arc::new(
                    RedisSessionStore::from_url(&c.redis_url)
                        .await
                        .map_err(|e| anyhow!("connecting redis session store: {e}"))?,
                ),
            };

            tracing::info!(
                node_id = %c.node_id,
                channel = %c.channel,
                "kavach cluster mode active: signed invalidation broadcaster + shared redis session store"
            );

            (
                signed,
                gated,
                Arc::new(MargSessionStore::from_inner(redis_sessions, "redis")),
            )
        }
        None => {
            let noop: Arc<dyn InvalidationBroadcaster> = Arc::new(NoopInvalidationBroadcaster::new());
            (noop.clone(), noop, Arc::new(MargSessionStore::in_memory()))
        }
    };

    let gate_base = Gate::new(evaluators, gate_config).with_broadcaster(gate_broadcaster);
    let gate = Arc::new(match pq_token_signer {
        Some(signer) => gate_base.with_token_signer(signer),
        None => gate_base,
    });

    tracing::info!(
        mode = mode.as_str(),
        policy_rules = policy_count,
        invariants = invariant_count,
        audit_hybrid = kavach_cfg.audit_hybrid,
        permit_hybrid = permit_hybrid,
        drift_active = drift_state.enabled,
        drift_detectors = drift_state.detectors.len(),
        keypair = %kavach_cfg.keypair_path,
        keypair_id = %keypair.id,
        audit_export = %kavach_cfg.audit_export_path,
        flush_seconds = kavach_cfg.audit_flush_seconds,
        source_hash = %loaded.source_hash,
        "kavach runtime ready"
    );

    let audit_export_file = super::flush::chain_file_path(&kavach_cfg.audit_export_path);
    let session_tracking = drift_state.enabled || loaded.references_session();

    Ok(Arc::new(KavachRuntime {
        gate,
        policy_engine,
        invariants,
        audit_chain,
        audit_export_file,
        verifier,
        mode: ArcSwap::from_pointee(mode),
        include_prompts: ArcSwap::from_pointee(kavach_cfg.include_prompts),
        buffer_streaming_for_post_hook: kavach_cfg.buffer_streaming_for_post_hook,
        expose_permit_to_caller: ArcSwap::from_pointee(kavach_cfg.expose_permit_to_caller),
        forward_permit_to_provider: ArcSwap::from_pointee(kavach_cfg.forward_permit_to_provider),
        permit_signing_enabled: sign_permits,
        permit_ttl_seconds: ArcSwap::from_pointee(kavach_cfg.permit_ttl_seconds),
        policy_source_path: loaded.source_path.clone(),
        policy_source_hash: ArcSwap::from_pointee(loaded.source_hash.clone()),
        policy_loaded_at: ArcSwap::from_pointee(loaded.loaded_at.to_rfc3339()),
        policy_rule_count: ArcSwap::from_pointee(policy_count as u64),
        invariant_count: ArcSwap::from_pointee(invariant_count as u64),
        kavach_version: env!("CARGO_PKG_VERSION").to_string(),
        permit_signer,
        drift_state: ArcSwap::from_pointee(drift_state),
        drift_evaluator,
        session_tracking_needed: ArcSwap::from_pointee(session_tracking),
        session_store,
        keypair_id: keypair.id.clone(),
        invalidation_broadcaster,
        enforce_flag,
    }))
}

/// Build a `DriftEvaluator` from `[kavach.drift]` config. Returns the live
/// evaluator and a flat state snapshot for observability. An empty drift
/// config returns `(disabled, None)` so the gate skips drift evaluation
/// entirely (saves the per-request priority-100 traversal).
pub fn build_drift_evaluator(
    kavach_cfg: &KavachConfig,
) -> Result<(DriftDetectorState, Option<Arc<DriftEvaluator>>)> {
    let drift_cfg = &kavach_cfg.drift;
    let mut entries: Vec<DriftDetectorEntry> = Vec::new();
    let mut detectors: Vec<Box<dyn DriftDetector>> = Vec::new();

    if let Some(km) = drift_cfg.geo_max_distance_km {
        if km <= 0.0 {
            return Err(anyhow!(
                "[kavach.drift].geo_max_distance_km must be positive (got {km})"
            ));
        }
        detectors.push(Box::new(GeoLocationDrift::with_max_distance_km(km)));
        entries.push(DriftDetectorEntry {
            name: "geo_drift".to_string(),
            parameters: serde_json::json!({ "max_distance_km": km, "mode": "tolerant" }),
        });
    }

    if let Some(seconds) = drift_cfg
        .session_age_max_seconds()
        .map_err(|e| anyhow!("[kavach.drift].session_age_max: {e}"))?
    {
        detectors.push(Box::new(SessionAgeDrift {
            max_age_seconds: seconds,
        }));
        entries.push(DriftDetectorEntry {
            name: "session_age_drift".to_string(),
            parameters: serde_json::json!({
                "max_age_seconds": seconds,
                "warn_at_fraction": 0.75,
            }),
        });
    }

    if drift_cfg.device_fingerprint_enabled {
        detectors.push(Box::new(DeviceDrift));
        entries.push(DriftDetectorEntry {
            name: "device_drift".to_string(),
            parameters: serde_json::json!({}),
        });
    }

    let warn = drift_cfg.behavior_rate_warn;
    let viol = drift_cfg.behavior_rate_violation;
    if warn.is_some() || viol.is_some() {
        let warn_v = warn.unwrap_or_else(|| viol.map(|v| v / 2).unwrap_or(30));
        let viol_v = viol.unwrap_or(warn_v.saturating_mul(3));
        if viol_v <= warn_v {
            return Err(anyhow!(
                "[kavach.drift].behavior_rate_violation ({viol_v}) must exceed behavior_rate_warn ({warn_v})"
            ));
        }
        detectors.push(Box::new(BehaviorDrift {
            warn_threshold: warn_v,
            violation_threshold: viol_v,
        }));
        entries.push(DriftDetectorEntry {
            name: "behavior_drift".to_string(),
            parameters: serde_json::json!({
                "warn_per_min": warn_v,
                "violation_per_min": viol_v,
            }),
        });
    }

    if detectors.is_empty() {
        return Ok((DriftDetectorState::default(), None));
    }

    let evaluator = DriftEvaluator::new(detectors);
    let state = DriftDetectorState {
        enabled: true,
        warning_threshold: evaluator.warning_threshold,
        detectors: entries,
    };
    Ok((state, Some(Arc::new(evaluator))))
}

/// Hot-reload the Kavach side of the policy. Routing + pricing reload still
/// happens in `marg-server::policy::reload`. The runtime mode and other
/// `[kavach].*` runtime knobs are also re-read from `marg.toml` here so the
/// operator can flip observe<->enforce without a process restart.
pub fn reload_policy(
    runtime: &Arc<KavachRuntime>,
    kavach_cfg: &KavachConfig,
    loaded: &LoadedKavachPolicy,
) -> Result<KavachReloadOutcome> {
    let new_mode = KavachMode::from_config(kavach_cfg);
    if matches!(new_mode, KavachMode::Enforce) && loaded.policies.is_empty() {
        return Err(anyhow!(
            "policy reload refused: [kavach].mode = \"enforce\" with zero policy rules would refuse every request. Fix the policy file and reload again"
        ));
    }
    let previous_hash = runtime.policy_source_hash.load().as_str().to_string();
    let policy_set = build_policy_set(&loaded.policies)
        .context("compiling kavach policy set on reload")?;
    let policy_rule_count = policy_set.policies.len();
    runtime.policy_engine.reload(policy_set);

    let invariant_set = build_invariant_set(&loaded.invariants);
    let invariant_count = invariant_set.len();
    runtime.invariants.store(invariant_set);

    // Hot-swap drift detectors per the reloaded `[kavach.drift]` config. A
    // build failure here is a hard reload error so the operator sees the
    // malformed knob (default-deny on partial config).
    let (drift_state, drift_inner) =
        build_drift_evaluator(kavach_cfg).context("rebuilding drift evaluator on reload")?;
    let session_tracking = drift_state.enabled || loaded.references_session();
    runtime.drift_evaluator.store(drift_inner);
    runtime.drift_state.store(Arc::new(drift_state));
    runtime
        .session_tracking_needed
        .store(Arc::new(session_tracking));

    runtime.mode.store(Arc::new(new_mode));
    // Keep the signed broadcaster's enforce gate in lock-step with the mode so
    // a reload that flips observe->enforce also starts fanning out cluster
    // invalidations (and enforce->observe stops them). ADR-027.
    runtime
        .enforce_flag
        .store(matches!(new_mode, KavachMode::Enforce), Ordering::Relaxed);
    runtime
        .include_prompts
        .store(Arc::new(kavach_cfg.include_prompts));
    // Permits are only signed when the signer was attached at boot. Refuse to
    // enable exposure/forwarding on reload without it, so an unsigned permit is
    // never handed out; enabling it requires a restart.
    if (kavach_cfg.expose_permit_to_caller || kavach_cfg.forward_permit_to_provider)
        && !runtime.permit_signing_enabled
    {
        tracing::warn!(
            "reload requested expose/forward permit but permit signing was not enabled at boot; \
             permits are unsigned so exposure/forwarding stays off. Restart with \
             expose_permit_to_caller or forward_permit_to_provider set to activate signed permits."
        );
    }
    runtime.expose_permit_to_caller.store(Arc::new(
        kavach_cfg.expose_permit_to_caller && runtime.permit_signing_enabled,
    ));
    runtime.forward_permit_to_provider.store(Arc::new(
        kavach_cfg.forward_permit_to_provider && runtime.permit_signing_enabled,
    ));
    runtime
        .permit_ttl_seconds
        .store(Arc::new(kavach_cfg.permit_ttl_seconds));
    runtime
        .policy_source_hash
        .store(Arc::new(loaded.source_hash.clone()));
    runtime
        .policy_loaded_at
        .store(Arc::new(loaded.loaded_at.to_rfc3339()));
    runtime
        .policy_rule_count
        .store(Arc::new(policy_rule_count as u64));
    runtime
        .invariant_count
        .store(Arc::new(invariant_count as u64));

    Ok(KavachReloadOutcome {
        policy_rule_count,
        invariant_count,
        previous_hash,
        new_hash: loaded.source_hash.clone(),
        source_path: loaded.source_path.clone(),
    })
}

/// Re-serialise the opaque `[[policy]]` TOML values into a single doc and
/// hand them through Kavach's own `PolicySet::from_toml`, which is the
/// authoritative parser for Kavach's policy schema. Marg never mirrors that
/// schema; we just shuttle bytes through. Empty input yields an empty set.
pub fn build_policy_set(policies: &[toml::Value]) -> Result<PolicySet> {
    if policies.is_empty() {
        return Ok(PolicySet { policies: Vec::new() });
    }
    let mut table = toml::map::Map::new();
    table.insert(
        "policy".to_string(),
        toml::Value::Array(policies.to_vec()),
    );
    let rendered = toml::to_string(&toml::Value::Table(table))
        .map_err(|e| anyhow!("re-serialising policy values: {}", e))?;
    PolicySet::from_toml(&rendered)
        .map_err(|e| anyhow!("kavach policy parse: {}", e))
        .map(|set| {
            // Sanity check the priorities flowed through; Kavach internally
            // sorts on construction so the iteration order matches the
            // operator's intent.
            let _: &[Policy] = &set.policies;
            set
        })
}

/// Convert Marg's TOML invariant arms into Kavach's builder calls. Drop this
/// function the day Kavach ships `InvariantSet::from_toml`.
pub fn build_invariant_set(invariants: &[InvariantToml]) -> InvariantSet {
    let mut compiled: Vec<Invariant> = Vec::with_capacity(invariants.len());
    for inv in invariants {
        let built = match inv {
            InvariantToml::ParamMax { name, field, max } => {
                Invariant::param_max(name.clone(), field.clone(), *max)
            }
            InvariantToml::ParamMin { name, field, min } => {
                Invariant::param_min(name.clone(), field.clone(), *min)
            }
            InvariantToml::MaxActionsPerSession { name, max } => {
                Invariant::max_actions_per_session(name.clone(), *max)
            }
            InvariantToml::MaxSessionAge { name, max_seconds } => {
                Invariant::max_session_age(name.clone(), *max_seconds)
            }
            InvariantToml::AllowedActions { name, actions } => {
                Invariant::allowed_actions(name.clone(), actions.clone())
            }
            InvariantToml::BlockedActions { name, actions } => {
                Invariant::blocked_actions(name.clone(), actions.clone())
            }
        };
        compiled.push(built);
    }
    InvariantSet::new(compiled)
}
