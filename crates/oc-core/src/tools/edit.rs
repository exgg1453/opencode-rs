use async_trait::async_trait;
use serde_json::{json, Value};

use super::{resolve, str_arg};
use crate::{Tool, ToolContext};

pub struct EditTool;

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }
    fn description(&self) -> &str {
        "Performs an exact string replacement in a file. old_string must match exactly (including whitespace) and be unique in the file unless replace_all is true."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "file_path":{"type":"string","description":"Path of the file to modify"},
                "old_string":{"type":"string","description":"The exact text to replace"},
                "new_string":{"type":"string","description":"The replacement text"},
                "replace_all":{"type":"boolean","description":"Replace every occurrence (default false)"}
            },
            "required":["file_path","old_string","new_string"]
        })
    }
    async fn execute(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<String> {
        let path = resolve(&ctx.cwd, str_arg(&input, "file_path")?);
        let old = str_arg(&input, "old_string")?;
        let new = str_arg(&input, "new_string")?;
        let all = input.get("replace_all").and_then(Value::as_bool).unwrap_or(false);
        if old.is_empty() {
            anyhow::bail!("old_string must not be empty");
        }
        if old == new {
            anyhow::bail!("old_string and new_string are identical");
        }
        let text = tokio::fs::read_to_string(&path).await.map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
        let n = text.matches(old).count();
        if n == 0 {
            anyhow::bail!("old_string not found in {}", path.display());
        }
        if n > 1 && !all {
            anyhow::bail!(
                "old_string appears {n} times in {}; provide more context to make it unique or set replace_all",
                path.display()
            );
        }
        let updated = if all { text.replace(old, new) } else { text.replacen(old, new, 1) };
        tokio::fs::write(&path, updated).await?;
        Ok(format!("Edited {} ({} replacement{})", path.display(), n, if n == 1 { "" } else { "s" }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unique_and_replace_all() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("f.txt");
        std::fs::write(&f, "a b a c").unwrap();
        let ctx = ToolContext { cwd: d.path().to_path_buf() };
        let e = EditTool.execute(json!({"file_path":"f.txt","old_string":"a","new_string":"x"}), &ctx).await;
        assert!(e.unwrap_err().to_string().contains("2 times"));
        EditTool.execute(json!({"file_path":"f.txt","old_string":"b","new_string":"y"}), &ctx).await.unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "a y a c");
        EditTool.execute(json!({"file_path":"f.txt","old_string":"a","new_string":"z","replace_all":true}), &ctx).await.unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "z y z c");
        assert!(EditTool.execute(json!({"file_path":"f.txt","old_string":"nope","new_string":"z"}), &ctx).await.is_err());
    }
}
