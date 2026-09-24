use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Arc,
};

use axum::{
    Form, Json, Router,
    body::Bytes,
    extract::{ConnectInfo, Query, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{ACCEPT, ALLOW, AUTHORIZATION, CONTENT_TYPE, HOST, LOCATION, ORIGIN},
    },
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info};
use uuid::Uuid;

use crate::{
    app::ForgeMcp,
    auth::{
        AuthContext, OAuthService,
        oauth::{AuthorizeRequest, OAuthError, RegistrationRequest, TokenRequest},
    },
    config::AuthMode,
    protocol::{
        jsonrpc::{JsonRpcMessage, JsonRpcResponse},
        mcp::PROTOCOL_VERSION,
    },
    server::ForgeServer,
};

const SESSION_HEADER: &str = "mcp-session-id";
const PROTOCOL_VERSION_HEADER: &str = "mcp-protocol-version";

type Session = Arc<Mutex<SessionEntry>>;

struct SessionEntry {
    server: ForgeServer,
    owner: Option<String>,
}

#[derive(Clone)]
struct HttpState {
    app: ForgeMcp,
    oauth: OAuthService,
    sessions: Arc<RwLock<HashMap<String, Session>>>,
    allowed_origins: Arc<Vec<String>>,
}

pub async fn serve(app: ForgeMcp, host: &str, port: u16) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind((host, port)).await?;
    let local_addr = listener.local_addr()?;
    let oauth = OAuthService::new(app.oauth_config().clone())?;
    let oauth_enabled = app.auth_mode() == AuthMode::OAuth;

    let state = HttpState {
        app,
        oauth,
        sessions: Arc::new(RwLock::new(HashMap::new())),
        allowed_origins: Arc::new(allowed_origins(host, port)),
    };

    let router = Router::new()
        .route("/health", get(health))
        .route("/mcp", post(post_mcp).get(get_mcp).delete(delete_mcp))
        .route(
            "/.well-known/oauth-protected-resource",
            get(oauth_protected_resource),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(oauth_server_metadata),
        )
        .route("/oauth/register", post(oauth_register))
        .route(
            "/oauth/authorize",
            get(oauth_authorize_get).post(oauth_authorize_post),
        )
        .route("/oauth/token", post(oauth_token))
        .with_state(state);

    eprintln!("ForgeMCP listening on http://{local_addr} (MCP endpoint: http://{local_addr}/mcp)");

    info!(
        address = %local_addr,
        endpoint = %format!("http://{local_addr}/mcp"),
        oauth = oauth_enabled,
        "ForgeMCP Streamable HTTP transport listening"
    );

    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

async fn health() -> Response {
    (
        StatusCode::OK,
        Json(json!({
            "status": "ok"
        })),
    )
        .into_response()
}

