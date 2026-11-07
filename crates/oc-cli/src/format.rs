//! Pure event formatting helpers.

use oc_core::AgentEvent;

pub const DIM: &str = "\x1b[2m";
pub const ITALIC: &str = "\x1b[3m";
pub const RESET: &str = "\x1b[0m";

/// Where a formatted chunk should be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sink {
    Stdout,
    Stderr,
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Format an event for human output. Returns `None` for events with no output.
pub fn format_event(ev: &AgentEvent) -> Option<(Sink, String)> {
    match ev {
        AgentEvent::TextDelta { text } => Some((Sink::Stdout, text.clone())),
        AgentEvent::ToolStart { name, input, .. } => {
            let args = truncate(&input.to_string(), 100);
            Some((Sink::Stderr, format!("{DIM}> {name} {args}{RESET}\n")))
        }
        AgentEvent::ToolEnd { output, is_error, .. } => {
            let first = output.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            let status = if *is_error { "error" } else { "done" };
            Some((Sink::Stderr, format!("{DIM}  {status}: {}{RESET}\n", truncate(first.trim(), 100))))
        }
        AgentEvent::ThinkingDelta { text } => Some((Sink::Stderr, format!("{DIM}{ITALIC}{text}{RESET}"))),
        AgentEvent::Retry { attempt, max, reason } => Some((
            Sink::Stderr,
            format!("{DIM}Connection lost, retrying ({attempt}/{max}): {}{RESET}\n", truncate(reason.lines().next().unwrap_or(""), 120)),
        )),
        AgentEvent::Done => Some((Sink::Stdout, "\n".to_string())),
        AgentEvent::Error { message } => Some((Sink::Stderr, format!("error: {message}\n"))),
    }
}

/// Stateful wrapper around [`format_event`] for the text output mode: thinking is
/// introduced once per block with a "Thinking:" prefix and closed with a newline, and a
/// retry also separates the restarted text from the partial text already on stdout.
#[derive(Debug, Default)]
pub struct Printer {
    pub hide_thinking: bool,
    in_thinking: bool,
}

impl Printer {
    pub fn new(hide_thinking: bool) -> Self {
        Printer { hide_thinking, in_thinking: false }
    }

    pub fn format(&mut self, ev: &AgentEvent) -> Vec<(Sink, String)> {
        let mut out = vec![];
        if let AgentEvent::ThinkingDelta { text } = ev {
            if self.hide_thinking {
                return out;
            }
            let prefix = if self.in_thinking { "" } else { "Thinking: " };
            self.in_thinking = true;
            out.push((Sink::Stderr, format!("{DIM}{ITALIC}{prefix}{text}{RESET}")));
            return out;
        }
        if std::mem::take(&mut self.in_thinking) {
            out.push((Sink::Stderr, "\n".to_string()));
        }
        out.extend(format_event(ev));
        if matches!(ev, AgentEvent::Retry { .. }) {
            out.push((Sink::Stdout, "\n".to_string()));
        }
        out
    }
}

