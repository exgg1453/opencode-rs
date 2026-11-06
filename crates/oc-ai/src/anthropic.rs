//! Anthropic Messages API provider.

use std::collections::HashMap;

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use serde_json::{json, Value};

use crate::sse::{drive, EventProcessor, SseEvent};
use crate::{ChatRequest, ContentBlock, Provider, Role, StreamEvent};

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

/// Build the JSON request body.
pub fn build_body(req: &ChatRequest) -> Value {
    let messages: Vec<Value> = req
        .messages
        .iter()
        .map(|m| {
            let role = match m.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            let content: Vec<Value> = m
                .content
                .iter()
                .map(|b| match b {
                    ContentBlock::Text { text } => json!({"type":"text","text":text}),
                    ContentBlock::ToolUse { id, name, input } => {
                        json!({"type":"tool_use","id":id,"name":name,"input":input})
                    }
                    ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                        json!({"type":"tool_result","tool_use_id":tool_use_id,"content":content,"is_error":is_error})
                    }
                })
                .collect();
            json!({"role":role,"content":content})
        })
        .collect();
    let mut body = json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "messages": messages,
        "stream": true,
    });
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
                    Some("input_json_delta") => {
                        if let (Some(p), Some(t)) = (d["partial_json"].as_str(), self.tools.get_mut(&idx)) {
                            t.2.push_str(p);
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
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
                return Err(anyhow!("anthropic stream error: {msg}"));
            }
            _ => {}
        }
        Ok(out)
    }

    fn finish(&mut self) -> Vec<StreamEvent> {
        if self.finished {
            vec![]
        } else {
            self.finished = true;
            vec![StreamEvent::Finish { stop_reason: self.stop_reason.clone().unwrap_or_else(|| "end_turn".into()) }]
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
        let bytes = resp.bytes_stream().map(|r| r.map(|b| b.to_vec()).map_err(anyhow::Error::from)).boxed();
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
        };
        let b = build_body(&req);
        assert_eq!(b["system"], "sys");
        assert_eq!(b["messages"][0]["content"][0]["type"], "tool_result");
        assert_eq!(b["tools"][0]["name"], "t");
        assert_eq!(b["stream"], true);
    }
}
