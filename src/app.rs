use crate::{
    audit::AuditLog,
    config::{AppConfig, AuthMode, OAuthConfig},
    workspace::WorkspaceGuard,
};

#[derive(Clone)]
pub struct ForgeMcp {
    config: AppConfig,
    workspace: WorkspaceGuard,
    audit: AuditLog,
}

impl ForgeMcp {
    pub fn new(config: AppConfig) -> Self {
        let workspace = WorkspaceGuard::new(config.workspace_root.clone());
        let audit = AuditLog::new(config.oauth.state_dir.join("audit.jsonl"));
        Self {
            config,
            workspace,
            audit,
        }
    }

    pub fn workspace_root(&self) -> &std::path::Path {
        &self.config.workspace_root
    }

    pub fn workspace(&self) -> &WorkspaceGuard {
        &self.workspace
    }

    pub fn audit(&self) -> &AuditLog {
        &self.audit
    }

    pub fn max_batch_items(&self) -> usize {
        self.config.max_batch_items
    }

    pub fn max_concurrency(&self) -> usize {
        self.config.max_concurrency
    }

    pub fn max_read_bytes(&self) -> usize {
        self.config.max_read_bytes
    }

    pub fn max_write_bytes(&self) -> usize {
        self.config.max_write_bytes
    }

    pub fn max_shell_output_bytes(&self) -> usize {
        self.config.max_shell_output_bytes
    }

    pub fn default_shell_timeout_ms(&self) -> u64 {
        self.config.default_shell_timeout_ms
    }

    pub fn auth_mode(&self) -> AuthMode {
        self.config.oauth.auth_mode
    }

    pub fn oauth_config(&self) -> &OAuthConfig {
        &self.config.oauth
    }
}
