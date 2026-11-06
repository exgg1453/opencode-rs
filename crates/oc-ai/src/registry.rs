//! Provider lookup by id. Owner: providers agent; keep these signatures.

use std::sync::Arc;

use anyhow::{anyhow, bail};

use crate::google::GoogleProvider;
use crate::openai::{AuthStyle, UrlStyle};
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

/// Static description of a known provider, for listing in the CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub env_vars: &'static [&'static str],
    pub default_base_url: Option<&'static str>,
}

const fn info(
    id: &'static str,
    name: &'static str,
    env_vars: &'static [&'static str],
    default_base_url: Option<&'static str>,
) -> ProviderInfo {
    ProviderInfo { id, name, env_vars, default_base_url }
}

/// OpenAI-compatible presets (id, name, env vars, base url). Local ones need no key.
const PRESETS: &[ProviderInfo] = &[
    info("openrouter", "OpenRouter", &["OPENROUTER_API_KEY"], Some("https://openrouter.ai/api/v1")),
    info("groq", "Groq", &["GROQ_API_KEY"], Some("https://api.groq.com/openai/v1")),
    info("deepseek", "DeepSeek", &["DEEPSEEK_API_KEY"], Some("https://api.deepseek.com/v1")),
    info("mistral", "Mistral", &["MISTRAL_API_KEY"], Some("https://api.mistral.ai/v1")),
    info("xai", "xAI", &["XAI_API_KEY"], Some("https://api.x.ai/v1")),
    info("together", "Together AI", &["TOGETHER_API_KEY", "TOGETHER_AI_API_KEY"], Some("https://api.together.xyz/v1")),
    info("fireworks", "Fireworks AI", &["FIREWORKS_API_KEY"], Some("https://api.fireworks.ai/inference/v1")),
    info("cerebras", "Cerebras", &["CEREBRAS_API_KEY"], Some("https://api.cerebras.ai/v1")),
    info("perplexity", "Perplexity", &["PERPLEXITY_API_KEY"], Some("https://api.perplexity.ai")),
    info("ollama", "Ollama", &[], Some("http://localhost:11434/v1")),
    info("lmstudio", "LM Studio", &[], Some("http://localhost:1234/v1")),
];

fn preset(id: &str) -> Option<&'static ProviderInfo> {
    PRESETS.iter().find(|p| p.id == id)
}

/// All providers `build_provider` knows by id.
pub fn known_providers() -> Vec<ProviderInfo> {
    let mut v = vec![
        info("anthropic", "Anthropic", &["ANTHROPIC_API_KEY"], Some("https://api.anthropic.com")),
        info("openai", "OpenAI", &["OPENAI_API_KEY"], Some("https://api.openai.com/v1")),
        info(
            "google",
            "Google Gemini",
            &["GEMINI_API_KEY", "GOOGLE_GENERATIVE_AI_API_KEY"],
            Some(crate::google::DEFAULT_BASE_URL),
        ),
        info("azure", "Azure OpenAI", &["AZURE_API_KEY", "AZURE_RESOURCE_NAME"], None),
    ];
    v.extend(PRESETS.iter().cloned());
    v
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn first_env(names: &[&str]) -> Option<String> {
    names.iter().find_map(|n| env_nonempty(n))
}

fn extra_str(opts: &ProviderOptions, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| opts.extra.get(*k).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(str::to_string))
}

fn nonempty(s: &Option<String>) -> Option<String> {
    s.clone().filter(|v| !v.is_empty())
}

/// Azure OpenAI provider from options + env (`AZURE_API_KEY`, `AZURE_RESOURCE_NAME`).
fn build_azure(opts: &ProviderOptions) -> anyhow::Result<OpenAiProvider> {
    let key = nonempty(&opts.api_key)
        .or_else(|| env_nonempty("AZURE_API_KEY"))
        .ok_or_else(|| anyhow!("AZURE_API_KEY is not set"))?;
    let base = match nonempty(&opts.base_url).or_else(|| extra_str(opts, &["baseURL", "baseUrl", "endpoint"])) {
        Some(b) => b,
        None => {
            let resource = extra_str(opts, &["resourceName", "resource_name"])
                .or_else(|| env_nonempty("AZURE_RESOURCE_NAME"))
                .ok_or_else(|| anyhow!("azure needs a base_url or resourceName option (or AZURE_RESOURCE_NAME)"))?;
            format!("https://{resource}.openai.azure.com")
        }
    };
    let api_version = extra_str(opts, &["apiVersion", "api_version", "api-version"]);
    let v1 = opts.extra.get("useV1").and_then(|v| v.as_bool()).unwrap_or(false);
    let style = if v1 {
        UrlStyle::AzureV1 { api_version: api_version.unwrap_or_else(|| "preview".into()) }
    } else {
        UrlStyle::AzureDeployments { api_version: api_version.unwrap_or_else(|| "2024-10-21".into()) }
    };
    Ok(OpenAiProvider::new(key)
        .with_id("azure")
        .with_base_url(base)
        .with_auth(AuthStyle::Header("api-key".into()))
        .with_url_style(style))
}

