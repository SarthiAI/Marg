//! Kavach configuration and policy-file types.
//!
//! `marg-core` carries only the operator-visible shape (TOML structs, file
//! parsers, hash helpers). The compiled `Gate`, `SignedAuditChain`, and
//! `Invariant` instances live in `marg-server`, which depends on `kavach-core`
//! and `kavach-pq` directly. This split keeps `marg-core` free of any Kavach
//! Cargo dependency, the CLI and other thin consumers can read the same config
//! without pulling the post-quantum crypto stack.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use crate::error::ConfigError;

/// `[kavach]` block in `marg.toml`. Operations knobs only; policy rules and
/// invariants live in the file named by `policy_path` (or inline at the
/// top of `marg.toml` for single-operator dev). See ADR-014 / ADR-015.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KavachConfig {
    /// `"observe"` (gate evaluates and logs would-refuse events but never
    /// blocks) or `"enforce"` (default-deny: refused requests return 403).
    /// First-boot default per ADR-003 is observe.
    #[serde(default = "default_mode")]
    pub mode: String,

    /// Path to the policy file or directory. Three accepted shapes (see
    /// ADR-014):
    ///   - `"/etc/marg/policy.toml"` (single file)
    ///   - `"/etc/marg/policies/"` (directory of `*.toml` files, read in
    ///     lexicographic order)
    ///   - omit and write `[[policy]]` / `[[invariant]]` blocks directly in
    ///     `marg.toml` for dev.
    #[serde(default)]
    pub policy_path: Option<String>,

    /// Path to the Marg Kavach signing keypair. Generated on first boot
    /// (mode 0640) if missing. Operator owns rotation.
    #[serde(default = "default_keypair_path")]
    pub keypair_path: String,

    /// Directory for periodic signed audit-chain exports. One JSONL file per
    /// process lifetime in v1.0; cross-restart chain merging is a v1.1
    /// concern documented in `docs/cluster-deployment.md`.
    #[serde(default = "default_audit_export_path")]
    pub audit_export_path: String,

    /// Seconds between background flushes of new signed entries to disk.
    /// Default 60 keeps the crash-loss bound at one minute of traffic.
    #[serde(default = "default_audit_flush_seconds")]
    pub audit_flush_seconds: u64,

    /// `true` (default) signs every audit entry with ML-DSA-65 + Ed25519
    /// (hybrid). `false` signs with ML-DSA-65 only. ChainMode is fixed at
    /// chain construction and enforced by Kavach's verifier; flipping this
    /// across restarts produces a fresh chain.
    #[serde(default = "default_audit_hybrid")]
    pub audit_hybrid: bool,

    /// If `true`, the audit `context_snapshot` includes the raw prompt text
    /// for forensic replay. Default `false` to keep privacy-sensitive content
    /// out of the signed chain unless the operator opts in.
    #[serde(default)]
    pub include_prompts: bool,

    /// If `true`, every permitted request includes an `X-Kavach-Permit`
    /// response header (base64-url of the signed `PermitToken` JSON) so the
    /// caller can pass the permit downstream. Default `false`.
    #[serde(default)]
    pub expose_permit_to_caller: bool,

    /// If `true`, the inbound `PermitToken` is also forwarded to the upstream
    /// provider as an `X-Kavach-Permit` header. Default `false` (most
    /// upstream providers do not consume this).
    #[serde(default)]
    pub forward_permit_to_provider: bool,

    /// Permit TTL in seconds. Kavach's library default is 30s; Marg surfaces
    /// it here so operators with slow upstream regions can raise it without
    /// rebuilding.
    #[serde(default = "default_permit_ttl_seconds")]
    pub permit_ttl_seconds: u64,

    /// If set, overrides `audit_hybrid` for permit token signing. Operators
    /// that need to run the audit chain in hybrid mode (ML-DSA-65 + Ed25519)
    /// but PQ-only permit tokens (or vice versa) set this explicitly. Default
    /// `None` means permit signing inherits `audit_hybrid`.
    #[serde(default)]
    pub permit_signer_hybrid: Option<bool>,

    /// Drift-detector tuning. Empty means every detector stays inert.
    #[serde(default)]
    pub drift: KavachDriftConfig,
}

