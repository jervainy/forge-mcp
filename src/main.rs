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
use config::{AppConfig, AuthMode, ConfigOverrides, RunMode};
use server::ForgeServer;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = cli::parse()?;
    if cli.command == Command::Help {
        println!("{}", cli::usage());
        return Ok(());
    }

    let overrides = match &cli.command {
        Command::Auto => ConfigOverrides::default(),
        Command::Stdio => ConfigOverrides {
            mode: Some(RunMode::Stdio),
            ..ConfigOverrides::default()
        },
        Command::Serve { host, port } => ConfigOverrides {
            mode: Some(RunMode::Serve),
            host: host.clone(),
            port: *port,
        },
        Command::Help => unreachable!(),
    };

    let config = AppConfig::load(cli.config.as_deref(), overrides)?;
    let app = ForgeMcp::new(config);

    info!(
        config = ?app.config_path().map(|path| path.display().to_string()),
        mode = ?app.run_mode(),
        workspace = %app.workspace_root().display(),
        max_batch_items = app.max_batch_items(),
        max_concurrency = app.max_concurrency(),
        oauth = app.auth_mode() == AuthMode::OAuth,
        "starting ForgeMCP"
    );

    match app.run_mode() {
        RunMode::Stdio => transport::stdio::serve(ForgeServer::new(app)).await,
        RunMode::Serve => {
            let host = app.serve_host().to_string();
            let port = app.serve_port();

            if !is_loopback_host(&host) && app.auth_mode() == AuthMode::None {
                warn!(
                    host,
                    port,
                    "ForgeMCP is binding to a non-loopback interface with authentication disabled"
                );
            }

            transport::http::serve(app, &host, port).await
        }
    }
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "::1" | "localhost")
}
