use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

use super::str_arg;
use crate::{Tool, ToolContext};

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const MAX_OUTPUT: usize = 30_000;

pub struct BashTool;

pub(crate) fn truncate_output(s: String) -> String {
    if s.len() <= MAX_OUTPUT {
        return s;
    }
    let mut end = MAX_OUTPUT;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n\n[output truncated: {} of {} bytes shown]", &s[..end], end, s.len())
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }
    fn description(&self) -> &str {
        "Executes a bash command in the working directory with an optional timeout (default 120000 ms, max 600000 ms). Returns combined stdout and stderr, truncated if very long, and the exit code on failure."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "command":{"type":"string","description":"The command to execute"},
                "timeout":{"type":"integer","description":"Timeout in milliseconds"},
                "description":{"type":"string","description":"Short description of what the command does"}
            },
            "required":["command"]
        })
    }
    async fn execute(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<String> {
        let command = str_arg(&input, "command")?;
        let timeout_ms = input.get("timeout").and_then(Value::as_u64).unwrap_or(DEFAULT_TIMEOUT_MS).clamp(1, MAX_TIMEOUT_MS);
        let mut child = tokio::process::Command::new("bash")
            .arg("-c")
            .arg(command)
            .current_dir(&ctx.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to spawn bash: {e}"))?;
        let mut so = child.stdout.take().unwrap();
        let mut se = child.stderr.take().unwrap();
        let out_task = tokio::spawn(async move {
            let mut b = Vec::new();
            let _ = so.read_to_end(&mut b).await;
            b
        });
        let err_task = tokio::spawn(async move {
            let mut b = Vec::new();
            let _ = se.read_to_end(&mut b).await;
            b
        });
        let status = match tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait()).await {
            Ok(s) => s?,
            Err(_) => {
                let _ = child.kill().await;
                out_task.abort();
                err_task.abort();
                anyhow::bail!("command timed out after {timeout_ms} ms");
            }
        };
        let stdout = out_task.await.unwrap_or_default();
        let stderr = err_task.await.unwrap_or_default();
        let mut text = String::from_utf8_lossy(&stdout).into_owned();
        let e = String::from_utf8_lossy(&stderr);
        if !e.is_empty() {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&e);
        }
        let mut text = truncate_output(text);
        if !status.success() {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&format!("[exit code: {}]", status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into())));
            anyhow::bail!("{text}");
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_captures_and_fails() {
        let d = tempfile::tempdir().unwrap();
        let ctx = ToolContext { cwd: d.path().to_path_buf() };
        let out = BashTool.execute(json!({"command":"echo hi; echo err >&2; pwd"}), &ctx).await.unwrap();
        assert!(out.contains("hi") && out.contains("err"));
        assert!(out.contains(d.path().file_name().unwrap().to_str().unwrap()));
        let e = BashTool.execute(json!({"command":"echo bad; exit 3"}), &ctx).await.unwrap_err().to_string();
        assert!(e.contains("bad") && e.contains("exit code: 3"));
    }

    #[tokio::test]
    async fn timeout_and_truncation() {
        let d = tempfile::tempdir().unwrap();
        let ctx = ToolContext { cwd: d.path().to_path_buf() };
        let e = BashTool.execute(json!({"command":"sleep 5","timeout":200}), &ctx).await.unwrap_err();
        assert!(e.to_string().contains("timed out"));
        let out = BashTool.execute(json!({"command":"head -c 100000 /dev/zero | tr '\\0' a"}), &ctx).await.unwrap();
        assert!(out.contains("output truncated"));
        assert!(out.len() < 31_000);
    }
}
