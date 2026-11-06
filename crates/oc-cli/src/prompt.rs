//! Interactive permission prompt for `opencode run`.

use std::io::{BufRead, Write};

use async_trait::async_trait;
use oc_core::permission::Asker;
use oc_core::{PermissionDecision, PermissionRequest};

/// Parse an answer to the `[y]es / [a]lways / [n]o` prompt. Empty input means no.
pub fn parse_answer(line: &str) -> Option<PermissionDecision> {
    match line.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => Some(PermissionDecision::Allow),
        "a" | "always" => Some(PermissionDecision::AllowAlways),
        "" | "n" | "no" => Some(PermissionDecision::Deny { reason: Some("rejected by the user".into()) }),
        _ => None,
    }
}

/// One-line description of what the tool is about to do.
pub fn describe(req: &PermissionRequest) -> String {
    let s = |k: &str| req.input.get(k).and_then(|v| v.as_str());
    let detail = s("command")
        .or_else(|| s("file_path"))
        .or_else(|| s("filePath"))
        .or_else(|| s("path"))
        .or_else(|| s("url"))
        .map(str::to_string)
        .unwrap_or_else(|| req.input.to_string());
    let detail: String = detail.chars().take(300).collect();
    format!("{}: {}", req.tool, detail)
}

/// Prompts on stderr and reads the answer from stdin (stdin must be a TTY).
pub struct TtyAsker {
    lock: tokio::sync::Mutex<()>,
}

impl TtyAsker {
    pub fn new() -> Self {
        TtyAsker { lock: tokio::sync::Mutex::new(()) }
    }
}

impl Default for TtyAsker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Asker for TtyAsker {
    async fn ask(&self, req: &PermissionRequest) -> PermissionDecision {
        // Serialize prompts so parallel tool calls don't interleave.
        let _g = self.lock.lock().await;
        let text = describe(req);
        tokio::task::spawn_blocking(move || {
            let stdin = std::io::stdin();
            loop {
                eprint!("\nPermission required: {text}\nAllow? [y]es / [a]lways / [n]o: ");
                let _ = std::io::stderr().flush();
                let mut line = String::new();
                match stdin.lock().read_line(&mut line) {
                    Ok(0) | Err(_) => {
                        return PermissionDecision::Deny { reason: Some("no answer (stdin closed)".into()) }
                    }
                    Ok(_) => {}
                }
                if let Some(d) = parse_answer(&line) {
                    return d;
                }
                eprintln!("please answer y, a or n");
            }
        })
        .await
        .unwrap_or(PermissionDecision::Deny { reason: Some("prompt failed".into()) })
    }
}

/// Reason given when a rule needs approval but nobody can be asked.
pub const NON_INTERACTIVE_REASON: &str = "this action requires approval but stdin is not a terminal; \
rerun interactively, pass --yes to auto-approve, or allow it in the `permission` section of opencode.json";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_answers() {
        assert_eq!(parse_answer("y\n"), Some(PermissionDecision::Allow));
        assert_eq!(parse_answer(" YES "), Some(PermissionDecision::Allow));
        assert_eq!(parse_answer("a"), Some(PermissionDecision::AllowAlways));
        assert!(matches!(parse_answer("n"), Some(PermissionDecision::Deny { .. })));
        assert!(matches!(parse_answer("\n"), Some(PermissionDecision::Deny { .. })));
        assert_eq!(parse_answer("maybe"), None);
    }

    #[test]
    fn describes_requests() {
        let r = PermissionRequest {
            session_id: "s".into(),
            call_id: "c".into(),
            tool: "bash".into(),
            input: json!({"command": "ls"}),
        };
        assert_eq!(describe(&r), "bash: ls");
        let r = PermissionRequest { input: json!({"x": 1}), ..r };
        assert_eq!(describe(&r), "bash: {\"x\":1}");
    }
}
