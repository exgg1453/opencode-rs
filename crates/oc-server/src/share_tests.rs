use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::{router, AppState};

struct NoRepo;
impl crate::backend::SessionRepo for NoRepo {
    fn save(&self, _: &oc_core::Session) -> anyhow::Result<()> {
        Ok(())
    }
    fn load(&self, _: &str) -> anyhow::Result<Option<oc_core::Session>> {
        Ok(None)
    }
    fn list(&self) -> anyhow::Result<Vec<oc_core::Session>> {
        Ok(vec![])
    }
    fn delete(&self, _: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
}
struct NoRunner;
#[async_trait::async_trait]
impl crate::backend::TurnRunner for NoRunner {
    async fn run(
        &self,
        _: &mut oc_core::Session,
        _: String,
        _: tokio::sync::mpsc::UnboundedSender<oc_core::AgentEvent>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

fn app(dir: &std::path::Path) -> axum::Router {
    router(AppState::new(Arc::new(NoRepo), Arc::new(NoRunner)).with_share_dir(dir.to_path_buf()).with_security(crate::Security::disabled()))
}

async fn req(app: &axum::Router, method: &str, uri: &str, secret: Option<&str>, body: Option<Value>) -> (StatusCode, String) {
    let mut r = Request::builder().method(method).uri(uri).header("host", "share.test:9");
    if let Some(s) = secret {
        r = r.header("x-share-secret", s);
    }
    let b = match body {
        Some(v) => {
            r = r.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(r.body(b).unwrap()).await.unwrap();
    let st = resp.status();
    (st, String::from_utf8_lossy(&resp.into_body().collect().await.unwrap().to_bytes()).into_owned())
}

fn session(text: &str) -> Value {
    json!({"id":"s1","title":"My <b>title</b>","cwd":"/tmp","messages":[
        {"role":"user","content":[{"type":"text","text":text}]},
        {"role":"assistant","content":[{"type":"tool_use","id":"t","name":"bash","input":{"command":"ls"}},
                                       {"type":"tool_result","tool_use_id":"t","content":"out","is_error":false}]}]})
}

#[tokio::test]
async fn share_lifecycle() {
    let d = tempfile::tempdir().unwrap();
    let app = app(d.path());
    let (st, body) = req(&app, "POST", "/share", None, Some(session("hello"))).await;
    assert_eq!(st, StatusCode::CREATED);
    let v: Value = serde_json::from_str(&body).unwrap();
    let id = v["id"].as_str().unwrap().to_string();
    let secret = v["secret"].as_str().unwrap().to_string();
    assert_eq!(v["url"], format!("http://share.test:9/share/{id}"));

    let (st, body) = req(&app, "GET", &format!("/share/{id}/data"), None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["messages"][0]["content"][0]["text"], "hello");

    // update requires the secret
    let (st, _) = req(&app, "PUT", &format!("/share/{id}"), None, Some(session("v2"))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = req(&app, "PUT", &format!("/share/{id}"), Some("wrong"), Some(session("v2"))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = req(&app, "PUT", &format!("/share/{id}"), Some(&secret), Some(session("v2"))).await;
    assert_eq!(st, StatusCode::OK);
    let (_, body) = req(&app, "GET", &format!("/share/{id}/data"), None, None).await;
    assert!(body.contains("v2"));

    let (st, _) = req(&app, "DELETE", &format!("/share/{id}"), Some("nope"), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = req(&app, "DELETE", &format!("/share/{id}"), Some(&secret), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = req(&app, "GET", &format!("/share/{id}/data"), None, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn html_page_escapes() {
    let d = tempfile::tempdir().unwrap();
    let app = app(d.path());
    let (_, body) = req(&app, "POST", "/share", None, Some(session("<script>alert(1)</script>"))).await;
    let id = serde_json::from_str::<Value>(&body).unwrap()["id"].as_str().unwrap().to_string();
    let (st, html) = req(&app, "GET", &format!("/share/{id}"), None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(!html.contains("<script>alert"));
    assert!(html.contains("My &lt;b&gt;title&lt;/b&gt;"));
    assert!(html.contains("<details"));
}

#[tokio::test]
async fn rejects_bad_input() {
    let d = tempfile::tempdir().unwrap();
    let app = app(d.path());
    let (st, _) = req(&app, "POST", "/share", None, Some(json!({"nope": 1}))).await;

    assert!(st.is_client_error());
    let (st, _) = req(&app, "GET", "/share/..%2Fetc", None, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = req(&app, "GET", "/share/doesnotexist/data", None, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}
