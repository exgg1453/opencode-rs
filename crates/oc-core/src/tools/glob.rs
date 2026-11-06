use async_trait::async_trait;
use ignore::WalkBuilder;
use serde_json::{json, Value};

use super::{resolve, str_arg};
use crate::{Tool, ToolContext};

const MAX_RESULTS: usize = 200;

pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }
    fn description(&self) -> &str {
        "Finds files by glob pattern (e.g. **/*.rs), respecting .gitignore. Returns paths sorted by modification time, newest first, capped at 200."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "pattern":{"type":"string","description":"Glob pattern, matched relative to the search directory"},
                "path":{"type":"string","description":"Directory to search (default: working directory)"}
            },
            "required":["pattern"]
        })
    }
    async fn execute(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<String> {
        let pattern = str_arg(&input, "pattern")?.to_string();
        let root = resolve(&ctx.cwd, input.get("path").and_then(Value::as_str).unwrap_or("."));
        tokio::task::spawn_blocking(move || {
            let m = globset::GlobBuilder::new(&pattern).literal_separator(true).build()?.compile_matcher();
            // Bare patterns like "*.rs" match at any depth.
            let any_depth = !pattern.contains('/');
            let mut found: Vec<(std::time::SystemTime, String)> = vec![];
            for entry in WalkBuilder::new(&root).build() {
                let Ok(entry) = entry else { continue };
                if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    continue;
                }
                let p = entry.path();
                let rel = p.strip_prefix(&root).unwrap_or(p);
                let hit = m.is_match(rel) || (any_depth && p.file_name().map(|n| m.is_match(std::path::Path::new(n))).unwrap_or(false));
                if hit {
                    let t = entry.metadata().ok().and_then(|m| m.modified().ok()).unwrap_or(std::time::UNIX_EPOCH);
                    found.push((t, p.display().to_string()));
                }
            }
            if found.is_empty() {
                return Ok("No files found".to_string());
            }
            found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            let truncated = found.len() > MAX_RESULTS;
            let mut out: Vec<String> = found.into_iter().take(MAX_RESULTS).map(|x| x.1).collect();
            if truncated {
                out.push(format!("(results truncated at {MAX_RESULTS})"));
            }
            Ok(out.join("\n"))
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn matches_patterns() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("src/deep")).unwrap();
        std::fs::write(d.path().join("src/a.rs"), "").unwrap();
        std::fs::write(d.path().join("src/deep/b.rs"), "").unwrap();
        std::fs::write(d.path().join("c.txt"), "").unwrap();
        let ctx = ToolContext { cwd: d.path().to_path_buf() };
        let out = GlobTool.execute(json!({"pattern":"**/*.rs"}), &ctx).await.unwrap();
        assert!(out.contains("a.rs") && out.contains("b.rs") && !out.contains("c.txt"));
        let out = GlobTool.execute(json!({"pattern":"*.txt"}), &ctx).await.unwrap();
        assert!(out.contains("c.txt"));
        let out = GlobTool.execute(json!({"pattern":"src/*.rs"}), &ctx).await.unwrap();
        assert!(out.contains("a.rs") && !out.contains("b.rs"));
        assert_eq!(GlobTool.execute(json!({"pattern":"*.zip"}), &ctx).await.unwrap(), "No files found");
    }
}
