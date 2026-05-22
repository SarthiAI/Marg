use anyhow::Context;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use clap::{Parser, ValueEnum};
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Clone)]
#[command(name = "marg-provider-stub", version, about = "Deterministic fake provider for Marg benchmarks (openai/anthropic/google modes + failure injection).")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1:18081")]
    bind: String,

    /// Wire protocol the stub speaks.
    #[arg(long, value_enum, default_value_t = Mode::Openai)]
    mode: Mode,

    /// Fixed latency added before the response (milliseconds).
    #[arg(long, default_value_t = 0)]
    latency_ms: u64,

    /// Per-token latency for streaming responses (milliseconds).
    #[arg(long, default_value_t = 0)]
    token_ms: u64,

    /// Number of completion tokens to emit per request.
    #[arg(long, default_value_t = 32)]
    output_tokens: u32,

    /// Inject this HTTP status code on a fraction of requests (see --inject-rate).
    #[arg(long, default_value_t = 0)]
    inject_status: u16,

    /// Fraction of requests that should receive the injected status, 0.0..=1.0.
    #[arg(long, default_value_t = 0.0)]
    inject_rate: f64,

    /// Inject for the first N requests, regardless of rate. 0 disables.
    #[arg(long, default_value_t = 0)]
    inject_first_n: u64,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Mode {
    Openai,
    Anthropic,
    Google,
}

#[derive(Clone)]
struct StubState {
    cfg: Arc<Cli>,
    request_counter: Arc<AtomicU64>,
}

#[derive(Debug, Deserialize)]
struct OpenAiRequest {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    stream: bool,
    #[serde(default)]
    messages: Vec<Message>,
    #[serde(default)]
    max_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct AnthropicRequest {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    stream: bool,
    #[serde(default)]
    messages: Vec<Message>,
    #[serde(default)]
    max_tokens: Option<u32>,
    #[serde(default)]
    system: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct GoogleRequest {
    #[serde(default)]
    contents: Vec<Value>,
    #[serde(default, rename = "generationConfig")]
    generation_config: Option<Value>,
    #[serde(default, rename = "systemInstruction")]
    system_instruction: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct Message {
    #[serde(default)]
    #[allow(dead_code)]
    role: Option<String>,
    #[serde(default)]
    content: Option<Value>,
}

#[derive(Debug, Clone, Copy)]
struct Usage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();

    let cli = Cli::parse();
    let addr: SocketAddr = cli.bind.parse().context("parsing --bind address")?;
    let state = StubState {
        cfg: Arc::new(cli.clone()),
        request_counter: Arc::new(AtomicU64::new(0)),
    };

    let app = match cli.mode {
        Mode::Openai => Router::new()
            .route("/v1/chat/completions", post(openai_chat))
            .route("/v1/models", get(openai_models))
            .with_state(state),
        Mode::Anthropic => Router::new()
            .route("/v1/messages", post(anthropic_messages))
            .with_state(state),
        Mode::Google => Router::new()
            .route(
                "/v1beta/models/:model_action",
                post(google_generate),
            )
            .with_state(state),
    };

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, mode = ?cli.mode, "marg-provider-stub listening");
    axum::serve(listener, app).await?;
    Ok(())
}

fn should_inject(state: &StubState) -> bool {
    let n = state.request_counter.fetch_add(1, Ordering::SeqCst) + 1;
    if state.cfg.inject_status == 0 {
        return false;
    }
    if state.cfg.inject_first_n > 0 && n <= state.cfg.inject_first_n {
        return true;
    }
    if state.cfg.inject_rate <= 0.0 {
        return false;
    }
    let rate = state.cfg.inject_rate.clamp(0.0, 1.0);
    let pick = (n.wrapping_mul(2654435761) % 1_000_000) as f64 / 1_000_000.0;
    pick < rate
}

fn inject_response(code: u16, mode: Mode) -> Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::SERVICE_UNAVAILABLE);
    let body = match mode {
        Mode::Openai => json!({
            "error": {"message": "injected failure", "type": "server_error", "code": code}
        }),
        Mode::Anthropic => json!({"type": "error", "error": {"type": "overloaded_error", "message": "injected failure"}}),
        Mode::Google => json!({"error": {"code": code, "message": "injected failure", "status": "UNAVAILABLE"}}),
    };
    (status, Json(body)).into_response()
}

