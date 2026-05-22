use anyhow::Context;
use axum::body::Body;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::post;
use axum::Router;
use clap::Parser;
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::time::Duration;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Clone)]
#[command(name = "marg-provider-stub", version, about = "OpenAI-compatible fake provider for Marg benchmarks")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1:18081")]
    bind: String,

    /// Fixed latency added before the response (milliseconds).
    #[arg(long, default_value_t = 0)]
    latency_ms: u64,

    /// Per-token latency for streaming responses (milliseconds).
    #[arg(long, default_value_t = 0)]
    token_ms: u64,

    /// Number of completion tokens to emit per request.
    #[arg(long, default_value_t = 32)]
    output_tokens: u32,
}

#[derive(Clone)]
struct StubState {
    cfg: Cli,
}

#[derive(Debug, Deserialize)]
struct ChatRequest {
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
struct Message {
    #[serde(default)]
    #[allow(dead_code)]
    role: Option<String>,
    #[serde(default)]
    content: Option<Value>,
}

#[derive(Debug, Serialize, Clone, Copy)]
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
    let state = StubState { cfg: cli.clone() };

    let app = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/models", axum::routing::get(list_models))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "marg-provider-stub listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn list_models() -> impl IntoResponse {
    Json(json!({
        "object": "list",
        "data": [
            { "id": "gpt-4o", "object": "model" },
            { "id": "gpt-4o-mini", "object": "model" },
            { "id": "gpt-3.5-turbo", "object": "model" }
        ]
    }))
}

async fn chat_completions(
    State(state): State<StubState>,
    Json(req): Json<ChatRequest>,
) -> Result<Response, (StatusCode, String)> {
    if state.cfg.latency_ms > 0 {
        tokio::time::sleep(Duration::from_millis(state.cfg.latency_ms)).await;
    }

    let model = req.model.clone().unwrap_or_else(|| "gpt-4o-mini".to_string());
    let prompt = collect_prompt(&req);
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
        Ok(stream_response(state, model, completion_text, usage).await)
    } else {
        Ok(json_response(model, completion_text, usage))
    }
}

fn collect_prompt(req: &ChatRequest) -> String {
    let mut out = String::new();
    for m in &req.messages {
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

fn json_response(model: String, completion_text: String, usage: Usage) -> Response {
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

async fn stream_response(
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
                format!("token-0")
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
                        "finish_reason": serde_json::Value::Null
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
    let _ = stream::iter::<Vec<u32>>(vec![]).next();

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(body)
        .unwrap()
}
