//! Kavach integration (P09).
//!
//! Builds the runtime gate, signed audit chain, swappable policy + invariant
//! evaluators, and the per-request lifecycle audit emitter. Mandatory: every
//! Marg binary boots with this module wired in (ADR-011). Mode (observe vs
//! enforce) is a runtime flag, not a build flag, so the same binary can ship
//! everywhere and operators promote to enforce after tuning the policy.

mod audit_target;
mod context;
mod flush;
mod invalidation;
mod keys;
mod lifecycle;
mod runtime;
mod session_store;
mod sink;

pub use audit_target::AuditChainHandle;
pub use context::{action_context_from_request, parse_caller_headers, RequestLifecycle};
pub use flush::{spawn_audit_flush_task, AuditFlushTaskHandle};
pub use invalidation::{
    cluster_invalidation_scope, spawn_remote_invalidation_listener, EnforceGatedBroadcaster,
    SignedRedisBroadcaster,
};
pub use keys::{load_or_generate_keypair, MargKavachKeyFile};
pub use lifecycle::{
    audit_request_lifecycle, emit_key_event, emit_policy_reload, encode_permit_header,
    verdict_kind_str, KeyEventKind,
};
pub use runtime::{
    build_drift_evaluator, build_invariant_set, build_policy_set, build_runtime, reload_policy,
    ClusterRuntimeParams, DriftDetectorEntry, DriftDetectorState, KavachMode, KavachReloadOutcome,
    KavachRuntime, PermitSignerState, SwappableDriftEvaluator, SwappableInvariantSet,
};
pub use session_store::{CallerHeaders, MargSessionStore};
pub use sink::SignedChainSink;
