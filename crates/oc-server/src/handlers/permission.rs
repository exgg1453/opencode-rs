//! Interactive permission requests over HTTP.
//!
//! `PermissionBroker` is the server's `Asker`: it publishes a `permission.asked` SSE event
//! and waits for `POST /session/{id}/permission/{permission_id}` with
//! `{ "response": "once" | "always" | "reject" }`. No answer in time denies.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::{Path, State};
use axum::Json;
use oc_core::permission::Asker;
use oc_core::{PermissionDecision, PermissionRequest};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{broadcast, oneshot};

use crate::error::ApiError;
use crate::state::AppState;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// Payload of the `permission.asked` SSE event.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PermissionAsked {
    pub session_id: String,
    pub call_id: String,
    pub tool: String,
    pub input: Value,
    pub permission_id: String,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Reply {
    Once,
    Always,
    Reject,
}

impl Reply {
    fn decision(self) -> PermissionDecision {
        match self {
            Reply::Once => PermissionDecision::Allow,
            Reply::Always => PermissionDecision::AllowAlways,
            Reply::Reject => PermissionDecision::Deny { reason: Some("rejected by the user".into()) },
        }
    }
}

struct Pending {
    session_id: String,
    tx: oneshot::Sender<Reply>,
}

struct Inner {
    events: broadcast::Sender<PermissionAsked>,
    pending: Mutex<HashMap<String, Pending>>,
    counter: AtomicU64,
    timeout: Duration,
}

#[derive(Clone)]
pub struct PermissionBroker(Arc<Inner>);

impl Default for PermissionBroker {
    fn default() -> Self {
        Self::new(DEFAULT_TIMEOUT)
    }
}

