use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use oc_ai::{ChatMessage, ContentBlock, Role};
use oc_core::{AgentEvent, Session};
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;
use tower::ServiceExt;

use crate::backend::{SessionRepo, TurnRunner};
use crate::{router, AppState};

#[derive(Default)]
struct MemRepo(Mutex<HashMap<String, Session>>);

impl SessionRepo for MemRepo {
    fn save(&self, s: &Session) -> anyhow::Result<()> {
        self.0.lock().unwrap().insert(s.id.clone(), s.clone());
        Ok(())
    }
    fn load(&self, id: &str) -> anyhow::Result<Option<Session>> {
        Ok(self.0.lock().unwrap().get(id).cloned())
    }
    fn list(&self) -> anyhow::Result<Vec<Session>> {
        Ok(self.0.lock().unwrap().values().cloned().collect())
    }
    fn delete(&self, id: &str) -> anyhow::Result<bool> {
        Ok(self.0.lock().unwrap().remove(id).is_some())
    }
}

/// Echoes the input; input "slow" blocks forever; "fail" errors.
struct MockRunner;

#[async_trait]
impl TurnRunner for MockRunner {
    async fn run(&self, s: &mut Session, input: String, ev: UnboundedSender<AgentEvent>) -> anyhow::Result<()> {
        s.messages.push(ChatMessage { role: Role::User, content: vec![ContentBlock::Text { text: input.clone() }] });
        match input.as_str() {
            "slow" => {
                let _ = ev.send(AgentEvent::TextDelta { text: "working".into() });
                std::future::pending::<()>().await;
            }
            "fail" => anyhow::bail!("boom"),
            _ => {}
        }
        let reply = format!("echo: {input}");
        let _ = ev.send(AgentEvent::TextDelta { text: reply.clone() });
        s.messages.push(ChatMessage { role: Role::Assistant, content: vec![ContentBlock::Text { text: reply }] });
        let _ = ev.send(AgentEvent::Done);
        Ok(())
    }
}

