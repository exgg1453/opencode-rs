//! Pure event formatting helpers.

use oc_core::AgentEvent;

pub const DIM: &str = "\x1b[2m";
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
        AgentEvent::Done => Some((Sink::Stdout, "\n".to_string())),
        AgentEvent::Error { message } => Some((Sink::Stderr, format!("error: {message}\n"))),
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

    #[test]
    fn json_line() {
        let l = format_json(&AgentEvent::TextDelta { text: "a".into() });
        assert_eq!(l, r#"{"type":"text_delta","text":"a"}"#);
        assert!(!l.contains('\n'));
    }

    #[test]
    fn truncate_unicode() {
        assert_eq!(truncate("héllo", 10), "héllo");
        assert_eq!(truncate("héllo", 3), "hé…");
    }
}
