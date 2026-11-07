//! Google Gemini API provider (`streamGenerateContent` over SSE).

use std::collections::HashMap;

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use serde_json::{json, Value};

use crate::sse::{drive, EventProcessor, SseEvent};
use crate::{ChatRequest, ContentBlock, Provider, ProviderError, Role, StreamEvent};

pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

pub struct GoogleProvider {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
    backoff: std::time::Duration,
}

impl GoogleProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: DEFAULT_BASE_URL.into(),
            client: reqwest::Client::new(),
            backoff: crate::http::DEFAULT_BACKOFF,
        }
    }

    /// Base URL including the version segment, e.g. `https://generativelanguage.googleapis.com/v1beta`.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_string();
        self
    }

    pub fn with_backoff(mut self, d: std::time::Duration) -> Self {
        self.backoff = d;
        self
    }

    /// Reads GEMINI_API_KEY or GOOGLE_GENERATIVE_AI_API_KEY.
    pub fn from_env() -> anyhow::Result<Self> {
        let key = std::env::var("GEMINI_API_KEY")
            .or_else(|_| std::env::var("GOOGLE_GENERATIVE_AI_API_KEY"))
            .map_err(|_| anyhow!("GEMINI_API_KEY (or GOOGLE_GENERATIVE_AI_API_KEY) is not set"))?;
        Ok(Self::new(key))
    }

    pub fn endpoint(&self, model: &str) -> String {
        let model = model.strip_prefix("models/").unwrap_or(model);
        format!("{}/models/{}:streamGenerateContent?alt=sse", self.base_url, model)
    }
}

/// Gemini accepts only an OpenAPI subset of JSON Schema; drop unsupported keywords.
fn sanitize_schema(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut out = serde_json::Map::new();
            for (k, val) in m {
                if matches!(k.as_str(), "$schema" | "additionalProperties" | "$id" | "$ref" | "definitions" | "$defs") {
                    continue;
                }
                // Keep property maps intact (keys are user names), sanitize their values.
                if k == "properties" {
                    if let Value::Object(props) = val {
                        let p: serde_json::Map<String, Value> = props.iter().map(|(n, s)| (n.clone(), sanitize_schema(s))).collect();
                        out.insert(k.clone(), Value::Object(p));
                        continue;
                    }
                }
                out.insert(k.clone(), sanitize_schema(val));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(sanitize_schema).collect()),
        o => o.clone(),
    }
}

pub fn build_body(req: &ChatRequest) -> Value {
    // Gemini function responses need the function name; map tool_use ids to names.
    let mut names: HashMap<&str, &str> = HashMap::new();
    for m in &req.messages {
        for b in &m.content {
            if let ContentBlock::ToolUse { id, name, .. } = b {
                names.insert(id, name);
            }
        }
    }
    let contents: Vec<Value> = req
        .messages
        .iter()
        .filter_map(|m| {
            let parts: Vec<Value> = m
                .content
                .iter()
                .filter_map(|b| {
                    Some(match b {
                        ContentBlock::Text { text } => json!({"text": text}),
                        ContentBlock::ToolUse { name, input, .. } => json!({"functionCall": {"name": name, "args": input}}),
                        ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                            let name = names.get(tool_use_id.as_str()).copied().unwrap_or("tool");
                            let response = if *is_error { json!({"error": content}) } else { json!({"output": content}) };
                            json!({"functionResponse": {"name": name, "response": response}})
                        }
                        // Reasoning is not sent back to Gemini.
                        ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. } => return None,
                    })
                })
                .collect();
            if parts.is_empty() {
                return None;
            }
            let role = match m.role {
                Role::User => "user",
                Role::Assistant => "model",
            };
            Some(json!({"role": role, "parts": parts}))
        })
        .collect();
    let mut body = json!({
        "contents": contents,
        "generationConfig": {"maxOutputTokens": req.max_tokens},
    });
    if let Some(level) = req.thinking {
        // Gemini 2.5 Flash caps the budget at 24576.
        let budget = level.budget_tokens().min(24576);
        body["generationConfig"]["thinkingConfig"] = json!({"thinkingBudget": budget, "includeThoughts": true});
    }
    if let Some(s) = &req.system {
        body["systemInstruction"] = json!({"parts": [{"text": s}]});
    }
    if !req.tools.is_empty() {
        let decls: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                let mut d = json!({"name": t.name, "description": t.description});
                let schema = sanitize_schema(&t.input_schema);
                let has_props = schema.get("properties").and_then(|p| p.as_object()).is_some_and(|p| !p.is_empty());
                if has_props {
                    d["parameters"] = schema;
                }
                d
            })
            .collect();
        body["tools"] = json!([{"functionDeclarations": decls}]);
    }
    body
}

#[derive(Default)]
pub struct GoogleProcessor {
    saw_call: bool,
    counter: u32,
    finished: bool,
}