fn default_mode() -> String { "observe".to_string() }
fn default_keypair_path() -> String { "/etc/marg/marg.key".to_string() }
fn default_audit_export_path() -> String { "/var/lib/marg/audit/".to_string() }
fn default_audit_flush_seconds() -> u64 { 60 }
fn default_audit_hybrid() -> bool { true }
fn default_permit_ttl_seconds() -> u64 { 30 }

impl Default for KavachConfig {
    fn default() -> Self {
        Self {
            mode: default_mode(),
            policy_path: None,
            keypair_path: default_keypair_path(),
            audit_export_path: default_audit_export_path(),
            audit_flush_seconds: default_audit_flush_seconds(),
            audit_hybrid: default_audit_hybrid(),
            include_prompts: false,
            expose_permit_to_caller: false,
            forward_permit_to_provider: false,
            permit_ttl_seconds: default_permit_ttl_seconds(),
            permit_signer_hybrid: None,
            drift: KavachDriftConfig::default(),
        }
    }
}

impl KavachConfig {
    /// `"enforce"` after lower-casing and trimming.
    pub fn is_enforce(&self) -> bool {
        self.mode.trim().eq_ignore_ascii_case("enforce")
    }

    /// Whether permit signing should produce a hybrid ML-DSA + Ed25519
    /// envelope. Falls back to `audit_hybrid` when the explicit knob is unset.
    pub fn permit_hybrid(&self) -> bool {
        self.permit_signer_hybrid.unwrap_or(self.audit_hybrid)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KavachDriftConfig {
    /// Maximum cumulative geo distance (km) before drift triggers. None
    /// disables the geo drift detector. When set, the geo detector runs in
    /// tolerant mode: drift within the threshold is a warning, beyond the
    /// threshold is a violation.
    #[serde(default)]
    pub geo_max_distance_km: Option<f64>,
    /// Maximum session age before drift triggers, e.g. `"24h"`, `"30m"`,
    /// `"3600s"`. None disables the session-age detector.
    #[serde(default)]
    pub session_age_max: Option<String>,
    /// Behavior rate (actions/minute) that triggers a warning. None disables
    /// the warning band on the behavior detector.
    #[serde(default)]
    pub behavior_rate_warn: Option<u64>,
    /// Behavior rate (actions/minute) that triggers a hard violation. None
    /// disables the behavior detector entirely.
    #[serde(default)]
    pub behavior_rate_violation: Option<u64>,
    /// If `true`, the device fingerprint detector is enabled. Operators set
    /// this to `true` when their load balancer attaches an
    /// `x-marg-device-fingerprint` header so Marg can compare origin vs current
    /// device hashes per session. Default `false` (detector inert).
    #[serde(default)]
    pub device_fingerprint_enabled: bool,
}

impl KavachDriftConfig {
    /// Parse `session_age_max` ("24h", "30m", "3600s", or a bare seconds
    /// integer) into a seconds count. Returns `Ok(None)` when the field is
    /// unset. Returns `Err` when the field is set but the format is invalid,
    /// in which case the operator's intent is unclear and refusing to boot is
    /// safer than silently disabling the detector.
    pub fn session_age_max_seconds(&self) -> Result<Option<i64>, String> {
        let Some(raw) = self.session_age_max.as_ref() else {
            return Ok(None);
        };
        let s = raw.trim();
        if s.is_empty() {
            return Ok(None);
        }
        // Allow plain integer seconds.
        if let Ok(n) = s.parse::<i64>() {
            if n <= 0 {
                return Err(format!("session_age_max '{}' must be positive", raw));
            }
            return Ok(Some(n));
        }
        let (num_part, unit) = s.split_at(s.len().saturating_sub(1));
        let n: i64 = num_part
            .parse()
            .map_err(|_| format!("session_age_max '{}' is not numeric", raw))?;
        if n <= 0 {
            return Err(format!("session_age_max '{}' must be positive", raw));
        }
        let multiplier = match unit {
            "s" | "S" => 1,
            "m" | "M" => 60,
            "h" | "H" => 60 * 60,
            "d" | "D" => 60 * 60 * 24,
            _ => return Err(format!("session_age_max '{}' uses an unknown unit (expected s/m/h/d)", raw)),
        };
        Ok(Some(n * multiplier))
    }
}

/// Marg-side TOML shape for `[[invariant]]` blocks. Kavach 0.1.0 ships
/// `Invariant::*` builders but no TOML loader; this enum mirrors the builder
/// surface and `marg-server::kavach` converts each variant into the
/// corresponding `kavach_core::Invariant` at boot and on every policy reload.
/// Drop the parser when Kavach ships its own `InvariantSet::from_toml`.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InvariantToml {
    ParamMax { name: String, field: String, max: f64 },
    ParamMin { name: String, field: String, min: f64 },
    MaxActionsPerSession { name: String, max: u64 },
    MaxSessionAge { name: String, max_seconds: i64 },
    AllowedActions { name: String, actions: Vec<String> },
    BlockedActions { name: String, actions: Vec<String> },
}

/// Combined view of the policy source on disk. `policies` holds the
/// `[[policy]]` blocks as opaque `toml::Value`s so the Kavach side can
/// re-serialise them and feed them through its own `PolicySet::from_toml`,
/// without Marg duplicating Kavach's policy schema. `invariants` is the
/// Marg-side TOML enum that gets turned into a `kavach_core::InvariantSet` on
/// the server side.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct KavachPolicyFile {
    #[serde(rename = "policy", alias = "policies", default)]
    pub policies: Vec<toml::Value>,
    #[serde(rename = "invariant", alias = "invariants", default)]
    pub invariants: Vec<InvariantToml>,
}

