//! Application state and the event -> state reducer (pure, no I/O).

use oc_ai::{ContentBlock, Role, ThinkingLevel};
use oc_core::{AgentEvent, Session};

use crate::input::Editor;

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    User(String),
    Assistant(String),
    /// Model reasoning, shown muted and collapsible ahead of the step's answer.
    Thinking(String),
    Tool { id: String, name: String, summary: String, output: Option<String>, is_error: bool },
    Error(String),
    Info(String),
}

pub struct SessionPopup {
    pub items: Vec<Session>,
    pub selected: usize,
}

pub struct ThemePopup {
    pub names: Vec<String>,
    pub selected: usize,
    /// Theme to restore on cancel.
    pub original: String,
}

pub struct AppState {
    pub theme_popup: Option<ThemePopup>,
    pub perm: crate::perm::PermissionDialog,
    pub blocks: Vec<Block>,
    pub editor: Editor,
    pub busy: bool,
    pub tick: usize,
    /// Lines scrolled up from the bottom of the transcript (0 = follow).
    pub scroll: usize,
    pub expand_tools: bool,
    /// Show thinking blocks in full instead of their first line.
    pub expand_thinking: bool,
    /// Reasoning level used for the next turn (shown in the status bar).
    pub thinking: Option<ThinkingLevel>,
    pub popup: Option<SessionPopup>,
    pub model: String,
    pub cwd: String,
    pub title: String,
    pub should_quit: bool,
}

impl AppState {
    pub fn new(model: String, cwd: String, title: String) -> Self {
        AppState {
            theme_popup: None,
            perm: Default::default(),
            blocks: Vec::new(),
            editor: Editor::default(),
            busy: false,
            tick: 0,
            scroll: 0,
            expand_tools: false,
            expand_thinking: false,
            thinking: None,
            popup: None,
            model,
            cwd,
            title,
            should_quit: false,
        }
    }

    /// Fold one agent event into the transcript.
    pub fn apply(&mut self, ev: AgentEvent) {
        match ev {
            AgentEvent::TextDelta { text } => {
                if let Some(Block::Assistant(s)) = self.blocks.last_mut() {
                    s.push_str(&text);
                } else {
                    self.blocks.push(Block::Assistant(text));
                }
            }
            AgentEvent::ThinkingDelta { text } => {
                if let Some(Block::Thinking(s)) = self.blocks.last_mut() {
                    s.push_str(&text);
                } else {
                    self.blocks.push(Block::Thinking(text));
                }
            }
            AgentEvent::Retry { attempt, max, .. } => {
                // The step restarts from scratch: drop what it streamed so far.
                while matches!(self.blocks.last(), Some(Block::Assistant(_) | Block::Thinking(_))) {
                    self.blocks.pop();
                }
                self.blocks.push(Block::Info(format!("Connection lost, retrying ({attempt}/{max})…")));
            }
            AgentEvent::ToolStart { id, name, input } => {
                let summary = summarize_input(&name, &input);
                self.blocks.push(Block::Tool { id, name, summary, output: None, is_error: false });
            }
            AgentEvent::ToolEnd { id, output, is_error } => {
                for b in self.blocks.iter_mut().rev() {
                    if let Block::Tool { id: tid, output: o, is_error: e, .. } = b {
                        if *tid == id {
                            *o = Some(output);
                            *e = is_error;
                            break;
                        }
                    }
                }
            }
            AgentEvent::Done => self.busy = false,
            AgentEvent::Error { message } => {
                self.blocks.push(Block::Error(message));
                self.busy = false;
            }
        }
    }

    pub fn toggle_thinking(&mut self) {
        self.expand_thinking = !self.expand_thinking;
    }

    pub fn load_session(&mut self, s: &Session) {
        self.blocks = blocks_from_session(s);
        self.title = s.title.clone();
        self.scroll = 0;
        self.busy = false;
    }
}

/// Short human summary of a tool call's input.
pub fn summarize_input(name: &str, input: &serde_json::Value) -> String {
    let pick = ["command", "filePath", "file_path", "path", "pattern", "url", "query"];
    if let Some(obj) = input.as_object() {
        for k in pick {
            if let Some(v) = obj.get(k).and_then(|v| v.as_str()) {
                return truncate(&v.replace('\n', " "), 80);
            }
        }
        if let Some((k, v)) = obj.iter().next() {
            let vs = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
            return truncate(&format!("{k}={vs}"), 80);
        }
    }
    let _ = name;
    String::new()
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{t}…")
    }
}

