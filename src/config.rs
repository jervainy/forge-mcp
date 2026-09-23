use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use serde::Deserialize;
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Stdio,
    Serve,
}

impl RunMode {
    fn parse(value: &str, source: &str) -> anyhow::Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "stdio" => Ok(Self::Stdio),
            "serve" | "http" | "mcp" => Ok(Self::Serve),
            value => bail!("{source} has unsupported mode: {value}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    None,
    OAuth,
}

impl AuthMode {
    fn parse(value: &str, source: &str) -> anyhow::Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(Self::None),
            "oauth" => Ok(Self::OAuth),
            value => bail!("{source} has unsupported auth_mode: {value}"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ConfigOverrides {
    pub mode: Option<RunMode>,
    pub host: Option<String>,
    pub port: Option<u16>,
}

#[derive(Clone)]
pub struct OAuthConfig {
    pub auth_mode: AuthMode,
    pub auth_bypass_localhost: bool,
    pub public_base_url: Option<String>,
    pub oauth_issuer: Option<String>,
    pub oauth_resource: Option<String>,
    pub oauth_admin_pin: Option<String>,
    pub oauth_jwt_secret: String,
    pub oauth_access_token_ttl_s: u64,
    pub oauth_code_ttl_s: u64,
    pub state_dir: PathBuf,
}

impl OAuthConfig {
    pub fn enabled(&self) -> bool {
        self.auth_mode == AuthMode::OAuth
    }
}

#[derive(Clone)]
pub struct AppConfig {
    pub mode: RunMode,
    pub host: String,
    pub port: u16,
    pub config_path: Option<PathBuf>,
    pub workspace_root: PathBuf,
    pub max_batch_items: usize,
    pub max_concurrency: usize,
    pub max_read_bytes: usize,
    pub max_write_bytes: usize,
    pub max_shell_output_bytes: usize,
    pub default_shell_timeout_ms: u64,
    pub oauth: OAuthConfig,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct FileConfig {
    mode: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    workspace_root: Option<PathBuf>,
    state_dir: Option<PathBuf>,
    max_batch_items: Option<usize>,
    max_concurrency: Option<usize>,
    max_read_bytes: Option<usize>,
    max_write_bytes: Option<usize>,
    max_shell_output_bytes: Option<usize>,
    shell_timeout_ms: Option<u64>,
    auth_mode: Option<String>,
    auth_bypass_localhost: Option<bool>,
    public_base_url: Option<String>,
    oauth_issuer: Option<String>,
    oauth_resource: Option<String>,
    oauth_admin_pin: Option<String>,
    oauth_jwt_secret: Option<String>,
    oauth_access_token_ttl_s: Option<u64>,
    oauth_code_ttl_s: Option<u64>,
}

struct Settings {
    mode: RunMode,
    host: String,
    port: u16,
    workspace_root: PathBuf,
    state_dir: PathBuf,
    max_batch_items: usize,
    max_concurrency: usize,
    max_read_bytes: usize,
    max_write_bytes: usize,
    max_shell_output_bytes: usize,
    shell_timeout_ms: u64,
    auth_mode: AuthMode,
    auth_bypass_localhost: bool,
    public_base_url: Option<String>,
    oauth_issuer: Option<String>,
    oauth_resource: Option<String>,
    oauth_admin_pin: Option<String>,
    oauth_jwt_secret: Option<String>,
    oauth_access_token_ttl_s: u64,
    oauth_code_ttl_s: u64,
}

impl Settings {
    fn defaults() -> anyhow::Result<Self> {
        Ok(Self {
            mode: RunMode::Stdio,
            host: "127.0.0.1".to_string(),
            port: 8765,
            workspace_root: std::env::current_dir()?,
            state_dir: default_state_dir()?,
            max_batch_items: 100,
            max_concurrency: 16,
            max_read_bytes: 1024 * 1024,
            max_write_bytes: 4 * 1024 * 1024,
            max_shell_output_bytes: 1024 * 1024,
            shell_timeout_ms: 30_000,
            auth_mode: AuthMode::OAuth,
            auth_bypass_localhost: true,
            public_base_url: None,
            oauth_issuer: None,
            oauth_resource: None,
            oauth_admin_pin: None,
            oauth_jwt_secret: None,
            oauth_access_token_ttl_s: 0,
            oauth_code_ttl_s: 300,
        })
    }

    fn apply_file(&mut self, file: FileConfig) -> anyhow::Result<()> {
        if let Some(value) = file.mode {
            self.mode = RunMode::parse(&value, "YAML mode")?;
        }
        set_some(&mut self.host, file.host);
        set_some(&mut self.port, file.port);
        set_some(&mut self.workspace_root, file.workspace_root);
        set_some(&mut self.state_dir, file.state_dir);
        set_some(&mut self.max_batch_items, file.max_batch_items);
        set_some(&mut self.max_concurrency, file.max_concurrency);
        set_some(&mut self.max_read_bytes, file.max_read_bytes);
        set_some(&mut self.max_write_bytes, file.max_write_bytes);
        set_some(&mut self.max_shell_output_bytes, file.max_shell_output_bytes);
        set_some(&mut self.shell_timeout_ms, file.shell_timeout_ms);
        if let Some(value) = file.auth_mode {
            self.auth_mode = AuthMode::parse(&value, "YAML auth_mode")?;
        }
        set_some(&mut self.auth_bypass_localhost, file.auth_bypass_localhost);
        set_option(&mut self.public_base_url, file.public_base_url);
        set_option(&mut self.oauth_issuer, file.oauth_issuer);
        set_option(&mut self.oauth_resource, file.oauth_resource);
        set_option(&mut self.oauth_admin_pin, file.oauth_admin_pin);
        set_option(&mut self.oauth_jwt_secret, file.oauth_jwt_secret);
        set_some(
            &mut self.oauth_access_token_ttl_s,
            file.oauth_access_token_ttl_s,
        );
        set_some(&mut self.oauth_code_ttl_s, file.oauth_code_ttl_s);
        Ok(())
    }

    fn apply_environment(&mut self) -> anyhow::Result<()> {
        if let Some(value) = env_string("FORGE_MCP_MODE") {
            self.mode = RunMode::parse(&value, "FORGE_MCP_MODE")?;
        }
        if let Some(value) = env_string("FORGE_MCP_HOST") {
            self.host = value;
        }
        if let Some(value) = env_parse::<u16>("FORGE_MCP_PORT")? {
            self.port = value;
        }
        if let Some(value) = std::env::var_os("FORGE_MCP_WORKSPACE_ROOT")
            .or_else(|| std::env::var_os("FORGE_MCP_WORKSPACE"))
        {
            self.workspace_root = PathBuf::from(value);
        }
        if let Some(value) = std::env::var_os("FORGE_MCP_STATE_DIR") {
            self.state_dir = PathBuf::from(value);
        }
        set_env_usize("FORGE_MCP_MAX_BATCH_ITEMS", &mut self.max_batch_items)?;
        set_env_usize("FORGE_MCP_MAX_CONCURRENCY", &mut self.max_concurrency)?;
        set_env_usize("FORGE_MCP_MAX_READ_BYTES", &mut self.max_read_bytes)?;
        set_env_usize("FORGE_MCP_MAX_WRITE_BYTES", &mut self.max_write_bytes)?;
        set_env_usize(
            "FORGE_MCP_MAX_SHELL_OUTPUT_BYTES",
            &mut self.max_shell_output_bytes,
        )?;
        if let Some(value) = env_parse::<u64>("FORGE_MCP_SHELL_TIMEOUT_MS")? {
            self.shell_timeout_ms = value;
        }
        if let Some(value) = env_string("FORGE_MCP_AUTH_MODE") {
            self.auth_mode = AuthMode::parse(&value, "FORGE_MCP_AUTH_MODE")?;
        }
        if let Some(value) = env_bool("FORGE_MCP_AUTH_BYPASS_LOCALHOST")? {
            self.auth_bypass_localhost = value;
        }

        overlay_optional_string("FORGE_MCP_PUBLIC_BASE_URL", &mut self.public_base_url);
        overlay_optional_string("FORGE_MCP_OAUTH_ISSUER", &mut self.oauth_issuer);
        overlay_optional_string("FORGE_MCP_OAUTH_RESOURCE", &mut self.oauth_resource);
        overlay_optional_string("FORGE_MCP_OAUTH_ADMIN_PIN", &mut self.oauth_admin_pin);
        overlay_optional_string("FORGE_MCP_OAUTH_JWT_SECRET", &mut self.oauth_jwt_secret);

        if let Some(value) = env_parse::<u64>("FORGE_MCP_OAUTH_ACCESS_TOKEN_TTL_S")? {
            self.oauth_access_token_ttl_s = value;
        }
        if let Some(value) = env_parse::<u64>("FORGE_MCP_OAUTH_CODE_TTL_S")? {
            self.oauth_code_ttl_s = value;
        }
        Ok(())
    }

    fn apply_cli(&mut self, overrides: ConfigOverrides) {
        set_some(&mut self.mode, overrides.mode);
        set_some(&mut self.host, overrides.host);
        set_some(&mut self.port, overrides.port);
    }
}

impl AppConfig {
    pub fn load(
        cli_config_path: Option<&Path>,
        overrides: ConfigOverrides,
    ) -> anyhow::Result<Self> {
        let mut settings = Settings::defaults()?;
        let config_path = select_config_path(cli_config_path);

        if let Some(path) = config_path.as_deref() {
            let content = fs::read_to_string(path)
                .with_context(|| format!("failed to read config {}", path.display()))?;
            let file = serde_yaml::from_str::<FileConfig>(&content)
                .with_context(|| format!("failed to parse YAML config {}", path.display()))?;
            settings.apply_file(file)?;
        }

        settings.apply_environment()?;
        settings.apply_cli(overrides);

        settings.workspace_root = expand_home(settings.workspace_root);
        settings.state_dir = expand_home(settings.state_dir);
        let workspace_root = fs::canonicalize(&settings.workspace_root).with_context(|| {
            format!(
                "failed to resolve workspace {}",
                settings.workspace_root.display()
            )
        })?;
        if !workspace_root.is_dir() {
            bail!(
                "workspace_root must be a directory: {}",
                workspace_root.display()
            );
        }

        validate_positive("max_batch_items", settings.max_batch_items)?;
        validate_positive("max_concurrency", settings.max_concurrency)?;
        validate_positive("max_read_bytes", settings.max_read_bytes)?;
        validate_positive("max_write_bytes", settings.max_write_bytes)?;
        validate_positive(
            "max_shell_output_bytes",
            settings.max_shell_output_bytes,
        )?;
        if settings.shell_timeout_ms == 0 {
            bail!("shell_timeout_ms must be greater than zero");
        }
        if settings.oauth_code_ttl_s == 0 {
            bail!("oauth_code_ttl_s must be greater than zero");
        }
        if settings.host.trim().is_empty() {
            bail!("host must not be empty");
        }

        let public_base_url =
            normalize_url(settings.public_base_url, "public_base_url", true)?;
        let oauth_issuer = normalize_url(settings.oauth_issuer, "oauth_issuer", false)?;
        let oauth_resource =
            normalize_url(settings.oauth_resource, "oauth_resource", false)?;

        if settings.auth_mode == AuthMode::OAuth {
            fs::create_dir_all(&settings.state_dir)
                .with_context(|| format!("failed to create {}", settings.state_dir.display()))?;
        }

        if public_base_url.is_some() {
            validate_admin_pin(settings.oauth_admin_pin.as_deref())?;
        }

        let oauth_jwt_secret = if settings.auth_mode == AuthMode::OAuth {
            match settings.oauth_jwt_secret {
                Some(secret) => {
                    validate_jwt_secret(&secret)?;
                    secret
                }
                None => get_or_create_oauth_secret(&settings.state_dir)?,
            }
        } else {
            String::new()
        };

        Ok(Self {
            mode: settings.mode,
            host: settings.host,
            port: settings.port,
            config_path,
            workspace_root,
            max_batch_items: settings.max_batch_items,
            max_concurrency: settings.max_concurrency,
            max_read_bytes: settings.max_read_bytes,
            max_write_bytes: settings.max_write_bytes,
            max_shell_output_bytes: settings.max_shell_output_bytes,
            default_shell_timeout_ms: settings.shell_timeout_ms,
            oauth: OAuthConfig {
                auth_mode: settings.auth_mode,
                auth_bypass_localhost: settings.auth_bypass_localhost,
                public_base_url,
                oauth_issuer,
                oauth_resource,
                oauth_admin_pin: settings.oauth_admin_pin,
                oauth_jwt_secret,
                oauth_access_token_ttl_s: settings.oauth_access_token_ttl_s,
                oauth_code_ttl_s: settings.oauth_code_ttl_s,
                state_dir: settings.state_dir,
            },
        })
    }

    #[cfg(test)]
    pub fn test(workspace_root: PathBuf) -> Self {
        let workspace_root = fs::canonicalize(workspace_root).unwrap();
        Self {
            mode: RunMode::Stdio,
            host: "127.0.0.1".to_string(),
            port: 8765,
            config_path: None,
            workspace_root,
            max_batch_items: 100,
            max_concurrency: 16,
            max_read_bytes: 1024 * 1024,
            max_write_bytes: 4 * 1024 * 1024,
            max_shell_output_bytes: 1024 * 1024,
            default_shell_timeout_ms: 30_000,
            oauth: OAuthConfig {
                auth_mode: AuthMode::None,
                auth_bypass_localhost: true,
                public_base_url: None,
                oauth_issuer: None,
                oauth_resource: None,
                oauth_admin_pin: None,
                oauth_jwt_secret: String::new(),
                oauth_access_token_ttl_s: 0,
                oauth_code_ttl_s: 300,
                state_dir: std::env::temp_dir().join(format!("forge-mcp-test-{}", Uuid::new_v4())),
            },
        }
    }
}

fn select_config_path(cli_config_path: Option<&Path>) -> Option<PathBuf> {
    cli_config_path
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("FORGE_MCP_CONFIG").map(PathBuf::from))
        .map(expand_home)
}

fn default_state_dir() -> anyhow::Result<PathBuf> {
    if let Some(value) = std::env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(value).join("forge-mcp"));
    }
    if let Some(value) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(value).join(".config").join("forge-mcp"));
    }
    Ok(std::env::current_dir()?.join(".forge-mcp"))
}

