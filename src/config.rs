use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    None,
    OAuth,
}

impl AuthMode {
    fn from_env() -> anyhow::Result<Self> {
        match std::env::var("FORGE_MCP_AUTH_MODE")
            .unwrap_or_else(|_| "oauth".to_string())
            .to_ascii_lowercase()
            .as_str()
        {
            "none" => Ok(Self::None),
            "oauth" => Ok(Self::OAuth),
            value => bail!("unsupported FORGE_MCP_AUTH_MODE: {value}"),
        }
    }
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
    pub workspace_root: PathBuf,
    pub max_batch_items: usize,
    pub max_concurrency: usize,
    pub oauth: OAuthConfig,
}

impl AppConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let workspace_root = std::env::var_os("FORGE_MCP_WORKSPACE")
            .map(PathBuf::from)
            .unwrap_or(std::env::current_dir()?);

        let auth_mode = AuthMode::from_env()?;
        let auth_bypass_localhost = env_bool("FORGE_MCP_AUTH_BYPASS_LOCALHOST", true)?;
        let state_dir = state_dir()?;
        if auth_mode == AuthMode::OAuth {
            fs::create_dir_all(&state_dir)
                .with_context(|| format!("failed to create {}", state_dir.display()))?;
        }

        let public_base_url = env_url("FORGE_MCP_PUBLIC_BASE_URL")?;
        let oauth_issuer = env_url("FORGE_MCP_OAUTH_ISSUER")?;
        let oauth_resource = env_url("FORGE_MCP_OAUTH_RESOURCE")?;
        let oauth_admin_pin = std::env::var("FORGE_MCP_OAUTH_ADMIN_PIN").ok();

        if public_base_url.is_some() {
            validate_admin_pin(oauth_admin_pin.as_deref())?;
        }

        let oauth_jwt_secret = if auth_mode == AuthMode::OAuth {
            match std::env::var("FORGE_MCP_OAUTH_JWT_SECRET") {
                Ok(secret) => {
                    validate_jwt_secret(&secret)?;
                    secret
                }
                Err(_) => get_or_create_oauth_secret(&state_dir)?,
            }
        } else {
            String::new()
        };

        Ok(Self {
            workspace_root,
            max_batch_items: env_usize("FORGE_MCP_MAX_BATCH_ITEMS", 100)?,
            max_concurrency: env_usize("FORGE_MCP_MAX_CONCURRENCY", 16)?,
            oauth: OAuthConfig {
                auth_mode,
                auth_bypass_localhost,
                public_base_url,
                oauth_issuer,
                oauth_resource,
                oauth_admin_pin,
                oauth_jwt_secret,
                oauth_access_token_ttl_s: env_u64(
                    "FORGE_MCP_OAUTH_ACCESS_TOKEN_TTL_S",
                    0,
                )?,
                oauth_code_ttl_s: env_u64("FORGE_MCP_OAUTH_CODE_TTL_S", 300)?,
                state_dir,
            },
        })
    }

    #[cfg(test)]
    pub fn test(workspace_root: PathBuf) -> Self {
        Self {
            workspace_root,
            max_batch_items: 100,
            max_concurrency: 16,
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
                state_dir: std::env::temp_dir()
                    .join(format!("forge-mcp-test-{}", Uuid::new_v4())),
            },
        }
    }
}

fn state_dir() -> anyhow::Result<PathBuf> {
    if let Some(value) = std::env::var_os("FORGE_MCP_STATE_DIR") {
        return Ok(PathBuf::from(value));
    }

    if let Some(value) = std::env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(value).join("forge-mcp"));
    }

    if let Some(value) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(value).join(".config").join("forge-mcp"));
    }

    Ok(std::env::current_dir()?.join(".forge-mcp"))
}

fn env_url(name: &str) -> anyhow::Result<Option<String>> {
    let Ok(value) = std::env::var(name) else {
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
    if name == "FORGE_MCP_PUBLIC_BASE_URL" && parsed.scheme() != "https" {
        let is_loopback = parsed
            .host_str()
            .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "::1"));
        if !is_loopback {
            bail!("{name} must use https for non-loopback deployments");
        }
    }

    Ok(Some(value))
}

fn env_bool(name: &str, default: bool) -> anyhow::Result<bool> {
    let Ok(value) = std::env::var(name) else {
        return Ok(default);
    };
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => bail!("{name} must be true or false"),
    }
}

fn env_u64(name: &str, default: u64) -> anyhow::Result<u64> {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<u64>()
            .with_context(|| format!("{name} must be an unsigned integer")),
        Err(_) => Ok(default),
    }
}

fn env_usize(name: &str, default: usize) -> anyhow::Result<usize> {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<usize>()
            .with_context(|| format!("{name} must be a positive integer")),
        Err(_) => Ok(default),
    }
}

fn validate_admin_pin(pin: Option<&str>) -> anyhow::Result<()> {
    let weak = ["", "change-me", "change-me-long-random-pin"];
    let value = pin.unwrap_or("").trim();
    if value.len() < 8 || weak.contains(&value) {
        bail!(
            "FORGE_MCP_OAUTH_ADMIN_PIN must be a non-placeholder value of at least 8 characters when FORGE_MCP_PUBLIC_BASE_URL is configured"
        );
    }
    Ok(())
}

fn validate_jwt_secret(secret: &str) -> anyhow::Result<()> {
    let weak = ["", "change-me", "dev-change-me"];
    if secret.as_bytes().len() < 32 || weak.contains(&secret) {
        bail!("FORGE_MCP_OAUTH_JWT_SECRET must contain at least 32 bytes of strong random data");
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
    fs::write(&path, &secret)
        .with_context(|| format!("failed to write {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to secure {}", path.display()))?;
    }

    Ok(secret)
}
