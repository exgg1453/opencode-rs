use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::stream::{self, BoxStream, StreamExt};
use oc_ai::{ChatRequest, ContentBlock, Provider, Role, StreamEvent};
use serde_json::json;

use crate::*;

struct Mock {
    turns: Mutex<VecDeque<Vec<StreamEvent>>>,
    seen: Mutex<Vec<ChatRequest>>,
}

#[async_trait]
impl Provider for Mock {
    fn id(&self) -> &str {
        "mock"
    }
    async fn stream(&self, req: ChatRequest) -> anyhow::Result<BoxStream<'static, anyhow::Result<StreamEvent>>> {
        self.seen.lock().unwrap().push(req);
        let t = self.turns.lock().unwrap().pop_front().ok_or_else(|| anyhow::anyhow!("no more turns"))?;
        Ok(stream::iter(t.into_iter().map(Ok)).boxed())
    }
}

fn agent(turns: Vec<Vec<StreamEvent>>) -> (Agent, Arc<Mock>) {
    let m = Arc::new(Mock { turns: Mutex::new(turns.into()), seen: Mutex::new(vec![]) });
    (
        Agent { provider: m.clone(), model: "m".into(), tools: ToolRegistry::builtin(), system_prompt: DEFAULT_SYSTEM_PROMPT.into() },
        m,
    )
}

fn drain(mut rx: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>) -> Vec<AgentEvent> {
    let mut v = vec![];
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}

#[tokio::test]
async fn loop_runs_tool_then_finishes() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("hello.txt"), "world\n").unwrap();
    let (a, mock) = agent(vec![
        vec![
            StreamEvent::TextDelta("Let me look. ".into()),
            StreamEvent::ToolCall { id: "t1".into(), name: "read".into(), input: json!({"file_path":"hello.txt"}) },
            StreamEvent::ToolCall { id: "t2".into(), name: "nonexistent".into(), input: json!({}) },
            StreamEvent::Finish { stop_reason: "tool_use".into() },
        ],
        vec![StreamEvent::TextDelta("It says world.".into()), StreamEvent::Finish { stop_reason: "end_turn".into() }],
    ]);
    let mut s = Session::new(d.path().to_path_buf());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    a.prompt(&mut s, "read hello".into(), tx).await.unwrap();
    let evs = drain(rx);
    assert!(matches!(evs.first(), Some(AgentEvent::TextDelta { .. })));
    assert!(matches!(evs.last(), Some(AgentEvent::Done)));
    let ends: Vec<_> = evs.iter().filter_map(|e| if let AgentEvent::ToolEnd { id, output, is_error } = e { Some((id.clone(), output.clone(), *is_error)) } else { None }).collect();
    assert_eq!(ends.len(), 2);
    assert!(ends[0].1.contains("world") && !ends[0].2);
    assert!(ends[1].2);
    // user, assistant(text+2 tool_use), user(results), assistant(text)
    assert_eq!(s.messages.len(), 4);
    assert_eq!(s.messages[2].role, Role::User);
    assert!(matches!(s.messages[2].content[0], ContentBlock::ToolResult { .. }));
    assert_eq!(s.title, "read hello");
    // second request carried tool results and tool specs
    let seen = mock.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1].messages.len(), 3);
    assert_eq!(seen[0].tools.len(), 7);
}

#[tokio::test]
async fn provider_failure_emits_error() {
    let (a, _) = agent(vec![]);
    let d = tempfile::tempdir().unwrap();
    let mut s = Session::new(d.path().to_path_buf());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    a.prompt(&mut s, "hi".into(), tx).await.unwrap();
    let evs = drain(rx);
    assert!(matches!(evs.last(), Some(AgentEvent::Error { .. })));
}

#[test]
fn provider_from_model_parses() {
    assert!(provider_from_model("noslash").is_err());
    assert!(provider_from_model("weird/x").is_err());
    std::env::set_var("OPENAI_API_KEY", "k");
    let (p, m) = provider_from_model("openai/gpt-4o").unwrap();
    assert_eq!(p.id(), "openai");
    assert_eq!(m, "gpt-4o");
    std::env::set_var("ANTHROPIC_API_KEY", "k");
    let (p, m) = provider_from_model("anthropic/claude-x").unwrap();
    assert_eq!(p.id(), "anthropic");
    assert_eq!(m, "claude-x");
}
