use async_trait::async_trait;
use ignore::WalkBuilder;
use serde_json::{json, Value};

use super::resolve;
use crate::{Tool, ToolContext};

const MAX_ENTRIES: usize = 500;

pub struct ListTool;

#[async_trait]
impl Tool for ListTool {
    fn name(&self) -> &str {
        "list"
    }
    fn description(&self) -> &str {
        "Lists files and directories under a path as an indented tree (respects .gitignore). Directories end with /. depth defaults to 2."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "path":{"type":"string","description":"Directory to list (default: working directory)"},
                "depth":{"type":"integer","description":"Maximum depth to descend (default 2)"}
            }
        })
    }
    async fn execute(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<String> {
        let root = resolve(&ctx.cwd, input.get("path").and_then(Value::as_str).unwrap_or("."));
        let depth = input.get("depth").and_then(Value::as_u64).unwrap_or(2).max(1) as usize;
        if !root.is_dir() {
            anyhow::bail!("{} is not a directory", root.display());
        }
        tokio::task::spawn_blocking(move || {
            let mut lines = vec![format!("{}/", root.display())];
            let mut count = 0;
            let walker = WalkBuilder::new(&root)
                .max_depth(Some(depth))
                .sort_by_file_name(|a, b| a.cmp(b))
                .build();
            for entry in walker {
                let Ok(entry) = entry else { continue };
                if entry.depth() == 0 {
                    continue;
                }
                if count >= MAX_ENTRIES {
                    lines.push("(truncated)".into());
                    break;
                }
                count += 1;
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                lines.push(format!(
                    "{}{}{}",
                    "  ".repeat(entry.depth()),
                    entry.file_name().to_string_lossy(),
                    if is_dir { "/" } else { "" }
                ));
            }
            Ok(lines.join("\n"))
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lists_tree() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("src/deep/er")).unwrap();
        std::fs::write(d.path().join("src/a.rs"), "").unwrap();
        std::fs::write(d.path().join("z.txt"), "").unwrap();
        let ctx = ToolContext { cwd: d.path().to_path_buf() };
        let out = ListTool.execute(json!({}), &ctx).await.unwrap();
        assert!(out.contains("  src/\n    a.rs") || out.contains("  src/"));
        assert!(out.contains("z.txt") && out.contains("deep/"));
        assert!(!out.contains("      er/"));
        assert!(ListTool.execute(json!({"path":"z.txt"}), &ctx).await.is_err());
    }
}