async fn post_mcp(
    State(state): State<HttpState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(response) = validate_origin(&state, &headers) {
        return response;
    }

    if let Err(response) = validate_post_headers(&headers) {
        return response;
    }

    let auth = match authenticate(&state, peer, &headers) {
        Ok(auth) => auth,
        Err(response) => return response,
    };

    let value = match serde_json::from_slice::<Value>(&body) {
        Ok(value) => value,
        Err(_) => {
            return json_rpc_response(
                StatusCode::BAD_REQUEST,
                JsonRpcResponse::parse_error(),
                None,
            );
        }
    };

    if !value.is_object() {
        return json_rpc_response(
            StatusCode::BAD_REQUEST,
            JsonRpcResponse::invalid_request("HTTP transport expects one JSON-RPC object"),
            None,
        );
    }

    if value.get("method").is_none() {
        if is_jsonrpc_response(&value) {
            return StatusCode::ACCEPTED.into_response();
        }

        return json_rpc_response(
            StatusCode::BAD_REQUEST,
            JsonRpcResponse::invalid_request(
                "message must be a request, notification, or response",
            ),
            None,
        );
    }

    let message = match serde_json::from_value::<JsonRpcMessage>(value) {
        Ok(message) => message,
        Err(error) => {
            return json_rpc_response(
                StatusCode::BAD_REQUEST,
                JsonRpcResponse::invalid_request(error.to_string()),
                None,
            );
        }
    };

    if message.method == "initialize" {
        return initialize_session(state, headers, message, auth).await;
    }

    let session_id = match required_header(&headers, SESSION_HEADER) {
        Ok(value) => value.to_string(),
        Err(response) => return response,
    };

    if let Some(version) = optional_header(&headers, PROTOCOL_VERSION_HEADER) {
        if version != PROTOCOL_VERSION {
            return plain_response(
                StatusCode::BAD_REQUEST,
                format!("unsupported MCP protocol version: {version}"),
            );
        }
    }

    let session = {
        let sessions = state.sessions.read().await;
        sessions.get(&session_id).cloned()
    };

    let Some(session) = session else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let is_notification = message.is_notification();
    let response = {
        let mut entry = session.lock().await;
        if entry.owner != auth.identity_key() {
            return plain_response(
                StatusCode::FORBIDDEN,
                "MCP session belongs to a different authenticated client",
            );
        }
        entry.server.handle_with_auth(message, &auth).await
    };

    if is_notification {
        return match response {
            None => StatusCode::ACCEPTED.into_response(),
            Some(response) => json_rpc_response(StatusCode::BAD_REQUEST, response, None),
        };
    }

    match response {
        Some(response) => json_rpc_response(StatusCode::OK, response, None),
        None => json_rpc_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            JsonRpcResponse::error(
                crate::protocol::jsonrpc::JsonRpcId::Null,
                crate::protocol::jsonrpc::INTERNAL_ERROR,
                "Internal error",
                None,
            ),
            None,
        ),
    }
}

async fn initialize_session(
    state: HttpState,
    headers: HeaderMap,
    message: JsonRpcMessage,
    auth: AuthContext,
) -> Response {
    if headers.contains_key(SESSION_HEADER) {
        return plain_response(
            StatusCode::BAD_REQUEST,
            "initialize must not reuse an MCP-Session-Id",
        );
    }

    let mut server = ForgeServer::new(state.app.clone());
    let response = server.handle_with_auth(message, &auth).await;

    let Some(response) = response else {
        return plain_response(
            StatusCode::BAD_REQUEST,
            "initialize must be a JSON-RPC request",
        );
    };

    if matches!(response, JsonRpcResponse::Error(_)) {
        return json_rpc_response(StatusCode::OK, response, None);
    }

    let session_id = Uuid::new_v4().to_string();
    state.sessions.write().await.insert(
        session_id.clone(),
        Arc::new(Mutex::new(SessionEntry {
            server,
            owner: auth.identity_key(),
        })),
    );

    debug!(session_id, "created MCP HTTP session");

    json_rpc_response(StatusCode::OK, response, Some(&session_id))
}

async fn get_mcp(
    State(state): State<HttpState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = validate_origin(&state, &headers) {
        return response;
    }
    if let Err(response) = authenticate(&state, peer, &headers) {
        return response;
    }

    let mut response = StatusCode::METHOD_NOT_ALLOWED.into_response();
    response
        .headers_mut()
        .insert(ALLOW, HeaderValue::from_static("POST, DELETE"));
    response
}

async fn delete_mcp(
    State(state): State<HttpState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = validate_origin(&state, &headers) {
        return response;
    }
    let auth = match authenticate(&state, peer, &headers) {
        Ok(auth) => auth,
        Err(response) => return response,
    };

    let session_id = match required_header(&headers, SESSION_HEADER) {
        Ok(value) => value.to_string(),
        Err(response) => return response,
    };

    let session = {
        let sessions = state.sessions.read().await;
        sessions.get(&session_id).cloned()
    };
    let Some(session) = session else {
        return StatusCode::NOT_FOUND.into_response();
    };

    if session.lock().await.owner != auth.identity_key() {
        return plain_response(
            StatusCode::FORBIDDEN,
            "MCP session belongs to a different authenticated client",
        );
    }

    state.sessions.write().await.remove(&session_id);
    debug!(session_id, "terminated MCP HTTP session");
    StatusCode::NO_CONTENT.into_response()
}

