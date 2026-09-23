use crate::config::{AppConfig, AuthMode, OAuthConfig};

#[derive(Clone)]
pub struct ForgeMcp {
    config: AppConfig,
}

impl ForgeMcp {
    pub fn new(config: AppConfig) -> Self {
        Self { config }
    }

    pub fn workspace_root(&self) -> &std::path::Path {
        &self.config.workspace_root
    }

    pub fn max_batch_items(&self) -> usize {
        self.config.max_batch_items
    }

    pub fn max_concurrency(&self) -> usize {
        self.config.max_concurrency
    }

    pub fn auth_mode(&self) -> AuthMode {
        self.config.oauth.auth_mode
    }

    pub fn oauth_config(&self) -> &OAuthConfig {
        &self.config.oauth
    }
}