async fn openai_models() -> impl IntoResponse {
    Json(json!({
        "object": "list",
        "data": [
            { "id": "gpt-4o", "object": "model" },
            { "id": "gpt-4o-mini", "object": "model" },
            { "id": "gpt-3.5-turbo", "object": "model" }
        ]
    }))
}

async fn openai_chat(
    State(state): State<StubState>,
    Json(req): Json<OpenAiRequest>,
) -> Result<Response, (StatusCode, String)> {
    if should_inject(&state) {
        return Ok(inject_response(state.cfg.inject_status, Mode::Openai));
    }
    if state.cfg.latency_ms > 0 {
        tokio::time::sleep(Duration::from_millis(state.cfg.latency_ms)).await;
    }
    let model = req.model.clone().unwrap_or_else(|| "gpt-4o-mini".to_string());
    let prompt = collect_messages_prompt(&req.messages);
    let prompt_tokens = estimate_tokens(&prompt);
    let target_output = req
        .max_tokens
        .map(|m| m.min(state.cfg.output_tokens))
        .unwrap_or(state.cfg.output_tokens);
    let completion_text = build_completion_text(&prompt, target_output as usize);
    let usage = Usage {
        prompt_tokens,
        completion_tokens: target_output,
        total_tokens: prompt_tokens + target_output,
    };

    if req.stream {
        Ok(openai_stream(state, model, completion_text, usage).await)
    } else {
        Ok(openai_json(model, completion_text, usage))
    }
}

async fn anthropic_messages(
    State(state): State<StubState>,
    Json(req): Json<AnthropicRequest>,
) -> Result<Response, (StatusCode, String)> {
    if should_inject(&state) {
        return Ok(inject_response(state.cfg.inject_status, Mode::Anthropic));
    }
    if state.cfg.latency_ms > 0 {
        tokio::time::sleep(Duration::from_millis(state.cfg.latency_ms)).await;
    }
    let model = req
        .model
        .clone()
        .unwrap_or_else(|| "claude-3-5-sonnet".to_string());
    let mut prompt = collect_messages_prompt(&req.messages);
    if let Some(sys) = &req.system {
        if let Some(s) = sys.as_str() {
            prompt.push_str(s);
        }
    }
    let prompt_tokens = estimate_tokens(&prompt);
    let target_output = req
        .max_tokens
        .map(|m| m.min(state.cfg.output_tokens))
        .unwrap_or(state.cfg.output_tokens);
    let completion_text = build_completion_text(&prompt, target_output as usize);
    let usage = Usage {
        prompt_tokens,
        completion_tokens: target_output,
        total_tokens: prompt_tokens + target_output,
    };
    if req.stream {
        Ok(anthropic_stream(state, model, completion_text, usage).await)
    } else {
        Ok(anthropic_json(model, completion_text, usage))
    }
}

async fn google_generate(
    State(state): State<StubState>,
    Path(model_action): Path<String>,
    Json(req): Json<GoogleRequest>,
) -> Result<Response, (StatusCode, String)> {
    if should_inject(&state) {
        return Ok(inject_response(state.cfg.inject_status, Mode::Google));
    }
    if state.cfg.latency_ms > 0 {
        tokio::time::sleep(Duration::from_millis(state.cfg.latency_ms)).await;
    }
    let (model, action) = match model_action.split_once(':') {
        Some((m, a)) => (m.to_string(), a.to_string()),
        None => (model_action.clone(), "generateContent".to_string()),
    };
    let stream = action == "streamGenerateContent";
    let prompt = collect_google_prompt(&req);
    let prompt_tokens = estimate_tokens(&prompt);
    let target_output = state.cfg.output_tokens;
    let completion_text = build_completion_text(&prompt, target_output as usize);
    let usage = Usage {
        prompt_tokens,
        completion_tokens: target_output,
        total_tokens: prompt_tokens + target_output,
    };
    let _ = req.generation_config;
    let _ = req.system_instruction;
    if stream {
        Ok(google_stream(state, model, completion_text, usage).await)
    } else {
        Ok(google_json(model, completion_text, usage))
    }
}

