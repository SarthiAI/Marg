//! `SignedChainSink`: a thin wrapper that lets Kavach's `AuditSink` trait
//! drop entries straight into the post-quantum signed chain.
//!
//! Marg does **not** plumb this into `Gate::with_audit` for the per-evaluation
//! verdict (ADR-015). The chain is appended explicitly at end-of-request with
//! the full lifecycle packed into `context_snapshot`. The trait impl here is
//! kept for the rare case where a non-request lifecycle event wants to flow
//! through the standard `AuditSink` surface (e.g. a custom future evaluator).
//!
//! The append call is synchronous on `SignedAuditChain` (it computes one
//! ML-DSA signature plus optional Ed25519 in hybrid mode and one SHA-256 hash
//! chain link). We spawn it on `tokio::task::spawn_blocking` so a slow
//! signature does not stall the request runtime, though in practice
//! ML-DSA-65 signing on modern hardware is well under a millisecond.

use async_trait::async_trait;
use kavach_core::audit::{AuditEntry, AuditSink};
use kavach_core::error::KavachError;
use kavach_pq::SignedAuditChain;
use std::sync::Arc;

pub struct SignedChainSink {
    chain: Arc<SignedAuditChain>,
}

impl SignedChainSink {
    pub fn new(chain: Arc<SignedAuditChain>) -> Self {
        Self { chain }
    }
}

#[async_trait]
impl AuditSink for SignedChainSink {
    async fn record(&self, entry: AuditEntry) -> Result<(), KavachError> {
        let chain = self.chain.clone();
        let join = tokio::task::spawn_blocking(move || chain.append(&entry)).await;
        match join {
            Ok(Ok(_signed)) => Ok(()),
            Ok(Err(e)) => Err(KavachError::Serialization(format!(
                "signed audit chain append: {}",
                e
            ))),
            Err(e) => Err(KavachError::Serialization(format!(
                "signed audit chain task join: {}",
                e
            ))),
        }
    }
}
