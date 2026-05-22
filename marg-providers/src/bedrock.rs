use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use chrono::Utc;
use data_encoding::BASE64;
use futures_util::StreamExt;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;

use crate::error::ProviderError;
use crate::event_stream::try_decode;
use crate::request::{ChatRequest, ChatResponse, ChatStream, ChatUsage};
use crate::sigv4::{sign, Credentials};
use crate::ChatCompletionsClient;

pub struct BedrockClient {
    http: Client,
    region: String,
    access_key_id: SecretString,
    secret_access_key: SecretString,
    session_token: Option<SecretString>,
    default_max_tokens: u32,
    anthropic_version: String,
}

impl BedrockClient {
    pub fn new(
        region: String,
        access_key_id: SecretString,
        secret_access_key: SecretString,
        session_token: Option<SecretString>,
        default_max_tokens: u32,
        anthropic_version: String,
        timeout: Duration,
    ) -> Result<Self, ProviderError> {
        let http = Client::builder()
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(64)
            .build()
            .map_err(|e| ProviderError::Internal(format!("build reqwest client: {}", e)))?;
        Ok(Self {
            http,
            region,
            access_key_id,
            secret_access_key,
            session_token,
            default_max_tokens: default_max_tokens.max(1),
            anthropic_version,
        })
    }

    fn host(&self) -> String {
        format!("bedrock-runtime.{}.amazonaws.com", self.region)
    }

    fn endpoint(&self, model_id: &str, stream: bool) -> (String, String) {
        let path = if stream {
            format!(
                "/model/{}/invoke-with-response-stream",
                utf8_percent_encode(model_id, NON_ALPHANUMERIC)
            )
        } else {
            format!(
                "/model/{}/invoke",
                utf8_percent_encode(model_id, NON_ALPHANUMERIC)
            )
        };
        let url = format!("https://{}{}", self.host(), path);
        (url, path)
    }
}

fn build_bedrock_anthropic_body(req: &ChatRequest, default_max_tokens: u32, anthropic_version: &str) -> Value {
    let mut system_parts: Vec<String> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();
    for raw in req.messages() {
        let role = raw.get("role").and_then(|v| v.as_str()).unwrap_or("user");
        let content = content_to_anthropic(raw.get("content"));
        if role == "system" {
            if let Some(text) = content_as_text(raw.get("content")) {
                system_parts.push(text);
            }
            continue;
        }
        let anth_role = match role {
            "assistant" => "assistant",
            _ => "user",
        };
        messages.push(json!({"role": anth_role, "content": content}));
    }
    let max_tokens = req.max_output_tokens.unwrap_or(default_max_tokens);
    let mut body = json!({
        "anthropic_version": anthropic_version,
        "max_tokens": max_tokens,
        "messages": messages,
    });
    if !system_parts.is_empty() {
        body["system"] = Value::String(system_parts.join("\n\n"));
    }
    if let Some(t) = req.temperature() {
        body["temperature"] = json!(t);
    }
    if let Some(p) = req.top_p() {
        body["top_p"] = json!(p);
    }
    let stops = req.stop_sequences();
    if !stops.is_empty() {
        body["stop_sequences"] = json!(stops);
    }
    body
}

fn content_to_anthropic(content: Option<&Value>) -> Value {
    match content {
        Some(Value::String(s)) => json!([{"type": "text", "text": s}]),
        Some(Value::Array(parts)) => {
            let mut out = Vec::new();
            for part in parts {
                let kind = part.get("type").and_then(|v| v.as_str()).unwrap_or("text");
                if kind == "text" {
                    if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                        out.push(json!({"type": "text", "text": text}));
                    }
                }
            }
            if out.is_empty() {
                json!([{"type": "text", "text": ""}])
            } else {
                Value::Array(out)
            }
        }
        _ => json!([{"type": "text", "text": ""}]),
    }
}

fn content_as_text(content: Option<&Value>) -> Option<String> {
    match content {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Array(parts)) => {
            let mut buf = String::new();
            for part in parts {
                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                    if !buf.is_empty() {
                        buf.push('\n');
                    }
                    buf.push_str(text);
                }
            }
            if buf.is_empty() {
                None
            } else {
                Some(buf)
            }
        }
        _ => None,
    }
}