fn collect_messages_prompt(messages: &[Message]) -> String {
    let mut out = String::new();
    for m in messages {
        if let Some(content) = &m.content {
            if let Some(s) = content.as_str() {
                out.push_str(s);
                out.push(' ');
            } else if let Some(parts) = content.as_array() {
                for p in parts {
                    if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
                        out.push_str(t);
                        out.push(' ');
                    }
                }
            }
        }
    }
    out
}

fn collect_google_prompt(req: &GoogleRequest) -> String {
    let mut out = String::new();
    for c in &req.contents {
        if let Some(parts) = c.get("parts").and_then(|v| v.as_array()) {
            for p in parts {
                if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
                    out.push_str(t);
                    out.push(' ');
                }
            }
        }
    }
    out
}

fn estimate_tokens(text: &str) -> u32 {
    ((text.chars().count() as f64) / 4.0).ceil().max(1.0) as u32
}

fn build_completion_text(prompt: &str, tokens: usize) -> String {
    let base = if prompt.trim().is_empty() {
        "stub reply".to_string()
    } else {
        format!("stub reply to: {}", prompt.trim())
    };
    let pieces = std::iter::repeat("token").take(tokens).collect::<Vec<_>>().join(" ");
    if base.is_empty() {
        pieces
    } else {
        format!("{} | {}", base, pieces)
    }
}

fn openai_json(model: String, completion_text: String, usage: Usage) -> Response {
    let id = format!("chatcmpl-stub-{}", uuid::Uuid::new_v4());
    let created = chrono::Utc::now().timestamp();
    let body = json!({
        "id": id,
        "object": "chat.completion",
        "created": created,
        "model": model,
        "choices": [
            {
                "index": 0,
                "message": { "role": "assistant", "content": completion_text },
                "finish_reason": "stop"
            }
        ],
        "usage": {
            "prompt_tokens": usage.prompt_tokens,
            "completion_tokens": usage.completion_tokens,
            "total_tokens": usage.total_tokens
        }
    });
    Json(body).into_response()
}

async fn openai_stream(
    state: StubState,
    model: String,
    completion_text: String,
    usage: Usage,
) -> Response {
    let id = format!("chatcmpl-stub-{}", uuid::Uuid::new_v4());
    let created = chrono::Utc::now().timestamp();
    let token_count = usage.completion_tokens as usize;
    let token_ms = state.cfg.token_ms;

    let chunks: Vec<String> = (0..token_count)
        .map(|i| {
            let frag = if i == 0 {
                "token-0".to_string()
            } else {
                format!(" token-{}", i)
            };
            let chunk = json!({
                "id": id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model,
                "choices": [
                    {
                        "index": 0,
                        "delta": { "content": frag },
                        "finish_reason": Value::Null
                    }
                ]
            });
            format!("data: {}\n\n", chunk)
        })
        .collect();
    let _ = completion_text;

    let stop_chunk = format!(
        "data: {}\n\n",
        json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model.clone(),
            "choices": [ { "index": 0, "delta": {}, "finish_reason": "stop" } ]
        })
    );

    let usage_chunk = format!(
        "data: {}\n\n",
        json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [],
            "usage": {
                "prompt_tokens": usage.prompt_tokens,
                "completion_tokens": usage.completion_tokens,
                "total_tokens": usage.total_tokens
            }
        })
    );

    let done = "data: [DONE]\n\n".to_string();

    let body_stream = async_stream::stream! {
        for chunk in chunks {
            if token_ms > 0 {
                tokio::time::sleep(Duration::from_millis(token_ms)).await;
            }
            yield Ok::<_, std::io::Error>(bytes::Bytes::from(chunk));
        }
        yield Ok(bytes::Bytes::from(stop_chunk));
        yield Ok(bytes::Bytes::from(usage_chunk));
        yield Ok(bytes::Bytes::from(done));
    };

    let body = Body::from_stream(body_stream);
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(body)
        .unwrap()
}

fn anthropic_json(model: String, completion_text: String, usage: Usage) -> Response {
    let id = format!("msg_stub_{}", uuid::Uuid::new_v4());
    let body = json!({
        "id": id,
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": [{"type": "text", "text": completion_text}],
        "stop_reason": "end_turn",
        "stop_sequence": Value::Null,
        "usage": {
            "input_tokens": usage.prompt_tokens,
            "output_tokens": usage.completion_tokens
        }
    });
    Json(body).into_response()
}