#[derive(Debug, Clone)]
pub struct LoadedKavachPolicy {
    pub policies: Vec<toml::Value>,
    pub invariants: Vec<InvariantToml>,
    /// SHA-256 over the concatenated bytes of every file that contributed to
    /// this policy set. Reported in `marg.policy_reload.v1` audit entries so
    /// an auditor can prove which exact bytes were in effect at any given
    /// time.
    pub source_hash: String,
    /// Where the policy came from. `None` means the inline fallback in
    /// `marg.toml`. `Some(path)` is the file or directory the operator set
    /// in `[kavach].policy_path`.
    pub source_path: Option<PathBuf>,
    /// When the load happened.
    pub loaded_at: DateTime<Utc>,
}

impl LoadedKavachPolicy {
    /// Build a `[[policy]]` TOML document from the opaque policy values, so
    /// Kavach's `PolicySet::from_toml` can ingest it. Returns an empty string
    /// when there are no policies (the caller decides whether that is a
    /// fatal startup condition based on the active mode).
    pub fn render_policy_toml(&self) -> String {
        if self.policies.is_empty() {
            return String::new();
        }
        let mut table = toml::map::Map::new();
        table.insert(
            "policy".to_string(),
            toml::Value::Array(self.policies.clone()),
        );
        toml::to_string(&toml::Value::Table(table)).unwrap_or_default()
    }
}

