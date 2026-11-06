//! JSON-RPC 2.0 framing helpers.
use serde_json::{json, Value};

pub const PROTOCOL_VERSION: &str = "2025-03-26";

pub fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
}

pub fn notification(method: &str, params: Option<Value>) -> Value {
    match params {
        Some(p) => json!({"jsonrpc":"2.0","method":method,"params":p}),
        None => json!({"jsonrpc":"2.0","method":method}),
    }
}

/// Serialize a message as a single newline-terminated line.
pub fn frame(msg: &Value) -> String {
    let mut s = serde_json::to_string(msg).expect("json serialize");
    s.push('\n');
    s
}

/// Numeric id of a response message (has id and result/error, no method).
pub fn response_id(msg: &Value) -> Option<u64> {
    if msg.get("method").is_some() {
        return None;
    }
    msg.get("id")?.as_u64()
}

/// Turn a response message into result or error.
pub fn into_result(msg: Value) -> anyhow::Result<Value> {
    if let Some(err) = msg.get("error") {
        let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
        let m = err.get("message").and_then(Value::as_str).unwrap_or("unknown error");
        anyhow::bail!("JSON-RPC error {code}: {m}");
    }
    Ok(msg.get("result").cloned().unwrap_or(Value::Null))
}

/// Extract JSON payloads (`data:` events) from an SSE body.
pub fn parse_sse(body: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut data = String::new();
    let flush = |data: &mut String, out: &mut Vec<Value>| {
        if !data.is_empty() {
            if let Ok(v) = serde_json::from_str::<Value>(data) {
                out.push(v);
            }
            data.clear();
        }
    };
    for line in body.lines() {
        if line.is_empty() {
            flush(&mut data, &mut out);
        } else if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
        }
    }
    flush(&mut data, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framing() {
        let f = frame(&request(3, "tools/list", json!({})));
        assert!(f.ends_with('\n') && f.matches('\n').count() == 1);
        let v: Value = serde_json::from_str(f.trim()).unwrap();
        assert_eq!(v["id"], 3);
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(response_id(&json!({"id":3,"result":{}})), Some(3));
        assert_eq!(response_id(&json!({"id":3,"method":"ping"})), None);
        assert!(into_result(json!({"id":1,"error":{"code":-1,"message":"bad"}})).is_err());
        assert_eq!(into_result(json!({"id":1,"result":{"a":1}})).unwrap()["a"], 1);
    }
    #[test]
    fn sse() {
        let v = parse_sse("event: message\ndata: {\"id\":1}\n\ndata: {\"id\":2}\n\n");
        assert_eq!(v.len(), 2);
        assert_eq!(v[1]["id"], 2);
    }
}
