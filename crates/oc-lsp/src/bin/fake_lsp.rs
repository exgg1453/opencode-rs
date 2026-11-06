//! Tiny fake language server for tests: publishes an error diagnostic for any
//! opened/changed document whose text contains "BAD", else an empty list.

use std::io::{Read, Write};

use oc_lsp::framing::{encode, Decoder};
use serde_json::{json, Value};

fn send(v: &Value) {
    let mut o = std::io::stdout();
    o.write_all(&encode(v)).unwrap();
    o.flush().unwrap();
}

fn main() {
    let mut dec = Decoder::default();
    let mut buf = [0u8; 4096];
    let mut stdin = std::io::stdin();
    loop {
        let n = match stdin.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        for msg in dec.push(&buf[..n]) {
            let method = msg["method"].as_str().unwrap_or("");
            match method {
                "initialize" => {
                    // Exercise server->client requests before replying.
                    send(&json!({"jsonrpc":"2.0","id":900,"method":"workspace/configuration","params":{"items":[{}]}}));
                    send(&json!({"jsonrpc":"2.0","id":901,"method":"client/registerCapability","params":{"registrations":[]}}));
                    send(&json!({"jsonrpc":"2.0","id":msg["id"],"result":{"capabilities":{"textDocumentSync":1}}}));
                }
                "shutdown" => send(&json!({"jsonrpc":"2.0","id":msg["id"],"result":null})),
                "exit" => return,
                "textDocument/didOpen" | "textDocument/didChange" => {
                    let td = &msg["params"]["textDocument"];
                    let text = msg["params"]["contentChanges"][0]["text"].as_str().or(td["text"].as_str()).unwrap_or("");
                    let diags = if text.contains("BAD") {
                        vec![json!({"range":{"start":{"line":1,"character":4},"end":{"line":1,"character":7}},"severity":1,"message":"bad token"}),
                             json!({"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"severity":2,"message":"just a warning"})]
                    } else {
                        vec![]
                    };
                    send(&json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":td["uri"],"diagnostics":diags}}));
                }
                _ => {}
            }
        }
    }
}
