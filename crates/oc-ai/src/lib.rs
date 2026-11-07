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
    /// Model reasoning. `signature` is Anthropic's opaque proof, required to send the block back.
    Thinking {
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// Reasoning the provider encrypted; must be echoed back verbatim (Anthropic).
    RedactedThinking { data: String },
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
    /// Reasoning effort; `None` leaves thinking off.
    pub thinking: Option<ThinkingLevel>,
}

/// How much the model should think before answering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    Low,
    Medium,
    High,
}

impl ThinkingLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            ThinkingLevel::Low => "low",
            ThinkingLevel::Medium => "medium",
            ThinkingLevel::High => "high",
        }
    }

    /// Thinking token budget for providers that take one (Anthropic, Gemini).
    pub fn budget_tokens(self) -> u32 {
        match self {
            ThinkingLevel::Low => 4096,
            ThinkingLevel::Medium => 16384,
            ThinkingLevel::High => 32768,
        }
    }

    /// Parse a setting: "off" (or "none") gives `Ok(None)`.
    pub fn parse_setting(s: &str) -> Result<Option<ThinkingLevel>, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "none" => Ok(None),
            "low" => Ok(Some(ThinkingLevel::Low)),
            "medium" => Ok(Some(ThinkingLevel::Medium)),
            "high" => Ok(Some(ThinkingLevel::High)),
            other => Err(format!("invalid thinking level \"{other}\" (expected off, low, medium or high)")),
        }
    }
}

impl std::fmt::Display for ThinkingLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Typed provider failure so callers can decide whether a retry makes sense.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError {
    pub status: Option<u16>,
    pub retryable: bool,
    pub message: String,
}

impl ProviderError {
    pub fn new(status: Option<u16>, retryable: bool, message: impl Into<String>) -> Self {
        Self { status, retryable, message: message.into() }
    }

    /// 408/429/5xx (including Anthropic's 529 overloaded) are worth retrying.
    pub fn status_retryable(status: u16) -> bool {
        matches!(status, 408 | 429) || (500..600).contains(&status)
    }
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProviderError {}

/// True when `err` (or any cause) is a retryable [`ProviderError`].
pub fn is_retryable(err: &anyhow::Error) -> bool {
    err.chain().any(|c| c.downcast_ref::<ProviderError>().is_some_and(|p| p.retryable))
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    TextDelta(String),
    /// Incremental reasoning text.
    ThinkingDelta(String),
    /// Closes the current thinking block with its signature (Anthropic).
    ThinkingSignature(String),
    /// A complete redacted thinking block (Anthropic).
    RedactedThinking(String),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_setting_parses() {
        assert_eq!(ThinkingLevel::parse_setting("off"), Ok(None));
        assert_eq!(ThinkingLevel::parse_setting("High"), Ok(Some(ThinkingLevel::High)));
        assert!(ThinkingLevel::parse_setting("max").is_err());
    }

    #[test]
    fn thinking_block_serde_round_trip() {
        let b = ContentBlock::Thinking { thinking: "x".into(), signature: None };
        let j = serde_json::to_string(&b).unwrap();
        assert_eq!(j, r#"{"type":"thinking","thinking":"x"}"#);
        assert_eq!(serde_json::from_str::<ContentBlock>(&j).unwrap(), b);
        let r = ContentBlock::RedactedThinking { data: "d".into() };
        assert_eq!(serde_json::to_string(&r).unwrap(), r#"{"type":"redacted_thinking","data":"d"}"#);
    }

    #[test]
    fn retryable_through_context() {
        let e = anyhow::Error::new(ProviderError::new(Some(529), true, "overloaded")).context("while streaming");
        assert!(is_retryable(&e));
        assert!(!is_retryable(&anyhow::anyhow!("plain")));
    }
}
