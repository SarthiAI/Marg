use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BudgetSpec {
    pub key_id: String,
    pub daily_usd: f64,
    pub rpm: u32,
}

impl BudgetSpec {
    pub fn unlimited(key_id: String) -> Self {
        Self { key_id, daily_usd: 0.0, rpm: 0 }
    }

    pub fn is_unlimited_usd(&self) -> bool {
        self.daily_usd <= 0.0
    }

    pub fn is_unlimited_rpm(&self) -> bool {
        self.rpm == 0
    }
}

#[derive(Debug, Clone)]
pub struct BudgetCounter {
    pub key_id: String,
    pub day: NaiveDate,
    pub spent_usd: f64,
}