async fn oauth_protected_resource(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    if !state.oauth.enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }
    oauth_json(
        StatusCode::OK,
        state
            .oauth
            .protected_resource_metadata(&request_base_url(&state, &headers)),
    )
}

async fn oauth_server_metadata(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    if !state.oauth.enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }
    oauth_json(
        StatusCode::OK,
        state
            .oauth
            .authorization_server_metadata(&request_base_url(&state, &headers)),
    )
}

async fn oauth_register(
    State(state): State<HttpState>,
    Json(request): Json<RegistrationRequest>,
) -> Response {
    if !state.oauth.enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }

    match state.oauth.register(request) {
        Ok(response) => oauth_json(StatusCode::CREATED, response),
        Err(error) => oauth_error_response(error),
    }
}

async fn oauth_authorize_get(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(request): Query<AuthorizeRequest>,
) -> Response {
    if !state.oauth.enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let base = request_base_url(&state, &headers);
    let resource = state.oauth.resource(&base);
    match state.oauth.validate_authorize(&request, &resource) {
        Ok(details) => {
            authorization_page(&request, &details.client_name, &details.scope, None, 200)
        }
        Err(error) => authorization_page(
            &request,
            "OAuth client",
            request.scope.as_deref().unwrap_or(""),
            Some(&error.description),
            error.status,
        ),
    }
}

#[derive(Debug, Deserialize)]
struct AuthorizeForm {
    response_type: String,
    client_id: String,
    redirect_uri: String,
    code_challenge: String,
    code_challenge_method: String,
    state: Option<String>,
    scope: Option<String>,
    resource: Option<String>,
    pin: Option<String>,
}

impl AuthorizeForm {
    fn request(&self) -> AuthorizeRequest {
        AuthorizeRequest {
            response_type: self.response_type.clone(),
            client_id: self.client_id.clone(),
            redirect_uri: self.redirect_uri.clone(),
            code_challenge: self.code_challenge.clone(),
            code_challenge_method: self.code_challenge_method.clone(),
            state: self.state.clone(),
            scope: self.scope.clone(),
            resource: self.resource.clone(),
        }
    }
}

async fn oauth_authorize_post(
    State(state): State<HttpState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(form): Form<AuthorizeForm>,
) -> Response {
    if !state.oauth.enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let request = form.request();
    let base = request_base_url(&state, &headers);
    let issuer = state.oauth.issuer(&base);
    let resource = state.oauth.resource(&base);

    match state.oauth.authorize(
        &request,
        form.pin.as_deref().unwrap_or(""),
        &peer.ip().to_string(),
        &issuer,
        &resource,
    ) {
        Ok(location) => {
            let mut response = StatusCode::FOUND.into_response();
            if let Ok(value) = HeaderValue::from_str(&location) {
                response.headers_mut().insert(LOCATION, value);
            }
            response
        }
        Err(error) => {
            let client_name = state
                .oauth
                .validate_authorize(&request, &resource)
                .map(|details| details.client_name)
                .unwrap_or_else(|_| "OAuth client".to_string());
            let mut response = authorization_page(
                &request,
                &client_name,
                request.scope.as_deref().unwrap_or(""),
                Some(&error.description),
                error.status,
            );
            if let Some(retry_after) = error.retry_after {
                if let Ok(value) = HeaderValue::from_str(&retry_after.to_string()) {
                    response.headers_mut().insert("retry-after", value);
                }
            }
            response
        }
    }
}

async fn oauth_token(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Form(request): Form<TokenRequest>,
) -> Response {
    if !state.oauth.enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let base = request_base_url(&state, &headers);
    let issuer = state.oauth.issuer(&base);
    let resource = state.oauth.resource(&base);

    match state.oauth.exchange(request, &issuer, &resource) {
        Ok(response) => oauth_json(StatusCode::OK, response),
        Err(error) => oauth_error_response(error),
    }
}

