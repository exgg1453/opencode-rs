//! OpenAI-compatible Chat Completions provider.

use std::collections::BTreeMap;

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use serde_json::{json, Value};

use crate::sse::{drive, EventProcessor, SseEvent};
use crate::{ChatRequest, ContentBlock, Provider, Role, StreamEvent};

pub struct OpenAiProvider {
    id: String,
    api_key: String,
    base_url: String,
    client: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            id: "openai".into(),
            api_key: api_key.into(),
            base_url: "https://api.openai.com/v1".into(),
            client: reqwest::Client::new(),
        }
    }

    /// Base URL up to and including the version segment, e.g. `https://api.openai.com/v1`.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_string();
        self
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Reads OPENAI_API_KEY (and optional OPENAI_BASE_URL).
    pub fn from_env() -> anyhow::Result<Self> {
        let key = std::env::var("OPENAI_API_KEY").map_err(|_| anyhow!("OPENAI_API_KEY is not set"))?;
        let mut p = Self::new(key);
        if let Ok(u) = std::env::var("OPENAI_BASE_URL") {
            if !u.is_empty() {
                p = p.with_base_url(u);
            }
        }
        Ok(p)
    }
}

pub fn build_body(req: &ChatRequest) -> Value {
    let mut messages: Vec<Value> = vec![];
    if let Some(s) = &req.system {
        messages.push(json!({"role":"system","content":s}));
    }
    for m in &req.messages {
        match m.role {
            Role::User => {
                let mut text = String::new();
                for b in &m.content {
                    match b {
                        ContentBlock::ToolResult { tool_use_id, content, .. } => {
                            messages.push(json!({"role":"tool","tool_call_id":tool_use_id,"content":content}));
                        }
                        ContentBlock::Text { text: t } => text.push_str(t),
                        ContentBlock::ToolUse { .. } => {}
                    }
                }
                if !text.is_empty() {
                    messages.push(json!({"role":"user","content":text}));
                }
            }
            Role::Assistant => {
                let mut text = String::new();
                let mut calls = vec![];
                for b in &m.content {
                    match b {
                        ContentBlock::Text { text: t } => text.push_str(t),
                        ContentBlock::ToolUse { id, name, input } => calls.push(json!({
                            "id": id, "type": "function",
                            "function": {"name": name, "arguments": input.to_string()}
                        })),
                        ContentBlock::ToolResult { .. } => {}
                    }
                }
                let mut msg = json!({"role":"assistant","content": if text.is_empty() { Value::Null } else { json!(text) }});
                if !calls.is_empty() {
                    msg["tool_calls"] = json!(calls);
                }
                messages.push(msg);
            }
        }
    }
    let mut body = json!({
        "model": req.model,
        "messages": messages,
        "max_tokens": req.max_tokens,
        "stream": true,
    });
    if !req.tools.is_empty() {
        body["tools"] = req
            .tools
            .iter()
            .map(|t| json!({"type":"function","function":{"name":t.name,"description":t.description,"parameters":t.input_schema}}))
            .collect();
    }
    body
}

#[derive(Default)]
pub struct OpenAiProcessor {
    calls: BTreeMap<u64, (String, String, String)>,
    finished: bool,
}

impl OpenAiProcessor {
    fn flush_calls(&mut self) -> anyhow::Result<Vec<StreamEvent>> {
        let mut out = vec![];
        for (_, (id, name, args)) in std::mem::take(&mut self.calls) {
            let input = if args.trim().is_empty() {
                json!({})
            } else {
                serde_json::from_str(&args).with_context(|| format!("bad tool arguments for {name}"))?
            };
            out.push(StreamEvent::ToolCall { id, name, input });
        }
        Ok(out)
    }
}