/// One JSON line per event.
pub fn format_json(ev: &AgentEvent) -> String {
    serde_json::to_string(ev).unwrap_or_else(|_| "{}".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text_is_raw_stdout() {
        let r = format_event(&AgentEvent::TextDelta { text: "hi".into() });
        assert_eq!(r, Some((Sink::Stdout, "hi".into())));
    }

    #[test]
    fn tool_start_dim_stderr() {
        let (s, t) = format_event(&AgentEvent::ToolStart { id: "1".into(), name: "read".into(), input: json!({"p": 1}) }).unwrap();
        assert_eq!(s, Sink::Stderr);
        assert!(t.starts_with(DIM) && t.contains("read") && t.contains("{\"p\":1}") && t.ends_with("\n"));
    }

    #[test]
    fn tool_start_truncates() {
        let big = "x".repeat(500);
        let (_, t) = format_event(&AgentEvent::ToolStart { id: "1".into(), name: "w".into(), input: json!(big) }).unwrap();
        assert!(t.contains('…') && t.len() < 200);
    }

    #[test]
    fn tool_end_first_line_and_status() {
        let (_, t) = format_event(&AgentEvent::ToolEnd { id: "1".into(), output: "\nline1\nline2".into(), is_error: true }).unwrap();
        assert!(t.contains("error: line1") && !t.contains("line2"));
        let (_, t) = format_event(&AgentEvent::ToolEnd { id: "1".into(), output: "ok".into(), is_error: false }).unwrap();
        assert!(t.contains("done: ok"));
    }

    #[test]
    fn error_and_done() {
        assert_eq!(format_event(&AgentEvent::Error { message: "boom".into() }), Some((Sink::Stderr, "error: boom\n".into())));
        assert_eq!(format_event(&AgentEvent::Done), Some((Sink::Stdout, "\n".into())));
    }

    fn texts(v: Vec<(Sink, String)>) -> Vec<(Sink, String)> {
        v.into_iter().map(|(s, t)| (s, t.replace(DIM, "<d>").replace(ITALIC, "<i>").replace(RESET, "</>"))).collect()
    }

    #[test]
    fn thinking_prefixed_once_per_block() {
        let mut p = Printer::new(false);
        let a = texts(p.format(&AgentEvent::ThinkingDelta { text: "a".into() }));
        let b = texts(p.format(&AgentEvent::ThinkingDelta { text: "b".into() }));
        assert_eq!(a, vec![(Sink::Stderr, "<d><i>Thinking: a</>".to_string())]);
        assert_eq!(b, vec![(Sink::Stderr, "<d><i>b</>".to_string())]);
        // text closes the block with a newline on stderr, then prints raw on stdout
        let c = texts(p.format(&AgentEvent::TextDelta { text: "hi".into() }));
        assert_eq!(c, vec![(Sink::Stderr, "\n".to_string()), (Sink::Stdout, "hi".to_string())]);
        // a new block gets a new prefix
        let d = texts(p.format(&AgentEvent::ThinkingDelta { text: "c".into() }));
        assert_eq!(d, vec![(Sink::Stderr, "<d><i>Thinking: c</>".to_string())]);
    }

    #[test]
    fn hide_thinking_prints_nothing() {
        let mut p = Printer::new(true);
        assert!(p.format(&AgentEvent::ThinkingDelta { text: "a".into() }).is_empty());
        assert_eq!(p.format(&AgentEvent::TextDelta { text: "x".into() }), vec![(Sink::Stdout, "x".to_string())]);
    }

    #[test]
    fn retry_notice_and_separator() {
        let mut p = Printer::new(false);
        let r = p.format(&AgentEvent::Retry { attempt: 1, max: 3, reason: "reset".into() });
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].0, Sink::Stderr);
        assert!(r[0].1.contains("retrying (1/3)") && r[0].1.contains("reset"));
        assert_eq!(r[1], (Sink::Stdout, "\n".to_string()));
    }

    #[test]
    fn json_line() {
        let l = format_json(&AgentEvent::TextDelta { text: "a".into() });
        assert_eq!(l, r#"{"type":"text_delta","text":"a"}"#);
        assert!(!l.contains('\n'));
    }

    #[test]
    fn json_new_events() {
        assert_eq!(format_json(&AgentEvent::ThinkingDelta { text: "a".into() }), r#"{"type":"thinking_delta","text":"a"}"#);
        assert_eq!(
            format_json(&AgentEvent::Retry { attempt: 1, max: 3, reason: "x".into() }),
            r#"{"type":"retry","attempt":1,"max":3,"reason":"x"}"#
        );
    }

    #[test]
    fn truncate_unicode() {
        assert_eq!(truncate("héllo", 10), "héllo");
        assert_eq!(truncate("héllo", 3), "hé…");
    }
}
