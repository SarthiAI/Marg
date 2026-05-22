use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use chrono::Utc;
use futures_util::StreamExt;
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;

use crate::error::ProviderError;
use crate::request::{ChatRequest, ChatResponse, ChatStream, ChatUsage};
use crate::ChatCompletionsClient;

pub struct AnthropicClient {
    http: Client,
    base_url: String,
    api_key: SecretString,
    api_version: String,
    default_max_tokens: u32,
}

impl AnthropicClient {
    pub fn new(
        base_url: String,
        api_key: SecretString,
        api_version: String,
        default_max_tokens: u32,
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
            default_max_tokens: default_max_tokens.max(1),
        })
    }

    fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.base_url)
    }
}

fn build_anthropic_body(req: &ChatRequest, default_max_tokens: u32) -> Value {
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
        messages.push(json!({
            "role": anth_role,
            "content": content,
        }));
    }
    let max_tokens = req.max_output_tokens.unwrap_or(default_max_tokens);
    let mut body = json!({
        "model": req.model,
        "max_tokens": max_tokens,
        "messages": messages,
        "stream": req.stream,
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
                } else if kind == "image_url" {
                    let url = part
                        .get("image_url")
                        .and_then(|v| v.get("url"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if let Some(data) = url.strip_prefix("data:") {
                        if let Some((meta, b64)) = data.split_once(",") {
                            let media_type = meta
                                .split(';')
                                .next()
                                .unwrap_or("image/png")
                                .to_string();
                            out.push(json!({
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": media_type,
                                    "data": b64,
                                }
                            }));
                            continue;
                        }
                    }
                    out.push(json!({
                        "type": "image",
                        "source": {"type": "url", "url": url}
                    }));
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

fn anthropic_to_openai_response(model: &str, value: &Value) -> (Value, ChatUsage) {
    let id = value
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("chatcmpl-anth-{}", Uuid::new_v4()));
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
impl ChatCompletionsClient for AnthropicClient {
    fn provider_name(&self) -> &'static str {
        "anthropic"
    }

    async fn chat_completion(&self, mut req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        req.stream = false;
        if let Some(obj) = req.raw.as_object_mut() {
            obj.insert("stream".to_string(), Value::Bool(false));
        }
        let body = build_anthropic_body(&req, self.default_max_tokens);
        let resp = self
            .http
            .post(self.endpoint())
            .header("x-api-key", self.api_key.expose_secret())
            .header("anthropic-version", &self.api_version)
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
                message: format!("anthropic response was not valid json: {}", e),
            })?;
        let (translated, usage) = anthropic_to_openai_response(&req.model, &parsed);
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

    async fn chat_completion_stream(&self, mut req: ChatRequest) -> Result<ChatStream, ProviderError> {
        req.stream = true;
        if let Some(obj) = req.raw.as_object_mut() {
            obj.insert("stream".to_string(), Value::Bool(true));
        }
        let model = req.model.clone();
        let body = build_anthropic_body(&req, self.default_max_tokens);
        let resp = self
            .http
            .post(self.endpoint())
            .header("x-api-key", self.api_key.expose_secret())
            .header("anthropic-version", &self.api_version)
            .header("accept", "text/event-stream")
            .header("content-type", "application/json")
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
        let upstream = resp.bytes_stream();
        let translated = translate_anthropic_stream(upstream, model);
        Ok(ChatStream {
            status,
            byte_stream: translated.boxed(),
        })
    }
}

fn translate_anthropic_stream<S>(
    stream: S,
    model: String,
) -> impl futures::Stream<Item = Result<Bytes, std::io::Error>> + Send
where
    S: futures::Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    async_stream::try_stream! {
        let mut buf = BytesMut::new();
        let id = format!("chatcmpl-anth-{}", Uuid::new_v4());
        let created = Utc::now().timestamp();
        let mut sent_role = false;
        let mut prompt_tokens: u64 = 0;
        let mut completion_tokens: u64 = 0;
        let mut emitted_done = false;
        futures::pin_mut!(stream);
        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
            buf.extend_from_slice(&bytes);
            while let Some(event) = pop_sse_event(&mut buf) {
                let Some(parsed) = parse_anthropic_event(&event) else { continue; };
                match parsed.kind.as_str() {
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
                        if let Some(msg) = parsed.data.get("message") {
                            if let Some(u) = msg.get("usage") {
                                if let Some(p) = u.get("input_tokens").and_then(|v| v.as_u64()) {
                                    prompt_tokens = p;
                                }
                                if let Some(c) = u.get("output_tokens").and_then(|v| v.as_u64()) {
                                    completion_tokens = c;
                                }
                            }
                        }
                    }
                    "content_block_delta" => {
                        let delta_type = parsed
                            .data
                            .get("delta")
                            .and_then(|d| d.get("type"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if delta_type == "text_delta" {
                            if let Some(text) = parsed
                                .data
                                .get("delta")
                                .and_then(|d| d.get("text"))
                                .and_then(|v| v.as_str())
                            {
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
                        if let Some(u) = parsed.data.get("usage") {
                            if let Some(c) = u.get("output_tokens").and_then(|v| v.as_u64()) {
                                completion_tokens = c;
                            }
                        }
                        let stop_reason = parsed
                            .data
                            .get("delta")
                            .and_then(|d| d.get("stop_reason"))
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
                    "error" => {
                        let msg = parsed
                            .data
                            .get("error")
                            .and_then(|e| e.get("message"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("anthropic stream error")
                            .to_string();
                        Err(std::io::Error::new(std::io::ErrorKind::Other, msg))?;
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

struct AnthropicEvent {
    kind: String,
    data: Value,
}

fn parse_anthropic_event(event: &[u8]) -> Option<AnthropicEvent> {
    let text = std::str::from_utf8(event).ok()?;
    let mut kind = String::new();
    let mut data_buf = String::new();
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("event:") {
            kind = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("data:") {
            if !data_buf.is_empty() {
                data_buf.push('\n');
            }
            data_buf.push_str(rest.trim_start());
        }
    }
    if data_buf.is_empty() {
        return None;
    }
    let data: Value = serde_json::from_str(&data_buf).ok()?;
    if kind.is_empty() {
        kind = data
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
    }
    Some(AnthropicEvent { kind, data })
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
