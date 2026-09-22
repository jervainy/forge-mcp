mod app;
mod batch;
mod config;
mod tools;

use app::ForgeMcp;
use config::AppConfig;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let config = AppConfig::from_env()?;
    let app = ForgeMcp::new(config);

    info!(
        workspace = %app.workspace_root().display(),
        max_batch_items = app.max_batch_items(),
        max_concurrency = app.max_concurrency(),
        tools = ?tools::TOOL_NAMES,
        "starting ForgeMCP"
    );

    // MCP stdio transport and tool handlers are wired in the next implementation step.
    tokio::signal::ctrl_c().await?;
    Ok(())
}