fn authenticate(
    state: &HttpState,
    peer: SocketAddr,
    headers: &HeaderMap,
) -> Result<AuthContext, Response> {
    if state.app.auth_mode() == AuthMode::None {
        return Ok(AuthContext::Unrestricted);
    }

    if state.oauth.bypass_localhost() && is_direct_localhost_request(peer, headers) {
        return Ok(AuthContext::Unrestricted);
    }

    let base = request_base_url(state, headers);
    let issuer = state.oauth.issuer(&base);
    let resource = state.oauth.resource(&base);
    let metadata_url = format!("{resource}/.well-known/oauth-protected-resource");

    let token = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value
                .strip_prefix("Bearer ")
                .or_else(|| value.strip_prefix("bearer "))
        })
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let Some(token) = token else {
        return Err(unauthorized_response(
            &metadata_url,
            "invalid_token",
            "No OAuth bearer token was provided",
        ));
    };

    state
        .oauth
        .verify_bearer(token, &issuer, &resource, metadata_url.clone())
        .map(AuthContext::OAuth)
        .map_err(|error| {
            unauthorized_response(
                &metadata_url,
                "invalid_token",
                &format!("OAuth bearer token is invalid: {error}"),
            )
        })
}

fn unauthorized_response(metadata_url: &str, error: &str, description: &str) -> Response {
    let challenge = format!(
        "Bearer resource_metadata=\"{metadata_url}\", error=\"{error}\", error_description=\"{}\", scope=\"{}\"",
        sanitize_header_value(description),
        crate::auth::ALL_OAUTH_SCOPES.join(" ")
    );

    let mut response = (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": "unauthorized",
            "message": description
        })),
    )
        .into_response();
    if let Ok(value) = HeaderValue::from_str(&challenge) {
        response.headers_mut().insert("www-authenticate", value);
    }
    response
}

fn validate_post_headers(headers: &HeaderMap) -> Result<(), Response> {
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();

    if !content_type
        .to_ascii_lowercase()
        .starts_with("application/json")
    {
        return Err(plain_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Content-Type must be application/json",
        ));
    }

    let accept = headers
        .get(ACCEPT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if !accept.contains("application/json") || !accept.contains("text/event-stream") {
        return Err(plain_response(
            StatusCode::NOT_ACCEPTABLE,
            "Accept must include application/json and text/event-stream",
        ));
    }

    Ok(())
}

fn validate_origin(state: &HttpState, headers: &HeaderMap) -> Result<(), Response> {
    let Some(origin) = optional_header(headers, ORIGIN.as_str()) else {
        return Ok(());
    };

    if state
        .allowed_origins
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(origin))
        || state
            .oauth
            .public_base_url()
            .is_some_and(|base| base.eq_ignore_ascii_case(origin.trim_end_matches('/')))
    {
        Ok(())
    } else {
        Err(plain_response(StatusCode::FORBIDDEN, "invalid Origin"))
    }
}

fn request_base_url(state: &HttpState, headers: &HeaderMap) -> String {
    if let Some(value) = state.oauth.public_base_url() {
        return value.trim_end_matches('/').to_string();
    }

    let scheme = optional_header(headers, "x-forwarded-proto").unwrap_or("http");
    let host = optional_header(headers, "x-forwarded-host")
        .or_else(|| optional_header(headers, HOST.as_str()))
        .unwrap_or("127.0.0.1:8765");

    format!("{scheme}://{host}")
        .trim_end_matches('/')
        .to_string()
}

fn is_direct_localhost_request(peer: SocketAddr, headers: &HeaderMap) -> bool {
    if !peer.ip().is_loopback() || !host_header_is_loopback(headers) {
        return false;
    }

    let forwarded = [
        "forwarded",
        "x-forwarded-for",
        "x-forwarded-host",
        "x-forwarded-proto",
        "x-real-ip",
    ];
    !forwarded.iter().any(|name| headers.contains_key(*name))
}

