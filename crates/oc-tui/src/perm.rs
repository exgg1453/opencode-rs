//! Permission prompts: an `Asker` that forwards requests to the UI over a
//! channel, plus the modal dialog state machine and its renderer.

use std::collections::VecDeque;

use async_trait::async_trait;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use oc_core::hooks::{PermissionDecision, PermissionRequest};
use oc_core::permission::Asker;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block as UiBlock, Borders, Clear, Paragraph};
use ratatui::Frame;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use crate::state::truncate;
use crate::theme::Theme;

pub struct AskMsg {
    pub req: PermissionRequest,
    pub reply: oneshot::Sender<PermissionDecision>,
}

/// Sends requests to the UI and waits for the answer. Any failure (UI gone,
/// dialog dropped) is a denial, so a turn can never hang on the prompt.
pub struct ChannelAsker {
    tx: UnboundedSender<AskMsg>,
}

impl ChannelAsker {
    pub fn new(tx: UnboundedSender<AskMsg>) -> Self {
        ChannelAsker { tx }
    }
}

#[async_trait]
impl Asker for ChannelAsker {
    async fn ask(&self, req: &PermissionRequest) -> PermissionDecision {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(AskMsg { req: req.clone(), reply }).is_err() {
            return denied("permission prompt unavailable");
        }
        rx.await.unwrap_or_else(|_| denied("permission request was cancelled"))
    }
}

fn denied(reason: &str) -> PermissionDecision {
    PermissionDecision::Deny { reason: Some(reason.into()) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Plain,
    Add,
    Remove,
}

/// Human-readable description of a request.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub headline: String,
    pub lines: Vec<(DiffKind, String)>,
}

const PREVIEW_LINES: usize = 10;

fn str_of<'a>(v: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| v.get(*k).and_then(|x| x.as_str()))
}

pub fn describe(req: &PermissionRequest) -> Summary {
    let inp = &req.input;
    let path = str_of(inp, &["file_path", "filePath", "path"]);
    let mut lines = Vec::new();
    let headline;
    match req.tool.as_str() {
        "bash" => {
            headline = "Run command".to_string();
            if let Some(d) = str_of(inp, &["description"]) {
                lines.push((DiffKind::Plain, d.to_string()));
            }
            for l in str_of(inp, &["command"]).unwrap_or("").lines().take(PREVIEW_LINES) {
                lines.push((DiffKind::Plain, format!("$ {l}")));
            }
        }
        "edit" | "multiedit" => {
            headline = format!("Edit {}", path.unwrap_or("(unknown file)"));
            push_diff(&mut lines, str_of(inp, &["old_string", "oldString"]).unwrap_or(""), str_of(inp, &["new_string", "newString"]).unwrap_or(""));
        }
        "write" => {
            headline = format!("Write {}", path.unwrap_or("(unknown file)"));
            push_diff(&mut lines, "", str_of(inp, &["content"]).unwrap_or(""));
        }
        _ => {
            headline = match path {
                Some(p) => format!("{} {p}", req.tool),
                None => format!("Use tool {}", req.tool),
            };
            if let Some(o) = inp.as_object() {
                for (k, v) in o.iter().take(PREVIEW_LINES) {
                    let vs = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
                    lines.push((DiffKind::Plain, format!("{k}: {}", vs.replace('\n', " "))));
                }
            }
        }
    }
    Summary { headline, lines }
}

fn push_diff(out: &mut Vec<(DiffKind, String)>, old: &str, new: &str) {
    let (o, n): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    let half = if o.is_empty() || n.is_empty() { PREVIEW_LINES } else { PREVIEW_LINES / 2 };
    for l in o.iter().take(half) {
        out.push((DiffKind::Remove, format!("- {l}")));
    }
    if o.len() > half {
        out.push((DiffKind::Plain, format!("  … {} more removed lines", o.len() - half)));
    }
    for l in n.iter().take(half) {
        out.push((DiffKind::Add, format!("+ {l}")));
    }
    if n.len() > half {
        out.push((DiffKind::Plain, format!("  … {} more added lines", n.len() - half)));
    }
}

/// Queue of pending prompts; the front one is shown.
#[derive(Default)]
pub struct PermissionDialog {
    queue: VecDeque<AskMsg>,
}

impl PermissionDialog {
    pub fn push(&mut self, m: AskMsg) {
        self.queue.push_back(m);
    }

    pub fn current(&self) -> Option<&PermissionRequest> {
        self.queue.front().map(|m| &m.req)
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Drops requests whose asker has gone away (turn interrupted).
    pub fn prune_closed(&mut self) {
        self.queue.retain(|m| !m.reply.is_closed());
    }

    /// Denies everything pending.
    pub fn clear(&mut self) {
        for m in self.queue.drain(..) {
            let _ = m.reply.send(denied("interrupted"));
        }
    }

    pub fn resolve(&mut self, d: PermissionDecision) {
        if let Some(m) = self.queue.pop_front() {
            let _ = m.reply.send(d);
        }
        self.prune_closed();
    }

    /// Returns true when the key was consumed.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if self.queue.is_empty() {
            return false;
        }
        if key.kind == KeyEventKind::Release {
            return true;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return false; // let Ctrl+C quit
        }
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => self.resolve(PermissionDecision::Allow),
            KeyCode::Char('a') | KeyCode::Char('A') => self.resolve(PermissionDecision::AllowAlways),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.resolve(PermissionDecision::Deny { reason: Some("denied by user".into()) })
            }
            _ => {}
        }
        true
    }
}