impl EventProcessor for GoogleProcessor {
    fn process(&mut self, ev: &SseEvent) -> anyhow::Result<Vec<StreamEvent>> {
        let data = ev.data.trim();
        if data.is_empty() || self.finished {
            return Ok(vec![]);
        }
        let v: Value = serde_json::from_str(data).with_context(|| format!("bad SSE json: {data}"))?;
        if let Some(e) = v.get("error") {
            let msg = e["message"].as_str().map(str::to_string).unwrap_or_else(|| e.to_string());
            let retryable = matches!(e["status"].as_str(), Some("UNAVAILABLE" | "INTERNAL" | "RESOURCE_EXHAUSTED" | "DEADLINE_EXCEEDED"));
            return Err(anyhow::Error::new(ProviderError::new(
                e["code"].as_u64().map(|c| c as u16),
                retryable,
                format!("google stream error: {msg}"),
            )));
        }
        let mut out = vec![];
        if let Some(reason) = v["promptFeedback"]["blockReason"].as_str() {
            return Err(anyhow!("google blocked the prompt: {reason}"));
        }
        let Some(cand) = v["candidates"].get(0) else { return Ok(out) };
        if let Some(parts) = cand["content"]["parts"].as_array() {
            for p in parts {
                if let Some(fc) = p.get("functionCall") {
                    self.saw_call = true;
                    self.counter += 1;
                    let name = fc["name"].as_str().unwrap_or_default().to_string();
                    let input = fc.get("args").cloned().unwrap_or_else(|| json!({}));
                    out.push(StreamEvent::ToolCall { id: format!("call_{}_{}", self.counter, name), name, input });
                } else if let Some(t) = p["text"].as_str().filter(|t| !t.is_empty()) {
                    if p["thought"].as_bool() == Some(true) {
                        out.push(StreamEvent::ThinkingDelta(t.to_string()));
                    } else {
                        out.push(StreamEvent::TextDelta(t.to_string()));
                    }
                }
            }
        }
        if let Some(fr) = cand["finishReason"].as_str() {
            self.finished = true;
            let reason = match fr {
                "MAX_TOKENS" => "max_tokens",
                _ if self.saw_call => "tool_use",
                "STOP" => "end_turn",
                o => o,
            };
            out.push(StreamEvent::Finish { stop_reason: reason.to_string() });
        }
        Ok(out)
    }

    fn finish(&mut self) -> anyhow::Result<Vec<StreamEvent>> {
        if self.finished {
            Ok(vec![])
        } else {
            Err(crate::sse::truncated("google"))
        }
    }
}

#[async_trait]
impl Provider for GoogleProvider {
    fn id(&self) -> &str {
        "google"
    }

