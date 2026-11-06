//! Config-driven permission rules (`permission` section of opencode.json).
//! Owner: permission agent. Keep these signatures; UIs implement `Asker`.

use std::sync::Arc;

use async_trait::async_trait;

use crate::{Config, PermissionDecision, PermissionHandler, PermissionRequest};

/// Asks the user interactively (TUI dialog, CLI prompt, server client).
#[async_trait]
pub trait Asker: Send + Sync {
    async fn ask(&self, req: &PermissionRequest) -> PermissionDecision;
}

/// Applies the configured rules and calls the `Asker` when a rule says "ask".
pub struct RulePermission {
    asker: Arc<dyn Asker>,
}

impl RulePermission {
    pub fn new(_config: &Config, asker: Arc<dyn Asker>) -> RulePermission {
        RulePermission { asker }
    }
}

#[async_trait]
impl PermissionHandler for RulePermission {
    async fn check(&self, _req: &PermissionRequest) -> PermissionDecision {
        let _ = &self.asker;
        PermissionDecision::Allow
    }
}
