use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    fs,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use url::Url;
use uuid::Uuid;

use crate::{
    auth::{Principal, jwt},
    config::OAuthConfig,
};

pub const ALL_OAUTH_SCOPES: [&str; 4] =
    ["forge:read", "forge:write", "forge:execute", "forge:audit"];

const MAX_OAUTH_CLIENTS: usize = 256;
const MAX_OAUTH_CODES: usize = 1024;
const PIN_FAILURE_LIMIT: usize = 5;
const PIN_FAILURE_WINDOW_S: u64 = 5 * 60;

#[derive(Debug, Clone, Deserialize)]
pub struct RegistrationRequest {
    pub redirect_uris: Vec<String>,
    pub client_name: Option<String>,
    pub grant_types: Option<Vec<String>>,
    pub response_types: Option<Vec<String>>,
    pub token_endpoint_auth_method: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegistrationResponse {
    pub client_id: String,
    pub client_id_issued_at: u64,
    pub redirect_uris: Vec<String>,
    pub client_name: Option<String>,
    pub grant_types: Vec<String>,
    pub response_types: Vec<String>,
    pub token_endpoint_auth_method: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthorizeRequest {
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub state: Option<String>,
    pub scope: Option<String>,
    pub resource: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AuthorizeDetails {
    pub client_name: String,
    pub scope: String,
    pub resource: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TokenRequest {
    pub grant_type: String,
    pub code: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_verifier: String,
    pub resource: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct OAuthError {
    pub error: String,
    pub description: String,
    pub status: u16,
    pub retry_after: Option<u64>,
}

impl OAuthError {
    fn bad_request(error: &str, description: impl Into<String>) -> Self {
        Self {
            error: error.to_string(),
            description: description.into(),
            status: 400,
            retry_after: None,
        }
    }

    fn temporarily_unavailable(description: impl Into<String>) -> Self {
        Self {
            error: "temporarily_unavailable".to_string(),
            description: description.into(),
            status: 503,
            retry_after: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OAuthClient {
    client_id: String,
    redirect_uris: Vec<String>,
    client_name: Option<String>,
    approved: bool,
    created_at: u64,
}

#[derive(Debug, Clone)]
struct AuthorizationCode {
    client_id: String,
    redirect_uri: String,
    scope: String,
    resource: String,
    code_challenge: String,
    created_at: u64,
}

#[derive(Debug, Default)]
struct OAuthState {
    clients: HashMap<String, OAuthClient>,
    codes: HashMap<String, AuthorizationCode>,
    pin_failures: HashMap<String, VecDeque<u64>>,
}

#[derive(Clone)]
pub struct OAuthService {
    config: OAuthConfig,
    state: Arc<Mutex<OAuthState>>,
}

impl OAuthService {
    pub fn new(config: OAuthConfig) -> anyhow::Result<Self> {
        let clients = load_clients(&config)?;
        Ok(Self {
            config,
            state: Arc::new(Mutex::new(OAuthState {
                clients,
                ..OAuthState::default()
            })),
        })
    }

    pub fn enabled(&self) -> bool {
        self.config.enabled()
    }

    pub fn bypass_localhost(&self) -> bool {
        self.config.auth_bypass_localhost
    }

    pub fn public_base_url(&self) -> Option<&str> {
        self.config.public_base_url.as_deref()
    }

    pub fn issuer(&self, request_base_url: &str) -> String {
        self.config
            .oauth_issuer
            .as_deref()
            .unwrap_or(request_base_url)
            .trim_end_matches('/')
            .to_string()
    }

    pub fn resource(&self, request_base_url: &str) -> String {
        self.config
            .oauth_resource
            .as_deref()
            .unwrap_or(request_base_url)
            .trim_end_matches('/')
            .to_string()
    }

    pub fn protected_resource_metadata(&self, request_base_url: &str) -> Value {
        let resource = self.resource(request_base_url);
        json!({
            "resource": resource,
            "authorization_servers": [self.issuer(request_base_url)],
            "scopes_supported": ALL_OAUTH_SCOPES,
            "resource_documentation": format!("{request_base_url}/")
        })
    }

    pub fn authorization_server_metadata(&self, request_base_url: &str) -> Value {
        let issuer = self.issuer(request_base_url);
        json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/oauth/authorize"),
            "token_endpoint": format!("{issuer}/oauth/token"),
            "registration_endpoint": format!("{issuer}/oauth/register"),
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code"],
            "code_challenge_methods_supported": ["S256"],
            "token_endpoint_auth_methods_supported": ["none"],
            "scopes_supported": ALL_OAUTH_SCOPES,
            "authorization_response_iss_parameter_supported": true,
            "resource_parameter_supported": true,
            "client_id_metadata_document_supported": false
        })
    }

    pub fn register(
        &self,
        request: RegistrationRequest,
    ) -> Result<RegistrationResponse, OAuthError> {
        if request.redirect_uris.is_empty() || request.redirect_uris.len() > 10 {
            return Err(OAuthError::bad_request(
                "invalid_redirect_uri",
                "Provide between 1 and 10 redirect_uris",
            ));
        }

        for redirect_uri in &request.redirect_uris {
            validate_redirect_uri(redirect_uri)?;
        }

        if request
            .grant_types
            .as_ref()
            .is_some_and(|values| values.iter().any(|value| value != "authorization_code"))
        {
            return Err(OAuthError::bad_request(
                "invalid_client_metadata",
                "Only grant_types=[authorization_code] is supported",
            ));
        }

        if request
            .response_types
            .as_ref()
            .is_some_and(|values| values.iter().any(|value| value != "code"))
        {
            return Err(OAuthError::bad_request(
                "invalid_client_metadata",
                "Only response_types=[code] is supported",
            ));
        }

        if request
            .token_endpoint_auth_method
            .as_deref()
            .is_some_and(|value| value != "none")
        {
            return Err(OAuthError::bad_request(
                "invalid_client_metadata",
                "Only token_endpoint_auth_method=none is supported",
            ));
        }

        if request
            .client_name
            .as_ref()
            .is_some_and(|name| name.len() > 200)
        {
            return Err(OAuthError::bad_request(
                "invalid_client_metadata",
                "client_name is too long",
            ));
        }

        let now = unix_time();
        let mut state = self
            .state
            .lock()
            .map_err(|_| OAuthError::temporarily_unavailable("OAuth state lock is unavailable"))?;

        if state.clients.len() >= MAX_OAUTH_CLIENTS {
            return Err(OAuthError::temporarily_unavailable(
                "OAuth client registry is full",
            ));
        }

        if let Some(existing) = state.clients.values().find(|client| {
            client.redirect_uris == request.redirect_uris
                && client.client_name == request.client_name
        }) {
            return Ok(registration_response(existing));
        }

        let client = OAuthClient {
            client_id: format!("forge-mcp-{}", random_token()),
            redirect_uris: request.redirect_uris,
            client_name: request.client_name,
            approved: false,
            created_at: now,
        };
        state
            .clients
            .insert(client.client_id.clone(), client.clone());
        persist_clients(&self.config, &state.clients).map_err(|error| {
            OAuthError::temporarily_unavailable(format!("failed to persist OAuth client: {error}"))
        })?;

        Ok(registration_response(&client))
    }

    pub fn validate_authorize(
        &self,
        request: &AuthorizeRequest,
        expected_resource: &str,
    ) -> Result<AuthorizeDetails, OAuthError> {
        if request.response_type != "code" {
            return Err(OAuthError::bad_request(
                "unsupported_response_type",
                "Only response_type=code is supported",
            ));
        }
        if request.code_challenge.trim().is_empty() {
            return Err(OAuthError::bad_request(
                "invalid_request",
                "Missing code_challenge",
            ));
        }
        if request.code_challenge_method != "S256" {
            return Err(OAuthError::bad_request(
                "invalid_request",
                "Only code_challenge_method=S256 is supported",
            ));
        }
        if request
            .resource
            .as_deref()
            .is_some_and(|resource| resource != expected_resource)
        {
            return Err(OAuthError::bad_request(
                "invalid_target",
                "resource does not match this MCP server",
            ));
        }

        let state = self
            .state
            .lock()
            .map_err(|_| OAuthError::temporarily_unavailable("OAuth state lock is unavailable"))?;
        let client = state
            .clients
            .get(&request.client_id)
            .ok_or_else(|| OAuthError::bad_request("invalid_request", "Unknown client_id"))?;
        if !client.redirect_uris.contains(&request.redirect_uri) {
            return Err(OAuthError::bad_request(
                "invalid_request",
                "redirect_uri is not registered for this client",
            ));
        }

        let scopes = normalize_scopes(request.scope.as_deref())?;
        Ok(AuthorizeDetails {
            client_name: client
                .client_name
                .clone()
                .unwrap_or_else(|| "OAuth client".to_string()),
            scope: scopes.join(" "),
            resource: expected_resource.to_string(),
        })
    }

    pub fn authorize(
        &self,
        request: &AuthorizeRequest,
        submitted_pin: &str,
        source: &str,
        issuer: &str,
        expected_resource: &str,
    ) -> Result<String, OAuthError> {
        let details = self.validate_authorize(request, expected_resource)?;
        let now = unix_time();

        let mut state = self
            .state
            .lock()
            .map_err(|_| OAuthError::temporarily_unavailable("OAuth state lock is unavailable"))?;
        prune_codes(&self.config, &mut state, now);
        prune_pin_failures(&mut state, now);

        if let Some(failures) = state.pin_failures.get(source) {
            if failures.len() >= PIN_FAILURE_LIMIT {
                let first = failures.front().copied().unwrap_or(now);
                return Err(OAuthError {
                    error: "access_denied".to_string(),
                    description: "Too many failed PIN attempts. Try again later.".to_string(),
                    status: 429,
                    retry_after: Some(
                        first
                            .saturating_add(PIN_FAILURE_WINDOW_S)
                            .saturating_sub(now)
                            .max(1),
                    ),
                });
            }
        }

        if let Some(expected_pin) = self.config.oauth_admin_pin.as_deref() {
            if !constant_time_eq(submitted_pin, expected_pin) {
                let failures = state.pin_failures.entry(source.to_string()).or_default();
                failures.push_back(now);
                let retry_after = if failures.len() >= PIN_FAILURE_LIMIT {
                    failures
                        .front()
                        .copied()
                        .unwrap_or(now)
                        .saturating_add(PIN_FAILURE_WINDOW_S)
                        .saturating_sub(now)
                        .max(1)
                } else {
                    0
                };

                return Err(OAuthError {
                    error: "access_denied".to_string(),
                    description: if retry_after > 0 {
                        "Too many failed PIN attempts. Try again later.".to_string()
                    } else {
                        "Invalid admin PIN".to_string()
                    },
                    status: if retry_after > 0 { 429 } else { 403 },
                    retry_after: (retry_after > 0).then_some(retry_after),
                });
            }
            state.pin_failures.remove(source);
        }

        let client = state
            .clients
            .get_mut(&request.client_id)
            .ok_or_else(|| OAuthError::bad_request("invalid_request", "Unknown client_id"))?;
        client.approved = true;

        if state.codes.len() >= MAX_OAUTH_CODES {
            return Err(OAuthError::temporarily_unavailable(
                "Too many pending authorization requests",
            ));
        }

        let code = random_token();
        state.codes.insert(
            code.clone(),
            AuthorizationCode {
                client_id: request.client_id.clone(),
                redirect_uri: request.redirect_uri.clone(),
                scope: details.scope,
                resource: details.resource,
                code_challenge: request.code_challenge.clone(),
                created_at: now,
            },
        );

        persist_clients(&self.config, &state.clients).map_err(|error| {
            OAuthError::temporarily_unavailable(format!(
                "failed to persist OAuth client approval: {error}"
            ))
        })?;

        let mut redirect = Url::parse(&request.redirect_uri)
            .map_err(|_| OAuthError::bad_request("invalid_request", "Invalid redirect_uri"))?;
        {
            let mut query = redirect.query_pairs_mut();
            query.append_pair("code", &code);
            query.append_pair("iss", issuer);
            if let Some(state_value) = request.state.as_deref() {
                query.append_pair("state", state_value);
            }
        }

        Ok(redirect.to_string())
    }

    pub fn exchange(
        &self,
        request: TokenRequest,
        issuer: &str,
        expected_resource: &str,
    ) -> Result<TokenResponse, OAuthError> {
        if request.grant_type != "authorization_code" {
            return Err(OAuthError::bad_request(
                "unsupported_grant_type",
                "Only grant_type=authorization_code is supported",
            ));
        }

        let now = unix_time();
        let mut state = self
            .state
            .lock()
            .map_err(|_| OAuthError::temporarily_unavailable("OAuth state lock is unavailable"))?;
        prune_codes(&self.config, &mut state, now);

        let code = state.codes.get(&request.code).cloned().ok_or_else(|| {
            OAuthError::bad_request("invalid_grant", "Unknown, used, or expired code")
        })?;

        if code.client_id != request.client_id || code.redirect_uri != request.redirect_uri {
            return Err(OAuthError::bad_request(
                "invalid_grant",
                "Client or redirect mismatch",
            ));
        }

        if code.resource != expected_resource
            || request
                .resource
                .as_deref()
                .is_some_and(|resource| resource != code.resource)
        {
            return Err(OAuthError::bad_request(
                "invalid_target",
                "resource does not match the authorization grant",
            ));
        }

        if !verify_pkce(&code.code_challenge, &request.code_verifier) {
            return Err(OAuthError::bad_request(
                "invalid_grant",
                "PKCE verification failed",
            ));
        }

        state.codes.remove(&request.code);

        let claims = jwt::AccessTokenClaims {
            iss: issuer.to_string(),
            sub: "local-user".to_string(),
            aud: code.resource,
            iat: now,
            client_id: request.client_id,
            scope: code.scope.clone(),
            exp: (self.config.oauth_access_token_ttl_s > 0)
                .then_some(now.saturating_add(self.config.oauth_access_token_ttl_s)),
        };
        let access_token = jwt::issue(&self.config.oauth_jwt_secret, &claims).map_err(|error| {
            OAuthError::temporarily_unavailable(format!("failed to issue access token: {error}"))
        })?;

        Ok(TokenResponse {
            access_token,
            token_type: "Bearer".to_string(),
            scope: code.scope,
            expires_in: (self.config.oauth_access_token_ttl_s > 0)
                .then_some(self.config.oauth_access_token_ttl_s),
        })
    }

    pub fn verify_bearer(
        &self,
        token: &str,
        issuer: &str,
        resource: &str,
        resource_metadata_url: String,
    ) -> Result<Principal, String> {
        let claims = jwt::verify(
            &self.config.oauth_jwt_secret,
            token,
            issuer,
            resource,
            unix_time(),
        )?;

        Ok(Principal {
            subject: claims.sub,
            client_id: claims.client_id,
            scopes: claims
                .scope
                .split_whitespace()
                .map(ToString::to_string)
                .collect::<BTreeSet<_>>(),
            resource_metadata_url,
        })
    }
}

fn registration_response(client: &OAuthClient) -> RegistrationResponse {
    RegistrationResponse {
        client_id: client.client_id.clone(),
        client_id_issued_at: client.created_at,
        redirect_uris: client.redirect_uris.clone(),
        client_name: client.client_name.clone(),
        grant_types: vec!["authorization_code".to_string()],
        response_types: vec!["code".to_string()],
        token_endpoint_auth_method: "none".to_string(),
    }
}

fn validate_redirect_uri(value: &str) -> Result<(), OAuthError> {
    if value.is_empty() || value.len() > 2048 {
        return Err(OAuthError::bad_request(
            "invalid_redirect_uri",
            "redirect_uri is empty or too long",
        ));
    }

    let parsed = Url::parse(value).map_err(|_| {
        OAuthError::bad_request(
            "invalid_redirect_uri",
            "redirect_uri must be an absolute URI",
        )
    })?;
    if parsed.fragment().is_some() {
        return Err(OAuthError::bad_request(
            "invalid_redirect_uri",
            "redirect_uri must not contain a fragment",
        ));
    }
    if matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_none() {
        return Err(OAuthError::bad_request(
            "invalid_redirect_uri",
            "HTTP redirect_uri must include a host",
        ));
    }

    Ok(())
}

fn normalize_scopes(scope: Option<&str>) -> Result<Vec<String>, OAuthError> {
    let requested = match scope {
        Some(value) if !value.trim().is_empty() => value
            .split_whitespace()
            .map(ToString::to_string)
            .collect::<BTreeSet<_>>(),
        _ => ALL_OAUTH_SCOPES
            .iter()
            .map(|scope| scope.to_string())
            .collect::<BTreeSet<_>>(),
    };

    let supported = ALL_OAUTH_SCOPES.iter().copied().collect::<BTreeSet<_>>();
    if let Some(unsupported) = requested
        .iter()
        .find(|scope| !supported.contains(scope.as_str()))
    {
        return Err(OAuthError::bad_request(
            "invalid_scope",
            format!("Unsupported scope: {unsupported}"),
        ));
    }

    Ok(requested.into_iter().collect())
}

fn verify_pkce(expected_challenge: &str, verifier: &str) -> bool {
    if verifier.is_empty() {
        return false;
    }
    let digest = Sha256::digest(verifier.as_bytes());
    let actual = URL_SAFE_NO_PAD.encode(digest);
    constant_time_eq(&actual, expected_challenge)
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    left.len() == right.len() && bool::from(left.as_bytes().ct_eq(right.as_bytes()))
}

fn random_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn prune_codes(config: &OAuthConfig, state: &mut OAuthState, now: u64) {
    state
        .codes
        .retain(|_, code| now.saturating_sub(code.created_at) <= config.oauth_code_ttl_s.max(1));
}

fn prune_pin_failures(state: &mut OAuthState, now: u64) {
    let cutoff = now.saturating_sub(PIN_FAILURE_WINDOW_S);
    state.pin_failures.retain(|_, failures| {
        while failures.front().is_some_and(|value| *value <= cutoff) {
            failures.pop_front();
        }
        !failures.is_empty()
    });
}

fn load_clients(config: &OAuthConfig) -> anyhow::Result<HashMap<String, OAuthClient>> {
    let path = config.state_dir.join("oauth-clients.json");
    if !path.exists() {
        return Ok(HashMap::new());
    }

    let bytes = fs::read(path)?;
    let clients = serde_json::from_slice::<Vec<OAuthClient>>(&bytes)?;
    Ok(clients
        .into_iter()
        .map(|client| (client.client_id.clone(), client))
        .collect())
}

fn persist_clients(
    config: &OAuthConfig,
    clients: &HashMap<String, OAuthClient>,
) -> anyhow::Result<()> {
    fs::create_dir_all(&config.state_dir)?;

    let path = config.state_dir.join("oauth-clients.json");
    let temporary = config.state_dir.join("oauth-clients.json.tmp");
    let mut values = clients.values().cloned().collect::<Vec<_>>();
    values.sort_by(|left, right| left.client_id.cmp(&right.client_id));

    fs::write(&temporary, serde_json::to_vec_pretty(&values)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthMode, OAuthConfig};

    fn service() -> OAuthService {
        let dir = std::env::temp_dir().join(format!("forge-mcp-oauth-test-{}", Uuid::new_v4()));
        let config = OAuthConfig {
            auth_mode: AuthMode::OAuth,
            auth_bypass_localhost: false,
            public_base_url: Some("https://mcp.example".to_string()),
            oauth_issuer: None,
            oauth_resource: None,
            oauth_admin_pin: Some("12345678".to_string()),
            oauth_jwt_secret: "01234567890123456789012345678901".to_string(),
            oauth_access_token_ttl_s: 0,
            oauth_code_ttl_s: 300,
            state_dir: dir,
        };
        OAuthService::new(config).unwrap()
    }

    #[test]
    fn registration_authorization_and_pkce_exchange_work() {
        let service = service();
        let registration = service
            .register(RegistrationRequest {
                redirect_uris: vec![
                    "https://chatgpt.com/connector_platform_oauth_redirect".to_string(),
                ],
                client_name: Some("ChatGPT".to_string()),
                grant_types: None,
                response_types: None,
                token_endpoint_auth_method: None,
            })
            .unwrap();

        let verifier = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~";
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let authorize = AuthorizeRequest {
            response_type: "code".to_string(),
            client_id: registration.client_id.clone(),
            redirect_uri: registration.redirect_uris[0].clone(),
            code_challenge: challenge,
            code_challenge_method: "S256".to_string(),
            state: Some("state-1".to_string()),
            scope: Some("forge:read".to_string()),
            resource: Some("https://mcp.example".to_string()),
        };

        let redirect = service
            .authorize(
                &authorize,
                "12345678",
                "127.0.0.1",
                "https://mcp.example",
                "https://mcp.example",
            )
            .unwrap();
        let redirect = Url::parse(&redirect).unwrap();
        let code = redirect
            .query_pairs()
            .find(|(key, _)| key == "code")
            .map(|(_, value)| value.to_string())
            .unwrap();

        let token = service
            .exchange(
                TokenRequest {
                    grant_type: "authorization_code".to_string(),
                    code,
                    client_id: registration.client_id,
                    redirect_uri: authorize.redirect_uri,
                    code_verifier: verifier.to_string(),
                    resource: Some("https://mcp.example".to_string()),
                },
                "https://mcp.example",
                "https://mcp.example",
            )
            .unwrap();

        let principal = service
            .verify_bearer(
                &token.access_token,
                "https://mcp.example",
                "https://mcp.example",
                "https://mcp.example/.well-known/oauth-protected-resource".to_string(),
            )
            .unwrap();
        assert!(principal.scopes.contains("forge:read"));
    }
}
