//! Out-of-process plugins: separate executables speaking JSON-RPC 2.0 over stdio.
//! See `PLUGINS.md` for the protocol.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use oc_core::{AgentEvent, Tool, ToolContext, ToolHook};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::client::McpClient;

pub const PLUGIN_PROTOCOL_VERSION: u32 = 1;
const HOOK_TIMEOUT: Duration = Duration::from_secs(10);
const INIT_TIMEOUT: Duration = Duration::from_secs(15);
const TOOL_TIMEOUT: Duration = Duration::from_secs(120);

/// How to launch one plugin.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginSpec {
    pub name: String,
    pub command: Vec<String>,
    pub environment: HashMap<String, String>,
    /// Per-hook timeout in milliseconds.
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct RawPluginConfig {
    command: Vec<String>,
    #[serde(default)]
    environment: HashMap<String, String>,
    #[serde(default = "yes")]
    enabled: bool,
    #[serde(default)]
    timeout: Option<u64>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct InitResult {
    #[serde(default)]
    hooks: Vec<String>,
    #[serde(default)]
    tools: Vec<ToolDef>,
}

#[derive(Debug, Deserialize)]
struct ToolDef {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default = "default_schema", rename = "inputSchema")]
    input_schema: Value,
}

fn default_schema() -> Value {
    json!({"type":"object"})
}

/// Parse `config.plugin`; invalid or disabled entries are skipped.
pub fn parse_config(map: &serde_json::Map<String, Value>) -> Vec<PluginSpec> {
    let mut out = Vec::new();
    for (name, v) in map {
        match serde_json::from_value::<RawPluginConfig>(v.clone()) {
            Ok(c) if c.enabled && !c.command.is_empty() => out.push(PluginSpec {
                name: name.clone(),
                command: c.command,
                environment: c.environment,
                timeout_ms: c.timeout,
            }),
            Ok(c) if !c.enabled => {}
            Ok(_) => tracing::warn!("plugin '{name}': empty command, skipped"),
            Err(e) => tracing::warn!("plugin '{name}': invalid config: {e}"),
        }
    }
    out
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}
#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// Executables in a plugin directory (non-executable files are skipped with a warning).
pub fn discover_dir(dir: &Path) -> Vec<PluginSpec> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            continue;
        }
        let Some(name) = p.file_name().and_then(|n| n.to_str()).map(str::to_string) else { continue };
        if name.starts_with('.') {
            continue;
        }
        if !is_executable(&p) {
            tracing::warn!("plugin file {} is not executable, skipped", p.display());
            continue;
        }
        out.push(PluginSpec {
            name,
            command: vec![p.to_string_lossy().into_owned()],
            environment: HashMap::new(),
            timeout_ms: None,
        });
    }
    out
}

/// Config entries first, then `~/.config/opencode/plugins`, then `<cwd>/.opencode/plugins`.
/// Names are unique; the first definition wins.
pub fn discover(config: &oc_core::Config, cwd: &Path) -> Vec<PluginSpec> {
    let mut specs = parse_config(&config.plugin);
    if let Some(home) = std::env::var_os("HOME") {
        specs.extend(discover_dir(&PathBuf::from(home).join(".config/opencode/plugins")));
    }
    specs.extend(discover_dir(&cwd.join(".opencode/plugins")));
    let mut seen = HashSet::new();
    specs.retain(|s| seen.insert(s.name.clone()));
    specs
}

/// A running plugin process.
pub struct ProcessPlugin {
    name: String,
    client: McpClient,
    hooks: HashSet<String>,
    timeout: Duration,
    disabled: AtomicBool,
}

