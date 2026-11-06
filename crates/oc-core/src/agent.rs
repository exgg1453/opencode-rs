use std::sync::Arc;

use futures::StreamExt;
use oc_ai::{
    AnthropicProvider, ChatMessage, ChatRequest, ContentBlock, OpenAiProvider, Provider, Role, StreamEvent, ToolSpec,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::{Agent, AgentEvent, Session, ToolContext};

const MAX_STEPS: usize = 100;
const MAX_TOKENS: u32 = 8192;

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
        match self.run_loop(session, &events).await {
            Ok(()) => {
                let _ = events.send(AgentEvent::Done);
            }
            Err(e) => {
                let _ = events.send(AgentEvent::Error { message: format!("{e:#}") });
            }
        }
        Ok(())
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
            };
            let mut stream = self.provider.stream(req).await?;
            let mut text = String::new();
            let mut calls: Vec<(String, String, serde_json::Value)> = vec![];
            while let Some(ev) = stream.next().await {
                match ev? {
                    StreamEvent::TextDelta(t) => {
                        text.push_str(&t);
                        let _ = events.send(AgentEvent::TextDelta { text: t });
                    }
                    StreamEvent::ToolCall { id, name, input } => calls.push((id, name, input)),
                    StreamEvent::Finish { .. } => {}
                }
            }

            let mut blocks = vec![];
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
                let _ = events.send(AgentEvent::ToolStart { id: id.clone(), name: name.clone(), input: input.clone() });
                let (output, is_error) = match self.tools.tools.iter().find(|t| t.name() == name) {
                    None => (format!("Unknown tool: {name}"), true),
                    Some(tool) => match tool.execute(input, &ctx).await {
                        Ok(o) => (o, false),
                        Err(e) => (format!("{e:#}"), true),
                    },
                };
                let _ = events.send(AgentEvent::ToolEnd { id: id.clone(), output: output.clone(), is_error });
                results.push(ContentBlock::ToolResult { tool_use_id: id, content: output, is_error });
            }
            session.messages.push(ChatMessage { role: Role::User, content: results });
        }
        anyhow::bail!("agent stopped after {MAX_STEPS} steps without finishing")
    }
}

/// Builds a provider from "provider/model" using env vars (ANTHROPIC_API_KEY, OPENAI_API_KEY).
pub fn provider_from_model(model: &str) -> anyhow::Result<(Arc<dyn Provider>, String)> {
    let (prov, name) = model
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("model must be in \"provider/model\" form, got \"{model}\""))?;
    anyhow::ensure!(!name.is_empty(), "model name is empty in \"{model}\"");
    let p: Arc<dyn Provider> = match prov {
        "anthropic" => Arc::new(AnthropicProvider::from_env()?),
        "openai" => Arc::new(OpenAiProvider::from_env()?),
        other => anyhow::bail!("unknown provider \"{other}\" (supported: anthropic, openai)"),
    };
    Ok((p, name.to_string()))
}
