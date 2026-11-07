//! `GET /event` Server-Sent Events stream.

use std::convert::Infallible;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::{self, Stream, StreamExt};
use tokio_stream::wrappers::BroadcastStream;

use crate::state::{AppState, SessionEvent};

/// SSE event name for an agent event (its serde `type` tag).
pub fn event_name(e: &SessionEvent) -> String {
    serde_json::to_value(&e.event)
        .ok()
        .and_then(|v| v.get("type").and_then(|t| t.as_str().map(String::from)))
        .unwrap_or_else(|| "message".into())
}

pub fn to_sse(e: &SessionEvent) -> Event {
    Event::default()
        .event(event_name(e))
        .json_data(e)
        .unwrap_or_else(|_| Event::default().data("{}"))
}

pub async fn stream(State(state): State<AppState>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.events.subscribe();
    let hello = stream::once(async {
        Ok(Event::default().event("server.connected").data("{}"))
    });
    // Lagged receivers drop missed events rather than ending the stream.
    let live = BroadcastStream::new(rx).filter_map(|r| async move { r.ok().map(|e| Ok(to_sse(&e))) });
    let asked = BroadcastStream::new(state.permissions.subscribe()).filter_map(|r| async move {
        r.ok().map(|p| Ok(Event::default().event("permission.asked").json_data(&p).unwrap_or_else(|_| Event::default().data("{}"))))
    });
    Sse::new(hello.chain(stream::select(live, asked))).keep_alive(KeepAlive::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oc_core::AgentEvent;

    fn ev(event: AgentEvent) -> SessionEvent {
        SessionEvent { session_id: "s".into(), event }
    }

    #[test]
    fn thinking_and_retry_events_have_sse_names() {
        assert_eq!(event_name(&ev(AgentEvent::ThinkingDelta { text: "t".into() })), "thinking_delta");
        assert_eq!(event_name(&ev(AgentEvent::Retry { attempt: 1, max: 3, reason: "r".into() })), "retry");
        assert_eq!(event_name(&ev(AgentEvent::TextDelta { text: "t".into() })), "text_delta");
    }
}