/// Load the Kavach policy source, in this precedence order (ADR-014):
///
/// 1. If `policy_path` is a directory, read every `*.toml` file in
///    lexicographic order and concatenate the `[[policy]]` and
///    `[[invariant]]` arrays. Lex order is the conflict-resolution order;
///    document the convention so teams know `00-baseline.toml` outranks
///    `90-team-x.toml`.
/// 2. If `policy_path` is a single file, read it.
/// 3. If `policy_path` is unset, fall back to the inline `policy` /
///    `invariant` arrays at the top of `marg.toml` (`inline_policies` /
///    `inline_invariants` here). Logged at warn if both an inline block and
///    `policy_path` are set, so a misconfigured deployment surfaces the
///    dead inline rules.
pub fn load_kavach_policy(
    kavach: &KavachConfig,
    inline_policies: &[toml::Value],
    inline_invariants: &[InvariantToml],
) -> Result<LoadedKavachPolicy, ConfigError> {
    let now = Utc::now();
    match &kavach.policy_path {
        Some(path_str) => {
            let path = PathBuf::from(path_str);
            if !path.exists() {
                return Err(ConfigError::Validation(format!(
                    "[kavach].policy_path '{}' does not exist",
                    path_str
                )));
            }
            if !inline_policies.is_empty() || !inline_invariants.is_empty() {
                tracing::warn!(
                    path = %path_str,
                    "[kavach].policy_path is set, ignoring inline policy / invariant blocks in marg.toml"
                );
            }
            if path.is_dir() {
                load_from_dir(&path).map(|mut v| {
                    v.loaded_at = now;
                    v
                })
            } else {
                load_from_file(&path).map(|mut v| {
                    v.loaded_at = now;
                    v
                })
            }
        }
        None => {
            // Inline fallback: hash the serialised inline arrays for the
            // `marg.policy_reload.v1` snapshot.
            let mut hasher = Sha256::new();
            for p in inline_policies {
                if let Ok(s) = toml::to_string(p) {
                    hasher.update(s.as_bytes());
                }
            }
            for inv in inline_invariants {
                if let Ok(s) = serde_json::to_string(inv) {
                    hasher.update(s.as_bytes());
                }
            }
            let source_hash = data_encoding::HEXLOWER.encode(&hasher.finalize());
            Ok(LoadedKavachPolicy {
                policies: inline_policies.to_vec(),
                invariants: inline_invariants.to_vec(),
                source_hash,
                source_path: None,
                loaded_at: now,
            })
        }
    }
}

fn load_from_file(path: &Path) -> Result<LoadedKavachPolicy, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|err| ConfigError::Io {
        path: path.display().to_string(),
        source: err,
    })?;
    let parsed: KavachPolicyFile = toml::from_str(&text)?;
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let source_hash = data_encoding::HEXLOWER.encode(&hasher.finalize());
    Ok(LoadedKavachPolicy {
        policies: parsed.policies,
        invariants: parsed.invariants,
        source_hash,
        source_path: Some(path.to_path_buf()),
        loaded_at: Utc::now(),
    })
}

fn load_from_dir(path: &Path) -> Result<LoadedKavachPolicy, ConfigError> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
        .map_err(|err| ConfigError::Io {
            path: path.display().to_string(),
            source: err,
        })?
        .filter_map(|res| res.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|s| s.to_str())
                    .map(|ext| ext.eq_ignore_ascii_case("toml"))
                    .unwrap_or(false)
        })
        .collect();
    entries.sort();

    let mut policies: Vec<toml::Value> = Vec::new();
    let mut invariants: Vec<InvariantToml> = Vec::new();
    let mut hasher = Sha256::new();

    for entry in &entries {
        let text = std::fs::read_to_string(entry).map_err(|err| ConfigError::Io {
            path: entry.display().to_string(),
            source: err,
        })?;
        hasher.update(entry.display().to_string().as_bytes());
        hasher.update(b"\0");
        hasher.update(text.as_bytes());
        let parsed: KavachPolicyFile = toml::from_str(&text)?;
        policies.extend(parsed.policies);
        invariants.extend(parsed.invariants);
    }

    let source_hash = data_encoding::HEXLOWER.encode(&hasher.finalize());
    Ok(LoadedKavachPolicy {
        policies,
        invariants,
        source_hash,
        source_path: Some(path.to_path_buf()),
        loaded_at: Utc::now(),
    })
}
