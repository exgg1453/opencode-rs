//! Provider lookup by id. Owner: providers agent; keep these signatures.

use std::sync::Arc;

use crate::{AnthropicProvider, OpenAiProvider, Provider};

/// Per-provider settings resolved by oc-core from config and stored credentials.
#[derive(Debug, Clone, Default)]
pub struct ProviderOptions {
    /// Stored credential; providers fall back to their env var when None.
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    /// The provider's raw `options` object from opencode.json.
    pub extra: serde_json::Map<String, serde_json::Value>,
}

pub fn build_provider(id: &str, opts: &ProviderOptions) -> anyhow::Result<Arc<dyn Provider>> {
    let _ = opts;
    Ok(match id {
        "anthropic" => Arc::new(AnthropicProvider::from_env()?),
        "openai" => Arc::new(OpenAiProvider::from_env()?),
        other => anyhow::bail!("unknown provider \"{other}\" (supported: anthropic, openai)"),
    })
}
