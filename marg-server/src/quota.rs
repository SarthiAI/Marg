use chrono::Utc;

use marg_core::BudgetSpec;
use marg_providers::ChatRequest;

use crate::errors::ChatError;
use crate::state::AppState;

pub async fn check(
    state: &AppState,
    key_id: &str,
    budget: &BudgetSpec,
    req: &ChatRequest,
    quota_model: &str,
) -> Result<(), ChatError> {
    if budget.is_unlimited_usd() {
        return Ok(());
    }

    let pricing = state.pricing.load();
    let estimated_cost_usd = pricing.cost_usd(
        quota_model,
        req.estimated_input_tokens,
        req.max_output_tokens.unwrap_or(1024) as u64,
    );

    let day = Utc::now().date_naive();
    let spent = state
        .storage
        .current_spend(key_id, day)
        .await
        .map_err(|e| ChatError::Storage(e.to_string()))?;

    if spent + estimated_cost_usd > budget.daily_usd {
        return Err(ChatError::BudgetExceeded {
            spent_usd: spent,
            daily_usd: budget.daily_usd,
        });
    }

    Ok(())
}
