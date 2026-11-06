//! Example process plugin (see PLUGINS.md): blocks destructive bash commands,
//! tags bash output, and provides a `current_time` tool.
use std::io::{BufRead, Write};

use serde_json::{json, Value};

fn dangerous(cmd: &str) -> bool {
    let c = cmd.split_whitespace().collect::<Vec<_>>().join(" ");
    c.contains("rm -rf /") && !c.contains("rm -rf /tmp") || c.contains("mkfs") || c.contains(":(){")
}

fn main() {
    let mut out = std::io::stdout();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        let p = &msg["params"];
        let reply: Result<Value, (i64, String)> = match msg["method"].as_str().unwrap_or("") {
            "initialize" => Ok(json!({
                "hooks": ["tool.execute.before", "tool.execute.after", "event"],
                "tools": [{"name": "current_time", "description": "Current UTC time as unix seconds",
                           "inputSchema": {"type": "object", "properties": {}}}]
            })),
            "tool.execute.before" => {
                let cmd = p["input"]["command"].as_str().unwrap_or("");
                if p["tool"] == "bash" && dangerous(cmd) {
                    Err((-32000, format!("refusing dangerous command: {cmd}")))
                } else {
                    Ok(json!({}))
                }
            }
            "tool.execute.after" => {
                if p["tool"] == "bash" {
                    Ok(json!({"output": format!("{}\n[checked by example_plugin]", p["output"].as_str().unwrap_or(""))}))
                } else {
                    Ok(json!({}))
                }
            }
            "tool.execute" if p["name"] == "current_time" => {
                let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                Ok(json!({"output": t.to_string()}))
            }
            "event" => {
                if let Ok(path) = std::env::var("EXAMPLE_PLUGIN_EVENT_LOG") {
                    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                        let _ = writeln!(f, "{}", p["event"]["type"].as_str().unwrap_or("?"));
                    }
                }
                continue;
            }
            "crash" => std::process::exit(1),
            m => Err((-32601, format!("method not found: {m}"))),
        };
        let Some(id) = msg.get("id") else { continue };
        let r = match reply {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err((code, message)) => json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}}),
        };
        writeln!(out, "{r}").unwrap();
        out.flush().unwrap();
    }
}
