use serde::Deserialize;
use std::path::Path;

use crate::error::ConfigError;
use crate::pricing::ModelPrice;
use crate::routing::RouteSpec;

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
    #[serde(default, rename = "routes")]
    pub routes: Vec<RouteSpec>,
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
    #[serde(default)]
    pub dsn: Option<String>,
    #[serde(default)]
    pub hot: Option<HotStoreConfig>,
}

fn default_storage_backend() -> String { "sqlite".to_string() }
fn default_sqlite_path() -> String { "./marg.db".to_string() }

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            backend: default_storage_backend(),
            path: default_sqlite_path(),
            dsn: None,
            hot: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HotStoreConfig {
    #[serde(default = "default_hot_backend")]
    pub backend: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub key_prefix: Option<String>,
}

fn default_hot_backend() -> String { "redis".to_string() }

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvidersConfig {
    pub openai: Option<OpenAiProviderConfig>,
    pub anthropic: Option<AnthropicProviderConfig>,
    pub google: Option<GoogleProviderConfig>,
    pub bedrock: Option<BedrockProviderConfig>,
    #[serde(default)]
    pub default: Option<String>,
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
pub struct AnthropicProviderConfig {
    pub api_key: String,
    #[serde(default = "default_anthropic_base_url")]
    pub base_url: String,
    #[serde(default = "default_anthropic_version")]
    pub api_version: String,
    #[serde(default = "default_anthropic_default_max_tokens")]
    pub default_max_tokens: u32,
    #[serde(default = "default_provider_timeout_seconds")]
    pub timeout_seconds: u64,
}

fn default_anthropic_base_url() -> String { "https://api.anthropic.com".to_string() }
fn default_anthropic_version() -> String { "2023-06-01".to_string() }
fn default_anthropic_default_max_tokens() -> u32 { 1024 }

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoogleProviderConfig {
    pub api_key: String,
    #[serde(default = "default_google_base_url")]
    pub base_url: String,
    #[serde(default = "default_google_api_version")]
    pub api_version: String,
    #[serde(default = "default_provider_timeout_seconds")]
    pub timeout_seconds: u64,
}

fn default_google_base_url() -> String { "https://generativelanguage.googleapis.com".to_string() }
fn default_google_api_version() -> String { "v1beta".to_string() }

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BedrockProviderConfig {
    pub region: String,
    #[serde(default)]
    pub access_key_id: Option<String>,
    #[serde(default)]
    pub secret_access_key: Option<String>,
    #[serde(default)]
    pub session_token: Option<String>,
    #[serde(default = "default_bedrock_default_max_tokens")]
    pub default_max_tokens: u32,
    #[serde(default = "default_bedrock_anthropic_version")]
    pub anthropic_version: String,
    #[serde(default = "default_provider_timeout_seconds")]
    pub timeout_seconds: u64,
}

fn default_bedrock_default_max_tokens() -> u32 { 1024 }
fn default_bedrock_anthropic_version() -> String { "bedrock-2023-05-31".to_string() }

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
            routes: Vec::new(),
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
        match self.storage.backend.as_str() {
            "sqlite" => {
                if self.storage.path.trim().is_empty() {
                    return Err(ConfigError::Validation(
                        "storage.path must be set when storage.backend = 'sqlite'".into(),
                    ));
                }
            }
            "postgres" => {
                if self.storage.dsn.as_deref().map(str::trim).unwrap_or("").is_empty() {
                    return Err(ConfigError::Validation(
                        "storage.dsn must be set when storage.backend = 'postgres' (e.g. \"postgres://user:pass@host/db\")".into(),
                    ));
                }
            }
            other => {
                return Err(ConfigError::Validation(format!(
                    "storage.backend '{}' is not supported: choose 'sqlite' or 'postgres'",
                    other
                )));
            }
        }
        if let Some(hot) = &self.storage.hot {
            match hot.backend.as_str() {
                "redis" => {
                    if hot.url.as_deref().map(str::trim).unwrap_or("").is_empty() {
                        return Err(ConfigError::Validation(
                            "storage.hot.url must be set when storage.hot.backend = 'redis'".into(),
                        ));
                    }
                }
                other => {
                    return Err(ConfigError::Validation(format!(
                        "storage.hot.backend '{}' is not supported: choose 'redis' or omit the [storage.hot] block",
                        other
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn registered_providers(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.providers.openai.is_some() { out.push("openai".to_string()); }
        if self.providers.anthropic.is_some() { out.push("anthropic".to_string()); }
        if self.providers.google.is_some() { out.push("google".to_string()); }
        if self.providers.bedrock.is_some() { out.push("bedrock".to_string()); }
        out
    }
}
