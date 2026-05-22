use serde::Deserialize;
use std::path::Path;

use crate::error::ConfigError;
use crate::pricing::ModelPrice;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub providers: ProvidersConfig,
    #[serde(default)]
    pub security: SecurityConfig,
    #[serde(default)]
    pub cors: CorsConfig,
    #[serde(default)]
    pub rate_limits: RateLimitsConfig,
    #[serde(default)]
    pub pricing: Vec<PricingEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: usize,
}

fn default_bind() -> String { "0.0.0.0:8080".to_string() }
fn default_max_body_bytes() -> usize { 1_048_576 } // 1 MiB

impl Default for ServerConfig {
    fn default() -> Self {
        Self { bind: default_bind(), max_body_bytes: default_max_body_bytes() }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageConfig {
    #[serde(default = "default_storage_backend")]
    pub backend: String,
    #[serde(default = "default_sqlite_path")]
    pub path: String,
}

fn default_storage_backend() -> String { "sqlite".to_string() }
fn default_sqlite_path() -> String { "./marg.db".to_string() }

impl Default for StorageConfig {
    fn default() -> Self {
        Self { backend: default_storage_backend(), path: default_sqlite_path() }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvidersConfig {
    pub openai: Option<OpenAiProviderConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenAiProviderConfig {
    pub api_key: String,
    #[serde(default = "default_openai_base_url")]
    pub base_url: String,
    #[serde(default = "default_provider_timeout_seconds")]
    pub timeout_seconds: u64,
}

fn default_openai_base_url() -> String { "https://api.openai.com".to_string() }
fn default_provider_timeout_seconds() -> u64 { 120 }

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityConfig {
    #[serde(default)]
    pub log_prompts: bool,
    #[serde(default)]
    pub log_responses: bool,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self { log_prompts: false, log_responses: false }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub allowed_origins: Vec<String>,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self { enabled: false, allowed_origins: Vec::new() }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateLimitsConfig {
    #[serde(default)]
    pub default_rpm: u32,
}

impl Default for RateLimitsConfig {
    fn default() -> Self {
        Self { default_rpm: 0 }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PricingEntry {
    pub model: String,
    pub input_per_1k_usd: f64,
    pub output_per_1k_usd: f64,
}

impl PricingEntry {
    pub fn price(&self) -> ModelPrice {
        ModelPrice {
            input_per_1k_usd: self.input_per_1k_usd,
            output_per_1k_usd: self.output_per_1k_usd,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            storage: StorageConfig::default(),
            providers: ProvidersConfig::default(),
            security: SecurityConfig::default(),
            cors: CorsConfig::default(),
            rate_limits: RateLimitsConfig::default(),
            pricing: Vec::new(),
        }
    }
}

impl Config {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let cfg: Config = toml::from_str(&text)?;
                cfg.validate()?;
                Ok(cfg)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(ConfigError::Io { path: path.display().to_string(), source: err }),
        }
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.server.max_body_bytes == 0 {
            return Err(ConfigError::Validation(
                "server.max_body_bytes must be greater than 0".into(),
            ));
        }
        if !matches!(self.storage.backend.as_str(), "sqlite") {
            return Err(ConfigError::Validation(format!(
                "storage.backend '{}' not supported in P01, only 'sqlite' is implemented (postgres + redis arrive in P03)",
                self.storage.backend
            )));
        }
        Ok(())
    }
}
