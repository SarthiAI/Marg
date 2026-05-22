pub mod error;
pub mod openai;
pub mod request;

pub use error::ProviderError;
pub use openai::OpenAIClient;
pub use request::{ChatRequest, ChatResponse, ChatStream, ChatUsage};

use async_trait::async_trait;

#[async_trait]
pub trait ChatCompletionsClient: Send + Sync {
    fn provider_name(&self) -> &'static str;

    async fn chat_completion(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError>;

    async fn chat_completion_stream(&self, req: ChatRequest) -> Result<ChatStream, ProviderError>;
}
