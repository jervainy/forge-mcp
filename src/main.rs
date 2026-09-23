mod app;
mod audit;
mod auth;
mod batch;
mod cli;
mod config;
mod protocol;
mod runtime;
mod server;
mod tools;
mod transport;
mod workspace;

use app::ForgeMcp;
use cli::Command;
use config::{AppConfig, AuthMode};
use server::ForgeServer;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let command = cli::parse()?;
    if command == Command::Help {
        println!("{}", cli::usage());
        return Ok(());
    }

    let config = AppConfig::from_env()?;
    let app = ForgeMcp::new(config);

    info!(
        workspace = %app.workspace_root().display(),
        max_batch_items = app.max_batch_items(),
        max_concurrency = app.max_concurrency(),
        oauth = app.auth_mode() == AuthMode::OAuth,
        "starting ForgeMCP"
    );

    match command {
        Command::Stdio => transport::stdio::serve(ForgeServer::new(app)).await,
        Command::Serve { host, port } => {
            if !is_loopback_host(&host) && app.auth_mode() == AuthMode::None {
                warn!(
                    host,
                    port,
                    "ForgeMCP is binding to a non-loopback interface with authentication disabled"
                );
            }

            transport::http::serve(app, &host, port).await
        }
        Command::Help => Ok(()),
    }
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "::1" | "localhost")
}
