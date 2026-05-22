use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use chrono::Utc;
use futures_util::StreamExt;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;

use crate::error::ProviderError;
use crate::request::{ChatRequest, ChatResponse, ChatStream, ChatUsage};
use crate::ChatCompletionsClient;

pub struct GoogleClient {
    http: Client,
    base_url: String,
    api_key: SecretString,
    api_version: String,
}

impl GoogleClient {
    pub fn new(
        base_url: String,
        api_key: SecretString,
        api_version: String,
        timeout: Duration,
    ) -> Result<Self, ProviderError> {
        let http = Client::builder()
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(64)
            .http2_keep_alive_interval(Some(Duration::from_secs(20)))
            .http2_keep_alive_while_idle(true)
            .build()
            .map_err(|e| ProviderError::Internal(format!("build reqwest client: {}", e)))?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            api_version,
        })
    }

    fn endpoint(&self, model: &str, stream: bool) -> String {
        let model_enc = utf8_percent_encode(model, NON_ALPHANUMERIC).to_string();
        let method = if stream {
            "streamGenerateContent"
        } else {
            "generateContent"
        };
        let key_enc = utf8_percent_encode(self.api_key.expose_secret(), NON_ALPHANUMERIC).to_string();
        if stream {
            format!(
                "{}/{}/models/{}:{}?alt=sse&key={}",
                self.base_url, self.api_version, model_enc, method, key_enc
            )
        } else {
            format!(
                "{}/{}/models/{}:{}?key={}",
                self.base_url, self.api_version, model_enc, method, key_enc
            )
        }
    }
}

fn build_gemini_body(req: &ChatRequest) -> Value {
    let mut system_parts: Vec<String> = Vec::new();
    let mut contents: Vec<Value> = Vec::new();
    for raw in req.messages() {
        let role = raw.get("role").and_then(|v| v.as_str()).unwrap_or("user");
        if role == "system" {
            if let Some(text) = content_as_text(raw.get("content")) {
                system_parts.push(text);
            }
            continue;
        }
        let parts = content_to_parts(raw.get("content"));
        let gemini_role = match role {
            "assistant" => "model",
            _ => "user",
        };
        contents.push(json!({"role": gemini_role, "parts": parts}));
    }
    let mut generation_config = serde_json::Map::new();
    if let Some(t) = req.temperature() {
        generation_config.insert("temperature".into(), json!(t));
    }
    if let Some(p) = req.top_p() {
        generation_config.insert("topP".into(), json!(p));
    }
    if let Some(m) = req.max_output_tokens {
        generation_config.insert("maxOutputTokens".into(), json!(m));
    }
    let stops = req.stop_sequences();
    if !stops.is_empty() {
        generation_config.insert("stopSequences".into(), json!(stops));
    }
    let mut body = json!({"contents": contents});
    if !system_parts.is_empty() {
        body["systemInstruction"] = json!({
            "parts": [{"text": system_parts.join("\n\n")}]
        });
    }
    if !generation_config.is_empty() {
        body["generationConfig"] = Value::Object(generation_config);
    }
    body
}

fn content_to_parts(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::String(s)) => vec![json!({"text": s})],
        Some(Value::Array(parts)) => {
            let mut out = Vec::new();
            for part in parts {
                let kind = part.get("type").and_then(|v| v.as_str()).unwrap_or("text");
                if kind == "text" {
                    if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                        out.push(json!({"text": text}));
                    }
                } else if kind == "image_url" {
                    let url = part
                        .get("image_url")
                        .and_then(|v| v.get("url"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if let Some(data) = url.strip_prefix("data:") {
                        if let Some((meta, b64)) = data.split_once(",") {
                            let mime = meta
                                .split(';')
                                .next()
                                .unwrap_or("image/png")
                                .to_string();
                            out.push(json!({
                                "inlineData": {"mimeType": mime, "data": b64}
                            }));
                        }
                    }
                }
            }
            if out.is_empty() {
                vec![json!({"text": ""})]
            } else {
                out
            }
        }
        _ => vec![json!({"text": ""})],
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

fn gemini_to_openai_response(model: &str, value: &Value) -> (Value, ChatUsage) {
    let id = format!("chatcmpl-google-{}", Uuid::new_v4());
    let mut text = String::new();
    let mut finish_reason: &str = "stop";
    if let Some(candidates) = value.get("candidates").and_then(|v| v.as_array()) {
        if let Some(first) = candidates.first() {
            if let Some(parts) = first.pointer("/content/parts").and_then(|v| v.as_array()) {
                for part in parts {
                    if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
                        text.push_str(t);
                    }
                }
            }
            if let Some(reason) = first.get("finishReason").and_then(|v| v.as_str()) {
                finish_reason = map_finish_reason(reason);
            }
        }
    }
    let prompt_tokens = value
        .pointer("/usageMetadata/promptTokenCount")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let completion_tokens = value
        .pointer("/usageMetadata/candidatesTokenCount")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let total_tokens = value
        .pointer("/usageMetadata/totalTokenCount")
        .and_then(|v| v.as_u64())
        .unwrap_or(prompt_tokens + completion_tokens);
    let usage = ChatUsage {
        prompt_tokens,
        completion_tokens,
        total_tokens,
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

fn map_finish_reason(reason: &str) -> &'static str {
    match reason {
        "STOP" => "stop",
        "MAX_TOKENS" => "length",
        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" => "content_filter",
        _ => "stop",
    }
}

#[async_trait]
impl ChatCompletionsClient for GoogleClient {
    fn provider_name(&self) -> &'static str {
        "google"
    }

    async fn chat_completion(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let body = build_gemini_body(&req);
        let resp = self
            .http
            .post(self.endpoint(&req.model, false))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;
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
                message: format!("google response was not valid json: {}", e),
            })?;
        let (translated, usage) = gemini_to_openai_response(&req.model, &parsed);
        let body_bytes = Bytes::from(serde_json::to_vec(&translated).map_err(|e| {
            ProviderError::Internal(format!("serialize openai response: {}", e))
        })?);
        Ok(ChatResponse {
            status,
            body: body_bytes,
            usage,
            model: req.model.clone(),
        })
    }

    async fn chat_completion_stream(&self, req: ChatRequest) -> Result<ChatStream, ProviderError> {
        let body = build_gemini_body(&req);
        let resp = self
            .http
            .post(self.endpoint(&req.model, true))
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .json(&body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let raw = resp.bytes().await?;
            let body_text = String::from_utf8_lossy(&raw).to_string();
            return Err(ProviderError::Upstream {
                status,
                message: body_text,
            });
        }
        let model = req.model.clone();
        let upstream = resp.bytes_stream();
        let translated = translate_gemini_stream(upstream, model);
        Ok(ChatStream {
            status,
            byte_stream: translated.boxed(),
        })
    }
}

