use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub workspace_root: PathBuf,
    pub max_batch_items: usize,
    pub max_concurrency: usize,
}

impl AppConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let workspace_root = std::env::var_os("FORGE_MCP_WORKSPACE")
            .map(PathBuf::from)
            .unwrap_or(std::env::current_dir()?);

        Ok(Self {
            workspace_root,
            max_batch_items: 100,
            max_concurrency: 16,
        })
    }
}
