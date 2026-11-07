use std::sync::Arc;

use futures::StreamExt;
use oc_ai::{ChatMessage, ChatRequest, ContentBlock, Provider, ProviderOptions, Role, StreamEvent, ToolSpec};
use tokio::sync::mpsc::UnboundedSender;

use crate::{Agent, AgentEvent, Config, PermissionDecision, PermissionRequest, Session, ToolContext};

/// Tools whose `file_path` input names a file the agent changed.
const FILE_WRITING_TOOLS: &[&str] = &["write", "edit"];

const MAX_STEPS: usize = 100;
const MAX_TOKENS: u32 = 8192;
/// Times a step is redone after a retryable stream failure.
const MAX_RETRIES: u32 = 3;
const DEFAULT_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

/// Everything one model response produced.
#[derive(Default)]
struct Step {
    thinking: Vec<ContentBlock>,
    text: String,
    calls: Vec<(String, String, serde_json::Value)>,
}

/// Store pending thinking text as an unsigned block (providers without signatures).
fn flush_thinking(blocks: &mut Vec<ContentBlock>, pending: &mut String) {
    if !pending.is_empty() {
        blocks.push(ContentBlock::Thinking { thinking: std::mem::take(pending), signature: None });
    }
}

impl Agent {
    /// Runs the model/tool loop for one user prompt, appending to `session.messages`
    /// and streaming progress to `events`. Ends with `AgentEvent::Done` or `Error`.
    pub async fn prompt(
        &self,
        session: &mut Session,
        input: String,
        events: UnboundedSender<AgentEvent>,
    ) -> anyhow::Result<()> {
        if session.messages.is_empty() && session.title == "New session" {
            let t: String = input.lines().next().unwrap_or("").chars().take(60).collect();
            if !t.trim().is_empty() {
                session.title = t.trim().to_string();
            }
        }
        session.messages.push(ChatMessage { role: Role::User, content: vec![ContentBlock::Text { text: input }] });
        let sid = session.id.clone();
        match self.run_loop(session, &events).await {
            Ok(()) => self.emit(&events, &sid, AgentEvent::Done).await,
            Err(e) => self.emit(&events, &sid, AgentEvent::Error { message: format!("{e:#}") }).await,
        }
        Ok(())
    }

    async fn emit(&self, events: &UnboundedSender<AgentEvent>, session_id: &str, ev: AgentEvent) {
        for h in &self.hooks.tools {
            h.on_event(session_id, &ev).await;
        }
        let _ = events.send(ev);
    }

    /// Permission check, plugin hooks, the tool itself, then diagnostics.
    async fn run_tool(
        &self,
        session_id: &str,
        call_id: &str,
        name: &str,
        mut input: serde_json::Value,
        ctx: &ToolContext,
    ) -> (String, bool) {
        let Some(tool) = self.tools.tools.iter().find(|t| t.name() == name) else {
            return (format!("Unknown tool: {name}"), true);
        };
        if let Some(p) = &self.hooks.permission {
            let req = PermissionRequest {
                session_id: session_id.to_string(),
                call_id: call_id.to_string(),
                tool: name.to_string(),
                input: input.clone(),
            };
            if let PermissionDecision::Deny { reason } = p.check(&req).await {
                let why = reason.map(|r| format!(": {r}")).unwrap_or_default();
                return (format!("The user denied permission to run {name}{why}"), true);
            }
        }
        for h in &self.hooks.tools {
            if let Err(e) = h.before_tool(name, &mut input).await {
                return (format!("{e:#}"), true);
            }
        }
        let mut output = match tool.execute(input.clone(), ctx).await {
            Ok(o) => o,
            Err(e) => return (format!("{e:#}"), true),
        };
        for h in &self.hooks.tools {
            if let Err(e) = h.after_tool(name, &input, &mut output).await {
                return (format!("{e:#}"), true);
            }
        }
        if let (Some(d), true) = (&self.hooks.diagnostics, FILE_WRITING_TOOLS.contains(&name)) {
            if let Some(path) = input.get("file_path").and_then(|v| v.as_str()) {
                if let Some(diag) = d.diagnostics(&crate::tools::resolve(&ctx.cwd, path)).await {
                    output.push_str("\n\n");
                    output.push_str(&diag);
                }
            }
        }
        (output, false)
    }

