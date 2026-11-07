//! Anthropic Messages API provider.

use std::collections::HashMap;

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use serde_json::{json, Value};

use crate::sse::{drive, EventProcessor, SseEvent};
use crate::{ChatRequest, ContentBlock, Provider, ProviderError, Role, StreamEvent};

pub struct AnthropicProvider {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
    backoff: std::time::Duration,
}

impl AnthropicProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: "https://api.anthropic.com".into(),
            client: reqwest::Client::new(),
            backoff: crate::http::DEFAULT_BACKOFF,
        }
    }

    pub fn with_backoff(mut self, d: std::time::Duration) -> Self {
        self.backoff = d;
        self
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_string();
        self
    }

    /// Reads ANTHROPIC_API_KEY (and optional ANTHROPIC_BASE_URL).
    pub fn from_env() -> anyhow::Result<Self> {
        let key = std::env::var("ANTHROPIC_API_KEY").map_err(|_| anyhow!("ANTHROPIC_API_KEY is not set"))?;
        let mut p = Self::new(key);
        if let Ok(u) = std::env::var("ANTHROPIC_BASE_URL") {
            if !u.is_empty() {
                p = p.with_base_url(u);
            }
        }
        Ok(p)
    }
}

/// Room for thinking plus answer on models that think by default; thinking counts toward
/// `max_tokens`, so the caller's smaller value would truncate responses.
const MODERN_MIN_MAX_TOKENS: u32 = 64_000;

/// Models from before adaptive thinking: they take `budget_tokens`. Newer ones reject it
/// with a 400, so unknown ids are treated as modern.
fn uses_budget_tokens(model: &str) -> bool {
    const LEGACY: &[&str] = &[
        "claude-3",
        "claude-haiku-4-5",
        "claude-sonnet-4-5",
        "claude-opus-4-5",
        "claude-opus-4-1",
        "claude-opus-4-0",
        "claude-sonnet-4-0",
        "claude-opus-4-2025",
        "claude-sonnet-4-2025",
    ];
    LEGACY.iter().any(|p| model.starts_with(p))
}

/// Build the JSON request body.
pub fn build_body(req: &ChatRequest) -> Value {
    let messages: Vec<Value> = req
        .messages
        .iter()
        .filter_map(|m| {
            let role = match m.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            let content: Vec<Value> = m
                .content
                .iter()
                .filter_map(|b| {
                    Some(match b {
                        ContentBlock::Text { text } => json!({"type":"text","text":text}),
                        ContentBlock::ToolUse { id, name, input } => {
                            json!({"type":"tool_use","id":id,"name":name,"input":input})
                        }
                        ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                            json!({"type":"tool_result","tool_use_id":tool_use_id,"content":content,"is_error":is_error})
                        }
                        // Anthropic rejects thinking blocks without their signature (e.g. ones
                        // that came from another provider), so only signed blocks go back.
                        ContentBlock::Thinking { thinking, signature: Some(signature) } => {
                            json!({"type":"thinking","thinking":thinking,"signature":signature})
                        }
                        ContentBlock::Thinking { signature: None, .. } => return None,
                        ContentBlock::RedactedThinking { data } => json!({"type":"redacted_thinking","data":data}),
                    })
                })
                .collect();
            if content.is_empty() {
                return None;
            }
            Some(json!({"role":role,"content":content}))
        })
        .collect();
    let legacy = uses_budget_tokens(&req.model);
    let max_tokens = match req.thinking {
        _ if !legacy => req.max_tokens.max(MODERN_MIN_MAX_TOKENS),
        // max_tokens must exceed the thinking budget, so the budget comes on top of the answer room.
        Some(level) => req.max_tokens.saturating_add(level.budget_tokens()),
        None => req.max_tokens,
    };
    let mut body = json!({
        "model": req.model,
        "max_tokens": max_tokens,
        "messages": messages,
        "stream": true,
    });
    match req.thinking {
        Some(level) if legacy => body["thinking"] = json!({"type":"enabled","budget_tokens":level.budget_tokens()}),
        Some(level) => {
            // Without display: "summarized" current models stream thinking blocks with empty text.
            body["thinking"] = json!({"type":"adaptive","display":"summarized"});
            body["output_config"] = json!({"effort":level.as_str()});
        }
        None => {}
    }
    if let Some(s) = &req.system {
        body["system"] = json!(s);
    }
    if !req.tools.is_empty() {
        body["tools"] = req
            .tools
            .iter()
            .map(|t| json!({"name":t.name,"description":t.description,"input_schema":t.input_schema}))
            .collect();
    }
    body
}

