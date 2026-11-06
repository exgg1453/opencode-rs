//! Tiny fake MCP stdio server used by tests.
use std::io::{BufRead, Write};

use serde_json::{json, Value};

fn main() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(id) = msg.get("id").cloned() else { continue };
        let params = &msg["params"];
        let result = match msg["method"].as_str().unwrap_or("") {
            "initialize" => json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"0"}}),
            "tools/list" => {
                if params.get("cursor").and_then(Value::as_str) == Some("p2") {
                    json!({"tools":[{"name":"fail","description":"always errors","inputSchema":{"type":"object"}}]})
                } else {
                    json!({"tools":[{"name":"echo","description":"echo text","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}}],"nextCursor":"p2"})
                }
            }
            "tools/call" => match params["name"].as_str() {
                Some("echo") => json!({"content":[{"type":"text","text":format!("echo: {}", params["arguments"]["text"].as_str().unwrap_or(""))}]}),
                _ => json!({"content":[{"type":"text","text":"boom"}],"isError":true}),
            },
            _ => {
                let r = json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"nope"}});
                writeln!(out, "{r}").unwrap();
                out.flush().unwrap();
                continue;
            }
        };
        writeln!(out, "{}", json!({"jsonrpc":"2.0","id":id,"result":result})).unwrap();
        out.flush().unwrap();
    }
}