fn anthropic_to_openai(model: &str, value: &Value) -> (Value, ChatUsage) {
    let id = value
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("chatcmpl-bedrock-{}", Uuid::new_v4()));
    let mut text = String::new();
    if let Some(parts) = value.get("content").and_then(|v| v.as_array()) {
        for part in parts {
            if part.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
                    text.push_str(t);
                }
            }
        }
    }
    let stop_reason = value
        .get("stop_reason")
        .and_then(|v| v.as_str())
        .unwrap_or("end_turn");
    let finish_reason = match stop_reason {
        "end_turn" | "stop_sequence" => "stop",
        "max_tokens" => "length",
        "tool_use" => "tool_calls",
        other => other,
    };
    let prompt_tokens = value
        .get("usage")
        .and_then(|u| u.get("input_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let completion_tokens = value
        .get("usage")
        .and_then(|u| u.get("output_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let usage = ChatUsage {
        prompt_tokens,
        completion_tokens,
        total_tokens: prompt_tokens + completion_tokens,
    };
    let openai = json!({
        "id": id,
        "object": "chat.completion",
        "created": Utc::now().timestamp(),
        "model": model,
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": text},
            "finish_reason": finish_reason,
        }],
        "usage": {
            "prompt_tokens": usage.prompt_tokens,
            "completion_tokens": usage.completion_tokens,
            "total_tokens": usage.total_tokens,
        }
    });
    (openai, usage)
}

#[async_trait]
impl ChatCompletionsClient for BedrockClient {
    fn provider_name(&self) -> &'static str {
        "bedrock"
    }

    async fn chat_completion(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let body = build_bedrock_anthropic_body(&req, self.default_max_tokens, &self.anthropic_version);
        let body_bytes = serde_json::to_vec(&body)
            .map_err(|e| ProviderError::Internal(format!("encode bedrock body: {}", e)))?;
        let (url, path) = self.endpoint(&req.model, false);
        let host = self.host();
        let creds = Credentials {
            access_key_id: self.access_key_id.expose_secret(),
            secret_access_key: self.secret_access_key.expose_secret(),
            session_token: self.session_token.as_ref().map(|s| s.expose_secret()),
        };
        let extra = vec![
            ("content-type".to_string(), "application/json".to_string()),
            ("accept".to_string(), "application/json".to_string()),
        ];
        let signed = sign(
            "POST",
            &host,
            &path,
            &self.region,
            "bedrock",
            &creds,
            &extra,
            &body_bytes,
            Utc::now(),
        );
        let mut builder = self
            .http
            .post(&url)
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .header("x-amz-date", &signed.amz_date)
            .header("x-amz-content-sha256", &signed.content_sha256)
            .header("authorization", &signed.authorization)
            .body(body_bytes);
        if let Some(token) = &signed.session_token {
            builder = builder.header("x-amz-security-token", token);
        }
        let resp = builder.send().await?;
        let status = resp.status().as_u16();
        let raw = resp.bytes().await?;
        if !(200..300).contains(&status) {
            return Ok(ChatResponse {
                status,
                body: raw,
                usage: ChatUsage::default(),
                model: req.model.clone(),
            });
        }
        let parsed: Value = serde_json::from_slice(&raw)
            .map_err(|e| ProviderError::Upstream {
                status,
                message: format!("bedrock response was not valid json: {}", e),
            })?;
        let (translated, usage) = anthropic_to_openai(&req.model, &parsed);
        let out_bytes = Bytes::from(serde_json::to_vec(&translated).map_err(|e| {
            ProviderError::Internal(format!("serialize openai response: {}", e))
        })?);
        Ok(ChatResponse {
            status,
            body: out_bytes,
            usage,
            model: req.model.clone(),
        })
    }

    async fn chat_completion_stream(&self, req: ChatRequest) -> Result<ChatStream, ProviderError> {
        let body = build_bedrock_anthropic_body(&req, self.default_max_tokens, &self.anthropic_version);
        let body_bytes = serde_json::to_vec(&body)
            .map_err(|e| ProviderError::Internal(format!("encode bedrock body: {}", e)))?;
        let (url, path) = self.endpoint(&req.model, true);
        let host = self.host();
        let creds = Credentials {
            access_key_id: self.access_key_id.expose_secret(),
            secret_access_key: self.secret_access_key.expose_secret(),
            session_token: self.session_token.as_ref().map(|s| s.expose_secret()),
        };
        let extra = vec![
            ("content-type".to_string(), "application/json".to_string()),
            (
                "accept".to_string(),
                "application/vnd.amazon.eventstream".to_string(),
            ),
        ];
        let signed = sign(
            "POST",
            &host,
            &path,
            &self.region,
            "bedrock",
            &creds,
            &extra,
            &body_bytes,
            Utc::now(),
        );
        let mut builder = self
            .http
            .post(&url)
            .header("content-type", "application/json")
            .header("accept", "application/vnd.amazon.eventstream")
            .header("x-amz-date", &signed.amz_date)
            .header("x-amz-content-sha256", &signed.content_sha256)
            .header("authorization", &signed.authorization)
            .body(body_bytes);
        if let Some(token) = &signed.session_token {
            builder = builder.header("x-amz-security-token", token);
        }
        let resp = builder.send().await?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let raw = resp.bytes().await?;
            return Err(ProviderError::Upstream {
                status,
                message: String::from_utf8_lossy(&raw).to_string(),
            });
        }
        let model = req.model.clone();
        let upstream = resp.bytes_stream();
        let translated = translate_bedrock_stream(upstream, model);
        Ok(ChatStream {
            status,
            byte_stream: translated.boxed(),
        })
    }
}

