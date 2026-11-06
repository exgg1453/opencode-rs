//! In-process Rust plugins; a `PluginRegistry` is itself a `ToolHook`.
use std::sync::Arc;

use async_trait::async_trait;
use oc_core::ToolHook;
use serde_json::Value;

#[async_trait]
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    /// Called before a tool runs; may rewrite the input. Return Err to block execution.
    async fn before_tool(&self, _tool: &str, input: Value) -> anyhow::Result<Value> {
        Ok(input)
    }
    /// Called after a tool ran; may rewrite the output.
    async fn after_tool(&self, _tool: &str, _input: &Value, output: anyhow::Result<String>) -> anyhow::Result<String> {
        output
    }
}

#[derive(Clone, Default)]
pub struct PluginRegistry {
    plugins: Vec<Arc<dyn Plugin>>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn register(&mut self, p: Arc<dyn Plugin>) {
        self.plugins.push(p);
    }
    pub fn len(&self) -> usize {
        self.plugins.len()
    }
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }
    pub async fn before_tool(&self, tool: &str, mut input: Value) -> anyhow::Result<Value> {
        for p in &self.plugins {
            input = p.before_tool(tool, input).await?;
        }
        Ok(input)
    }
    pub async fn after_tool(&self, tool: &str, input: &Value, mut out: anyhow::Result<String>) -> anyhow::Result<String> {
        for p in &self.plugins {
            out = p.after_tool(tool, input, out).await;
        }
        out
    }
}

#[async_trait]
impl ToolHook for PluginRegistry {
    async fn before_tool(&self, tool: &str, input: &mut Value) -> anyhow::Result<()> {
        *input = PluginRegistry::before_tool(self, tool, input.clone()).await?;
        Ok(())
    }
    async fn after_tool(&self, tool: &str, input: &Value, output: &mut String) -> anyhow::Result<()> {
        *output = PluginRegistry::after_tool(self, tool, input, Ok(std::mem::take(output))).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Upper;
    #[async_trait]
    impl Plugin for Upper {
        fn name(&self) -> &str {
            "upper"
        }
        async fn after_tool(&self, _t: &str, _i: &Value, o: anyhow::Result<String>) -> anyhow::Result<String> {
            o.map(|s| s.to_uppercase())
        }
    }
    #[tokio::test]
    async fn hooks_run() {
        let mut r = PluginRegistry::new();
        r.register(Arc::new(Upper));
        let i = r.before_tool("x", serde_json::json!({"a":1})).await.unwrap();
        assert_eq!(i["a"], 1);
        assert_eq!(r.after_tool("x", &i, Ok("hi".into())).await.unwrap(), "HI");
    }
}
