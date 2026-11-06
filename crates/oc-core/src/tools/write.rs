use async_trait::async_trait;
use serde_json::{json, Value};

use super::{resolve, str_arg};
use crate::{Tool, ToolContext};

pub struct WriteTool;

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }
    fn description(&self) -> &str {
        "Writes a file to the filesystem, creating parent directories and overwriting any existing file. Prefer edit for modifying existing files."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "file_path":{"type":"string","description":"Path of the file to write"},
                "content":{"type":"string","description":"The full content to write"}
            },
            "required":["file_path","content"]
        })
    }
    async fn execute(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<String> {
        let path = resolve(&ctx.cwd, str_arg(&input, "file_path")?);
        let content = str_arg(&input, "content")?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let existed = path.exists();
        tokio::fs::write(&path, content).await.map_err(|e| anyhow::anyhow!("cannot write {}: {e}", path.display()))?;
        Ok(format!("{} {} ({} bytes)", if existed { "Overwrote" } else { "Created" }, path.display(), content.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writes_nested() {
        let d = tempfile::tempdir().unwrap();
        let ctx = ToolContext { cwd: d.path().to_path_buf() };
        let out = WriteTool.execute(json!({"file_path":"x/y/z.txt","content":"hello"}), &ctx).await.unwrap();
        assert!(out.starts_with("Created"));
        assert_eq!(std::fs::read_to_string(d.path().join("x/y/z.txt")).unwrap(), "hello");
        let out = WriteTool.execute(json!({"file_path":"x/y/z.txt","content":"b"}), &ctx).await.unwrap();
        assert!(out.starts_with("Overwrote"));
    }
}
