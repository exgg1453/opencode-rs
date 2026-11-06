use async_trait::async_trait;
use serde_json::{json, Value};

use super::{resolve, str_arg, truncate_chars};
use crate::{Tool, ToolContext};

const DEFAULT_LIMIT: usize = 2000;
const MAX_LINE: usize = 2000;

pub struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }
    fn description(&self) -> &str {
        "Reads a file from the filesystem. Returns lines prefixed with line numbers. Use offset (1-based line) and limit to read a portion of large files. If given a directory, lists its entries."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "file_path":{"type":"string","description":"Path of the file to read (absolute or relative to the working directory)"},
                "offset":{"type":"integer","description":"1-based line number to start from"},
                "limit":{"type":"integer","description":"Maximum number of lines to read (default 2000)"}
            },
            "required":["file_path"]
        })
    }
    async fn execute(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<String> {
        let path = resolve(&ctx.cwd, str_arg(&input, "file_path")?);
        let meta = tokio::fs::metadata(&path).await.map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
        if meta.is_dir() {
            let mut names = vec![];
            let mut rd = tokio::fs::read_dir(&path).await?;
            while let Some(e) = rd.next_entry().await? {
                let mut n = e.file_name().to_string_lossy().into_owned();
                if e.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                    n.push('/');
                }
                names.push(n);
            }
            names.sort();
            return Ok(names.join("\n"));
        }
        let bytes = tokio::fs::read(&path).await.map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
        if bytes.iter().take(8192).any(|b| *b == 0) {
            anyhow::bail!("{} appears to be a binary file", path.display());
        }
        let text = String::from_utf8_lossy(&bytes);
        let offset = input.get("offset").and_then(Value::as_u64).unwrap_or(1).max(1) as usize;
        let limit = input.get("limit").and_then(Value::as_u64).map(|l| l as usize).unwrap_or(DEFAULT_LIMIT);
        let all: Vec<&str> = text.lines().collect();
        let total = all.len();
        let mut out = String::new();
        let mut last = offset - 1;
        for (i, line) in all.iter().enumerate().skip(offset - 1).take(limit) {
            let (l, cut) = truncate_chars(line, MAX_LINE);
            out.push_str(&format!("{:>6}\t{}{}\n", i + 1, l, if cut { "..." } else { "" }));
            last = i + 1;
        }
        if offset > total && total > 0 {
            anyhow::bail!("offset {offset} is beyond end of file ({total} lines)");
        }
        if last < total {
            out.push_str(&format!("\n(showing lines {offset}-{last} of {total}; use offset to read more)\n"));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_with_numbers_and_range() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let ctx = ToolContext { cwd: d.path().to_path_buf() };
        let out = ReadTool.execute(json!({"file_path":"a.txt"}), &ctx).await.unwrap();
        assert!(out.contains("     1\tone"));
        assert!(out.contains("     3\tthree"));
        let out = ReadTool.execute(json!({"file_path":"a.txt","offset":2,"limit":1}), &ctx).await.unwrap();
        assert!(out.contains("two") && !out.contains("three\n") || out.contains("showing"));
        assert!(ReadTool.execute(json!({"file_path":"nope"}), &ctx).await.is_err());
        let out = ReadTool.execute(json!({"file_path":"."}), &ctx).await.unwrap();
        assert_eq!(out, "a.txt");
    }
}
