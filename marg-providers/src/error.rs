use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("invalid request body: {0}")]
    InvalidRequest(String),

    #[error("missing required field: {0}")]
    MissingField(&'static str),

    #[error("upstream returned status {status}: {message}")]
    Upstream { status: u16, message: String },

    #[error("upstream io error: {0}")]
    Network(String),

    #[error("upstream timed out")]
    Timeout,

    #[error("internal provider error: {0}")]
    Internal(String),
}

impl From<reqwest::Error> for ProviderError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            ProviderError::Timeout
        } else {
            ProviderError::Network(e.to_string())
        }
    }
}

impl From<serde_json::Error> for ProviderError {
    fn from(e: serde_json::Error) -> Self {
        ProviderError::InvalidRequest(e.to_string())
    }
}
