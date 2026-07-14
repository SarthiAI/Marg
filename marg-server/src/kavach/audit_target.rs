//! `AuditChainHandle`: the internal seam that lets Marg's audit append + read
//! path work over either the raw `SignedAuditChain` or the bounded, self-pruning
//! `ManagedAuditChain`.
//!
//! Marg keeps every audit write behind this handle so the choice of chain shape
//! lives in exactly one place. Standalone (`run()`) and the raw-chain injection
//! (`with_audit_chain`) use `Raw`; an embedding host that injects a
//! `ManagedAuditChain` via `with_managed_audit_chain` uses `Managed`. A
//! `Managed` chain persists old entries through its own sink and prunes them
//! from memory on a background worker, so Marg does not run its `flush.rs` task
//! for it. A `Raw` chain is bounded by that flush task only when Marg created it
//! itself (the internal case); an injected raw chain is the host's to persist.
//! See ADR-031 and ADR-032.

use std::sync::Arc;

use kavach_core::audit::AuditEntry;
use kavach_pq::error::Result as PqResult;
use kavach_pq::{ManagedAuditChain, SignedAuditChain, SignedAuditEntry};

/// Either the raw signed chain or a bounded managed chain, behind one append +
/// read surface. Cheap to clone (an `Arc` bump).
#[derive(Clone)]
pub enum AuditChainHandle {
    /// The process-local or host-injected raw chain. Bounded by `flush.rs` only
    /// when Marg created it itself; a host-injected raw chain is the host's to
    /// persist.
    Raw(Arc<SignedAuditChain>),
    /// A host-injected bounded chain that persists + prunes itself.
    Managed(Arc<ManagedAuditChain>),
}

impl AuditChainHandle {
    /// Append one signed entry. Mirrors `SignedAuditChain::append` /
    /// `ManagedAuditChain::append`; the managed chain also applies its
    /// configured backpressure before accepting the entry.
    pub fn append(&self, entry: &AuditEntry) -> PqResult<SignedAuditEntry> {
        match self {
            AuditChainHandle::Raw(c) => c.append(entry),
            AuditChainHandle::Managed(c) => c.append(entry),
        }
    }

    /// Head hash of the full logical chain.
    pub fn head_hash(&self) -> String {
        match self {
            AuditChainHandle::Raw(c) => c.head_hash(),
            AuditChainHandle::Managed(c) => c.head_hash(),
        }
    }

    /// Total entries ever appended.
    pub fn len(&self) -> u64 {
        match self {
            AuditChainHandle::Raw(c) => c.len(),
            AuditChainHandle::Managed(c) => c.len(),
        }
    }

    /// Whether nothing has been appended yet.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Logical index of the oldest entry still resident in memory.
    pub fn base_index(&self) -> u64 {
        match self {
            AuditChainHandle::Raw(c) => c.base_index(),
            AuditChainHandle::Managed(c) => c.base_index(),
        }
    }

    /// The in-memory tail (`[base_index, len)`).
    pub fn entries(&self) -> Vec<SignedAuditEntry> {
        match self {
            AuditChainHandle::Raw(c) => c.entries(),
            AuditChainHandle::Managed(c) => c.chain().entries(),
        }
    }

    /// The in-memory entries from logical index `from`.
    pub fn entries_since(&self, from: u64) -> PqResult<Vec<SignedAuditEntry>> {
        match self {
            AuditChainHandle::Raw(c) => c.entries_since(from),
            AuditChainHandle::Managed(c) => c.chain().entries_since(from),
        }
    }

    /// `(resident_entries, resident_bytes)` for the in-memory window.
    pub fn resident_stats(&self) -> (u64, u64) {
        match self {
            AuditChainHandle::Raw(c) => c.resident_stats(),
            AuditChainHandle::Managed(c) => c.chain().resident_stats(),
        }
    }

    /// The inner raw chain, if this is a `Raw` handle. Used only to hand the
    /// process-local chain to the `flush.rs` task; a managed chain self-prunes
    /// and returns `None`.
    pub fn raw_arc(&self) -> Option<Arc<SignedAuditChain>> {
        match self {
            AuditChainHandle::Raw(c) => Some(c.clone()),
            AuditChainHandle::Managed(_) => None,
        }
    }
}