fn expand_home(path: PathBuf) -> PathBuf {
    let value = path.to_string_lossy();
    if value == "~" {
        return std::env::var_os("HOME").map(PathBuf::from).unwrap_or(path);
    }
    if let Some(rest) = value.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    path
}

fn normalize_url(
    value: Option<String>,
    name: &str,
    require_https: bool,
) -> anyhow::Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim().trim_end_matches('/').to_string();
    if value.is_empty() {
        return Ok(None);
    }

    let parsed = Url::parse(&value).with_context(|| format!("{name} must be an absolute URL"))?;
    if parsed.fragment().is_some() || parsed.query().is_some() {
        bail!("{name} must not include a query string or fragment");
    }
    if require_https && parsed.scheme() != "https" {
        let is_loopback = parsed
            .host_str()
            .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "::1"));
        if !is_loopback {
            bail!("{name} must use https for non-loopback deployments");
        }
    }
    Ok(Some(value))
}

fn env_string(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn overlay_optional_string(name: &str, target: &mut Option<String>) {
    if let Ok(value) = std::env::var(name) {
        *target = if value.trim().is_empty() {
            None
        } else {
            Some(value)
        };
    }
}

fn env_bool(name: &str) -> anyhow::Result<Option<bool>> {
    let Ok(value) = std::env::var(name) else {
        return Ok(None);
    };
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(Some(true)),
        "0" | "false" | "no" | "off" => Ok(Some(false)),
        _ => bail!("{name} must be true or false"),
    }
}

