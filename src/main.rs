mod app;
mod batch;
mod config;
mod protocol;
mod server;
mod tools;
mod transport;

use app::ForgeMcp;
use config::AppConfig;
use server::ForgeServer;
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
        "starting ForgeMCP over stdio"
    );

    transport::stdio::serve(ForgeServer::new(app)).await
}
