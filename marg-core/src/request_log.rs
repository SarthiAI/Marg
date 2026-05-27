use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestLogEntry {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub key_id: String,
    pub principal_id: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub latency_ms: u64,
    pub status: u16,
    pub stream: bool,
    pub error: Option<String>,
    #[serde(default)]
    pub team: Option<String>,
    #[serde(default)]
    pub attempts: Vec<RouteAttempt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteAttempt {
    pub provider: String,
    pub model: String,
    pub status: u16,
    pub latency_ms: u64,
    pub outcome: AttemptOutcome,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttemptOutcome {
    Success,
    Timeout,
    Network,
    Upstream5xx,
    Upstream4xx,
    Cancelled,
    Internal,
}

impl AttemptOutcome {
    pub fn is_retriable(self) -> bool {
        matches!(
            self,
            AttemptOutcome::Timeout | AttemptOutcome::Network | AttemptOutcome::Upstream5xx
        )
    }
}
