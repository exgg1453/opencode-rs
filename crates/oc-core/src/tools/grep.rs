use async_trait::async_trait;
use ignore::WalkBuilder;
use regex::Regex;
use serde_json::{json, Value};

use super::{resolve, str_arg, truncate_chars};
use crate::{Tool, ToolContext};

const MAX_MATCHES: usize = 200;

pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }
    fn description(&self) -> &str {
        "Searches file contents with a regular expression (Rust regex syntax). Respects .gitignore. Returns matches as path:line:text, capped at 200."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "pattern":{"type":"string","description":"Regular expression to search for"},
                "path":{"type":"string","description":"Directory or file to search (default: working directory)"},
                "include":{"type":"string","description":"Glob restricting files searched, e.g. *.rs"}
            },
            "required":["pattern"]
        })
    }
    async fn execute(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<String> {
        let pattern = str_arg(&input, "pattern")?.to_string();
        let root = resolve(&ctx.cwd, input.get("path").and_then(Value::as_str).unwrap_or("."));
        let include = input.get("include").and_then(Value::as_str).map(str::to_string);
        let cwd = ctx.cwd.clone();
        tokio::task::spawn_blocking(move || {
            let re = Regex::new(&pattern).map_err(|e| anyhow::anyhow!("invalid regex: {e}"))?;
            let inc = match include {
                Some(g) => Some(globset::Glob::new(&g)?.compile_matcher()),
                None => None,
            };
            let mut results = vec![];
            let mut total = 0usize;
            'outer: for entry in WalkBuilder::new(&root).build() {
                let Ok(entry) = entry else { continue };
                if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    continue;
                }
                let p = entry.path();
                if let Some(m) = &inc {
                    let name = p.file_name().map(std::path::Path::new);
                    if !(m.is_match(p) || name.map(|n| m.is_match(n)).unwrap_or(false)) {
                        continue;
                    }
                }
                let Ok(bytes) = std::fs::read(p) else { continue };
                if bytes.iter().take(8192).any(|b| *b == 0) {
                    continue;
                }
                let text = String::from_utf8_lossy(&bytes);
                let shown = p.strip_prefix(&cwd).unwrap_or(p).display().to_string();
                for (i, line) in text.lines().enumerate() {
                    if re.is_match(line) {
                        total += 1;
                        if results.len() < MAX_MATCHES {
                            let (l, _) = truncate_chars(line, 500);
                            results.push(format!("{}:{}:{}", shown, i + 1, l));
                        } else {
                            break 'outer;
                        }
                    }
                }
            }
            if results.is_empty() {
                return Ok("No matches found".to_string());
            }
            let mut out = results.join("\n");
            if total > MAX_MATCHES {
                out.push_str(&format!("\n\n(results truncated at {MAX_MATCHES} matches)"));
            }
            Ok(out)
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn finds_matches() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("src")).unwrap();
        std::fs::write(d.path().join("src/a.rs"), "fn main() {}\nlet x = 1;\n").unwrap();
        std::fs::write(d.path().join("b.txt"), "fn nope\n").unwrap();
        let ctx = ToolContext { cwd: d.path().to_path_buf() };
        let out = GrepTool.execute(json!({"pattern":r"fn \w+"}), &ctx).await.unwrap();
        assert!(out.contains("src/a.rs:1:fn main() {}"));
        assert!(out.contains("b.txt:1"));
        let out = GrepTool.execute(json!({"pattern":"fn","include":"*.rs"}), &ctx).await.unwrap();
        assert!(out.contains("a.rs") && !out.contains("b.txt"));
        assert_eq!(GrepTool.execute(json!({"pattern":"zzzz"}), &ctx).await.unwrap(), "No matches found");
        assert!(GrepTool.execute(json!({"pattern":"("}), &ctx).await.is_err());
    }
}