async fn anthropic_stream(
    state: StubState,
    model: String,
    completion_text: String,
    usage: Usage,
) -> Response {
    let id = format!("msg_stub_{}", uuid::Uuid::new_v4());
    let token_count = usage.completion_tokens as usize;
    let token_ms = state.cfg.token_ms;

    let message_start = format!(
        "event: message_start\ndata: {}\n\n",
        json!({
            "type": "message_start",
            "message": {
                "id": id,
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [],
                "stop_reason": Value::Null,
                "stop_sequence": Value::Null,
                "usage": {"input_tokens": usage.prompt_tokens, "output_tokens": 0}
            }
        })
    );
    let block_start = format!(
        "event: content_block_start\ndata: {}\n\n",
        json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": ""}
        })
    );
    let _ = completion_text;
    let block_deltas: Vec<String> = (0..token_count)
        .map(|i| {
            let text = if i == 0 {
                "token-0".to_string()
            } else {
                format!(" token-{}", i)
            };
            format!(
                "event: content_block_delta\ndata: {}\n\n",
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "text_delta", "text": text}
                })
            )
        })
        .collect();
    let block_stop = "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n".to_string();
    let message_delta = format!(
        "event: message_delta\ndata: {}\n\n",
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn", "stop_sequence": Value::Null},
            "usage": {"output_tokens": usage.completion_tokens}
        })
    );
    let message_stop = "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".to_string();

    let body_stream = async_stream::stream! {
        yield Ok::<_, std::io::Error>(bytes::Bytes::from(message_start));
        yield Ok(bytes::Bytes::from(block_start));
        for chunk in block_deltas {
            if token_ms > 0 {
                tokio::time::sleep(Duration::from_millis(token_ms)).await;
            }
            yield Ok(bytes::Bytes::from(chunk));
        }
        yield Ok(bytes::Bytes::from(block_stop));
        yield Ok(bytes::Bytes::from(message_delta));
        yield Ok(bytes::Bytes::from(message_stop));
    };

    let body = Body::from_stream(body_stream);
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(body)
        .unwrap()
}

fn google_json(model: String, completion_text: String, usage: Usage) -> Response {
    let body = json!({
        "candidates": [{
            "content": {"parts": [{"text": completion_text}], "role": "model"},
            "finishReason": "STOP",
            "index": 0,
        }],
        "usageMetadata": {
            "promptTokenCount": usage.prompt_tokens,
            "candidatesTokenCount": usage.completion_tokens,
            "totalTokenCount": usage.total_tokens
        },
        "modelVersion": model,
    });
    Json(body).into_response()
}

async fn google_stream(
    state: StubState,
    model: String,
    completion_text: String,
    usage: Usage,
) -> Response {
    let token_count = usage.completion_tokens as usize;
    let token_ms = state.cfg.token_ms;
    let _ = completion_text;
    let chunks: Vec<String> = (0..token_count)
        .map(|i| {
            let text = if i == 0 {
                "token-0".to_string()
            } else {
                format!(" token-{}", i)
            };
            format!(
                "data: {}\n\n",
                json!({
                    "candidates": [{
                        "content": {"parts": [{"text": text}], "role": "model"},
                        "index": 0,
                    }]
                })
            )
        })
        .collect();
    let final_chunk = format!(
        "data: {}\n\n",
        json!({
            "candidates": [{
                "content": {"parts": [{"text": ""}], "role": "model"},
                "finishReason": "STOP",
                "index": 0,
            }],
            "usageMetadata": {
                "promptTokenCount": usage.prompt_tokens,
                "candidatesTokenCount": usage.completion_tokens,
                "totalTokenCount": usage.total_tokens
            },
            "modelVersion": model,
        })
    );
    let body_stream = async_stream::stream! {
        for chunk in chunks {
            if token_ms > 0 {
                tokio::time::sleep(Duration::from_millis(token_ms)).await;
            }
            yield Ok::<_, std::io::Error>(bytes::Bytes::from(chunk));
        }
        yield Ok(bytes::Bytes::from(final_chunk));
    };
    let body = Body::from_stream(body_stream);
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(body)
        .unwrap()
}
