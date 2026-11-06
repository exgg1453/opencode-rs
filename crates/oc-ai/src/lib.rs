//! LLM provider abstraction (port of `packages/ai`).
//!
//! PUBLIC API CONTRACT — other crates depend on these signatures.
//! Owner: phase-1 agent. Additions are fine; do not change existing signatures.

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};

pub mod anthropic;
pub mod google;
pub mod http;
pub mod openai;
pub mod registry;
pub mod sse;

pub use registry::{build_provider, ProviderOptions};

pub use anthropic::AnthropicProvider;
pub use google::GoogleProvider;
pub use openai::OpenAiProvider;
pub use registry::{known_providers, ProviderInfo};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, content: String, is_error: bool },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema of the tool's input.
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    TextDelta(String),
    /// A complete tool call (input fully accumulated).
    ToolCall { id: String, name: String, input: serde_json::Value },
    Finish { stop_reason: String },
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &str;
    async fn stream(
        &self,
        req: ChatRequest,
    ) -> anyhow::Result<BoxStream<'static, anyhow::Result<StreamEvent>>>;
}