impl ProcessPlugin {
    /// Spawn, handshake, and return the plugin plus its custom tools.
    pub async fn start(spec: &PluginSpec, cwd: &Path) -> anyhow::Result<(Arc<ProcessPlugin>, Vec<Arc<dyn Tool>>)> {
        let timeout = spec.timeout_ms.map(Duration::from_millis).unwrap_or(HOOK_TIMEOUT);
        let client = McpClient::connect_stdio(&spec.name, &spec.command, &spec.environment, timeout).await?;
        let res = client
            .request_with_timeout(
                "initialize",
                json!({
                    "protocolVersion": PLUGIN_PROTOCOL_VERSION,
                    "cwd": cwd,
                    "opencodeVersion": env!("CARGO_PKG_VERSION"),
                }),
                INIT_TIMEOUT.max(timeout),
            )
            .await?;
        let init: InitResult = serde_json::from_value(res)?;
        let plugin = Arc::new(ProcessPlugin {
            name: spec.name.clone(),
            client,
            hooks: init.hooks.into_iter().collect(),
            timeout,
            disabled: AtomicBool::new(false),
        });
        let tools = init
            .tools
            .into_iter()
            .map(|t| {
                Arc::new(PluginTool {
                    name: t.name,
                    description: t.description,
                    schema: t.input_schema,
                    plugin: plugin.clone(),
                }) as Arc<dyn Tool>
            })
            .collect();
        Ok((plugin, tools))
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn subscribes(&self, hook: &str) -> bool {
        self.hooks.contains(hook)
    }

    fn active(&self, hook: &str) -> bool {
        !self.disabled.load(Ordering::Relaxed) && self.subscribes(hook)
    }

    /// Call a method. `Ok(Err(e))` is a plugin-level JSON-RPC error; `Err` means the plugin
    /// is broken (crash/timeout) and is now disabled.
    async fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<anyhow::Result<Value>, ()> {
        match self.client.request_with_timeout(method, params, timeout).await {
            Ok(v) => Ok(Ok(v)),
            Err(e) if e.root_cause().to_string().starts_with("JSON-RPC error") => Ok(Err(e)),
            Err(e) => {
                tracing::warn!("plugin '{}' failed ({method}): {e:#}; disabling it", self.name);
                self.disabled.store(true, Ordering::Relaxed);
                Err(())
            }
        }
    }
}

fn rpc_message(e: &anyhow::Error) -> String {
    let s = e.root_cause().to_string();
    match s.split_once(": ") {
        Some((head, rest)) if head.starts_with("JSON-RPC error") => rest.to_string(),
        _ => s,
    }
}

#[async_trait]
impl ToolHook for ProcessPlugin {
    async fn before_tool(&self, tool: &str, input: &mut Value) -> anyhow::Result<()> {
        if !self.active("tool.execute.before") {
            return Ok(());
        }
        let params = json!({"tool": tool, "input": input});
        match self.call("tool.execute.before", params, self.timeout).await {
            Ok(Ok(res)) => {
                if let Some(new) = res.get("input").filter(|v| !v.is_null()) {
                    *input = new.clone();
                }
                Ok(())
            }
            Ok(Err(e)) => anyhow::bail!("blocked by plugin '{}': {}", self.name, rpc_message(&e)),
            Err(()) => Ok(()),
        }
    }

    async fn after_tool(&self, tool: &str, input: &Value, output: &mut String) -> anyhow::Result<()> {
        if !self.active("tool.execute.after") {
            return Ok(());
        }
        let params = json!({"tool": tool, "input": input, "output": output});
        match self.call("tool.execute.after", params, self.timeout).await {
            Ok(Ok(res)) => {
                if let Some(s) = res.get("output").and_then(Value::as_str) {
                    *output = s.to_string();
                }
            }
            Ok(Err(e)) => tracing::warn!("plugin '{}' tool.execute.after error: {e:#}", self.name),
            Err(()) => {}
        }
        Ok(())
    }

    async fn on_event(&self, session_id: &str, event: &AgentEvent) {
        if !self.active("event") {
            return;
        }
        let params = json!({"sessionId": session_id, "event": event});
        if let Err(e) = self.client.notify("event", Some(params)).await {
            tracing::warn!("plugin '{}' event delivery failed: {e:#}; disabling it", self.name);
            self.disabled.store(true, Ordering::Relaxed);
        }
    }
}

/// A custom tool defined by a plugin.
pub struct PluginTool {
    name: String,
    description: String,
    schema: Value,
    plugin: Arc<ProcessPlugin>,
}

#[async_trait]
impl Tool for PluginTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn input_schema(&self) -> Value {
        self.schema.clone()
    }
    async fn execute(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<String> {
        if self.plugin.disabled.load(Ordering::Relaxed) {
            anyhow::bail!("plugin '{}' is disabled", self.plugin.name);
        }
        let params = json!({"name": self.name, "input": input, "cwd": ctx.cwd});
        match self.plugin.call("tool.execute", params, TOOL_TIMEOUT).await {
            Ok(Ok(res)) => Ok(match res.get("output") {
                Some(Value::String(s)) => s.clone(),
                Some(v) if !v.is_null() => v.to_string(),
                _ => String::new(),
            }),
            Ok(Err(e)) => anyhow::bail!("{}", rpc_message(&e)),
            Err(()) => anyhow::bail!("plugin '{}' failed and was disabled", self.plugin.name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_config() {
        let v = json!({
            "a": {"command": ["x", "y"], "environment": {"K": "V"}},
            "b": {"command": ["z"], "enabled": false},
            "c": {"nope": 1},
            "d": {"command": []}
        });
        let specs = parse_config(v.as_object().unwrap());
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "a");
        assert_eq!(specs[0].environment["K"], "V");
    }

    #[cfg(unix)]
    #[test]
    fn discovers_only_executables() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("oc-ext-disc-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let ok = d.join("good");
        let bad = d.join("bad.txt");
        std::fs::write(&ok, "#!/bin/sh\n").unwrap();
        std::fs::write(&bad, "x").unwrap();
        std::fs::set_permissions(&ok, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o644)).unwrap();
        let specs = discover_dir(&d);
        std::fs::remove_dir_all(&d).ok();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "good");
    }
}