fn translate_bedrock_stream<S>(
    stream: S,
    model: String,
) -> impl futures::Stream<Item = Result<Bytes, std::io::Error>> + Send
where
    S: futures::Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    async_stream::try_stream! {
        let mut buf = BytesMut::new();
        let id = format!("chatcmpl-bedrock-{}", Uuid::new_v4());
        let created = Utc::now().timestamp();
        let mut sent_role = false;
        let mut prompt_tokens: u64 = 0;
        let mut completion_tokens: u64 = 0;
        let mut emitted_done = false;
        futures::pin_mut!(stream);
        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
            buf.extend_from_slice(&bytes);
            loop {
                let msg = match try_decode(&mut buf) {
                    Ok(Some(m)) => m,
                    Ok(None) => break,
                    Err(e) => Err(std::io::Error::new(std::io::ErrorKind::Other, e))?,
                };
                let message_type = msg.header_str(":message-type").unwrap_or("");
                if message_type == "exception" {
                    let text = String::from_utf8_lossy(&msg.payload).to_string();
                    Err(std::io::Error::new(std::io::ErrorKind::Other, text))?;
                }
                if message_type != "event" {
                    continue;
                }
                let event_type = msg.header_str(":event-type").unwrap_or("").to_string();
                if event_type != "chunk" {
                    continue;
                }
                let payload_json: Value = match serde_json::from_slice(&msg.payload) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let inner_b64 = match payload_json.get("bytes").and_then(|v| v.as_str()) {
                    Some(s) => s,
                    None => continue,
                };
                let inner_bytes = match BASE64.decode(inner_b64.as_bytes()) {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                let inner: Value = match serde_json::from_slice(&inner_bytes) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let kind = inner.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
                match kind.as_str() {
                    "message_start" => {
                        if !sent_role {
                            let role_chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": model,
                                "choices": [{
                                    "index": 0,
                                    "delta": {"role": "assistant"},
                                    "finish_reason": Value::Null,
                                }],
                            });
                            yield Bytes::from(format!("data: {}\n\n", role_chunk));
                            sent_role = true;
                        }
                        if let Some(usage) = inner.pointer("/message/usage") {
                            if let Some(p) = usage.get("input_tokens").and_then(|v| v.as_u64()) {
                                prompt_tokens = p;
                            }
                            if let Some(c) = usage.get("output_tokens").and_then(|v| v.as_u64()) {
                                completion_tokens = c;
                            }
                        }
                    }
                    "content_block_delta" => {
                        let delta_type = inner
                            .pointer("/delta/type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if delta_type == "text_delta" {
                            if let Some(text) = inner.pointer("/delta/text").and_then(|v| v.as_str()) {
                                let chunk_json = json!({
                                    "id": id,
                                    "object": "chat.completion.chunk",
                                    "created": created,
                                    "model": model,
                                    "choices": [{
                                        "index": 0,
                                        "delta": {"content": text},
                                        "finish_reason": Value::Null,
                                    }],
                                });
                                yield Bytes::from(format!("data: {}\n\n", chunk_json));
                            }
                        }
                    }
                    "message_delta" => {
                        if let Some(usage) = inner.get("usage") {
                            if let Some(c) = usage.get("output_tokens").and_then(|v| v.as_u64()) {
                                completion_tokens = c;
                            }
                        }
                        let stop_reason = inner
                            .pointer("/delta/stop_reason")
                            .and_then(|v| v.as_str())
                            .unwrap_or("end_turn");
                        let finish_reason = match stop_reason {
                            "end_turn" | "stop_sequence" => "stop",
                            "max_tokens" => "length",
                            "tool_use" => "tool_calls",
                            other => other,
                        };
                        let stop_chunk = json!({
                            "id": id,
                            "object": "chat.completion.chunk",
                            "created": created,
                            "model": model,
                            "choices": [{
                                "index": 0,
                                "delta": {},
                                "finish_reason": finish_reason,
                            }],
                        });
                        yield Bytes::from(format!("data: {}\n\n", stop_chunk));
                    }
                    "message_stop" => {
                        let usage_chunk = json!({
                            "id": id,
                            "object": "chat.completion.chunk",
                            "created": created,
                            "model": model,
                            "choices": [],
                            "usage": {
                                "prompt_tokens": prompt_tokens,
                                "completion_tokens": completion_tokens,
                                "total_tokens": prompt_tokens + completion_tokens,
                            }
                        });
                        yield Bytes::from(format!("data: {}\n\n", usage_chunk));
                        yield Bytes::from_static(b"data: [DONE]\n\n");
                        emitted_done = true;
                    }
                    _ => {}
                }
            }
        }
        if !emitted_done {
            yield Bytes::from_static(b"data: [DONE]\n\n");
        }
    }
}