impl EventProcessor for OpenAiProcessor {
    fn process(&mut self, ev: &SseEvent) -> anyhow::Result<Vec<StreamEvent>> {
        let data = ev.data.trim();
        if data.is_empty() || self.finished {
            return Ok(vec![]);
        }
        if data == "[DONE]" {
            let mut out = self.flush_calls()?;
            self.finished = true;
            out.push(StreamEvent::Finish { stop_reason: "end_turn".into() });
            return Ok(out);
        }
        let v: Value = serde_json::from_str(data).with_context(|| format!("bad SSE json: {data}"))?;
        if let Some(e) = v.get("error") {
            let msg = e["message"].as_str().map(str::to_string).unwrap_or_else(|| e.to_string());
            return Err(anyhow!("openai stream error: {msg}"));
        }
        let mut out = vec![];
        let Some(choice) = v["choices"].get(0) else { return Ok(out) };
        let d = &choice["delta"];
        if let Some(t) = d["content"].as_str() {
            if !t.is_empty() {
                out.push(StreamEvent::TextDelta(t.to_string()));
            }
        }
        if let Some(tcs) = d["tool_calls"].as_array() {
            for tc in tcs {
                let idx = tc["index"].as_u64().unwrap_or(0);
                let e = self.calls.entry(idx).or_default();
                if let Some(id) = tc["id"].as_str() {
                    e.0 = id.to_string();
                }
                if let Some(n) = tc["function"]["name"].as_str() {
                    e.1.push_str(n);
                }
                if let Some(a) = tc["function"]["arguments"].as_str() {
                    e.2.push_str(a);
                }
            }
        }
        if let Some(fr) = choice["finish_reason"].as_str() {
            out.extend(self.flush_calls()?);
            self.finished = true;
            let reason = match fr {
                "tool_calls" | "function_call" => "tool_use",
                "stop" => "end_turn",
                "length" => "max_tokens",
                o => o,
            };
            out.push(StreamEvent::Finish { stop_reason: reason.into() });
        }
        Ok(out)
    }

    fn finish(&mut self) -> Vec<StreamEvent> {
        if self.finished {
            return vec![];
        }
        self.finished = true;
        let mut out = self.flush_calls().unwrap_or_default();
        out.push(StreamEvent::Finish { stop_reason: "end_turn".into() });
        out
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn id(&self) -> &str {
        &self.id
    }

    async fn stream(&self, req: ChatRequest) -> anyhow::Result<BoxStream<'static, anyhow::Result<StreamEvent>>> {
        let resp = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&build_body(&req))
            .send()
            .await
            .context("openai request failed")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("openai API error {status}: {text}"));
        }
        let bytes = resp.bytes_stream().map(|r| r.map(|b| b.to_vec()).map_err(anyhow::Error::from)).boxed();
        Ok(drive(bytes, OpenAiProcessor::default()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChatMessage;

    const CANNED: &str = "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hi\"}}]}\n\n\
data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\" there\"}}]}\n\n\
data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"bash\",\"arguments\":\"\"}}]}}]}\n\n\
data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"command\\\":\"}}]}}]}\n\n\
data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"ls\\\"}\"}}]}}]}\n\n\
data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n\
data: [DONE]\n\n";

    #[tokio::test]
    async fn parses_text_and_tool_calls() {
        let chunks: Vec<anyhow::Result<Vec<u8>>> = CANNED.as_bytes().chunks(50).map(|c| Ok(c.to_vec())).collect();
        let s = drive(futures::stream::iter(chunks), OpenAiProcessor::default());
        let evs: Vec<StreamEvent> = s.map(|e| e.unwrap()).collect().await;
        assert_eq!(
            evs,
            vec![
                StreamEvent::TextDelta("Hi".into()),
                StreamEvent::TextDelta(" there".into()),
                StreamEvent::ToolCall { id: "call_1".into(), name: "bash".into(), input: json!({"command":"ls"}) },
                StreamEvent::Finish { stop_reason: "tool_use".into() },
            ]
        );
    }

    #[test]
    fn converts_messages() {
        let req = ChatRequest {
            model: "m".into(),
            system: Some("s".into()),
            messages: vec![
                ChatMessage { role: Role::User, content: vec![ContentBlock::Text { text: "q".into() }] },
                ChatMessage {
                    role: Role::Assistant,
                    content: vec![ContentBlock::ToolUse { id: "c".into(), name: "n".into(), input: json!({"a":1}) }],
                },
                ChatMessage {
                    role: Role::User,
                    content: vec![ContentBlock::ToolResult { tool_use_id: "c".into(), content: "r".into(), is_error: false }],
                },
            ],
            tools: vec![],
            max_tokens: 5,
        };
        let b = build_body(&req);
        let m = b["messages"].as_array().unwrap();
        assert_eq!(m[0]["role"], "system");
        assert_eq!(m[2]["tool_calls"][0]["function"]["arguments"], "{\"a\":1}");
        assert_eq!(m[3]["role"], "tool");
        assert_eq!(m[3]["tool_call_id"], "c");
    }
}