fn host_header_is_loopback(headers: &HeaderMap) -> bool {
    let Some(value) = optional_header(headers, HOST.as_str()) else {
        return false;
    };

    let host = if let Some(rest) = value.strip_prefix('[') {
        rest.split(']').next().unwrap_or_default()
    } else {
        value.split(':').next().unwrap_or_default()
    };

    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }

    host.parse::<IpAddr>()
        .is_ok_and(|address| address.is_loopback())
}

fn authorization_page(
    request: &AuthorizeRequest,
    client_name: &str,
    scope: &str,
    error: Option<&str>,
    status: u16,
) -> Response {
    let scope = if scope.trim().is_empty() {
        crate::auth::ALL_OAUTH_SCOPES.join(" ")
    } else {
        scope.to_string()
    };
    let permissions = scope
        .split_whitespace()
        .map(scope_description)
        .map(|(name, description)| {
            format!(
                "<li><strong>{}</strong><br><span>{}</span></li>",
                html_escape(name),
                html_escape(description)
            )
        })
        .collect::<Vec<_>>()
        .join("");

    let error_html = error
        .map(|value| {
            format!(
                "<div class=\"error\"><strong>Authorization failed</strong><br>{}</div>",
                html_escape(value)
            )
        })
        .unwrap_or_default();

    let hidden = [
        ("response_type", Some(request.response_type.as_str())),
        ("client_id", Some(request.client_id.as_str())),
        ("redirect_uri", Some(request.redirect_uri.as_str())),
        ("code_challenge", Some(request.code_challenge.as_str())),
        (
            "code_challenge_method",
            Some(request.code_challenge_method.as_str()),
        ),
        ("state", request.state.as_deref()),
        ("scope", Some(scope.as_str())),
        ("resource", request.resource.as_deref()),
    ]
    .into_iter()
    .filter_map(|(name, value)| value.map(|value| (name, value)))
    .map(|(name, value)| {
        format!(
            "<input type=\"hidden\" name=\"{}\" value=\"{}\">",
            html_escape(name),
            html_escape(value)
        )
    })
    .collect::<Vec<_>>()
    .join("");

    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Authorize ForgeMCP</title><style>body{{font-family:system-ui,sans-serif;max-width:680px;margin:48px auto;padding:0 20px;color:#1f2937}}.card{{border:1px solid #d1d5db;border-radius:14px;padding:28px}}h1{{margin-top:0}}li{{margin:12px 0}}span{{color:#4b5563}}label{{display:block;margin:24px 0 8px}}input[type=password]{{box-sizing:border-box;width:100%;padding:12px;border:1px solid #9ca3af;border-radius:8px}}button{{margin-top:18px;padding:12px 18px;border:0;border-radius:8px;background:#111827;color:white;font-weight:600}}.error{{background:#fef2f2;color:#991b1b;padding:12px;border-radius:8px;margin-bottom:18px}}</style></head><body><div class=\"card\"><h1>Authorize ForgeMCP</h1>{error_html}<p><strong>{}</strong> is requesting access to this ForgeMCP server.</p><ul>{permissions}</ul><form method=\"post\" action=\"/oauth/authorize\">{hidden}<label for=\"pin\">Admin PIN</label><input id=\"pin\" type=\"password\" name=\"pin\" autocomplete=\"current-password\" placeholder=\"Enter FORGE_MCP_OAUTH_ADMIN_PIN\"><button type=\"submit\">Approve</button></form><p><small>Only approve this request if you initiated it from a trusted MCP client.</small></p></div></body></html>",
        html_escape(client_name)
    );

    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST);
    let mut response = (status, Html(html)).into_response();
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
}

