use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::admin::error::AdminError;
use crate::state::AppState;

/// Per-provider derived health snapshot. Built from the in-process metrics
/// registry: configured + recent-success + recent-error counts give operators
/// the same view Prometheus would. No active probe (an admin endpoint should
/// never trigger upstream API costs).
pub async fn health(State(state): State<AppState>) -> Result<Json<Value>, AdminError> {
    let metric_families = state.metrics.registry.gather();
    let mut providers: Vec<Value> = Vec::with_capacity(state.providers.len());
    for name in state.providers.keys() {
        let success = sum_counter(
            &metric_families,
            "marg_requests_total",
            &[("provider", name.as_str()), ("status", "200")],
        );
        let errors_5xx = sum_counter(
            &metric_families,
            "marg_provider_errors_total",
            &[("provider", name.as_str()), ("kind", "upstream_5xx")],
        );
        let errors_4xx = sum_counter(
            &metric_families,
            "marg_provider_errors_total",
            &[("provider", name.as_str()), ("kind", "upstream_4xx")],
        );
        let timeouts = sum_counter(
            &metric_families,
            "marg_provider_errors_total",
            &[("provider", name.as_str()), ("kind", "timeout")],
        );
        let network = sum_counter(
            &metric_families,
            "marg_provider_errors_total",
            &[("provider", name.as_str()), ("kind", "network")],
        );
        providers.push(json!({
            "name": name,
            "configured": true,
            "successes_total": success,
            "errors_5xx": errors_5xx,
            "errors_4xx": errors_4xx,
            "timeouts": timeouts,
            "network_errors": network,
        }));
    }
    Ok(Json(json!({ "providers": providers })))
}

fn sum_counter(
    families: &[prometheus::proto::MetricFamily],
    name: &str,
    matchers: &[(&str, &str)],
) -> u64 {
    let mut sum = 0u64;
    for family in families {
        if family.get_name() != name {
            continue;
        }
        for metric in family.get_metric() {
            let labels = metric.get_label();
            if matchers.iter().all(|(k, v)| {
                labels
                    .iter()
                    .any(|lp| lp.get_name() == *k && lp.get_value() == *v)
            }) {
                sum = sum.saturating_add(metric.get_counter().get_value() as u64);
            }
        }
    }
    sum
}
