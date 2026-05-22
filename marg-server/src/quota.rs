use chrono::{NaiveDate, Utc};

use marg_core::BudgetSpec;
use marg_providers::ChatRequest;

use crate::errors::ChatError;
use crate::state::AppState;

/// Result of the pre-flight quota check. The reservation needs to be settled
/// after the upstream returns so the actual cost replaces the estimate.
#[derive(Debug, Clone, Copy)]
pub struct QuotaReservation {
    pub day: NaiveDate,
    pub estimated_cost_usd: f64,
    pub enforced: bool,
}

pub async fn check(
    state: &AppState,
    key_id: &str,
    budget: &BudgetSpec,
    req: &ChatRequest,
    quota_model: &str,
) -> Result<QuotaReservation, ChatError> {
    // Rate limit first. A zero rpm disables the limit.
    if !budget.is_unlimited_rpm() {
        let allowed = state
            .hot
            .allow_request(key_id, budget.rpm)
            .await
            .map_err(|e| ChatError::HotStore(format!("rate limit check failed: {}", e)))?;
        if !allowed {
            return Err(ChatError::RateLimited { rpm: budget.rpm });
        }
    }

    let day = Utc::now().date_naive();

    if budget.is_unlimited_usd() {
        return Ok(QuotaReservation {
            day,
            estimated_cost_usd: 0.0,
            enforced: false,
        });
    }

    let pricing = state.pricing.load();
    let estimated_cost_usd = pricing.cost_usd(
        quota_model,
        req.estimated_input_tokens,
        req.max_output_tokens.unwrap_or(1024) as u64,
    );

    let reservation = state
        .hot
        .reserve_budget(key_id, day, estimated_cost_usd, budget.daily_usd)
        .await
        .map_err(|e| ChatError::HotStore(format!("budget reservation failed: {}", e)))?;

    if !reservation.granted {
        return Err(ChatError::BudgetExceeded {
            spent_usd: reservation.spent_after,
            daily_usd: budget.daily_usd,
        });
    }

    Ok(QuotaReservation {
        day,
        estimated_cost_usd,
        enforced: true,
    })
}
