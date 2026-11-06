//! Shared HTTP helpers: retry with backoff and readable API errors.

use std::time::Duration;

use anyhow::{anyhow, Context};

/// Maximum number of retries (after the first attempt) for 429/5xx responses.
pub const MAX_RETRIES: u32 = 3;
/// Default base delay for exponential backoff (500ms, 1s, 2s).
pub const DEFAULT_BACKOFF: Duration = Duration::from_millis(500);

pub fn is_retryable(status: reqwest::StatusCode) -> bool {
    status.as_u16() == 429 || status.is_server_error()
}

/// Extract the human-readable message from a provider error body, if any.
pub fn error_message(body: &str) -> String {
    let body = body.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        let candidates = [
            v.pointer("/error/message"),
            v.pointer("/message"),
            v.pointer("/error"),
            v.pointer("/detail"),
            v.pointer("/error_description"),
        ];
        for c in candidates.into_iter().flatten() {
            if let Some(s) = c.as_str() {
                if !s.is_empty() {
                    return s.to_string();
                }
            }
        }
        // Gemini and others may return an array wrapping the error.
        if let Some(s) = v.pointer("/0/error/message").and_then(|m| m.as_str()) {
            return s.to_string();
        }
    }
    if body.len() > 2000 {
        let mut end = 2000;
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &body[..end])
    } else {
        body.to_string()
    }
}

pub fn format_api_error(provider: &str, status: reqwest::StatusCode, body: &str) -> anyhow::Error {
    let msg = error_message(body);
    let hint = match status.as_u16() {
        401 | 403 => " (check your API key / credentials)",
        404 => " (check the model name and base URL)",
        429 => " (rate limited or quota exceeded)",
        _ => "",
    };
    if msg.is_empty() {
        anyhow!("{provider} API error {status}{hint}")
    } else {
        anyhow!("{provider} API error {status}{hint}: {msg}")
    }
}

fn retry_after(resp: &reqwest::Response) -> Option<Duration> {
    let secs: u64 = resp.headers().get("retry-after")?.to_str().ok()?.trim().parse().ok()?;
    Some(Duration::from_secs(secs.min(30)))
}

/// Send a request, retrying 429/5xx (and connection errors) up to [`MAX_RETRIES`]
/// times with exponential backoff. Returns the successful response, or a readable error.
pub async fn send_with_retry<F>(provider: &str, base: Duration, make: F) -> anyhow::Result<reqwest::Response>
where
    F: Fn() -> reqwest::RequestBuilder,
{
    let mut attempt = 0u32;
    loop {
        let delay = base * 2u32.pow(attempt);
        match make().send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    return Ok(resp);
                }
                let wait = retry_after(&resp).unwrap_or(delay);
                if is_retryable(status) && attempt < MAX_RETRIES {
                    attempt += 1;
                    tokio::time::sleep(wait).await;
                    continue;
                }
                let text = resp.text().await.unwrap_or_default();
                return Err(format_api_error(provider, status, &text));
            }
            Err(e) => {
                if attempt < MAX_RETRIES && (e.is_connect() || e.is_timeout()) {
                    attempt += 1;
                    tokio::time::sleep(delay).await;
                    continue;
                }
                return Err(e).with_context(|| format!("{provider} request failed"));
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    pub struct Mock {
        pub url: String,
        pub hits: Arc<AtomicUsize>,
        pub requests: Arc<Mutex<Vec<String>>>,
    }

    /// Tiny HTTP server: answers request N with `responses[min(N, len-1)]` (status, content-type, body).
    pub async fn mock(responses: Vec<(u16, &'static str, String)>) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(vec![]));
        let (h, r) = (hits.clone(), requests.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let n = h.fetch_add(1, Ordering::SeqCst);
                let (status, ctype, body) = responses[n.min(responses.len() - 1)].clone();
                let r = r.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 65536];
                    let mut got = 0;
                    loop {
                        let k = sock.read(&mut buf[got..]).await.unwrap_or(0);
                        if k == 0 {
                            break;
                        }
                        got += k;
                        let text = String::from_utf8_lossy(&buf[..got]).to_string();
                        if let Some(hend) = text.find("\r\n\r\n") {
                            let cl = text[..hend]
                                .lines()
                                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                                .unwrap_or(0);
                            if got >= hend + 4 + cl {
                                break;
                            }
                        }
                    }
                    r.lock().unwrap().push(String::from_utf8_lossy(&buf[..got]).to_string());
                    let resp = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: {ctype}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        Mock { url, hits, requests }
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::mock;
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn extracts_messages() {
        assert_eq!(error_message(r#"{"error":{"message":"bad key"}}"#), "bad key");
        assert_eq!(error_message(r#"{"message":"nope"}"#), "nope");
        assert_eq!(error_message("plain text"), "plain text");
    }

    #[tokio::test]
    async fn retries_then_succeeds() {
        let m = mock(vec![(429, "application/json", "{}".into()), (503, "text/plain", "down".into()), (200, "text/plain", "ok".into())]).await;
        let c = reqwest::Client::new();
        let resp = send_with_retry("t", Duration::from_millis(1), || c.get(&m.url)).await.unwrap();
        assert_eq!(resp.text().await.unwrap(), "ok");
        assert_eq!(m.hits.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn gives_up_after_three_retries() {
        let m = mock(vec![(500, "application/json", r#"{"error":{"message":"boom"}}"#.into())]).await;
        let c = reqwest::Client::new();
        let err = send_with_retry("t", Duration::from_millis(1), || c.get(&m.url)).await.unwrap_err().to_string();
        assert!(err.contains("500") && err.contains("boom"), "{err}");
        assert_eq!(m.hits.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn no_retry_on_4xx() {
        let m = mock(vec![(401, "application/json", r#"{"error":{"message":"bad key"}}"#.into())]).await;
        let c = reqwest::Client::new();
        let err = send_with_retry("t", Duration::from_millis(1), || c.get(&m.url)).await.unwrap_err().to_string();
        assert!(err.contains("401") && err.contains("bad key"), "{err}");
        assert_eq!(m.hits.load(Ordering::SeqCst), 1);
    }
}
