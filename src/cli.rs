use anyhow::{Context, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Stdio,
    Serve { host: String, port: u16 },
    Help,
}

pub fn parse() -> anyhow::Result<Command> {
    parse_from(std::env::args().skip(1))
}

fn parse_from<I, S>(args: I) -> anyhow::Result<Command>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut args = args.into_iter().map(Into::into);
    let Some(command) = args.next() else {
        return Ok(Command::Stdio);
    };

    match command.as_str() {
        "stdio" => {
            if let Some(extra) = args.next() {
                bail!("unexpected argument for stdio: {extra}");
            }
            Ok(Command::Stdio)
        }
        "serve" => {
            let mut host = "127.0.0.1".to_string();
            let mut port = 8765_u16;

            while let Some(argument) = args.next() {
                match argument.as_str() {
                    "--host" => {
                        host = args.next().context("--host requires a value")?;
                    }
                    "--port" => {
                        let value = args.next().context("--port requires a value")?;
                        port = value
                            .parse::<u16>()
                            .with_context(|| format!("invalid port: {value}"))?;
                    }
                    "--help" | "-h" => return Ok(Command::Help),
                    _ => bail!("unknown serve argument: {argument}"),
                }
            }

            Ok(Command::Serve { host, port })
        }
        "--help" | "-h" | "help" => Ok(Command::Help),
        _ => bail!("unknown command: {command}\n\n{}", usage()),
    }
}

pub fn usage() -> &'static str {
    "Usage:\n  forge-mcp [stdio]\n  forge-mcp serve [--host <host>] [--port <port>]\n  forge-mcp --help"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_stdio() {
        assert_eq!(parse_from(Vec::<String>::new()).unwrap(), Command::Stdio);
    }

    #[test]
    fn parses_serve_defaults() {
        assert_eq!(
            parse_from(["serve"]).unwrap(),
            Command::Serve {
                host: "127.0.0.1".to_string(),
                port: 8765,
            }
        );
    }

    #[test]
    fn parses_serve_options() {
        assert_eq!(
            parse_from(["serve", "--host", "0.0.0.0", "--port", "9000"]).unwrap(),
            Command::Serve {
                host: "0.0.0.0".to_string(),
                port: 9000,
            }
        );
    }
}
