//! Parsing of the `mcp` section of opencode.json.
use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum McpServerConfig {
    Local {
        command: Vec<String>,
        #[serde(default)]
        environment: HashMap<String, String>,
        #[serde(default = "yes")]
        enabled: bool,
        /// Request timeout in milliseconds.
        #[serde(default)]
        timeout: Option<u64>,
    },
    Remote {
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
        #[serde(default = "yes")]
        enabled: bool,
        #[serde(default)]
        timeout: Option<u64>,
    },
}

fn yes() -> bool {
    true
}

impl McpServerConfig {
    pub fn enabled(&self) -> bool {
        match self {
            Self::Local { enabled, .. } | Self::Remote { enabled, .. } => *enabled,
        }
    }
    pub fn timeout_ms(&self) -> Option<u64> {
        match self {
            Self::Local { timeout, .. } | Self::Remote { timeout, .. } => *timeout,
        }
    }
}

/// Parse every entry; invalid ones are returned as errors alongside their name.
pub fn parse_mcp(
    map: &serde_json::Map<String, serde_json::Value>,
) -> Vec<(String, anyhow::Result<McpServerConfig>)> {
    map.iter()
        .map(|(k, v)| {
            (k.clone(), serde_json::from_value::<McpServerConfig>(v.clone()).map_err(anyhow::Error::from))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_local_and_remote() {
        let m = json!({
            "a": {"type":"local","command":["npx","-y","srv"],"environment":{"K":"V"}},
            "b": {"type":"remote","url":"http://x/mcp","headers":{"Authorization":"Bearer t"},"enabled":false},
            "c": {"type":"bogus"}
        });
        let r = parse_mcp(m.as_object().unwrap());
        assert_eq!(r.len(), 3);
        match r[0].1.as_ref().unwrap() {
            McpServerConfig::Local { command, environment, enabled, .. } => {
                assert_eq!(command.len(), 3);
                assert_eq!(environment["K"], "V");
                assert!(*enabled);
            }
            _ => panic!(),
        }
        assert!(!r[1].1.as_ref().unwrap().enabled());
        assert!(r[2].1.is_err());
    }
}
