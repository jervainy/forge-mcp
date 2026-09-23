use std::path::PathBuf;

use anyhow::{Context, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cli {
    pub config: Option<PathBuf>,
    pub command: Command,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Auto,
    Stdio,
    Serve {
        host: Option<String>,
        port: Option<u16>,
    },
    Help,
}

pub fn parse() -> anyhow::Result<Cli> {
    parse_from(std::env::args().skip(1))
}

fn parse_from<I, S>(args: I) -> anyhow::Result<Cli>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut args = args.into_iter().map(Into::into);
    let mut config = None;
    let mut command = Command::Auto;

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--config" => {
                let value = args.next().context("--config requires a path")?;
                config = Some(PathBuf::from(value));
            }
            "stdio" => {
                ensure_auto(&command, "stdio")?;
                command = Command::Stdio;
            }
            "serve" => {
                ensure_auto(&command, "serve")?;
                command = Command::Serve {
                    host: None,
                    port: None,
                };
            }
            "--host" => {
                let value = args.next().context("--host requires a value")?;
                match &mut command {
                    Command::Serve { host, .. } => *host = Some(value),
                    _ => bail!("--host is only valid after the serve command"),
                }
            }
            "--port" => {
                let value = args.next().context("--port requires a value")?;
                let value = value
                    .parse::<u16>()
                    .with_context(|| format!("invalid port: {value}"))?;
                match &mut command {
                    Command::Serve { port, .. } => *port = Some(value),
                    _ => bail!("--port is only valid after the serve command"),
                }
            }
            "--help" | "-h" | "help" => command = Command::Help,
            _ => bail!("unknown argument: {argument}\n\n{}", usage()),
        }
    }

    Ok(Cli { config, command })
}

fn ensure_auto(command: &Command, requested: &str) -> anyhow::Result<()> {
    if matches!(command, Command::Auto) {
        Ok(())
    } else {
        bail!("cannot combine {requested} with another command")
    }
}

pub fn usage() -> &'static str {
    "Usage:\n  forge-mcp [--config <path>]\n  forge-mcp [--config <path>] stdio\n  forge-mcp [--config <path>] serve [--host <host>] [--port <port>]\n  forge-mcp --help\n\nConfiguration precedence:\n  defaults < YAML < FORGE_MCP_* environment < CLI"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_auto_mode() {
        assert_eq!(
            parse_from(Vec::<String>::new()).unwrap(),
            Cli {
                config: None,
                command: Command::Auto,
            }
        );
    }

    #[test]
    fn parses_config_before_command() {
        assert_eq!(
            parse_from([
                "--config",
                "/tmp/forge.yaml",
                "serve",
                "--host",
                "0.0.0.0",
                "--port",
                "9000"
            ])
            .unwrap(),
            Cli {
                config: Some(PathBuf::from("/tmp/forge.yaml")),
                command: Command::Serve {
                    host: Some("0.0.0.0".to_string()),
                    port: Some(9000),
                },
            }
        );
    }

    #[test]
    fn parses_config_after_command() {
        assert_eq!(
            parse_from(["serve", "--config", "forge.yaml"]).unwrap(),
            Cli {
                config: Some(PathBuf::from("forge.yaml")),
                command: Command::Serve {
                    host: None,
                    port: None,
                },
            }
        );
    }

    #[test]
    fn parses_stdio() {
        assert_eq!(parse_from(["stdio"]).unwrap().command, Command::Stdio);
    }
}