fn env_parse<T>(name: &str) -> anyhow::Result<Option<T>>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match std::env::var(name) {
        Ok(value) => value
            .parse::<T>()
            .map(Some)
            .with_context(|| format!("{name} has an invalid value")),
        Err(_) => Ok(None),
    }
}

fn set_env_usize(name: &str, target: &mut usize) -> anyhow::Result<()> {
    if let Some(value) = env_parse::<usize>(name)? {
        *target = value;
    }
    Ok(())
}

fn set_some<T>(target: &mut T, value: Option<T>) {
    if let Some(value) = value {
        *target = value;
    }
}

fn set_option<T>(target: &mut Option<T>, value: Option<T>) {
    if value.is_some() {
        *target = value;
    }
}

fn validate_positive(name: &str, value: usize) -> anyhow::Result<()> {
    if value == 0 {
        bail!("{name} must be greater than zero");
    }
    Ok(())
}

fn validate_admin_pin(pin: Option<&str>) -> anyhow::Result<()> {
    let weak = ["", "change-me", "change-me-long-random-pin"];
    let value = pin.unwrap_or("").trim();
    if value.len() < 8 || weak.contains(&value) {
        bail!(
            "oauth_admin_pin must be a non-placeholder value of at least 8 characters when public_base_url is configured"
        );
    }
    Ok(())
}

