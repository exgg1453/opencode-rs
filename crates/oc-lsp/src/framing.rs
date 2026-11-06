//! JSON-RPC over stdio with `Content-Length` framing.

use serde_json::Value;

pub fn encode(msg: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(msg).unwrap_or_default();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(&body);
    out
}

/// Incremental decoder: feed arbitrary byte chunks, get complete messages.
#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
}

impl Decoder {
    pub fn push(&mut self, data: &[u8]) -> Vec<Value> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        while let Some(hend) = self.buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let header = String::from_utf8_lossy(&self.buf[..hend]).to_string();
            let len = header.lines().find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.trim().eq_ignore_ascii_case("content-length").then(|| v.trim().parse::<usize>().ok())?
            });
            let Some(len) = len else {
                // Malformed header: drop it and resync.
                self.buf.drain(..hend + 4);
                continue;
            };
            let start = hend + 4;
            if self.buf.len() < start + len {
                break;
            }
            let body: Vec<u8> = self.buf.drain(..start + len).skip(start).collect();
            if let Ok(v) = serde_json::from_slice::<Value>(&body) {
                out.push(v);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn roundtrip_and_split_reads() {
        let a = json!({"jsonrpc":"2.0","id":1,"method":"x","params":{"s":"héllo"}});
        let b = json!({"jsonrpc":"2.0","method":"y"});
        let mut bytes = encode(&a);
        bytes.extend(encode(&b));
        let mut d = Decoder::default();
        let mut got = Vec::new();
        for chunk in bytes.chunks(3) {
            got.extend(d.push(chunk));
        }
        assert_eq!(got, vec![a, b]);
    }

    #[test]
    fn two_in_one_chunk() {
        let a = json!({"a":1});
        let mut bytes = encode(&a);
        bytes.extend(encode(&a));
        assert_eq!(Decoder::default().push(&bytes).len(), 2);
    }
}
