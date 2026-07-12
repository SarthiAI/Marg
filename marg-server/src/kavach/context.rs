//! Build a Kavach `ActionContext` from a Marg request.
//!
//! Marg's request pipeline carries a `MargKey` (principal id + kind + team) and
//! a `ChatRequest` (model, max tokens, streaming flag, raw JSON). Kavach wants
//! a full `(Principal, ActionDescriptor, SessionState, EnvContext)` tuple. This
//! module is the one place that translation happens.

use axum::http::HeaderMap;
use chrono::Utc;
use kavach_core::{
    ActionContext, ActionDescriptor, DeviceFingerprint, GeoLocation, Principal as KavachPrincipal,
    PrincipalKind as KavachPrincipalKind, SessionState,
};
use marg_core::{MargKey, PrincipalKind as MargPrincipalKind};
use marg_providers::ChatRequest;
use serde_json::json;
use std::net::IpAddr;
use std::sync::Arc;
use uuid::Uuid;

use crate::kavach::session_store::{CallerHeaders, MargSessionStore};

/// Collected lifecycle facts for one Marg request. Filled as the request
/// flows through the pipeline; consumed by `audit_request_lifecycle` at the
/// end of the request to build the `marg.request.v1` chain entry.
#[derive(Debug, Clone)]
pub struct RequestLifecycle {
    pub request_id: String,
    pub principal_id: String,
    pub principal_kind: String,
    pub action_name: String,
    pub model: String,
    pub max_tokens: Option<u32>,
    pub estimated_cost_usd: f64,
    pub input_token_count: u64,
    pub streaming: bool,
    pub provider: Option<String>,
    pub route_model: Option<String>,
    pub provider_status: Option<u16>,
    pub failovers: u32,
    pub attempts: Vec<serde_json::Value>,
    pub response_input_tokens: u64,
    pub response_output_tokens: u64,
    pub response_actual_cost_usd: f64,
    pub response_latency_ms: u64,
    pub response_status: Option<u16>,
    pub response_client_disconnect: bool,
    pub error_class: Option<String>,
    pub error_message: Option<String>,
    pub prompt_redacted_or_omitted: bool,
}

impl RequestLifecycle {
    pub fn new_from_request(
        key: &MargKey,
        req: &ChatRequest,
        estimated_cost_usd: f64,
    ) -> Self {
        Self {
            request_id: Uuid::new_v4().to_string(),
            principal_id: key.principal.id.clone(),
            principal_kind: marg_principal_kind_str(&key.principal.kind).to_string(),
            action_name: action_name_for(&req.model),
            model: req.model.clone(),
            max_tokens: req.max_output_tokens,
            estimated_cost_usd,
            input_token_count: req.estimated_input_tokens,
            streaming: req.stream,
            provider: None,
            route_model: None,
            provider_status: None,
            failovers: 0,
            attempts: Vec::new(),
            response_input_tokens: 0,
            response_output_tokens: 0,
            response_actual_cost_usd: 0.0,
            response_latency_ms: 0,
            response_status: None,
            response_client_disconnect: false,
            error_class: None,
            error_message: None,
            prompt_redacted_or_omitted: true,
        }
    }
}

/// Build a Kavach `ActionContext` for the gate. Notes on the mapping:
///
/// - `Principal.id` is the Marg key's principal id (the human or service that
///   owns the API key), not the key id itself. The key id ends up in
///   `metadata.marg_key_id` for downstream traceability.
/// - `Action.name` is `marg.chat.<model>`, following the convention from
///   `architecture/kavach-integration.md`.
/// - `SessionState.session_id` is **derived from the key id** so every
///   request on the same Marg key shares one session row. The session row is
///   the durable origin of `origin_ip`, `origin_geo`, `origin_device`, and
///   `started_at`, which the drift evaluator compares against the current
///   request's environment on every call. Originally synthetic in P09 (no
///   persistence); P10 wires the real `MargSessionStore`-backed row.
/// - `params` carry the numeric and string facts policies and invariants
///   reference (`model`, `max_tokens`, `estimated_cost_usd`,
///   `input_token_count`, `streaming`, `team`).
pub async fn action_context_from_request(
    key: &MargKey,
    req: &ChatRequest,
    estimated_cost_usd: f64,
    headers: CallerHeaders,
    session_store: &Arc<MargSessionStore>,
    session_tracking: bool,
) -> ActionContext {
    let action_name = action_name_for(&req.model);

    let principal = KavachPrincipal {
        id: key.principal.id.clone(),
        kind: map_principal_kind(&key.principal.kind),
        roles: collect_roles(key),
        credentials_issued_at: key.created_at,
        display_name: None,
    };

    let mut action = ActionDescriptor::new(action_name)
        .with_param("model", json!(req.model.clone()))
        .with_param("max_tokens", json!(req.max_output_tokens.unwrap_or(0)))
        .with_param("estimated_cost_usd", json!(estimated_cost_usd))
        .with_param("input_token_count", json!(req.estimated_input_tokens))
        .with_param("streaming", json!(req.stream))
        .with_param("tool_count", json!(tool_count(&req.raw)));
    if let Some(team) = &key.team {
        action = action.with_param("team", json!(team));
    }

    let session_id = derive_session_id(&key.id);
    let environment = headers.into_env();
    let session = if session_tracking {
        match session_store
            .lookup_or_create_session(session_id, key.created_at, &environment)
            .await
        {
            Ok(s) => s,
            Err(err) => {
                // Fail-closed on session store outage by handing the gate a
                // *fresh* invalidated session: drift detectors that compare
                // origin-vs-current cannot compare, but the gate will refuse
                // because `session.invalidated == true`.
                tracing::error!(
                    ?err,
                    key_id = %key.id,
                    "session store unavailable; falling back to invalidated session row"
                );
                SessionState {
                    session_id,
                    started_at: Utc::now(),
                    action_count: 0,
                    action_history: Vec::new(),
                    invalidated: true,
                    origin_ip: environment.ip,
                    origin_device: environment.device.clone(),
                    origin_geo: environment.geo.clone(),
                }
            }
        }
    } else {
        // Nothing consumes the session this request (no drift detectors, no
        // session-age policy condition), so the session-store round-trip is
        // pure per-request cost. Synthesize a live, non-invalidated session
        // from the request instead. This is what removes the dominant
        // cluster-mode hot-path Redis op. Fail-closed is unaffected: this
        // request genuinely depends on nothing in the session store, and the
        // hot store still fails closed for any limited key that needs it.
        SessionState {
            session_id,
            started_at: key.created_at,
            action_count: 0,
            action_history: Vec::new(),
            invalidated: false,
            origin_ip: environment.ip,
            origin_device: environment.device.clone(),
            origin_geo: environment.geo.clone(),
        }
    };

    ActionContext::new(principal, action, session, environment)
        .with_metadata("marg_key_id", json!(key.id))
        .with_metadata(
            "marg_team",
            json!(key.team.clone().unwrap_or_default()),
        )
}