/// Rebuild transcript blocks from persisted messages.
pub fn blocks_from_session(s: &Session) -> Vec<Block> {
    let mut blocks = Vec::new();
    for m in &s.messages {
        for c in &m.content {
            match (m.role, c) {
                (Role::User, ContentBlock::Text { text }) => blocks.push(Block::User(text.clone())),
                (Role::Assistant, ContentBlock::Text { text }) => blocks.push(Block::Assistant(text.clone())),
                (_, ContentBlock::Thinking { thinking, .. }) => {
                    if !thinking.trim().is_empty() {
                        blocks.push(Block::Thinking(thinking.clone()));
                    }
                }
                (_, ContentBlock::RedactedThinking { .. }) => {}
                (_, ContentBlock::ToolUse { id, name, input }) => blocks.push(Block::Tool {
                    id: id.clone(),
                    name: name.clone(),
                    summary: summarize_input(name, input),
                    output: None,
                    is_error: false,
                }),
                (_, ContentBlock::ToolResult { tool_use_id, content, is_error }) => {
                    for b in blocks.iter_mut().rev() {
                        if let Block::Tool { id, output, is_error: e, .. } = b {
                            if id == tool_use_id {
                                *output = Some(content.clone());
                                *e = *is_error;
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    use oc_ai::ChatMessage;
    use serde_json::json;

    fn st() -> AppState {
        AppState::new("m".into(), "/tmp".into(), "t".into())
    }

    #[test]
    fn reducer_streams_text_and_tools() {
        let mut s = st();
        s.busy = true;
        s.apply(AgentEvent::TextDelta { text: "he".into() });
        s.apply(AgentEvent::TextDelta { text: "llo".into() });
        s.apply(AgentEvent::ToolStart { id: "1".into(), name: "bash".into(), input: json!({"command": "ls -la"}) });
        s.apply(AgentEvent::ToolEnd { id: "1".into(), output: "out".into(), is_error: true });
        s.apply(AgentEvent::TextDelta { text: "after".into() });
        assert_eq!(s.blocks.len(), 3);
        assert_eq!(s.blocks[0], Block::Assistant("hello".into()));
        assert_eq!(
            s.blocks[1],
            Block::Tool { id: "1".into(), name: "bash".into(), summary: "ls -la".into(), output: Some("out".into()), is_error: true }
        );
        assert!(s.busy);
        s.apply(AgentEvent::Done);
        assert!(!s.busy);
    }

    #[test]
    fn error_stops_busy() {
        let mut s = st();
        s.busy = true;
        s.apply(AgentEvent::Error { message: "boom".into() });
        assert!(!s.busy);
        assert_eq!(s.blocks, vec![Block::Error("boom".into())]);
    }

    #[test]
    fn restores_from_session() {
        let mut sess = Session::new("/tmp".into());
        sess.messages.push(ChatMessage { role: Role::User, content: vec![ContentBlock::Text { text: "hi".into() }] });
        sess.messages.push(ChatMessage {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse { id: "a".into(), name: "read".into(), input: json!({"filePath": "x.rs"}) }],
        });
        sess.messages.push(ChatMessage {
            role: Role::User,
            content: vec![ContentBlock::ToolResult { tool_use_id: "a".into(), content: "data".into(), is_error: false }],
        });
        let b = blocks_from_session(&sess);
        assert_eq!(b.len(), 2);
        assert!(matches!(&b[1], Block::Tool { output: Some(o), summary, .. } if o == "data" && summary == "x.rs"));
    }

    #[test]
    fn thinking_streams_before_text() {
        let mut s = st();
        s.apply(AgentEvent::ThinkingDelta { text: "let me ".into() });
        s.apply(AgentEvent::ThinkingDelta { text: "think".into() });
        s.apply(AgentEvent::TextDelta { text: "answer".into() });
        assert_eq!(s.blocks, vec![Block::Thinking("let me think".into()), Block::Assistant("answer".into())]);
    }

    #[test]
    fn toggle_thinking_flag() {
        let mut s = st();
        assert!(!s.expand_thinking);
        s.toggle_thinking();
        assert!(s.expand_thinking);
        s.toggle_thinking();
        assert!(!s.expand_thinking);
    }

    #[test]
    fn retry_removes_partial_step_only() {
        let mut s = st();
        s.blocks.push(Block::User("q".into()));
        s.apply(AgentEvent::ThinkingDelta { text: "old step".into() });
        s.apply(AgentEvent::TextDelta { text: "old text".into() });
        s.apply(AgentEvent::ToolStart { id: "1".into(), name: "bash".into(), input: json!({"command":"ls"}) });
        s.apply(AgentEvent::ToolEnd { id: "1".into(), output: "o".into(), is_error: false });
        s.apply(AgentEvent::ThinkingDelta { text: "partial".into() });
        s.apply(AgentEvent::TextDelta { text: "partial text".into() });
        s.apply(AgentEvent::Retry { attempt: 1, max: 3, reason: "reset".into() });
        assert_eq!(s.blocks.len(), 5);
        assert_eq!(s.blocks[0], Block::User("q".into()));
        assert_eq!(s.blocks[1], Block::Thinking("old step".into()));
        assert_eq!(s.blocks[2], Block::Assistant("old text".into()));
        assert!(matches!(s.blocks[3], Block::Tool { .. }));
        assert_eq!(s.blocks[4], Block::Info("Connection lost, retrying (1/3)…".into()));
        // the restarted text is not glued onto anything
        s.apply(AgentEvent::TextDelta { text: "fresh".into() });
        assert_eq!(s.blocks[5], Block::Assistant("fresh".into()));
    }

    #[test]
    fn restores_thinking_from_session() {
        let mut sess = Session::new("/tmp".into());
        sess.messages.push(ChatMessage {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Thinking { thinking: "hmm".into(), signature: Some("s".into()) },
                ContentBlock::RedactedThinking { data: "x".into() },
                ContentBlock::Text { text: "ans".into() },
            ],
        });
        assert_eq!(blocks_from_session(&sess), vec![Block::Thinking("hmm".into()), Block::Assistant("ans".into())]);
    }

    #[test]
    fn truncates() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }
}
