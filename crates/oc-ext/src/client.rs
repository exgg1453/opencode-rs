//! MCP client: stdio and Streamable HTTP transports.
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use anyhow::{anyhow, Context};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};

use crate::rpc;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

type Pending = Arc<StdMutex<HashMap<u64, oneshot::Sender<Value>>>>;

#[derive(Debug, Clone)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

enum Transport {
    Stdio {
        stdin: Arc<Mutex<ChildStdin>>,
        pending: Pending,
        _child: Child, // kill_on_drop
        reader: tokio::task::JoinHandle<()>,
    },
    Http {
        http: reqwest::Client,
        url: String,
        headers: HashMap<String, String>,
        session: StdMutex<Option<String>>,
    },
}

pub struct McpClient {
    pub name: String,
    transport: Transport,
    next_id: AtomicU64,
    timeout: Duration,
}

impl Drop for McpClient {
    fn drop(&mut self) {
        if let Transport::Stdio { reader, .. } = &self.transport {
            reader.abort();
        }
    }
}

impl McpClient {
    pub async fn connect_stdio(
        name: &str,
        command: &[String],
        env: &HashMap<String, String>,
        timeout: Duration,
    ) -> anyhow::Result<McpClient> {
        let (prog, args) = command.split_first().ok_or_else(|| anyhow!("empty command"))?;
        let mut child = Command::new(prog)
            .args(args)
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawning {prog}"))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let pending: Pending = Arc::default();
        let p2 = pending.clone();
        let stdin = Arc::new(Mutex::new(stdin));
        let reply_stdin = stdin.clone();
        let reader = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(msg) = serde_json::from_str::<Value>(line.trim()) else { continue };
                if let Some(id) = rpc::response_id(&msg) {
                    if let Some(tx) = p2.lock().unwrap().remove(&id) {
                        let _ = tx.send(msg);
                    }
                } else if msg.get("method").is_some() && msg.get("id").is_some() {
                    // server->client request: answer ping, refuse others
                    let id = msg["id"].clone();
                    let reply = if msg["method"] == "ping" {
                        json!({"jsonrpc":"2.0","id":id,"result":{}})
                    } else {
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"method not found"}})
                    };
                    let mut w = reply_stdin.lock().await;
                    let _ = w.write_all(rpc::frame(&reply).as_bytes()).await;
                    let _ = w.flush().await;
                }
            }
            // EOF: fail all pending requests
            p2.lock().unwrap().clear();
        });
        let client = McpClient {
            name: name.to_string(),
            transport: Transport::Stdio { stdin, pending, _child: child, reader },
            next_id: AtomicU64::new(1),
            timeout,
        };
        Ok(client)
    }

    pub fn connect_http(
        name: &str,
        url: &str,
        headers: &HashMap<String, String>,
        timeout: Duration,
    ) -> anyhow::Result<McpClient> {
        Ok(McpClient {
            name: name.to_string(),
            transport: Transport::Http {
                http: reqwest::Client::builder().build()?,
                url: url.to_string(),
                headers: headers.clone(),
                session: StdMutex::new(None),
            },
            next_id: AtomicU64::new(1),
            timeout,
        })
    }

    async fn send_stdio(&self, msg: &Value) -> anyhow::Result<()> {
        let Transport::Stdio { stdin, .. } = &self.transport else { unreachable!() };
        let mut w = stdin.lock().await;
        w.write_all(rpc::frame(msg).as_bytes()).await?;
        w.flush().await?;
        Ok(())
    }

    pub async fn request(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        self.request_with_timeout(method, params, self.timeout).await
    }

    pub async fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let msg = rpc::request(id, method, params);
        let fut = async {
            match &self.transport {
                Transport::Stdio { pending, .. } => {
                    let (tx, rx) = oneshot::channel();
                    pending.lock().unwrap().insert(id, tx);
                    if let Err(e) = self.send_stdio(&msg).await {
                        pending.lock().unwrap().remove(&id);
                        return Err(e);
                    }
                    let resp = rx.await.map_err(|_| anyhow!("MCP server '{}' closed", self.name))?;
                    rpc::into_result(resp)
                }
                Transport::Http { .. } => self.http_request(id, &msg).await,
            }
        };
        match tokio::time::timeout(timeout, fut).await {
            Ok(r) => r.with_context(|| format!("{method} on MCP server '{}'", self.name)),
            Err(_) => {
                if let Transport::Stdio { pending, .. } = &self.transport {
                    pending.lock().unwrap().remove(&id);
                }
                Err(anyhow!("{method} on MCP server '{}' timed out after {:?}", self.name, timeout))
            }
        }
    }

    pub async fn notify(&self, method: &str, params: Option<Value>) -> anyhow::Result<()> {
        let msg = rpc::notification(method, params);
        match &self.transport {
            Transport::Stdio { .. } => self.send_stdio(&msg).await,
            Transport::Http { .. } => {
                let resp = self.http_post(&msg).await?;
                if !resp.status().is_success() {
                    anyhow::bail!("notification {method} failed: HTTP {}", resp.status());
                }
                Ok(())
            }
        }
    }

    async fn http_post(&self, msg: &Value) -> anyhow::Result<reqwest::Response> {
        let Transport::Http { http, url, headers, session } = &self.transport else { unreachable!() };
        let mut rb = http
            .post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", rpc::PROTOCOL_VERSION);
        for (k, v) in headers {
            rb = rb.header(k, v);
        }
        if let Some(s) = session.lock().unwrap().clone() {
            rb = rb.header("Mcp-Session-Id", s);
        }
        let resp = rb.body(serde_json::to_vec(msg)?).send().await?;
        if let Some(s) = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
            *session.lock().unwrap() = Some(s.to_string());
        }
        Ok(resp)
    }

    async fn http_request(&self, id: u64, msg: &Value) -> anyhow::Result<Value> {
        let resp = self.http_post(msg).await?;
        let status = resp.status();
        let ctype = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let body = resp.text().await?;
        if !status.is_success() {
            anyhow::bail!("HTTP {status}: {}", body.chars().take(300).collect::<String>());
        }
        let msgs = if ctype.contains("text/event-stream") {
            rpc::parse_sse(&body)
        } else {
            vec![serde_json::from_str::<Value>(&body).context("invalid JSON body")?]
        };
        let found = msgs
            .into_iter()
            .flat_map(|m| match m {
                Value::Array(a) => a,
                o => vec![o],
            })
            .find(|m| rpc::response_id(m) == Some(id))
            .ok_or_else(|| anyhow!("no response for request {id}"))?;
        rpc::into_result(found)
    }

    /// initialize handshake + notifications/initialized.
    pub async fn initialize(&self) -> anyhow::Result<Value> {
        let res = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": rpc::PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "opencode-rs", "version": env!("CARGO_PKG_VERSION")}
                }),
            )
            .await?;
        self.notify("notifications/initialized", None).await?;
        Ok(res)
    }

    pub async fn list_tools(&self) -> anyhow::Result<Vec<McpToolInfo>> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..1000 {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let res = self.request("tools/list", params).await?;
            for t in res["tools"].as_array().cloned().unwrap_or_default() {
                let Some(name) = t["name"].as_str() else { continue };
                out.push(McpToolInfo {
                    name: name.to_string(),
                    description: t["description"].as_str().unwrap_or("").to_string(),
                    input_schema: t.get("inputSchema").cloned().unwrap_or_else(|| json!({"type":"object"})),
                });
            }
            match res["nextCursor"].as_str() {
                Some(c) if !c.is_empty() => cursor = Some(c.to_string()),
                _ => break,
            }
        }
        Ok(out)
    }

    /// Returns (flattened text, is_error).
    pub async fn call_tool(&self, name: &str, args: Value) -> anyhow::Result<(String, bool)> {
        let args = if args.is_null() { json!({}) } else { args };
        let res = self
            .request_with_timeout(
                "tools/call",
                json!({"name": name, "arguments": args}),
                self.timeout.max(Duration::from_secs(120)),
            )
            .await?;
        let is_error = res["isError"].as_bool().unwrap_or(false);
        let mut parts = Vec::new();
        for c in res["content"].as_array().cloned().unwrap_or_default() {
            match c["type"].as_str() {
                Some("text") => parts.push(c["text"].as_str().unwrap_or("").to_string()),
                Some("resource") => {
                    if let Some(t) = c["resource"]["text"].as_str() {
                        parts.push(t.to_string());
                    } else if let Some(u) = c["resource"]["uri"].as_str() {
                        parts.push(format!("[resource: {u}]"));
                    }
                }
                Some(other) => parts.push(format!("[{other} content omitted]")),
                None => {}
            }
        }
        if parts.is_empty() {
            if let Some(sc) = res.get("structuredContent") {
                parts.push(sc.to_string());
            }
        }
        Ok((parts.join("\n"), is_error))
    }
}
