//! Minimal Server-Sent-Events parser and a generic event-stream driver.

use std::collections::VecDeque;

use futures::stream::{BoxStream, Stream, StreamExt};

use crate::StreamEvent;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

/// Incremental SSE parser; feed arbitrary byte chunks.
#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=pos).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = String::from_utf8_lossy(&line).into_owned();
            if line.is_empty() {
                if let Some(ev) = self.take() {
                    out.push(ev);
                }
            } else if line.starts_with(':') {
                // comment
            } else {
                let (field, value) = match line.split_once(':') {
                    Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
                    None => (line.as_str(), ""),
                };
                match field {
                    "event" => self.event = Some(value.to_string()),
                    "data" => self.data.push(value.to_string()),
                    _ => {}
                }
            }
        }
        out
    }

    /// Flush a trailing event not terminated by a blank line.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut out = self.feed(b"\n");
        out.extend(self.take());
        out
    }

    fn take(&mut self) -> Option<SseEvent> {
        if self.data.is_empty() && self.event.is_none() {
            return None;
        }
        let ev = SseEvent { event: self.event.take(), data: std::mem::take(&mut self.data).join("\n") };
        Some(ev)
    }
}

/// Turns SSE events into provider-neutral stream events.
pub trait EventProcessor: Send + 'static {
    fn process(&mut self, ev: &SseEvent) -> anyhow::Result<Vec<StreamEvent>>;
    /// Called when the byte stream ends. A stream that closes before the provider's
    /// end-of-response marker was cut off and must return [`truncated`].
    fn finish(&mut self) -> anyhow::Result<Vec<StreamEvent>>;
}

/// The connection closed cleanly but the response never finished. Retryable, like a reset.
pub fn truncated(provider: &str) -> anyhow::Error {
    anyhow::Error::new(crate::ProviderError::new(
        None,
        true,
        format!("{provider} stream ended before the response finished"),
    ))
}

struct State<S, P> {
    bytes: S,
    parser: SseParser,
    proc: P,
    pending: VecDeque<anyhow::Result<StreamEvent>>,
    done: bool,
}

/// Drive a byte stream through the SSE parser and a processor.
pub fn drive<S, P>(bytes: S, proc: P) -> BoxStream<'static, anyhow::Result<StreamEvent>>
where
    S: Stream<Item = anyhow::Result<Vec<u8>>> + Send + Unpin + 'static,
    P: EventProcessor,
{
    let st = State { bytes, parser: SseParser::new(), proc, pending: VecDeque::new(), done: false };
    futures::stream::unfold(st, |mut st| async move {
        loop {
            if let Some(item) = st.pending.pop_front() {
                if item.is_err() {
                    st.done = true;
                    st.pending.clear();
                }
                return Some((item, st));
            }
            if st.done {
                return None;
            }
            match st.bytes.next().await {
                Some(Ok(chunk)) => {
                    for ev in st.parser.feed(&chunk) {
                        match st.proc.process(&ev) {
                            Ok(v) => st.pending.extend(v.into_iter().map(Ok)),
                            Err(e) => {
                                st.pending.push_back(Err(e));
                                break;
                            }
                        }
                    }
                }
                Some(Err(e)) => st.pending.push_back(Err(e)),
                None => {
                    st.done = true;
                    for ev in st.parser.finish() {
                        match st.proc.process(&ev) {
                            Ok(v) => st.pending.extend(v.into_iter().map(Ok)),
                            Err(e) => {
                                st.pending.push_back(Err(e));
                                break;
                            }
                        }
                    }
                    match st.proc.finish() {
                        Ok(v) => st.pending.extend(v.into_iter().map(Ok)),
                        Err(e) => st.pending.push_back(Err(e)),
                    }
                }
            }
        }
    })
    .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_split_chunks_and_crlf() {
        let mut p = SseParser::new();
        assert!(p.feed(b"event: a\r\ndata: he").is_empty());
        let evs = p.feed(b"llo\r\ndata: world\r\n\r\n: comment\ndata: x\n\n");
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0], SseEvent { event: Some("a".into()), data: "hello\nworld".into() });
        assert_eq!(evs[1].data, "x");
        assert!(evs[1].event.is_none());
    }

    #[test]
    fn finish_flushes_trailing() {
        let mut p = SseParser::new();
        assert!(p.feed(b"data: tail").is_empty());
        let evs = p.finish();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "tail");
    }
}