pub fn draw_dialog(f: &mut Frame, area: Rect, dialog: &PermissionDialog, theme: &Theme) {
    let Some(req) = dialog.current() else { return };
    let sum = describe(req);
    let w = area.width.saturating_sub(4).clamp(10, 100).min(area.width);
    let inner_w = w.saturating_sub(2) as usize;
    let mut lines: Vec<Line> = vec![
        Line::from(vec![Span::styled("Permission required: ", theme.warning), Span::styled(req.tool.clone(), theme.tool_name)]),
        Line::styled(truncate(&sum.headline, inner_w), theme.text),
        Line::default(),
    ];
    for (k, l) in &sum.lines {
        let st = match k {
            DiffKind::Plain => theme.muted,
            DiffKind::Add => theme.diff_add,
            DiffKind::Remove => theme.diff_remove,
        };
        lines.push(Line::styled(truncate(l, inner_w), st));
    }
    lines.push(Line::default());
    let mut keys = vec![
        Span::styled("y/Enter", theme.success),
        Span::styled(" allow once  ", theme.muted),
        Span::styled("a", theme.primary),
        Span::styled(" always  ", theme.muted),
        Span::styled("n/Esc", theme.error),
        Span::styled(" deny", theme.muted),
    ];
    if dialog.len() > 1 {
        keys.push(Span::styled(format!("   (+{} queued)", dialog.len() - 1), theme.muted));
    }
    lines.push(Line::from(keys));
    let h = (lines.len() as u16 + 2).min(area.height);
    let r = Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h };
    f.render_widget(Clear, r);
    let blk = UiBlock::default().borders(Borders::ALL).border_style(theme.warning).style(theme.panel).title(" Permission ");
    f.render_widget(Paragraph::new(lines).block(blk), r);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use serde_json::json;
    use std::sync::Arc;

    fn req(tool: &str, input: serde_json::Value) -> PermissionRequest {
        PermissionRequest { session_id: "s".into(), call_id: "c".into(), tool: tool.into(), input }
    }

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn ask(d: &mut PermissionDialog, r: PermissionRequest) -> oneshot::Receiver<PermissionDecision> {
        let (reply, rx) = oneshot::channel();
        d.push(AskMsg { req: r, reply });
        rx
    }

    #[test]
    fn keys_resolve_in_order() {
        let mut d = PermissionDialog::default();
        let mut r1 = ask(&mut d, req("bash", json!({"command": "ls"})));
        let mut r2 = ask(&mut d, req("bash", json!({"command": "pwd"})));
        let mut r3 = ask(&mut d, req("edit", json!({})));
        assert_eq!(d.len(), 3);
        assert!(d.handle_key(key(KeyCode::Char('x')))); // ignored but consumed
        assert_eq!(d.len(), 3);
        d.handle_key(key(KeyCode::Enter));
        assert_eq!(r1.try_recv().unwrap(), PermissionDecision::Allow);
        assert_eq!(d.current().unwrap().input["command"], "pwd");
        d.handle_key(key(KeyCode::Char('a')));
        assert_eq!(r2.try_recv().unwrap(), PermissionDecision::AllowAlways);
        d.handle_key(key(KeyCode::Esc));
        assert!(matches!(r3.try_recv().unwrap(), PermissionDecision::Deny { .. }));
        assert!(d.is_empty());
        assert!(!d.handle_key(key(KeyCode::Enter)));
    }

    #[test]
    fn ctrl_c_not_consumed_and_closed_pruned() {
        let mut d = PermissionDialog::default();
        let rx = ask(&mut d, req("bash", json!({})));
        assert!(!d.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)));
        drop(rx);
        d.prune_closed();
        assert!(d.is_empty());
    }

    #[tokio::test]
    async fn asker_round_trip_and_deny_on_drop() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let asker = Arc::new(ChannelAsker::new(tx));
        let a = asker.clone();
        let h = tokio::spawn(async move { a.ask(&req("bash", json!({"command":"x"}))).await });
        let m = rx.recv().await.unwrap();
        m.reply.send(PermissionDecision::Allow).unwrap();
        assert_eq!(h.await.unwrap(), PermissionDecision::Allow);

        let a = asker.clone();
        let h = tokio::spawn(async move { a.ask(&req("bash", json!({}))).await });
        drop(rx.recv().await.unwrap()); // UI drops the request
        assert!(matches!(h.await.unwrap(), PermissionDecision::Deny { .. }));

        drop(rx);
        assert!(matches!(asker.ask(&req("bash", json!({}))).await, PermissionDecision::Deny { .. }));
    }

    #[test]
    fn describes_tools() {
        let s = describe(&req("bash", json!({"command": "cargo test\nls"})));
        assert_eq!(s.lines[0].1, "$ cargo test");
        let s = describe(&req("edit", json!({"file_path": "a.rs", "old_string": "x\ny", "new_string": "z"})));
        assert_eq!(s.headline, "Edit a.rs");
        assert_eq!(s.lines.iter().filter(|(k, _)| *k == DiffKind::Remove).count(), 2);
        assert_eq!(s.lines.iter().filter(|(k, _)| *k == DiffKind::Add).count(), 1);
        let long = (0..50).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
        let s = describe(&req("write", json!({"file_path": "b", "content": long})));
        assert!(s.lines.len() <= PREVIEW_LINES + 1);
    }

    #[test]
    fn renders_dialog() {
        let mut d = PermissionDialog::default();
        let _r = ask(&mut d, req("edit", json!({"file_path": "src/main.rs", "old_string": "foo", "new_string": "bar"})));
        let _r2 = ask(&mut d, req("bash", json!({"command": "ls"})));
        let theme = Theme::default();
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw_dialog(f, f.area(), &d, &theme)).unwrap();
        let buf = term.backend().buffer();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        for n in ["Permission required", "edit", "Edit src/main.rs", "- foo", "+ bar", "allow once", "always", "deny", "+1 queued"] {
            assert!(out.contains(n), "missing {n}:\n{out}");
        }
    }
}
