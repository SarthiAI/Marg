use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use marg_core::Config;

use crate::admin::error::AdminError;
use crate::kavach::emit_policy_reload;
use crate::policy;
use crate::state::AppState;

pub async fn view(State(state): State<AppState>) -> Result<Json<Value>, AdminError> {
    let cfg = Config::load(state.config_path.as_str())
        .map_err(|e| AdminError::Internal(format!("read config: {}", e)))?;
    let stored = state.storage.list_routes().await?;
    let providers: Vec<String> = state.providers.keys().cloned().collect();
    let mode = state.kavach.mode.load_full();
    let policy_path = state
        .kavach
        .policy_source_path
        .as_ref()
        .map(|p| p.display().to_string());
    let policy_hash = state.kavach.policy_source_hash.load_full();
    let loaded_at = state.kavach.policy_loaded_at.load_full();
    let signer = &state.kavach.permit_signer;
    let drift = state.kavach.drift_state.load_full();
    let drift_detectors_json: Vec<Value> = drift
        .detectors
        .iter()
        .map(|d| json!({ "name": d.name, "parameters": d.parameters }))
        .collect();
    Ok(Json(json!({
        "config_path": state.config_path.as_str(),
        "providers": providers,
        "default_provider": cfg.providers.default,
        "config_routes": cfg.routes,
        "stored_routes": stored,
        "pricing": cfg.pricing,
        "kavach": {
            "mode": mode.as_str(),
            "policy_path": policy_path,
            "policy_source_hash": policy_hash.as_str(),
            "loaded_at": loaded_at.as_str(),
            "policy_rule_count": *state.kavach.policy_rule_count.load_full(),
            "invariant_count": *state.kavach.invariant_count.load_full(),
            "audit_chain_length": state.kavach.audit_chain.len(),
            "audit_chain_head_hash": state.kavach.audit_chain.head_hash(),
            "core_version": crate::KAVACH_CORE_VERSION,
            "pq_version": crate::KAVACH_PQ_VERSION,
            "permit_signer": {
                "enabled": signer.enabled,
                "algorithm": signer.algorithm,
                "key_id": signer.key_id,
            },
            "drift": {
                "enabled": drift.enabled,
                "warning_threshold": drift.warning_threshold,
                "detectors": drift_detectors_json,
            },
        },
    })))
}

pub async fn reload(State(state): State<AppState>) -> Result<Json<Value>, AdminError> {
    let previous_hash = state.kavach.policy_source_hash.load_full().as_str().to_string();
    match policy::reload(&state).await {
        Ok(outcome) => {
            // Record the reload in the signed audit chain.
            let source_path = outcome.kavach_source_path.as_ref().map(std::path::PathBuf::from);
            emit_policy_reload(
                &state.kavach.audit_chain,
                "admin",
                &outcome.kavach_previous_hash,
                &outcome.kavach_new_hash,
                source_path.as_ref(),
                outcome.kavach_rules,
                outcome.kavach_invariants,
                true,
                None,
            );
            Ok(Json(json!({
                "reloaded": true,
                "config_routes": outcome.config_routes,
                "stored_routes": outcome.stored_routes,
                "pricing_entries": outcome.pricing_entries,
                "kavach": {
                    "mode": outcome.kavach_mode,
                    "previous_hash": outcome.kavach_previous_hash,
                    "new_hash": outcome.kavach_new_hash,
                    "policy_rule_count": outcome.kavach_rules,
                    "invariant_count": outcome.kavach_invariants,
                    "source_path": outcome.kavach_source_path,
                },
            })))
        }
        Err(err) => {
            emit_policy_reload(
                &state.kavach.audit_chain,
                "admin",
                &previous_hash,
                &previous_hash,
                None,
                0,
                0,
                false,
                Some(err.to_string()),
            );
            Err(AdminError::BadRequest(err.to_string()))
        }
    }
}