fn state() -> AppState {
    AppState::new(Arc::new(MemRepo::default()), Arc::new(MockRunner)).with_security(crate::Security::disabled())
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(b) => {
            req = req.header("content-type", "application/json");
            Body::from(b.to_string())
        }
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn create(app: &Router) -> String {
    let (st, v) = call(app, "POST", "/session", Some(json!({"cwd": "/tmp"}))).await;
    assert_eq!(st, StatusCode::CREATED);
    v["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn health_and_unknown_route() {
    let app = router(state());
    let (st, v) = call(&app, "GET", "/health", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["healthy"], true);
    let (st, v) = call(&app, "GET", "/nope", None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(v["error"]["code"], "not_found");
}

#[tokio::test]
async fn session_crud() {
    let app = router(state());
    let id = create(&app).await;
    let (_, v) = call(&app, "GET", "/session", None).await;
    assert_eq!(v.as_array().unwrap().len(), 1);
    let (st, v) = call(&app, "GET", &format!("/session/{id}"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["cwd"], "/tmp");
    let (st, _) = call(&app, "DELETE", &format!("/session/{id}"), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, v) = call(&app, "GET", &format!("/session/{id}"), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(v["error"]["code"], "not_found");
    let (st, _) = call(&app, "DELETE", &format!("/session/{id}"), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn prompt_returns_assistant_message_and_persists() {
    let app = router(state());
    let id = create(&app).await;
    let (st, v) = call(&app, "POST", &format!("/session/{id}/message"), Some(json!({"text": "hi"}))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["role"], "assistant");
    assert_eq!(v["content"][0]["text"], "echo: hi");
    let (_, msgs) = call(&app, "GET", &format!("/session/{id}/message"), None).await;
    assert_eq!(msgs.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn prompt_errors() {
    let app = router(state());
    let id = create(&app).await;
    let uri = format!("/session/{id}/message");
    let (st, _) = call(&app, "POST", "/session/missing/message", Some(json!({"text": "x"}))).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, v) = call(&app, "POST", &uri, Some(json!({"text": "  "}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["code"], "bad_request");
    let (st, _) = call(&app, "POST", &uri, Some(json!({"nope": 1}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, v) = call(&app, "POST", &uri, Some(json!({"text": "fail"}))).await;
    assert_eq!(st, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(v["error"]["message"].as_str().unwrap().contains("boom"));
    // slot released after failure
    let (st, _) = call(&app, "POST", &uri, Some(json!({"text": "ok"}))).await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn busy_then_abort() {
    let st = state();
    let app = router(st.clone());
    let id = create(&app).await;
    let uri = format!("/session/{id}/message");

    let app2 = app.clone();
    let uri2 = uri.clone();
    let slow = tokio::spawn(async move { call(&app2, "POST", &uri2, Some(json!({"text": "slow"}))).await });
    for _ in 0..100 {
        if st.is_running(&id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(st.is_running(&id));

    let (code, v) = call(&app, "POST", &uri, Some(json!({"text": "again"}))).await;
    assert_eq!(code, StatusCode::CONFLICT);
    assert_eq!(v["error"]["code"], "session_busy");
    let (code, _) = call(&app, "DELETE", &format!("/session/{id}"), None).await;
    assert_eq!(code, StatusCode::CONFLICT);

    let (code, v) = call(&app, "POST", &format!("/session/{id}/abort"), None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(v["aborted"], true);

    let (code, v) = slow.await.unwrap();
    assert_eq!(code, StatusCode::CONFLICT);
    assert_eq!(v["error"]["code"], "aborted");
    assert!(!st.is_running(&id));

    // idle abort
    let (_, v) = call(&app, "POST", &format!("/session/{id}/abort"), None).await;
    assert_eq!(v["aborted"], false);
    let (code, _) = call(&app, "POST", "/session/missing/abort", None).await;
    assert_eq!(code, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn sse_framing_tags_session_id() {
    let st = state();
    let app = router(st.clone());
    let id = create(&app).await;

    let resp = app.clone().oneshot(Request::get("/event").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "text/event-stream");
    let mut body = resp.into_body();

    let (code, _) = call(&app, "POST", &format!("/session/{id}/message"), Some(json!({"text": "hi"}))).await;
    assert_eq!(code, StatusCode::OK);

    let mut text = String::new();
    while !text.contains("event: done") {
        let frame = tokio::time::timeout(Duration::from_secs(2), body.frame())
            .await
            .expect("timed out")
            .expect("stream ended")
            .unwrap();
        if let Ok(d) = frame.into_data() {
            text.push_str(&String::from_utf8_lossy(&d));
        }
    }
    assert!(text.contains("event: server.connected"));
    assert!(text.contains("event: text_delta"));
    assert!(text.contains(&format!("\"session_id\":\"{id}\"")));
    assert!(text.contains("\"text\":\"echo: hi\""));
}

const TOKEN: &str = "test-token-0123456789abcdef";

fn secured() -> Router {
    let root = std::env::current_dir().unwrap();
    let sec = crate::Security::new(TOKEN, true).with_root(root).allow_origin("https://app.test");
    router(AppState::new(Arc::new(MemRepo::default()), Arc::new(MockRunner)).with_security(sec))
}

async fn send(app: &Router, method: &str, uri: &str, headers: &[(&str, &str)], body: Option<Value>) -> axum::http::Response<Body> {
    let mut req = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let body = match body {
        Some(b) => {
            req = req.header("content-type", "application/json");
            Body::from(b.to_string())
        }
        None => Body::empty(),
    };
    app.clone().oneshot(req.body(body).unwrap()).await.unwrap()
}

const HOST: (&str, &str) = ("host", "127.0.0.1:4096");
const AUTH: (&str, &str) = ("authorization", "Bearer test-token-0123456789abcdef");

#[tokio::test]
async fn requires_bearer_token() {
    let app = secured();
    assert_eq!(send(&app, "POST", "/session", &[HOST], None).await.status(), StatusCode::UNAUTHORIZED);
    let wrong = ("authorization", "Bearer nope");
    assert_eq!(send(&app, "POST", "/session", &[HOST, wrong], None).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(send(&app, "GET", "/session", &[HOST], None).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(send(&app, "GET", "/event", &[HOST], None).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(send(&app, "POST", "/session", &[HOST, AUTH], None).await.status(), StatusCode::CREATED);
    let q = format!("/event?token={TOKEN}");
    assert_eq!(send(&app, "GET", &q, &[HOST], None).await.status(), StatusCode::OK);
    // the query token is only honoured on /event
    let q = format!("/session?token={TOKEN}");
    assert_eq!(send(&app, "GET", &q, &[HOST], None).await.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn blocks_dns_rebinding_hosts() {
    let app = secured();
    let evil = ("host", "evil.test:4096");
    assert_eq!(send(&app, "POST", "/session", &[evil, AUTH], None).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(send(&app, "GET", "/health", &[evil], None).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(send(&app, "GET", "/health", &[("host", "localhost:4096")], None).await.status(), StatusCode::OK);
    assert_eq!(send(&app, "GET", "/health", &[("host", "[::1]:4096")], None).await.status(), StatusCode::OK);
    assert_eq!(send(&app, "GET", "/health", &[], None).await.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn blocks_cross_site_pages() {
    let app = secured();
    let evil = ("origin", "https://evil.test");
    // the drive-by PoC from SECURITY-AUDIT.md #1, even with a leaked token
    assert_eq!(send(&app, "POST", "/session", &[HOST, AUTH, evil], None).await.status(), StatusCode::FORBIDDEN);
    let pre = send(
        &app,
        "OPTIONS",
        "/session",
        &[HOST, evil, ("access-control-request-method", "POST"), ("access-control-request-headers", "authorization")],
        None,
    )
    .await;
    assert!(!pre.headers().contains_key("access-control-allow-origin"));
    let ok = ("origin", "https://app.test");
    let r = send(&app, "POST", "/session", &[HOST, AUTH, ok], None).await;
    assert_eq!(r.status(), StatusCode::CREATED);
    assert_eq!(r.headers()["access-control-allow-origin"], "https://app.test");
}

#[tokio::test]
async fn session_cwd_cannot_leave_root() {
    let app = secured();
    for cwd in ["/", "..", "/etc"] {
        let r = send(&app, "POST", "/session", &[HOST, AUTH], Some(json!({ "cwd": cwd }))).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "{cwd}");
    }
    let r = send(&app, "POST", "/session", &[HOST, AUTH], Some(json!({ "cwd": "src" }))).await;
    assert_eq!(r.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn share_creation_needs_token_but_pages_are_public() {
    let app = secured();
    let s = json!({"id":"x","title":"t","cwd":"/","messages":[]});
    assert_eq!(send(&app, "POST", "/share", &[HOST], Some(s)).await.status(), StatusCode::UNAUTHORIZED);
    // unknown share: 404 from the handler, not 401 from the auth layer
    assert_eq!(send(&app, "GET", "/share/doesnotexist", &[HOST], None).await.status(), StatusCode::NOT_FOUND);
    // a share page may be opened from any site
    let r = send(&app, "GET", "/share/doesnotexist/data", &[HOST, ("origin", "https://evil.test")], None).await;
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}

#[test]
fn default_state_is_locked() {
    // AppState::new without with_security must not leave the API open.
    let st = state_default();
    assert!(format!("{:?}", st.security).contains("token: Some"));
}

fn state_default() -> AppState {
    AppState::new(Arc::new(MemRepo::default()), Arc::new(MockRunner))
}

/// Needs phase-1 `Agent::prompt` / `SessionStore` (currently `todo!()`), with an
/// injected mock `oc_ai::Provider` via `AppState::from_agent`.
#[tokio::test]
async fn real_agent_with_mock_provider() {
    use futures::stream::{self, BoxStream};
    use oc_ai::{ChatRequest, Provider, StreamEvent};
    struct P;
    #[async_trait]
    impl Provider for P {
        fn id(&self) -> &str {
            "mock"
        }
        async fn stream(&self, _r: ChatRequest) -> anyhow::Result<BoxStream<'static, anyhow::Result<StreamEvent>>> {
            Ok(Box::pin(stream::iter(vec![
                Ok(StreamEvent::TextDelta("hello".into())),
                Ok(StreamEvent::Finish { stop_reason: "end_turn".into() }),
            ])))
        }
    }
    let agent = oc_core::Agent {
        provider: Arc::new(P),
        model: "mock/m".into(),
        tools: oc_core::ToolRegistry::default(),
        system_prompt: String::new(),
        hooks: Default::default(),
        thinking: None,
    };
    let store = oc_core::SessionStore::open_default().unwrap();
    let app = router(AppState::from_agent(agent, store).with_security(crate::Security::disabled()));
    let id = create(&app).await;
    let (st, v) = call(&app, "POST", &format!("/session/{id}/message"), Some(json!({"text": "hi"}))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["content"][0]["text"], "hello");
}