    /// Streams one model response, forwarding text/thinking deltas as they arrive.
    async fn stream_step(
        &self,
        session_id: &str,
        req: ChatRequest,
        events: &UnboundedSender<AgentEvent>,
    ) -> anyhow::Result<Step> {
        let mut stream = self.provider.stream(req).await?;
        let mut step = Step::default();
        let mut cur_thinking = String::new();
        while let Some(ev) = stream.next().await {
            match ev? {
                StreamEvent::TextDelta(t) => {
                    step.text.push_str(&t);
                    self.emit(events, session_id, AgentEvent::TextDelta { text: t }).await;
                }
                StreamEvent::ThinkingDelta(t) => {
                    cur_thinking.push_str(&t);
                    self.emit(events, session_id, AgentEvent::ThinkingDelta { text: t }).await;
                }
                StreamEvent::ThinkingSignature(sig) => {
                    let thinking = std::mem::take(&mut cur_thinking);
                    step.thinking.push(ContentBlock::Thinking { thinking, signature: Some(sig) });
                }
                StreamEvent::RedactedThinking(data) => {
                    flush_thinking(&mut step.thinking, &mut cur_thinking);
                    step.thinking.push(ContentBlock::RedactedThinking { data });
                }
                StreamEvent::ToolCall { id, name, input } => step.calls.push((id, name, input)),
                StreamEvent::Finish { .. } => {}
            }
        }
        flush_thinking(&mut step.thinking, &mut cur_thinking);
        Ok(step)
    }

    async fn run_loop(&self, session: &mut Session, events: &UnboundedSender<AgentEvent>) -> anyhow::Result<()> {
        let specs: Vec<ToolSpec> = self
            .tools
            .tools
            .iter()
            .map(|t| ToolSpec {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
            })
            .collect();
        let ctx = ToolContext { cwd: session.cwd.clone() };

        for _ in 0..MAX_STEPS {
            let req = ChatRequest {
                model: self.model.clone(),
                system: if self.system_prompt.is_empty() { None } else { Some(self.system_prompt.clone()) },
                messages: session.messages.clone(),
                tools: specs.clone(),
                max_tokens: MAX_TOKENS,
                thinking: self.thinking,
            };
            // A step whose stream fails with a retryable error is thrown away entirely
            // (nothing reaches session.messages, no tool runs) and redone.
            let mut attempt = 0u32;
            let Step { thinking, text, calls } = loop {
                match self.stream_step(&session.id, req.clone(), events).await {
                    Ok(step) => break step,
                    Err(e) if oc_ai::is_retryable(&e) && attempt < MAX_RETRIES => {
                        let delay = self.hooks.retry_base_delay.unwrap_or(DEFAULT_RETRY_DELAY) * 2u32.pow(attempt);
                        attempt += 1;
                        let ev = AgentEvent::Retry { attempt, max: MAX_RETRIES, reason: format!("{e:#}") };
                        self.emit(events, &session.id, ev).await;
                        tokio::time::sleep(delay).await;
                    }
                    Err(e) => return Err(e),
                }
            };

            let mut blocks = thinking;
            if !text.is_empty() {
                blocks.push(ContentBlock::Text { text });
            }
            for (id, name, input) in &calls {
                blocks.push(ContentBlock::ToolUse { id: id.clone(), name: name.clone(), input: input.clone() });
            }
            if !blocks.is_empty() {
                session.messages.push(ChatMessage { role: Role::Assistant, content: blocks });
            }
            if calls.is_empty() {
                return Ok(());
            }

            let mut results = vec![];
            for (id, name, input) in calls {
                let start = AgentEvent::ToolStart { id: id.clone(), name: name.clone(), input: input.clone() };
                self.emit(events, &session.id, start).await;
                let (output, is_error) = self.run_tool(&session.id, &id, &name, input, &ctx).await;
                let end = AgentEvent::ToolEnd { id: id.clone(), output: output.clone(), is_error };
                self.emit(events, &session.id, end).await;
                results.push(ContentBlock::ToolResult { tool_use_id: id, content: output, is_error });
            }
            session.messages.push(ChatMessage { role: Role::User, content: results });
        }
        anyhow::bail!("agent stopped after {MAX_STEPS} steps without finishing")
    }
}

/// Builds a provider from "provider/model" using env vars and stored credentials.
pub fn provider_from_model(model: &str) -> anyhow::Result<(Arc<dyn Provider>, String)> {
    provider_from_config(model, &Config::default())
}

/// Like [`provider_from_model`], also applying the `provider.<id>.options`
/// section of the config (base URL and provider-specific settings).
pub fn provider_from_config(model: &str, config: &Config) -> anyhow::Result<(Arc<dyn Provider>, String)> {
    let (prov, name) = model
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("model must be in \"provider/model\" form, got \"{model}\""))?;
    anyhow::ensure!(!name.is_empty(), "model name is empty in \"{model}\"");
    let extra = config
        .provider
        .get(prov)
        .and_then(|p| p.get("options"))
        .and_then(|o| o.as_object())
        .cloned()
        .unwrap_or_default();
    let opts = ProviderOptions {
        api_key: crate::auth::get_api_key(prov),
        base_url: extra.get("baseURL").and_then(|v| v.as_str()).map(str::to_string),
        extra,
    };
    Ok((oc_ai::build_provider(prov, &opts)?, name.to_string()))
}
