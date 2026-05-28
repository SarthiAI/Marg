use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::time::Duration;

use crate::error::ProviderError;
use crate::request::{ChatRequest, ChatResponse, ChatStream, ChatUsage};
use crate::ChatCompletionsClient;

pub struct OpenAIClient {
    http: Client,
    base_url: String,
    api_key: SecretString,
}

impl OpenAIClient {
    pub fn new(base_url: String, api_key: SecretString, timeout: Duration) -> Result<Self, ProviderError> {
        let http = Client::builder()
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(64)
            .http2_keep_alive_interval(Some(Duration::from_secs(20)))
            .http2_keep_alive_while_idle(true)
            .build()
            .map_err(|e| ProviderError::Internal(format!("build reqwest client: {}", e)))?;
        let base_url = base_url.trim_end_matches('/').to_string();
        Ok(Self { http, base_url, api_key })
    }

    fn endpoint(&self) -> String {
        // Accept both base_url shapes operators paste in:
        //   "https://api.openai.com"            -> add "/v1/chat/completions"
        //   "https://openrouter.ai/api/v1"      -> add "/chat/completions"
        // The OpenAI SDK convention is to set base_url to ".../v1", so most
        // upstream docs (OpenRouter, Cerebras, Groq, vLLM, ...) lead with
        // that form. Marg accepts either to match operator muscle memory.
        if self.base_url.ends_with("/v1") {
            format!("{}/chat/completions", self.base_url)
        } else {
            format!("{}/v1/chat/completions", self.base_url)
        }
    }
}

#[derive(Deserialize)]
struct OpenAIResponseEnvelope {
    #[serde(default)]
    usage: Option<ChatUsage>,
    #[serde(default)]
    model: Option<String>,
}

#[async_trait]
impl ChatCompletionsClient for OpenAIClient {
    fn provider_name(&self) -> &'static str { "openai" }

    async fn chat_completion(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let resp = self
            .http
            .post(self.endpoint())
            .bearer_auth(self.api_key.expose_secret())
            .json(&req.raw)
            .send()
            .await?;
        let status = resp.status().as_u16();
        let body = resp.bytes().await?;
        let envelope: OpenAIResponseEnvelope =
            serde_json::from_slice(&body).unwrap_or(OpenAIResponseEnvelope { usage: None, model: None });
        Ok(ChatResponse {
            status,
            body,
            usage: envelope.usage.unwrap_or_default(),
            model: envelope.model.unwrap_or(req.model.clone()),
        })
    }

    async fn chat_completion_stream(&self, mut req: ChatRequest) -> Result<ChatStream, ProviderError> {
        req.ensure_stream_usage();
        let resp = self
            .http
            .post(self.endpoint())
            .bearer_auth(self.api_key.expose_secret())
            .json(&req.raw)
            .send()
            .await?;
        let status = resp.status().as_u16();
        let upstream = resp.bytes_stream();
        let byte_stream = upstream
            .map(|chunk| {
                chunk.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
            })
            .boxed();
        Ok(ChatStream { status, byte_stream })
    }
}