impl PermissionBroker {
    pub fn new(timeout: Duration) -> Self {
        let (events, _) = broadcast::channel(256);
        PermissionBroker(Arc::new(Inner { events, pending: Mutex::new(HashMap::new()), counter: AtomicU64::new(1), timeout }))
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PermissionAsked> {
        self.0.events.subscribe()
    }

    /// Deliver a user's answer. False when the id is unknown (already answered or timed out)
    /// or belongs to another session.
    pub fn respond(&self, session_id: &str, permission_id: &str, reply: Reply) -> bool {
        let mut map = self.0.pending.lock().unwrap();
        match map.get(permission_id) {
            Some(p) if p.session_id == session_id => {}
            _ => return false,
        }
        let p = map.remove(permission_id).unwrap();
        p.tx.send(reply).is_ok()
    }

    pub fn pending_count(&self) -> usize {
        self.0.pending.lock().unwrap().len()
    }
}

#[async_trait]
impl Asker for PermissionBroker {
    async fn ask(&self, req: &PermissionRequest) -> PermissionDecision {
        let n = self.0.counter.fetch_add(1, Ordering::Relaxed);
        let permission_id = format!("per_{n}");
        let (tx, rx) = oneshot::channel();
        self.0.pending.lock().unwrap().insert(permission_id.clone(), Pending { session_id: req.session_id.clone(), tx });
        let _ = self.0.events.send(PermissionAsked {
            session_id: req.session_id.clone(),
            call_id: req.call_id.clone(),
            tool: req.tool.clone(),
            input: req.input.clone(),
            permission_id: permission_id.clone(),
        });
        let out = tokio::time::timeout(self.0.timeout, rx).await;
        // Removes the entry on timeout / cancellation (no-op after a response).
        self.0.pending.lock().unwrap().remove(&permission_id);
        match out {
            Ok(Ok(reply)) => reply.decision(),
            Ok(Err(_)) => PermissionDecision::Deny { reason: Some("permission request cancelled".into()) },
            Err(_) => PermissionDecision::Deny {
                reason: Some(format!("no response to the permission request within {}s", self.0.timeout.as_secs())),
            },
        }
    }
}

#[derive(Deserialize)]
pub struct ReplyBody {
    pub response: Reply,
}

/// `POST /session/{id}/permission/{permission_id}`
pub async fn respond(
    State(state): State<AppState>,
    Path((id, permission_id)): Path<(String, String)>,
    Json(body): Json<ReplyBody>,
) -> Result<Json<Value>, ApiError> {
    if state.permissions.respond(&id, &permission_id, body.response) {
        Ok(Json(json!(true)))
    } else {
        Err(ApiError::NotFound(format!("no pending permission {permission_id} for session {id}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(session: &str) -> PermissionRequest {
        PermissionRequest { session_id: session.into(), call_id: "call1".into(), tool: "bash".into(), input: json!({"command": "ls"}) }
    }

    async fn asked(rx: &mut broadcast::Receiver<PermissionAsked>) -> PermissionAsked {
        rx.recv().await.unwrap()
    }

    #[tokio::test]
    async fn publishes_event_and_waits_for_reply() {
        let b = PermissionBroker::default();
        let mut rx = b.subscribe();
        let b2 = b.clone();
        let task = tokio::spawn(async move { b2.ask(&req("s1")).await });
        let ev = asked(&mut rx).await;
        assert_eq!((ev.session_id.as_str(), ev.call_id.as_str(), ev.tool.as_str()), ("s1", "call1", "bash"));
        assert_eq!(ev.input, json!({"command": "ls"}));
        assert_eq!(b.pending_count(), 1);
        assert!(b.respond("s1", &ev.permission_id, Reply::Always));
        assert_eq!(task.await.unwrap(), PermissionDecision::AllowAlways);
        assert_eq!(b.pending_count(), 0);
        // Answering twice fails.
        assert!(!b.respond("s1", &ev.permission_id, Reply::Once));
    }

    #[tokio::test]
    async fn once_and_reject_map_to_decisions() {
        let b = PermissionBroker::default();
        let mut rx = b.subscribe();
        for (reply, want_allow) in [(Reply::Once, true), (Reply::Reject, false)] {
            let b2 = b.clone();
            let t = tokio::spawn(async move { b2.ask(&req("s")).await });
            let ev = asked(&mut rx).await;
            assert!(b.respond("s", &ev.permission_id, reply));
            let d = t.await.unwrap();
            assert_eq!(matches!(d, PermissionDecision::Allow), want_allow);
            assert_eq!(matches!(d, PermissionDecision::Deny { .. }), !want_allow);
        }
    }

    #[tokio::test]
    async fn wrong_session_cannot_answer() {
        let b = PermissionBroker::default();
        let mut rx = b.subscribe();
        let b2 = b.clone();
        let t = tokio::spawn(async move { b2.ask(&req("s1")).await });
        let ev = asked(&mut rx).await;
        assert!(!b.respond("other", &ev.permission_id, Reply::Once));
        assert!(!b.respond("s1", "per_nope", Reply::Once));
        assert!(b.respond("s1", &ev.permission_id, Reply::Once));
        assert_eq!(t.await.unwrap(), PermissionDecision::Allow);
    }

    #[tokio::test]
    async fn timeout_denies_and_cleans_up() {
        let b = PermissionBroker::new(Duration::from_millis(50));
        let d = b.ask(&req("s")).await;
        assert!(matches!(d, PermissionDecision::Deny { reason: Some(r) } if r.contains("within")));
        assert_eq!(b.pending_count(), 0);
    }

    #[tokio::test]
    async fn dropped_ask_is_cleaned_up() {
        let b = PermissionBroker::default();
        let mut rx = b.subscribe();
        let b2 = b.clone();
        let t = tokio::spawn(async move { b2.ask(&req("s")).await });
        let ev = asked(&mut rx).await;
        t.abort();
        let _ = t.await;
        // The entry may linger until the pending map is touched; responding must fail cleanly.
        assert!(!b.respond("s", &ev.permission_id, Reply::Once));
    }

    #[test]
    fn reply_parses() {
        let r: ReplyBody = serde_json::from_value(json!({"response": "reject"})).unwrap();
        assert_eq!(r.response, Reply::Reject);
        assert!(serde_json::from_value::<ReplyBody>(json!({"response": "maybe"})).is_err());
    }

    #[tokio::test]
    async fn http_route_answers_pending_request() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let broker = PermissionBroker::default();
        let mut rx = broker.subscribe();
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
        struct NoRun;
        #[async_trait]
        impl crate::backend::TurnRunner for NoRun {
            async fn run(
                &self,
                _: &mut oc_core::Session,
                _: String,
                _: tokio::sync::mpsc::UnboundedSender<oc_core::AgentEvent>,
            ) -> anyhow::Result<()> {
                Ok(())
            }
        }
        let state = crate::AppState::new(Arc::new(NoRepo), Arc::new(NoRun)).with_permissions(broker.clone());
        let app = crate::router(state);
        let b2 = broker.clone();
        let t = tokio::spawn(async move { b2.ask(&req("s1")).await });
        let ev = asked(&mut rx).await;
        let post = |sid: &str, pid: &str, body: &str| {
            Request::builder()
                .method("POST")
                .uri(format!("/session/{sid}/permission/{pid}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        let r = app.clone().oneshot(post("s1", "per_zzz", r#"{"response":"once"}"#)).await.unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        let r = app.clone().oneshot(post("s1", &ev.permission_id, r#"{"response":"reject"}"#)).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert!(matches!(t.await.unwrap(), PermissionDecision::Deny { .. }));
    }
}
