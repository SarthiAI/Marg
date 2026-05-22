pub mod anthropic;
pub mod bedrock;
pub mod error;
pub mod event_stream;
pub mod google;
pub mod openai;
pub mod request;
pub mod sigv4;

pub use anthropic::AnthropicClient;
pub use bedrock::BedrockClient;
pub use error::ProviderError;
pub use google::GoogleClient;
pub use openai::OpenAIClient;
pub use request::{ChatRequest, ChatResponse, ChatStream, ChatUsage};

use async_trait::async_trait;

#[async_trait]
pub trait ChatCompletionsClient: Send + Sync {
    fn provider_name(&self) -> &'static str;

    async fn chat_completion(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError>;

    async fn chat_completion_stream(&self, req: ChatRequest) -> Result<ChatStream, ProviderError>;
}
