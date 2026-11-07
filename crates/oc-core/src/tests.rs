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
        Agent { provider: m.clone(), model: "m".into(), tools: ToolRegistry::builtin(), system_prompt: DEFAULT_SYSTEM_PROMPT.into(), hooks: Default::default(), thinking: None },
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

/// Provider whose turns may fail: events are yielded first, then the error (if any).
struct Flaky {
    turns: Mutex<VecDeque<Turn>>,
    seen: Mutex<Vec<ChatRequest>>,
}

struct Turn {
    events: Vec<StreamEvent>,
    /// (retryable, fail before the stream starts)
    fail: Option<(bool, bool)>,
}

#[async_trait]
impl Provider for Flaky {
    fn id(&self) -> &str {
        "flaky"
    }
    async fn stream(&self, req: ChatRequest) -> anyhow::Result<BoxStream<'static, anyhow::Result<StreamEvent>>> {
        self.seen.lock().unwrap().push(req);
        let t = self.turns.lock().unwrap().pop_front().ok_or_else(|| anyhow::anyhow!("no more turns"))?;
        let err = || anyhow::Error::new(oc_ai::ProviderError::new(None, t.fail.unwrap().0, "connection reset"));
        if matches!(t.fail, Some((_, true))) {
            return Err(err());
        }
        let mut items: Vec<anyhow::Result<StreamEvent>> = t.events.iter().cloned().map(Ok).collect();
        if t.fail.is_some() {
            items.push(Err(err()));
        }
        Ok(stream::iter(items).boxed())
    }
}

fn flaky_agent(turns: Vec<Turn>) -> (Agent, Arc<Flaky>) {
    let m = Arc::new(Flaky { turns: Mutex::new(turns.into()), seen: Mutex::new(vec![]) });
    let hooks = AgentHooks { retry_base_delay: Some(std::time::Duration::ZERO), ..Default::default() };
    (
        Agent { provider: m.clone(), model: "m".into(), tools: ToolRegistry::builtin(), system_prompt: String::new(), hooks, thinking: None },
        m,
    )
}

fn ok_turn(text: &str) -> Turn {
    Turn { events: vec![StreamEvent::TextDelta(text.into()), StreamEvent::Finish { stop_reason: "end_turn".into() }], fail: None }
}

async fn run_flaky(turns: Vec<Turn>) -> (Session, Vec<AgentEvent>, Arc<Flaky>) {
    let d = tempfile::tempdir().unwrap();
    let (a, m) = flaky_agent(turns);
    let mut s = Session::new(d.path().to_path_buf());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    a.prompt(&mut s, "hi".into(), tx).await.unwrap();
    (s, drain(rx), m)
}

#[tokio::test]
async fn mid_stream_failure_retries_once_without_partial_content() {
    let partial = Turn {
        events: vec![
            StreamEvent::ThinkingDelta("partial thought".into()),
            StreamEvent::TextDelta("partial ".into()),
            StreamEvent::ToolCall { id: "t".into(), name: "bash".into(), input: json!({"command":"echo SHOULD_NOT_RUN"}) },
        ],
        fail: Some((true, false)),
    };
    let (s, evs, m) = run_flaky(vec![partial, ok_turn("final")]).await;
    let retries: Vec<_> = evs.iter().filter(|e| matches!(e, AgentEvent::Retry { .. })).collect();
    assert_eq!(retries.len(), 1);
    assert!(matches!(retries[0], AgentEvent::Retry { attempt: 1, max: 3, reason } if reason.contains("connection reset")));
    assert!(matches!(evs.last(), Some(AgentEvent::Done)));
    assert!(!evs.iter().any(|e| matches!(e, AgentEvent::ToolStart { .. })));
    // user + one assistant message holding only the successful attempt
    assert_eq!(s.messages.len(), 2);
    assert_eq!(s.messages[1].content, vec![ContentBlock::Text { text: "final".into() }]);
    assert_eq!(m.seen.lock().unwrap().len(), 2);
    // the partial output was streamed live before the Retry event
    let retry_pos = evs.iter().position(|e| matches!(e, AgentEvent::Retry { .. })).unwrap();
    assert!(matches!(&evs[0], AgentEvent::ThinkingDelta { text } if text == "partial thought"));
    assert!(retry_pos >= 2);
}