/// Header-only environment enrichment. Marg does not embed a GeoIP DB; the
/// load balancer is expected to populate `x-forwarded-geo` /
/// `x-marg-device-fingerprint` for clusters that want geo or device drift.
///
/// `x-forwarded-geo` format (documented in `docs/kavach.md`):
///     `<country>[;region=<r>][;city=<c>][;lat=<f>][;lon=<f>]`
///
/// Examples:
///     `IN`
///     `IN;city=Mumbai;lat=19.07;lon=72.87`
pub fn parse_caller_headers(headers: &HeaderMap) -> CallerHeaders {
    let ip = caller_ip_from_headers(headers);
    let geo = parse_geo_header(headers.get("x-forwarded-geo").and_then(|v| v.to_str().ok()));
    let device = headers
        .get("x-marg-device-fingerprint")
        .and_then(|v| v.to_str().ok())
        .map(|s| DeviceFingerprint {
            hash: s.trim().to_string(),
            description: None,
        });
    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    CallerHeaders {
        ip,
        geo,
        device,
        user_agent,
    }
}

fn caller_ip_from_headers(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim())
        .and_then(|s| s.parse().ok())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse().ok())
        })
}

fn parse_geo_header(raw: Option<&str>) -> Option<GeoLocation> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    let mut parts = raw.split(';').map(|s| s.trim());
    let country = parts.next()?.to_string();
    if country.is_empty() {
        return None;
    }
    let mut region = None;
    let mut city = None;
    let mut latitude = None;
    let mut longitude = None;
    for kv in parts {
        let Some((k, v)) = kv.split_once('=') else {
            continue;
        };
        let v = v.trim();
        match k.trim().to_ascii_lowercase().as_str() {
            "region" => region = Some(v.to_string()),
            "city" => city = Some(v.to_string()),
            "lat" | "latitude" => latitude = v.parse().ok(),
            "lon" | "long" | "longitude" => longitude = v.parse().ok(),
            _ => {}
        }
    }
    Some(GeoLocation {
        country_code: country,
        region,
        city,
        latitude,
        longitude,
    })
}

fn action_name_for(model: &str) -> String {
    format!("marg.chat.{}", model)
}

fn map_principal_kind(k: &MargPrincipalKind) -> KavachPrincipalKind {
    match k {
        MargPrincipalKind::User => KavachPrincipalKind::User,
        MargPrincipalKind::Service => KavachPrincipalKind::Service,
        MargPrincipalKind::Agent => KavachPrincipalKind::Agent,
    }
}

fn marg_principal_kind_str(k: &MargPrincipalKind) -> &'static str {
    match k {
        MargPrincipalKind::User => "user",
        MargPrincipalKind::Service => "service",
        MargPrincipalKind::Agent => "agent",
    }
}

/// Marg keys do not carry an explicit `roles` field in v1.0. We derive a
/// minimal set: the principal kind and (when set) the team name. Policies
/// reference them via the `identity_role` condition.
fn collect_roles(key: &MargKey) -> Vec<String> {
    let mut roles = Vec::with_capacity(2);
    roles.push(marg_principal_kind_str(&key.principal.kind).to_string());
    if let Some(team) = &key.team {
        roles.push(format!("team:{}", team));
    }
    roles
}

fn tool_count(raw: &serde_json::Value) -> u64 {
    raw.get("tools")
        .and_then(|v| v.as_array())
        .map(|a| a.len() as u64)
        .unwrap_or(0)
}

/// Deterministic UUID from a Marg key id so every request on the same key
/// hashes to the same Kavach `session_id`. This is intentional: drift detection
/// is per-key in Marg's model, not per-process or per-request.
fn derive_session_id(key_id: &str) -> Uuid {
    // Build a UUIDv5 in the OID namespace from the key id. Stable across
    // process restarts and across Marg replicas (P10 cluster), no extra state.
    let bytes = sha2_first16(key_id);
    Uuid::from_bytes(bytes)
}

fn sha2_first16(input: &str) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(input.as_bytes());
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest[..16]);
    // Force the version+variant bits to a sane UUIDv4-looking value so
    // downstream tools that validate the version nibble do not complain.
    out[6] = (out[6] & 0x0F) | 0x40;
    out[8] = (out[8] & 0x3F) | 0x80;
    out
}

/// Helper: stamp the current epoch into a lifecycle struct at request start.
pub fn _touch_now() -> chrono::DateTime<chrono::Utc> {
    Utc::now()
}