pub fn build_provider(id: &str, opts: &ProviderOptions) -> anyhow::Result<Arc<dyn Provider>> {
    Ok(match id {
        "anthropic" => {
            let key = nonempty(&opts.api_key)
                .or_else(|| env_nonempty("ANTHROPIC_API_KEY"))
                .ok_or_else(|| anyhow!("ANTHROPIC_API_KEY is not set"))?;
            let mut p = AnthropicProvider::new(key);
            if let Some(u) = nonempty(&opts.base_url).or_else(|| env_nonempty("ANTHROPIC_BASE_URL")) {
                p = p.with_base_url(u);
            }
            Arc::new(p)
        }
        "openai" => {
            let key = nonempty(&opts.api_key)
                .or_else(|| env_nonempty("OPENAI_API_KEY"))
                .ok_or_else(|| anyhow!("OPENAI_API_KEY is not set"))?;
            let mut p = OpenAiProvider::new(key);
            if let Some(u) = nonempty(&opts.base_url).or_else(|| env_nonempty("OPENAI_BASE_URL")) {
                p = p.with_base_url(u);
            }
            Arc::new(p)
        }
        "google" => {
            let key = nonempty(&opts.api_key)
                .or_else(|| first_env(&["GEMINI_API_KEY", "GOOGLE_GENERATIVE_AI_API_KEY"]))
                .ok_or_else(|| anyhow!("GEMINI_API_KEY (or GOOGLE_GENERATIVE_AI_API_KEY) is not set"))?;
            let mut p = GoogleProvider::new(key);
            if let Some(u) = nonempty(&opts.base_url) {
                p = p.with_base_url(u);
            }
            Arc::new(p)
        }
        "azure" => Arc::new(build_azure(opts)?),
        "amazon-bedrock" => bail!("amazon-bedrock is not supported yet (requires AWS SigV4 signing)"),
        other => {
            if let Some(pre) = preset(other) {
                let key = nonempty(&opts.api_key).or_else(|| first_env(pre.env_vars));
                if key.is_none() && !pre.env_vars.is_empty() {
                    bail!("{} is not set (needed for provider \"{}\")", pre.env_vars[0], other);
                }
                let base = nonempty(&opts.base_url).or_else(|| pre.default_base_url.map(str::to_string)).unwrap();
                let mut p = OpenAiProvider::new(key.clone().unwrap_or_default()).with_id(other).with_base_url(base);
                if key.is_none() {
                    p = p.with_auth(AuthStyle::None);
                }
                Arc::new(p)
            } else if let Some(base) = nonempty(&opts.base_url) {
                let env = format!("{}_API_KEY", other.to_uppercase().replace(['-', '.'], "_"));
                let key = nonempty(&opts.api_key).or_else(|| env_nonempty(&env));
                let mut p = OpenAiProvider::new(key.clone().unwrap_or_default()).with_id(other).with_base_url(base);
                if key.is_none() {
                    p = p.with_auth(AuthStyle::None);
                }
                Arc::new(p)
            } else {
                let ids: Vec<&str> = known_providers().iter().map(|p| p.id).collect();
                bail!("unknown provider \"{other}\" (supported: {}; or set base_url for a custom OpenAI-compatible provider)", ids.join(", "))
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openai::OpenAiProvider;
    use crate::{ChatRequest, StreamEvent};
    use futures::StreamExt;
    use serde_json::json;

    fn opts(key: Option<&str>, base: Option<&str>) -> ProviderOptions {
        ProviderOptions { api_key: key.map(Into::into), base_url: base.map(Into::into), extra: Default::default() }
    }

    #[test]
    fn presets_table() {
        let k = known_providers();
        for id in [
            "anthropic", "openai", "google", "azure", "openrouter", "groq", "deepseek", "mistral", "xai", "together",
            "fireworks", "cerebras", "perplexity", "ollama", "lmstudio",
        ] {
            assert!(k.iter().any(|p| p.id == id), "{id} missing");
        }
        let mut ids: Vec<_> = k.iter().map(|p| p.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), k.len());
        assert_eq!(preset("ollama").unwrap().default_base_url, Some("http://localhost:11434/v1"));
        assert!(preset("lmstudio").unwrap().env_vars.is_empty());
        assert_eq!(preset("groq").unwrap().env_vars, &["GROQ_API_KEY"]);
    }

    #[test]
    fn builds_with_explicit_keys() {
        for id in ["anthropic", "openai", "google", "openrouter", "groq", "deepseek", "mistral", "xai", "together", "fireworks", "cerebras", "perplexity"] {
            let p = build_provider(id, &opts(Some("k"), None)).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(p.id(), id);
        }
        // local presets need no key
        assert_eq!(build_provider("ollama", &opts(None, None)).unwrap().id(), "ollama");
        assert_eq!(build_provider("lmstudio", &opts(None, None)).unwrap().id(), "lmstudio");
    }

    #[test]
    fn custom_and_unknown() {
        let p = build_provider("my-llm", &opts(None, Some("http://x/v1"))).unwrap();
        assert_eq!(p.id(), "my-llm");
        let e = build_provider("nope", &opts(Some("k"), None)).err().unwrap().to_string();
        assert!(e.contains("unknown provider"), "{e}");
    }

    #[test]
    fn azure_url_and_header() {
        let mut o = opts(Some("azkey"), None);
        o.extra.insert("resourceName".into(), json!("myres"));
        o.extra.insert("apiVersion".into(), json!("2025-01-01"));
        let p = build_azure(&o).unwrap();
        assert_eq!(
            p.endpoint("gpt-4o"),
            "https://myres.openai.azure.com/openai/deployments/gpt-4o/chat/completions?api-version=2025-01-01"
        );
        // explicit base_url + v1 style
        let mut o = opts(Some("azkey"), Some("https://corp.example.com/"));
        o.extra.insert("useV1".into(), json!(true));
        let p = build_azure(&o).unwrap();
        assert_eq!(p.endpoint("m"), "https://corp.example.com/openai/v1/chat/completions?api-version=preview");
        // missing resource
        std::env::remove_var("AZURE_RESOURCE_NAME");
        assert!(build_azure(&opts(Some("k"), None)).is_err());
    }

    #[tokio::test]
    async fn azure_sends_api_key_header_to_mock() {
        let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let m = crate::http::testutil::mock(vec![(200, "text/event-stream", sse.into())]).await;
        let o = opts(Some("azkey"), Some(&m.url));
        let p = build_azure(&o).unwrap();
        let req = ChatRequest { model: "dep".into(), system: None, messages: vec![], tools: vec![], max_tokens: 1 };
        let evs: Vec<_> = p.stream(req).await.unwrap().collect().await;
        assert_eq!(evs[0].as_ref().unwrap(), &StreamEvent::TextDelta("ok".into()));
        let r = m.requests.lock().unwrap()[0].to_ascii_lowercase();
        assert!(r.starts_with("post /openai/deployments/dep/chat/completions?api-version=2024-10-21"), "{r}");
        assert!(r.contains("api-key: azkey"));
        assert!(!r.contains("authorization:"));
    }

    #[tokio::test]
    async fn openai_retries_429_before_stream() {
        let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let m = crate::http::testutil::mock(vec![
            (429, "application/json", r#"{"error":{"message":"slow down"}}"#.into()),
            (502, "text/plain", "bad gateway".into()),
            (200, "text/event-stream", sse.into()),
        ])
        .await;
        let p = OpenAiProvider::new("k").with_base_url(m.url.clone()).with_backoff(std::time::Duration::from_millis(1));
        let req = ChatRequest { model: "m".into(), system: None, messages: vec![], tools: vec![], max_tokens: 1 };
        let evs: Vec<_> = p.stream(req).await.unwrap().collect().await;
        assert_eq!(evs[0].as_ref().unwrap(), &StreamEvent::TextDelta("hi".into()));
        assert_eq!(m.hits.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn openai_error_is_readable() {
        let m = crate::http::testutil::mock(vec![(401, "application/json", r#"{"error":{"message":"Incorrect API key"}}"#.into())]).await;
        let p = OpenAiProvider::new("k").with_base_url(m.url.clone());
        let req = ChatRequest { model: "m".into(), system: None, messages: vec![], tools: vec![], max_tokens: 1 };
        let e = p.stream(req).await.err().unwrap().to_string();
        assert!(e.contains("401") && e.contains("Incorrect API key"), "{e}");
    }
}
