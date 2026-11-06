//! One language-server process: JSON-RPC plumbing plus document sync.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex, Notify};

use crate::framing::{encode, Decoder};

#[derive(Debug, Clone, PartialEq)]
pub struct Diag {
    /// 0-based, as on the wire.
    pub line: u32,
    pub col: u32,
    /// 1 error, 2 warning, 3 info, 4 hint.
    pub severity: u8,
    pub message: String,
}

#[derive(Default)]
struct State {
    /// uri -> (publish count, latest diagnostics)
    diags: StdMutex<HashMap<String, (u64, Vec<Diag>)>>,
    notify: Notify,
}

type Pending = Arc<StdMutex<HashMap<i64, oneshot::Sender<Value>>>>;

pub struct Client {
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Pending,
    next_id: AtomicI64,
    state: Arc<State>,
    alive: Arc<AtomicBool>,
    versions: Mutex<HashMap<String, i32>>,
    /// Serializes sync+wait cycles.
    op: Mutex<()>,
    _child: Child,
}

pub fn path_to_uri(p: &Path) -> String {
    let mut s = String::from("file://");
    for b in p.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

fn uri_key(uri: &str) -> String {
    let rest = uri.strip_prefix("file://").unwrap_or(uri);
    let b = rest.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(v) = hex {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_diags(params: &Value) -> Vec<Diag> {
    params["diagnostics"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|d| Diag {
                    line: d["range"]["start"]["line"].as_u64().unwrap_or(0) as u32,
                    col: d["range"]["start"]["character"].as_u64().unwrap_or(0) as u32,
                    severity: d["severity"].as_u64().unwrap_or(1) as u8,
                    message: d["message"].as_str().unwrap_or("").to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn write_msg(stdin: &Mutex<ChildStdin>, msg: &Value) -> std::io::Result<()> {
    let mut g = stdin.lock().await;
    g.write_all(&encode(msg)).await?;
    g.flush().await
}

impl Client {
    pub async fn start(command: &[String], root: &Path) -> anyhow::Result<Arc<Client>> {
        let mut cmd = Command::new(&command[0]);
        cmd.args(&command[1..])
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = cmd.spawn()?;
        let stdin = Arc::new(Mutex::new(child.stdin.take().ok_or_else(|| anyhow::anyhow!("no stdin"))?));
        let mut stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("no stdout"))?;
        let pending: Pending = Default::default();
        let state = Arc::new(State::default());
        let alive = Arc::new(AtomicBool::new(true));

        {
            let (pending, state, alive, stdin) = (pending.clone(), state.clone(), alive.clone(), stdin.clone());
            tokio::spawn(async move {
                let mut dec = Decoder::default();
                let mut buf = [0u8; 8192];
                loop {
                    let n = match stdout.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    for msg in dec.push(&buf[..n]) {
                        handle_incoming(msg, &pending, &state, &stdin).await;
                    }
                }
                alive.store(false, Ordering::SeqCst);
                pending.lock().unwrap().clear(); // drops senders, failing waiters
                state.notify.notify_waiters();
            });
        }

        let client = Arc::new(Client {
            stdin,
            pending,
            next_id: AtomicI64::new(1),
            state,
            alive,
            versions: Default::default(),
            op: Default::default(),
            _child: child,
        });
        let uri = path_to_uri(root);
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        client
            .request(
                "initialize",
                json!({
                    "processId": std::process::id(),
                    "rootUri": uri,
                    "workspaceFolders": [{"uri": uri, "name": name}],
                    "clientInfo": {"name": "opencode-rs"},
                    "capabilities": {
                        "workspace": {"configuration": true, "workspaceFolders": true},
                        "textDocument": {
                            "synchronization": {"didSave": true},
                            "publishDiagnostics": {"relatedInformation": false}
                        },
                        "window": {"workDoneProgress": true}
                    }
                }),
                Duration::from_secs(45),
            )
            .await?;
        client.notify("initialized", json!({})).await?;
        Ok(client)
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    async fn notify(&self, method: &str, params: Value) -> anyhow::Result<()> {
        write_msg(&self.stdin, &json!({"jsonrpc":"2.0","method":method,"params":params})).await?;
        Ok(())
    }

    async fn request(&self, method: &str, params: Value, timeout: Duration) -> anyhow::Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        write_msg(&self.stdin, &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})).await?;
        let resp = tokio::time::timeout(timeout, rx)
            .await
            .map_err(|_| anyhow::anyhow!("{method} timed out"))?
            .map_err(|_| anyhow::anyhow!("server exited"))?;
        if let Some(e) = resp.get("error") {
            anyhow::bail!("{method} failed: {e}");
        }
        Ok(resp["result"].clone())
    }

    /// Sync `text` to the server and wait for the next publishDiagnostics for the file.
    /// Returns the latest diagnostics (possibly stale on timeout).
    pub async fn sync_and_wait(&self, path: &Path, language_id: &str, text: &str, timeout: Duration) -> anyhow::Result<Vec<Diag>> {
        let _op = self.op.lock().await;
        let uri = path_to_uri(path);
        let key = uri_key(&uri);
        let before = self.state.diags.lock().unwrap().get(&key).map(|d| d.0).unwrap_or(0);
        let version = {
            let mut v = self.versions.lock().await;
            let e = v.entry(key.clone()).or_insert(0);
            *e += 1;
            *e
        };
        if version == 1 {
            self.notify(
                "textDocument/didOpen",
                json!({"textDocument": {"uri": uri, "languageId": language_id, "version": version, "text": text}}),
            )
            .await?;
        } else {
            self.notify(
                "textDocument/didChange",
                json!({"textDocument": {"uri": uri, "version": version}, "contentChanges": [{"text": text}]}),
            )
            .await?;
        }
        let _ = self.notify("textDocument/didSave", json!({"textDocument": {"uri": uri}, "text": text})).await;

        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.state.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some((n, d)) = self.state.diags.lock().unwrap().get(&key) {
                if *n > before {
                    return Ok(d.clone());
                }
            }
            if !self.is_alive() {
                anyhow::bail!("server exited");
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Ok(self.state.diags.lock().unwrap().get(&key).map(|d| d.1.clone()).unwrap_or_default());
            }
        }
    }
}

async fn handle_incoming(msg: Value, pending: &Pending, state: &State, stdin: &Mutex<ChildStdin>) {
    let method = msg.get("method").and_then(Value::as_str);
    match (method, msg.get("id")) {
        (None, Some(id)) => {
            if let Some(id) = id.as_i64() {
                if let Some(tx) = pending.lock().unwrap().remove(&id) {
                    let _ = tx.send(msg);
                }
            }
        }
        (Some(m), Some(id)) => {
            let result = match m {
                "workspace/configuration" => {
                    let n = msg["params"]["items"].as_array().map(Vec::len).unwrap_or(0);
                    Value::Array(vec![Value::Null; n])
                }
                _ => Value::Null,
            };
            let _ = write_msg(stdin, &json!({"jsonrpc":"2.0","id":id,"result":result})).await;
        }
        (Some("textDocument/publishDiagnostics"), None) => {
            let p = &msg["params"];
            if let Some(uri) = p["uri"].as_str() {
                let d = parse_diags(p);
                let mut g = state.diags.lock().unwrap();
                let e = g.entry(uri_key(uri)).or_insert((0, vec![]));
                e.0 += 1;
                e.1 = d;
                drop(g);
                state.notify.notify_waiters();
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_roundtrip() {
        let p = Path::new("/tmp/a b/é.rs");
        assert_eq!(uri_key(&path_to_uri(p)), "/tmp/a b/é.rs");
    }
}
