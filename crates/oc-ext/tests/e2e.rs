use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::{extract::State, http::HeaderMap, response::IntoResponse, routing::post, Json, Router};
use oc_core::{Config, ToolContext, ToolRegistry};
use serde_json::{json, Value};

fn ctx() -> ToolContext {
    ToolContext { cwd: std::env::temp_dir() }
}

fn config(v: Value) -> Config {
    Config { mcp: v.as_object().unwrap().clone(), ..Default::default() }
}

#[tokio::test]
async fn stdio_end_to_end() {
    let bin = env!("CARGO_BIN_EXE_fake_mcp");
    let cfg = config(json!({
        "fake.srv": {"type":"local","command":[bin]},
        "off": {"type":"local","command":[bin],"enabled":false},
        "broken": {"type":"local","command":["/nonexistent/binary"]},
        "garbage": {"type":"nope"}
    }));
    let mut reg = ToolRegistry::default();
    oc_ext::load_extensions(&cfg, &mut reg).await.unwrap();
    let mut names: Vec<_> = reg.tools.iter().map(|t| t.name().to_string()).collect();
    names.sort();
    assert_eq!(names, vec!["fake_srv_echo", "fake_srv_fail"]); // pagination + sanitize + skips
    let echo = reg.tools.iter().find(|t| t.name() == "fake_srv_echo").unwrap();
    assert_eq!(echo.execute(json!({"text":"hi"}), &ctx()).await.unwrap(), "echo: hi");
    let fail = reg.tools.iter().find(|t| t.name() == "fake_srv_fail").unwrap();
    let err = fail.execute(json!({}), &ctx()).await.unwrap_err();
    assert!(err.to_string().contains("boom"));
}

type Entry = (Option<String>, Option<String>, String);

#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<Entry>>>);

async fn handler(State(seen): State<Seen>, headers: HeaderMap, Json(msg): Json<Value>) -> axum::response::Response {
    let h = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).map(String::from);
    let method = msg["method"].as_str().unwrap_or("").to_string();
    seen.0.lock().unwrap().push((h("x-test"), h("mcp-session-id"), method.clone()));
    let Some(id) = msg.get("id").cloned() else {
        return axum::http::StatusCode::ACCEPTED.into_response();
    };
    match method.as_str() {
        "initialize" => (
            [("Mcp-Session-Id", "sess-1")],
            Json(json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2025-03-26","capabilities":{}}})),
        )
            .into_response(),
        "tools/list" => Json(json!({"jsonrpc":"2.0","id":id,"result":{"tools":[
            {"name":"ping","description":"d","inputSchema":{"type":"object"}}]}}))
        .into_response(),
        _ => {
            // respond as SSE
            let body = format!(
                "event: message\ndata: {}\n\n",
                json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":"pong"}]}})
            );
            ([("Content-Type", "text/event-stream")], body).into_response()
        }
    }
}

#[tokio::test]
async fn http_end_to_end() {
    let seen = Seen::default();
    let app = Router::new().route("/mcp", post(handler)).with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let mut headers = HashMap::new();
    headers.insert("x-test".to_string(), "yes".to_string());
    let cfg = config(json!({"web": {"type":"remote","url":format!("http://{addr}/mcp"),"headers":headers}}));
    let mut reg = ToolRegistry::default();
    oc_ext::load_extensions(&cfg, &mut reg).await.unwrap();
    assert_eq!(reg.tools.len(), 1);
    assert_eq!(reg.tools[0].name(), "web_ping");
    assert_eq!(reg.tools[0].execute(json!({}), &ctx()).await.unwrap(), "pong");

    let log = seen.0.lock().unwrap().clone();
    assert!(log.iter().all(|(x, _, _)| x.as_deref() == Some("yes")));
    // session id is sent on every request after initialize
    assert!(log.iter().skip(1).all(|(_, s, _)| s.as_deref() == Some("sess-1")));
    assert!(log.iter().any(|(_, _, m)| m == "notifications/initialized"));
}