#[derive(Default)]
pub struct AnthropicProcessor {
    /// block index -> (id, name, accumulated json)
    tools: HashMap<u64, (String, String, String)>,
    /// block index -> accumulated signature of a thinking block
    signatures: HashMap<u64, String>,
    stop_reason: Option<String>,
    finished: bool,
}

impl EventProcessor for AnthropicProcessor {
    fn process(&mut self, ev: &SseEvent) -> anyhow::Result<Vec<StreamEvent>> {
        if ev.data.trim().is_empty() {
            return Ok(vec![]);
        }
        let v: Value = serde_json::from_str(&ev.data).with_context(|| format!("bad SSE json: {}", ev.data))?;
        let kind = v.get("type").and_then(Value::as_str).or(ev.event.as_deref()).unwrap_or("");
        let idx = v.get("index").and_then(Value::as_u64).unwrap_or(0);
        let mut out = vec![];
        match kind {
            "content_block_start" => {
                let cb = &v["content_block"];
                if cb["type"] == "tool_use" {
                    self.tools.insert(
                        idx,
                        (
                            cb["id"].as_str().unwrap_or_default().to_string(),
                            cb["name"].as_str().unwrap_or_default().to_string(),
                            String::new(),
                        ),
                    );
                } else if cb["type"] == "thinking" {
                    self.signatures.insert(idx, cb["signature"].as_str().unwrap_or_default().to_string());
                    if let Some(t) = cb["thinking"].as_str().filter(|t| !t.is_empty()) {
                        out.push(StreamEvent::ThinkingDelta(t.to_string()));
                    }
                } else if cb["type"] == "redacted_thinking" {
                    out.push(StreamEvent::RedactedThinking(cb["data"].as_str().unwrap_or_default().to_string()));
                } else if let Some(t) = cb["text"].as_str() {
                    if !t.is_empty() {
                        out.push(StreamEvent::TextDelta(t.to_string()));
                    }
                }
            }
            "content_block_delta" => {
                let d = &v["delta"];
                match d["type"].as_str() {
                    Some("text_delta") => {
                        if let Some(t) = d["text"].as_str() {
                            out.push(StreamEvent::TextDelta(t.to_string()));
                        }
                    }
                    Some("thinking_delta") => {
                        if let Some(t) = d["thinking"].as_str() {
                            out.push(StreamEvent::ThinkingDelta(t.to_string()));
                        }
                    }
                    Some("signature_delta") => {
                        if let (Some(sig), Some(acc)) = (d["signature"].as_str(), self.signatures.get_mut(&idx)) {
                            acc.push_str(sig);
                        }
                    }
                    Some("input_json_delta") => {
                        if let (Some(p), Some(t)) = (d["partial_json"].as_str(), self.tools.get_mut(&idx)) {
                            t.2.push_str(p);
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                if let Some(sig) = self.signatures.remove(&idx) {
                    if !sig.is_empty() {
                        out.push(StreamEvent::ThinkingSignature(sig));
                    }
                }
                if let Some((id, name, json)) = self.tools.remove(&idx) {
                    let input = if json.trim().is_empty() {
                        json!({})
                    } else {
                        serde_json::from_str(&json).with_context(|| format!("bad tool input json for {name}"))?
                    };
                    out.push(StreamEvent::ToolCall { id, name, input });
                }
            }
            "message_delta" => {
                if let Some(s) = v["delta"]["stop_reason"].as_str() {
                    self.stop_reason = Some(s.to_string());
                }
            }
            "message_stop" => {
                self.finished = true;
                out.push(StreamEvent::Finish {
                    stop_reason: self.stop_reason.clone().unwrap_or_else(|| "end_turn".into()),
                });
            }
            "error" => {
                let msg = v["error"]["message"].as_str().unwrap_or("unknown error");
                let kind = v["error"]["type"].as_str().unwrap_or_default();
                let retryable = matches!(kind, "overloaded_error" | "api_error");
                let label = if kind.is_empty() { String::new() } else { format!(" ({kind})") };
                return Err(anyhow::Error::new(ProviderError::new(
                    None,
                    retryable,
                    format!("anthropic stream error{label}: {msg}"),
                )));
            }
            _ => {}
        }
        Ok(out)
    }

    fn finish(&mut self) -> anyhow::Result<Vec<StreamEvent>> {
        if self.finished {
            Ok(vec![])
        } else {
            Err(crate::sse::truncated("anthropic"))
        }
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn id(&self) -> &str {
        "anthropic"
    }

    async fn stream(&self, req: ChatRequest) -> anyhow::Result<BoxStream<'static, anyhow::Result<StreamEvent>>> {
        let body = build_body(&req);
        let url = format!("{}/v1/messages", self.base_url);
        let resp = crate::http::send_with_retry("anthropic", self.backoff, || {
            self.client
                .post(&url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body)
        })
        .await?;
        let bytes = resp.bytes_stream().map(|r| r.map(|b| b.to_vec()).map_err(|e| crate::http::stream_error("anthropic", e))).boxed();
        Ok(drive(bytes, AnthropicProcessor::default()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChatMessage, ToolSpec};

    const CANNED: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\"}}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n\
event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"read\",\"input\":{}}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"file_pa\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"th\\\":\\\"a.txt\\\"}\"}}\n\n\
event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n\
event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"}}\n\n\
event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

    #[tokio::test]
    async fn parses_text_and_tool_use() {
        // split into awkward chunks
        let data = CANNED.as_bytes();
        let chunks: Vec<anyhow::Result<Vec<u8>>> = data.chunks(37).map(|c| Ok(c.to_vec())).collect();
        let s = drive(futures::stream::iter(chunks), AnthropicProcessor::default());
        let evs: Vec<StreamEvent> = s.map(|e| e.unwrap()).collect().await;
        assert_eq!(
            evs,
            vec![
                StreamEvent::TextDelta("Hel".into()),
                StreamEvent::TextDelta("lo".into()),
                StreamEvent::ToolCall { id: "toolu_1".into(), name: "read".into(), input: json!({"file_path":"a.txt"}) },
                StreamEvent::Finish { stop_reason: "tool_use".into() },
            ]
        );
    }

    #[tokio::test]
    async fn error_event_fails() {
        let chunks: Vec<anyhow::Result<Vec<u8>>> =
            vec![Ok(b"event: error\ndata: {\"type\":\"error\",\"error\":{\"message\":\"overloaded\"}}\n\n".to_vec())];
        let s = drive(futures::stream::iter(chunks), AnthropicProcessor::default());
        let evs: Vec<_> = s.collect().await;
        assert!(evs[0].is_err());
    }

    #[test]
    fn body_shape() {
        let req = ChatRequest {
            model: "m".into(),
            system: Some("sys".into()),
            messages: vec![ChatMessage {
                role: Role::User,
                content: vec![ContentBlock::ToolResult { tool_use_id: "a".into(), content: "x".into(), is_error: false }],
            }],
            tools: vec![ToolSpec { name: "t".into(), description: "d".into(), input_schema: json!({"type":"object"}) }],
            max_tokens: 10,
            thinking: None,
        };
        let b = build_body(&req);
        assert_eq!(b["system"], "sys");
        assert_eq!(b["messages"][0]["content"][0]["type"], "tool_result");
        assert_eq!(b["tools"][0]["name"], "t");
        assert_eq!(b["stream"], true);
    }

    const THINKING: &str = "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Let me \"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"think.\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig-\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"abc\"}}\n\n\
event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"redacted_thinking\",\"data\":\"ENC\"}}\n\n\
event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"read\",\"input\":{}}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"file_path\\\":\\\"a\\\"}\"}}\n\n\
event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":2}\n\n\
event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"}}\n\n\
event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

    #[tokio::test]
    async fn parses_thinking_signature_and_redacted() {
        let chunks: Vec<anyhow::Result<Vec<u8>>> = THINKING.as_bytes().chunks(41).map(|c| Ok(c.to_vec())).collect();
        let s = drive(futures::stream::iter(chunks), AnthropicProcessor::default());
        let evs: Vec<StreamEvent> = s.map(|e| e.unwrap()).collect().await;
        assert_eq!(
            evs,
            vec![
                StreamEvent::ThinkingDelta("Let me ".into()),
                StreamEvent::ThinkingDelta("think.".into()),
                StreamEvent::ThinkingSignature("sig-abc".into()),
                StreamEvent::RedactedThinking("ENC".into()),
                StreamEvent::ToolCall { id: "toolu_1".into(), name: "read".into(), input: json!({"file_path":"a"}) },
                StreamEvent::Finish { stop_reason: "tool_use".into() },
            ]
        );
    }

    #[test]
    fn thinking_request_and_history_round_trip() {
        let req = ChatRequest {
            model: "m".into(),
            system: None,
            messages: vec![
                ChatMessage { role: Role::User, content: vec![ContentBlock::Text { text: "q".into() }] },
                ChatMessage {
                    role: Role::Assistant,
                    content: vec![
                        ContentBlock::Thinking { thinking: "hmm".into(), signature: Some("sig".into()) },
                        ContentBlock::Thinking { thinking: "unsigned".into(), signature: None },
                        ContentBlock::RedactedThinking { data: "ENC".into() },
                        ContentBlock::ToolUse { id: "t".into(), name: "read".into(), input: json!({}) },
                    ],
                },
                // only an unsigned block: the whole message is dropped rather than sent empty
                ChatMessage { role: Role::Assistant, content: vec![ContentBlock::Thinking { thinking: "x".into(), signature: None }] },
            ],
            tools: vec![],
            max_tokens: 1000,
            thinking: Some(crate::ThinkingLevel::Medium),
        };
        let b = build_body(&req);
        assert_eq!(b["thinking"], json!({"type":"adaptive","display":"summarized"}));
        assert_eq!(b["output_config"], json!({"effort":"medium"}));
        assert_eq!(b["max_tokens"], 64_000);
        let msgs = b["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        let c = msgs[1]["content"].as_array().unwrap();
        assert_eq!(c.len(), 3);
        assert_eq!(c[0], json!({"type":"thinking","thinking":"hmm","signature":"sig"}));
        assert_eq!(c[1], json!({"type":"redacted_thinking","data":"ENC"}));
        assert_eq!(c[2]["type"], "tool_use");
        let mut off = req.clone();
        off.thinking = None;
        let b = build_body(&off);
        assert!(b.get("thinking").is_none());
        assert!(b.get("output_config").is_none());
        assert_eq!(b["max_tokens"], 64_000);
    }

    #[test]
    fn current_models_never_get_budget_tokens() {
        for model in ["claude-sonnet-5-5", "claude-opus-5-5", "claude-haiku-5-5", "claude-sonnet-4-6", "some-future-model"] {
            let req = ChatRequest { model: model.into(), system: None, messages: vec![], tools: vec![], max_tokens: 8192, thinking: Some(crate::ThinkingLevel::High) };
            let b = build_body(&req);
            assert_eq!(b["thinking"]["type"], "adaptive", "{model}");
            assert!(b["thinking"].get("budget_tokens").is_none(), "{model}");
            assert_eq!(b["output_config"]["effort"], "high");
        }
    }

    #[test]
    fn legacy_models_use_budgets() {
        for (l, n) in [(crate::ThinkingLevel::Low, 4096), (crate::ThinkingLevel::High, 32768)] {
            let req = ChatRequest { model: "claude-haiku-4-5".into(), system: None, messages: vec![], tools: vec![], max_tokens: 8192, thinking: Some(l) };
            let b = build_body(&req);
            assert_eq!(b["thinking"], json!({"type":"enabled","budget_tokens":n}));
            assert_eq!(b["max_tokens"], 8192 + n);
            assert!(b.get("output_config").is_none());
        }
    }

    #[tokio::test]
    async fn clean_close_before_message_stop_is_retryable() {
        let data = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"par\"}}\n\n";
        let bytes = futures::stream::iter(vec![Ok(data.as_bytes().to_vec())]);
        let evs: Vec<_> = drive(bytes, AnthropicProcessor::default()).collect().await;
        assert_eq!(evs.len(), 2);
        assert!(crate::is_retryable(evs[1].as_ref().unwrap_err()));
    }

    #[tokio::test]
    async fn error_event_retryability() {
        for (kind, retry) in [("overloaded_error", true), ("api_error", true), ("invalid_request_error", false)] {
            let data = format!("event: error\ndata: {{\"type\":\"error\",\"error\":{{\"type\":\"{kind}\",\"message\":\"x\"}}}}\n\n");
            let chunks: Vec<anyhow::Result<Vec<u8>>> = vec![Ok(data.into_bytes())];
            let s = drive(futures::stream::iter(chunks), AnthropicProcessor::default());
            let evs: Vec<_> = s.collect().await;
            let e = evs[0].as_ref().unwrap_err();
            assert_eq!(crate::is_retryable(e), retry, "{kind}");
        }
    }
}
