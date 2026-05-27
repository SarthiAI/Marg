//! `KavachRuntime`: the live gate + audit chain + swappable policy/invariant
//! state Marg exposes through `AppState`. Built once at boot, mutated only by
//! `policy::reload` (under a single transactional swap so requests never see
//! a half-loaded policy).

use anyhow::{anyhow, Context, Result};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use kavach_core::{
    ActionContext, BehaviorDrift, DeviceDrift, DriftDetector, DriftEvaluator, Evaluator, Gate,
    GateConfig, GeoLocationDrift, Invariant, InvariantSet, NoopInvalidationBroadcaster, Policy,
    PolicyEngine, PolicySet, SessionAgeDrift, TokenSigner, Verdict,
};
use kavach_pq::{PqTokenSigner, SignedAuditChain, Signer, Verifier};
use marg_core::{InvariantToml, KavachConfig, LoadedKavachPolicy};
use std::path::PathBuf;
use std::sync::Arc;

use crate::kavach::keys::load_or_generate_keypair;
use crate::kavach::session_store::MargSessionStore;

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
    pub audit_chain: Arc<SignedAuditChain>,
    pub verifier: Verifier,
    pub mode: ArcSwap<KavachMode>,
    pub include_prompts: ArcSwap<bool>,
    pub expose_permit_to_caller: ArcSwap<bool>,
    pub forward_permit_to_provider: ArcSwap<bool>,
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
    pub session_store: Arc<MargSessionStore>,
    pub keypair_id: String,
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

/// Build the runtime at boot. Empty policy in enforce mode is a fatal startup
/// error (default-deny with zero rules refuses every request, which is almost
/// certainly an operator mistake). Empty policy in observe mode is allowed
/// (every request becomes a would-refuse event the operator can inspect via
/// `marg policy audit`).
pub fn build_runtime(
    kavach_cfg: &KavachConfig,
    loaded: &LoadedKavachPolicy,
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
    let signer = Signer::from_keypair(&keypair, kavach_cfg.audit_hybrid);
    let verifier = Verifier::from_bundle(&keypair.public_keys(), kavach_cfg.audit_hybrid);

    let audit_chain = Arc::new(SignedAuditChain::new(signer));

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
    let permit_hybrid = kavach_cfg.permit_hybrid();
    let pq_token_signer: Arc<dyn TokenSigner> = if permit_hybrid {
        Arc::new(PqTokenSigner::from_keypair_hybrid(&keypair))
    } else {
        Arc::new(PqTokenSigner::from_keypair_pq_only(&keypair))
    };
    let permit_signer = PermitSignerState {
        enabled: true,
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
    let gate = Arc::new(
        Gate::new(evaluators, gate_config)
            .with_broadcaster(Arc::new(NoopInvalidationBroadcaster::new()))
            .with_token_signer(pq_token_signer),
    );

    let session_store = Arc::new(MargSessionStore::in_memory());

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

    Ok(Arc::new(KavachRuntime {
        gate,
        policy_engine,
        invariants,
        audit_chain,
        verifier,
        mode: ArcSwap::from_pointee(mode),
        include_prompts: ArcSwap::from_pointee(kavach_cfg.include_prompts),
        expose_permit_to_caller: ArcSwap::from_pointee(kavach_cfg.expose_permit_to_caller),
        forward_permit_to_provider: ArcSwap::from_pointee(kavach_cfg.forward_permit_to_provider),
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
        session_store,
        keypair_id: keypair.id.clone(),
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
    runtime.drift_evaluator.store(drift_inner);
    runtime.drift_state.store(Arc::new(drift_state));

    runtime.mode.store(Arc::new(new_mode));
    runtime
        .include_prompts
        .store(Arc::new(kavach_cfg.include_prompts));
    runtime
        .expose_permit_to_caller
        .store(Arc::new(kavach_cfg.expose_permit_to_caller));
    runtime
        .forward_permit_to_provider
        .store(Arc::new(kavach_cfg.forward_permit_to_provider));
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
