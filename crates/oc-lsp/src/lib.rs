//! Language Server Protocol client: after the agent writes or edits a file,
//! ask the matching language server for diagnostics. Owner: LSP agent.

use std::path::Path;
use std::sync::Arc;

use oc_core::{Config, DiagnosticsProvider};

/// Returns None when LSP is disabled (`"lsp": false`) or no server is available.
/// Servers start lazily on first use. Keep this signature.
pub async fn start(_config: &Config, _cwd: &Path) -> Option<Arc<dyn DiagnosticsProvider>> {
    None
}
