use bytes::Bytes;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

use crate::error::ProviderError;

pub type ProviderByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>;

#[derive(Debug, Clone, Serialize, Deserialize, Default, Copy)]
pub struct ChatUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub raw: serde_json::Value,
    pub model: String,
    pub stream: bool,
    pub max_output_tokens: Option<u32>,
    pub estimated_input_tokens: u64,
}

impl ChatRequest {
    pub fn parse(body: &[u8]) -> Result<Self, ProviderError> {
        let raw: serde_json::Value = serde_json::from_slice(body)?;
        let model = raw
            .get("model")
            .and_then(|v| v.as_str())
            .ok_or(ProviderError::MissingField("model"))?
            .to_string();
        let stream = raw.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);
        let max_output_tokens = raw
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .or_else(|| raw.get("max_completion_tokens").and_then(|v| v.as_u64()))
            .map(|n| n.min(u32::MAX as u64) as u32);
        let input_chars = collect_input_chars(&raw);
        let estimated_input_tokens = ((input_chars as f64) / 4.0).ceil() as u64;
        Ok(Self {
            raw,
            model,
            stream,
            max_output_tokens,
            estimated_input_tokens,
        })
    }

    pub fn set_target_model(&mut self, model: &str) {
        self.model = model.to_string();
        if let Some(obj) = self.raw.as_object_mut() {
            obj.insert("model".to_string(), serde_json::Value::String(model.to_string()));
        }
    }

    pub fn ensure_stream_usage(&mut self) {
        if !self.stream {
            return;
        }
        if let Some(obj) = self.raw.as_object_mut() {
            let entry = obj
                .entry("stream_options".to_string())
                .or_insert(serde_json::json!({}));
            if let Some(opts) = entry.as_object_mut() {
                opts.entry("include_usage".to_string())
                    .or_insert(serde_json::Value::Bool(true));
            }
        }
    }

    pub fn messages(&self) -> &[serde_json::Value] {
        self.raw
            .get("messages")
            .and_then(|v| v.as_array())
            .map(|a| a.as_slice())
            .unwrap_or(&[])
    }

    pub fn temperature(&self) -> Option<f64> {
        self.raw.get("temperature").and_then(|v| v.as_f64())
    }

    pub fn top_p(&self) -> Option<f64> {
        self.raw.get("top_p").and_then(|v| v.as_f64())
    }

    pub fn stop_sequences(&self) -> Vec<String> {
        match self.raw.get("stop") {
            Some(serde_json::Value::String(s)) => vec![s.clone()],
            Some(serde_json::Value::Array(arr)) => arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect(),
            _ => Vec::new(),
        }
    }
}

fn collect_input_chars(raw: &serde_json::Value) -> usize {
    let Some(messages) = raw.get("messages").and_then(|v| v.as_array()) else {
        return 0;
    };
    let mut total = 0usize;
    for msg in messages {
        if let Some(content) = msg.get("content") {
            total += chars_in_content(content);
        }
    }
    total
}

fn chars_in_content(content: &serde_json::Value) -> usize {
    match content {
        serde_json::Value::String(s) => s.chars().count(),
        serde_json::Value::Array(parts) => parts
            .iter()
            .map(|p| {
                p.get("text")
                    .and_then(|t| t.as_str())
                    .map(|s| s.chars().count())
                    .unwrap_or(0)
            })
            .sum(),
        _ => 0,
    }
}

pub struct ChatResponse {
    pub status: u16,
    pub body: Bytes,
    pub usage: ChatUsage,
    pub model: String,
}

pub struct ChatStream {
    pub status: u16,
    pub byte_stream: ProviderByteStream,
}