fn scope_description(scope: &str) -> (&str, &str) {
    match scope {
        "forge:read" => ("Read workspace", "Inspect files and workspace information."),
        "forge:write" => ("Change workspace", "Create, edit, patch, or delete files."),
        "forge:execute" => ("Run commands", "Execute shell commands in the workspace."),
        "forge:audit" => ("Read audit log", "Inspect recent ForgeMCP actions."),
        value => (value, "Permission requested by the OAuth client."),
    }
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn oauth_error_response(error: OAuthError) -> Response {
    let status = StatusCode::from_u16(error.status).unwrap_or(StatusCode::BAD_REQUEST);
    let mut response = oauth_json(
        status,
        json!({
            "error": error.error,
            "error_description": error.description
        }),
    );
    if let Some(retry_after) = error.retry_after {
        if let Ok(value) = HeaderValue::from_str(&retry_after.to_string()) {
            response.headers_mut().insert("retry-after", value);
        }
    }
    response
}

fn oauth_json(status: StatusCode, value: impl serde::Serialize) -> Response {
    let mut response = (status, Json(value)).into_response();
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
}

fn sanitize_header_value(value: &str) -> String {
    value.replace(['\r', '\n', '"'], " ")
}

fn required_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, Response> {
    optional_header(headers, name).ok_or_else(|| {
        plain_response(
            StatusCode::BAD_REQUEST,
            format!("missing required header: {name}"),
        )
    })
}

fn optional_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

fn is_jsonrpc_response(value: &Value) -> bool {
    value.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
        && value.get("id").is_some()
        && (value.get("result").is_some() || value.get("error").is_some())
}

fn json_rpc_response(
    status: StatusCode,
    response: JsonRpcResponse,
    session_id: Option<&str>,
) -> Response {
    let mut response = (status, Json(response)).into_response();

    if let Some(session_id) = session_id {
        if let Ok(value) = HeaderValue::from_str(session_id) {
            response.headers_mut().insert(SESSION_HEADER, value);
        }
    }

    response
}

fn plain_response(status: StatusCode, message: impl Into<String>) -> Response {
    (status, message.into()).into_response()
}

fn allowed_origins(host: &str, port: u16) -> Vec<String> {
    let mut origins = vec![
        format!("http://{host}:{port}"),
        format!("https://{host}:{port}"),
    ];

    if matches!(host, "127.0.0.1" | "::1" | "localhost") {
        origins.extend([
            format!("http://127.0.0.1:{port}"),
            format!("http://localhost:{port}"),
            format!("http://[::1]:{port}"),
            format!("https://127.0.0.1:{port}"),
            format!("https://localhost:{port}"),
            format!("https://[::1]:{port}"),
        ]);
    }

    origins.sort();
    origins.dedup();
    origins
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    fn state() -> HttpState {
        let app = ForgeMcp::new(AppConfig::test(std::env::current_dir().unwrap()));
        HttpState {
            oauth: OAuthService::new(app.oauth_config().clone()).unwrap(),
            app,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            allowed_origins: Arc::new(allowed_origins("127.0.0.1", 8765)),
        }
    }

    #[tokio::test]
    async fn health_check_returns_ok() {
        let response = health().await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn accepts_missing_origin_for_non_browser_clients() {
        assert!(validate_origin(&state(), &HeaderMap::new()).is_ok());
    }

    #[test]
    fn rejects_untrusted_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(ORIGIN, HeaderValue::from_static("https://example.com"));

        assert!(validate_origin(&state(), &headers).is_err());
    }

    #[test]
    fn allows_loopback_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(ORIGIN, HeaderValue::from_static("http://localhost:8765"));

        assert!(validate_origin(&state(), &headers).is_ok());
    }

    #[test]
    fn localhost_bypass_rejects_forwarded_proxy_request() {
        let peer = "127.0.0.1:12345".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("localhost:8765"));
        assert!(is_direct_localhost_request(peer, &headers));

        headers.insert(
            "x-forwarded-host",
            HeaderValue::from_static("mcp.example.com"),
        );
        assert!(!is_direct_localhost_request(peer, &headers));
    }
}