    async fn stream(&self, req: ChatRequest) -> anyhow::Result<BoxStream<'static, anyhow::Result<StreamEvent>>> {
        let body = build_body(&req);
        let url = self.endpoint(&req.model);
        let resp = crate::http::send_with_retry("google", self.backoff, || {
            self.client.post(&url).header("x-goog-api-key", &self.api_key).json(&body)
        })
        .await?;
        let bytes = resp.bytes_stream().map(|r| r.map(|b| b.to_vec()).map_err(|e| crate::http::stream_error("google", e))).boxed();
        Ok(drive(bytes, GoogleProcessor::default()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChatMessage, ToolSpec};

    const CANNED: &str = "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Hel\"}]}}]}\r\n\r\n\
data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"lo\"}]}}]}\r\n\r\n\
data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"functionCall\":{\"name\":\"bash\",\"args\":{\"command\":\"ls\"}}}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{}}\r\n\r\n";

    #[tokio::test]
    async fn parses_text_and_function_call() {
        let chunks: Vec<anyhow::Result<Vec<u8>>> = CANNED.as_bytes().chunks(37).map(|c| Ok(c.to_vec())).collect();
        let s = drive(futures::stream::iter(chunks), GoogleProcessor::default());
        let evs: Vec<StreamEvent> = s.map(|e| e.unwrap()).collect().await;
        assert_eq!(
            evs,
            vec![
                StreamEvent::TextDelta("Hel".into()),
                StreamEvent::TextDelta("lo".into()),
                StreamEvent::ToolCall { id: "call_1_bash".into(), name: "bash".into(), input: json!({"command":"ls"}) },
                StreamEvent::Finish { stop_reason: "tool_use".into() },
            ]
        );
    }

    #[test]
    fn max_tokens_and_error() {
        let mut p = GoogleProcessor::default();
        let ev = SseEvent { event: None, data: r#"{"candidates":[{"finishReason":"MAX_TOKENS"}]}"#.into() };
        assert_eq!(p.process(&ev).unwrap(), vec![StreamEvent::Finish { stop_reason: "max_tokens".into() }]);
        let mut p = GoogleProcessor::default();
        let ev = SseEvent { event: None, data: r#"{"error":{"message":"quota"}}"#.into() };
        assert!(p.process(&ev).unwrap_err().to_string().contains("quota"));
    }

    #[test]
    fn builds_request_body() {
        let req = ChatRequest {
            model: "gemini-2.5-pro".into(),
            system: Some("sys".into()),
            messages: vec![
                ChatMessage { role: Role::User, content: vec![ContentBlock::Text { text: "q".into() }] },
                ChatMessage {
                    role: Role::Assistant,
                    content: vec![ContentBlock::ToolUse { id: "c1".into(), name: "read".into(), input: json!({"p":"a"}) }],
                },
                ChatMessage {
                    role: Role::User,
                    content: vec![ContentBlock::ToolResult { tool_use_id: "c1".into(), content: "data".into(), is_error: false }],
                },
            ],
            tools: vec![
                ToolSpec {
                    name: "read".into(),
                    description: "d".into(),
                    input_schema: json!({"$schema":"x","type":"object","additionalProperties":false,"properties":{"p":{"type":"string"}}}),
                },
                ToolSpec { name: "noargs".into(), description: "d".into(), input_schema: json!({"type":"object","properties":{}}) },
            ],
            max_tokens: 100,
            thinking: None,
        };
        let b = build_body(&req);
        assert_eq!(b["systemInstruction"]["parts"][0]["text"], "sys");
        assert_eq!(b["contents"][1]["role"], "model");
        assert_eq!(b["contents"][1]["parts"][0]["functionCall"]["name"], "read");
        assert_eq!(b["contents"][2]["parts"][0]["functionResponse"]["name"], "read");
        assert_eq!(b["contents"][2]["parts"][0]["functionResponse"]["response"]["output"], "data");
        let decls = &b["tools"][0]["functionDeclarations"];
        assert!(decls[0]["parameters"].get("additionalProperties").is_none());
        assert!(decls[0]["parameters"].get("$schema").is_none());
        assert!(decls[1].get("parameters").is_none());
        assert_eq!(b["generationConfig"]["maxOutputTokens"], 100);
    }

    #[test]
    fn endpoint_url() {
        let p = GoogleProvider::new("k");
        assert_eq!(
            p.endpoint("gemini-2.5-flash"),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
        );
        assert!(p.endpoint("models/x").contains("/models/x:"));
    }

    #[tokio::test]
    async fn streams_from_mock_server() {
        let m = crate::http::testutil::mock(vec![(200, "text/event-stream", CANNED.to_string())]).await;
        let p = GoogleProvider::new("secret").with_base_url(m.url.clone());
        let req = ChatRequest { model: "g".into(), system: None, messages: vec![], tools: vec![], max_tokens: 1, thinking: None };
        let evs: Vec<_> = p.stream(req).await.unwrap().collect().await;
        assert_eq!(evs.len(), 4);
        let reqs = m.requests.lock().unwrap();
        assert!(reqs[0].starts_with("POST /models/g:streamGenerateContent?alt=sse"));
        assert!(reqs[0].to_ascii_lowercase().contains("x-goog-api-key: secret"));
    }

    #[tokio::test]
    async fn thought_parts_become_thinking_deltas() {
        let data = "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"pondering\",\"thought\":true},{\"text\":\"Answer\"}]},\"finishReason\":\"STOP\"}]}\r\n\r\n";
        let chunks: Vec<anyhow::Result<Vec<u8>>> = vec![Ok(data.as_bytes().to_vec())];
        let evs: Vec<StreamEvent> = drive(futures::stream::iter(chunks), GoogleProcessor::default()).map(|e| e.unwrap()).collect().await;
        assert_eq!(
            evs,
            vec![
                StreamEvent::ThinkingDelta("pondering".into()),
                StreamEvent::TextDelta("Answer".into()),
                StreamEvent::Finish { stop_reason: "end_turn".into() },
            ]
        );
    }

    #[test]
    fn thinking_config_and_history() {
        let req = ChatRequest {
            model: "g".into(),
            system: None,
            messages: vec![ChatMessage {
                role: Role::Assistant,
                content: vec![ContentBlock::Thinking { thinking: "t".into(), signature: None }, ContentBlock::Text { text: "a".into() }],
            }],
            tools: vec![],
            max_tokens: 100,
            thinking: Some(crate::ThinkingLevel::Low),
        };
        let b = build_body(&req);
        assert_eq!(b["generationConfig"]["thinkingConfig"], json!({"thinkingBudget":4096,"includeThoughts":true}));
        assert_eq!(b["contents"][0]["parts"].as_array().unwrap().len(), 1);
        let mut high = req.clone();
        high.thinking = Some(crate::ThinkingLevel::High);
        assert_eq!(build_body(&high)["generationConfig"]["thinkingConfig"]["thinkingBudget"], 24576);
        high.thinking = None;
        assert!(build_body(&high)["generationConfig"].get("thinkingConfig").is_none());
    }
}