#[tokio::test]
async fn start_failure_is_retried_too() {
    let (s, evs, _) = run_flaky(vec![Turn { events: vec![], fail: Some((true, true)) }, ok_turn("ok")]).await;
    assert_eq!(evs.iter().filter(|e| matches!(e, AgentEvent::Retry { .. })).count(), 1);
    assert_eq!(s.messages.len(), 2);
}

#[tokio::test]
async fn non_retryable_error_fails_immediately() {
    let (s, evs, m) = run_flaky(vec![Turn { events: vec![StreamEvent::TextDelta("x".into())], fail: Some((false, false)) }, ok_turn("never")]).await;
    assert!(!evs.iter().any(|e| matches!(e, AgentEvent::Retry { .. })));
    assert!(matches!(evs.last(), Some(AgentEvent::Error { message }) if message.contains("connection reset")));
    assert_eq!(s.messages.len(), 1);
    assert_eq!(m.seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn retries_exhausted_ends_with_error() {
    let fail = || Turn { events: vec![StreamEvent::TextDelta("p".into())], fail: Some((true, false)) };
    let (s, evs, m) = run_flaky(vec![fail(), fail(), fail(), fail(), ok_turn("never")]).await;
    let attempts: Vec<u32> = evs.iter().filter_map(|e| if let AgentEvent::Retry { attempt, .. } = e { Some(*attempt) } else { None }).collect();
    assert_eq!(attempts, vec![1, 2, 3]);
    assert!(matches!(evs.last(), Some(AgentEvent::Error { .. })));
    assert_eq!(s.messages.len(), 1);
    assert_eq!(m.seen.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn thinking_blocks_are_stored_before_text_and_tools() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), "x").unwrap();
    let (mut a, mock) = agent(vec![
        vec![
            StreamEvent::ThinkingDelta("first ".into()),
            StreamEvent::ThinkingDelta("thought".into()),
            StreamEvent::ThinkingSignature("sig1".into()),
            StreamEvent::RedactedThinking("ENC".into()),
            StreamEvent::TextDelta("reading".into()),
            StreamEvent::ToolCall { id: "t1".into(), name: "read".into(), input: json!({"file_path":"a.txt"}) },
            StreamEvent::Finish { stop_reason: "tool_use".into() },
        ],
        vec![
            StreamEvent::ThinkingDelta("unsigned".into()),
            StreamEvent::TextDelta("done".into()),
            StreamEvent::Finish { stop_reason: "end_turn".into() },
        ],
    ]);
    a.thinking = Some(oc_ai::ThinkingLevel::High);
    let mut s = Session::new(d.path().to_path_buf());
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    a.prompt(&mut s, "go".into(), tx).await.unwrap();
    let evs = drain(rx);
    let thoughts: Vec<_> = evs.iter().filter_map(|e| if let AgentEvent::ThinkingDelta { text } = e { Some(text.as_str()) } else { None }).collect();
    assert_eq!(thoughts, vec!["first ", "thought", "unsigned"]);
    assert_eq!(
        s.messages[1].content,
        vec![
            ContentBlock::Thinking { thinking: "first thought".into(), signature: Some("sig1".into()) },
            ContentBlock::RedactedThinking { data: "ENC".into() },
            ContentBlock::Text { text: "reading".into() },
            ContentBlock::ToolUse { id: "t1".into(), name: "read".into(), input: json!({"file_path":"a.txt"}) },
        ]
    );
    assert_eq!(s.messages[3].content[0], ContentBlock::Thinking { thinking: "unsigned".into(), signature: None });
    // the thinking level reaches the request, and history (with blocks) is replayed on step two
    let seen = mock.seen.lock().unwrap();
    assert_eq!(seen[0].thinking, Some(oc_ai::ThinkingLevel::High));
    assert_eq!(seen[1].messages[1].content.len(), 4);
}