fn translate_gemini_stream<S>(
    stream: S,
    model: String,
) -> impl futures::Stream<Item = Result<Bytes, std::io::Error>> + Send
where
    S: futures::Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    async_stream::try_stream! {
        let mut buf = BytesMut::new();
        let id = format!("chatcmpl-google-{}", Uuid::new_v4());
        let created = Utc::now().timestamp();
        let mut sent_role = false;
        let mut prompt_tokens: u64 = 0;
        let mut completion_tokens: u64 = 0;
        let mut total_tokens: u64 = 0;
        let mut finish_reason: Option<String> = None;
        futures::pin_mut!(stream);
        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
            buf.extend_from_slice(&bytes);
            while let Some(event) = pop_sse_event(&mut buf) {
                let Some(payload) = extract_data_payload(&event) else { continue; };
                let Ok(value) = serde_json::from_str::<Value>(&payload) else { continue; };
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
                if let Some(candidates) = value.get("candidates").and_then(|v| v.as_array()) {
                    if let Some(first) = candidates.first() {
                        if let Some(parts) = first.pointer("/content/parts").and_then(|v| v.as_array()) {
                            let mut delta_text = String::new();
                            for part in parts {
                                if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
                                    delta_text.push_str(t);
                                }
                            }
                            if !delta_text.is_empty() {
                                let chunk_json = json!({
                                    "id": id,
                                    "object": "chat.completion.chunk",
                                    "created": created,
                                    "model": model,
                                    "choices": [{
                                        "index": 0,
                                        "delta": {"content": delta_text},
                                        "finish_reason": Value::Null,
                                    }],
                                });
                                yield Bytes::from(format!("data: {}\n\n", chunk_json));
                            }
                        }
                        if let Some(reason) = first.get("finishReason").and_then(|v| v.as_str()) {
                            finish_reason = Some(map_finish_reason(reason).to_string());
                        }
                    }
                }
                if let Some(meta) = value.get("usageMetadata") {
                    if let Some(p) = meta.get("promptTokenCount").and_then(|v| v.as_u64()) {
                        prompt_tokens = p;
                    }
                    if let Some(c) = meta.get("candidatesTokenCount").and_then(|v| v.as_u64()) {
                        completion_tokens = c;
                    }
                    if let Some(t) = meta.get("totalTokenCount").and_then(|v| v.as_u64()) {
                        total_tokens = t;
                    }
                }
            }
        }
        let reason = finish_reason.unwrap_or_else(|| "stop".to_string());
        let stop_chunk = json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": reason,
            }],
        });
        yield Bytes::from(format!("data: {}\n\n", stop_chunk));
        if total_tokens == 0 {
            total_tokens = prompt_tokens + completion_tokens;
        }
        let usage_chunk = json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [],
            "usage": {
                "prompt_tokens": prompt_tokens,
                "completion_tokens": completion_tokens,
                "total_tokens": total_tokens,
            }
        });
        yield Bytes::from(format!("data: {}\n\n", usage_chunk));
        yield Bytes::from_static(b"data: [DONE]\n\n");
    }
}

fn extract_data_payload(event: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(event).ok()?;
    let mut data_buf = String::new();
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("data:") {
            if !data_buf.is_empty() {
                data_buf.push('\n');
            }
            data_buf.push_str(rest.trim_start());
        }
    }
    if data_buf.is_empty() {
        None
    } else {
        Some(data_buf)
    }
}

fn pop_sse_event(buf: &mut BytesMut) -> Option<Vec<u8>> {
    let bytes = buf.as_ref();
    let mut boundary: Option<(usize, usize)> = None;
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'\n' && bytes[i + 1] == b'\n' {
            boundary = Some((i, 2));
            break;
        }
        if i + 3 < bytes.len()
            && bytes[i] == b'\r'
            && bytes[i + 1] == b'\n'
            && bytes[i + 2] == b'\r'
            && bytes[i + 3] == b'\n'
        {
            boundary = Some((i, 4));
            break;
        }
        i += 1;
    }
    let (pos, len) = boundary?;
    let mut event = Vec::with_capacity(pos);
    event.extend_from_slice(&buf[..pos]);
    use bytes::Buf;
    buf.advance(pos + len);
    Some(event)
}
