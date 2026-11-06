//! Stored provider credentials (`opencode auth login`).
//! Owner: share/auth agent. Keep these signatures.

/// API key saved for `provider` (e.g. "anthropic"), if any.
pub fn get_api_key(_provider: &str) -> Option<String> {
    None
}