fn validate_jwt_secret(secret: &str) -> anyhow::Result<()> {
    let weak = ["", "change-me", "dev-change-me"];
    if secret.len() < 32 || weak.contains(&secret) {
        bail!("oauth_jwt_secret must contain at least 32 bytes of strong random data");
    }
    Ok(())
}

fn get_or_create_oauth_secret(state_dir: &Path) -> anyhow::Result<String> {
    let path = state_dir.join("oauth-jwt-secret");
    if path.exists() {
        let secret = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let secret = secret.trim().to_string();
        validate_jwt_secret(&secret)?;
        return Ok(secret);
    }

    let secret = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    fs::write(&path, &secret).with_context(|| format!("failed to write {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to secure {}", path.display()))?;
    }

    Ok(secret)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_yaml_fields() {
        let file: FileConfig = serde_yaml::from_str(
            r#"
mode: serve
host: 0.0.0.0
port: 9000
workspace_root: /tmp
max_batch_items: 25
auth_mode: none
"#,
        )
        .unwrap();

        assert_eq!(file.mode.as_deref(), Some("serve"));
        assert_eq!(file.host.as_deref(), Some("0.0.0.0"));
        assert_eq!(file.port, Some(9000));
        assert_eq!(file.max_batch_items, Some(25));
        assert_eq!(file.auth_mode.as_deref(), Some("none"));
    }

    #[test]
    fn accepts_http_and_mcp_as_serve_aliases() {
        assert_eq!(RunMode::parse("http", "test").unwrap(), RunMode::Serve);
        assert_eq!(RunMode::parse("mcp", "test").unwrap(), RunMode::Serve);
    }
}
