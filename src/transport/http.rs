use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Arc,
};

use axum::{
    Form, Json, Router,
    body::Bytes,
    extract::{ConnectInfo, Query, State, rejection::JsonRejection},
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
use tracing::{debug, info, warn};
use url::Url;
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
    transport::sse::{self, SseHub, StreamError},
};

const SESSION_HEADER: &str = "mcp-session-id";
const PROTOCOL_VERSION_HEADER: &str = "mcp-protocol-version";

type Session = Arc<SessionEntry>;

struct SessionEntry {
    server: Mutex<ForgeServer>,
    owner: Option<String>,
    sse: SseHub,
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
    if let Err(response) = validate_origin(&state, peer, &headers) {
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

    if session.owner != auth.identity_key() {
        return plain_response(
            StatusCode::FORBIDDEN,
            "MCP session belongs to a different authenticated client",
        );
    }

    let is_notification = message.is_notification();

    // Tool execution continues after SSE disconnect so Last-Event-ID can
    // recover the response. The session lock does not cover SSE replay.
    if message.method == "tools/call" && !is_notification {
        let log = match session.sse.create().await {
            Ok(log) => log,
            Err(error) => return sse_error_response(error),
        };
        let listener = match log.subscribe(None) {
            Ok(listener) => listener,
            Err(error) => return sse_error_response(error),
        };
        let progress_token = message
            .params
            .as_ref()
            .and_then(|params| params.get("_meta"))
            .and_then(|meta| meta.get("progressToken"))
            .filter(|token| token.is_string() || token.is_i64() || token.is_u64())
            .cloned();

        tokio::spawn(async move {
            if let Some(token) = progress_token.as_ref() {
                log.emit_json(&progress_notification(token, 0)).await;
            }
            let result = session
                .server
                .lock()
                .await
                .handle_with_auth(message, &auth)
                .await;
            if let Some(response) = result {
                if let Some(token) = progress_token.as_ref() {
                    log.emit_json(&progress_notification(token, 1)).await;
                }
                log.emit_json(&response).await;
            }
            log.finish().await;
        });

        return sse::response(listener);
    }

    let response = session
        .server
        .lock()
        .await
        .handle_with_auth(message, &auth)
        .await;

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
        Arc::new(SessionEntry {
            server: Mutex::new(server),
            owner: auth.identity_key(),
            sse: SseHub::default(),
        }),
    );

    debug!(session_id, "created MCP HTTP session");

    json_rpc_response(StatusCode::OK, response, Some(&session_id))
}

async fn get_mcp(
    State(state): State<HttpState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = validate_origin(&state, peer, &headers) {
        return response;
    }

    if !optional_header(&headers, ACCEPT)
        .is_some_and(|value| value.to_ascii_lowercase().contains("text/event-stream"))
    {
        return plain_response(
            StatusCode::NOT_ACCEPTABLE,
            "Accept must include text/event-stream",
        );
    }

    let auth = match authenticate(&state, peer, &headers) {
        Ok(auth) => auth,
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

    let session_id = match required_header(&headers, SESSION_HEADER) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let session = {
        let sessions = state.sessions.read().await;
        sessions.get(session_id).cloned()
    };
    let Some(session) = session else {
        return StatusCode::NOT_FOUND.into_response();
    };

    if session.owner != auth.identity_key() {
        return plain_response(
            StatusCode::FORBIDDEN,
            "MCP session belongs to a different authenticated client",
        );
    }

    let (log, cursor) = match optional_header(&headers, "last-event-id") {
        Some(id) => match session.sse.resume(id).await {
            Ok((log, cursor)) => (log, Some(cursor)),
            Err(error) => return sse_error_response(error),
        },
        None => match session.sse.create().await {
            Ok(log) => (log, None),
            Err(error) => return sse_error_response(error),
        },
    };

    match log.subscribe(cursor) {
        Ok(listener) => sse::response(listener),
        Err(error) => sse_error_response(error),
    }
}

fn progress_notification(token: &Value, progress: u64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/progress",
        "params": {
            "progressToken": token,
            "progress": progress,
            "total": 1
        }
    })
}

fn sse_error_response(error: StreamError) -> Response {
    match error {
        StreamError::InvalidCursor => {
            plain_response(StatusCode::BAD_REQUEST, "invalid Last-Event-ID")
        }
        StreamError::UnknownStream => StatusCode::NOT_FOUND.into_response(),
        StreamError::ExpiredCursor => plain_response(StatusCode::GONE, "SSE replay window expired"),
        StreamError::AlreadyConnected => {
            plain_response(StatusCode::CONFLICT, "SSE stream is already connected")
        }
        StreamError::AtCapacity => plain_response(
            StatusCode::TOO_MANY_REQUESTS,
            "too many SSE streams for this session",
        ),
        StreamError::SessionClosed => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn delete_mcp(
    State(state): State<HttpState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = validate_origin(&state, peer, &headers) {
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

    if session.owner != auth.identity_key() {
        return plain_response(
            StatusCode::FORBIDDEN,
            "MCP session belongs to a different authenticated client",
        );
    }

    state.sessions.write().await.remove(&session_id);
    session.sse.shutdown().await;
    debug!(session_id, "terminated MCP HTTP session");
    StatusCode::NO_CONTENT.into_response()
}

async fn oauth_protected_resource(
    State(state): State<HttpState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if !state.oauth.enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let base = match request_base_url(&state, peer, &headers) {
        Ok(base) => base,
        Err(response) => return response,
    };
    oauth_json(
        StatusCode::OK,
        state.oauth.protected_resource_metadata(&base),
    )
}

async fn oauth_server_metadata(
    State(state): State<HttpState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if !state.oauth.enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let base = match request_base_url(&state, peer, &headers) {
        Ok(base) => base,
        Err(response) => return response,
    };
    oauth_json(
        StatusCode::OK,
        state.oauth.authorization_server_metadata(&base),
    )
}

async fn oauth_register(
    State(state): State<HttpState>,
    request: Result<Json<RegistrationRequest>, JsonRejection>,
) -> Response {
    if !state.oauth.enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let Json(request) = match request {
        Ok(request) => request,
        Err(error) => {
            warn!(reason = %error, "OAuth DCR request could not be parsed");
            return oauth_json(
                StatusCode::BAD_REQUEST,
                json!({
                    "error": "invalid_client_metadata",
                    "error_description": "Registration request must be a valid JSON object"
                }),
            );
        }
    };

    debug!(
        requested_grants = ?request.grant_types,
        requested_response_types = ?request.response_types,
        requested_auth_method = ?request.token_endpoint_auth_method,
        "OAuth DCR registration request"
    );

    match state.oauth.register(request) {
        Ok(response) => {
            info!(client_id = %response.client_id, "OAuth DCR client registered");
            oauth_json(StatusCode::CREATED, response)
        }
        Err(error) => {
            warn!(
                status = error.status,
                error = %error.error,
                reason = %error.description,
                "OAuth DCR client registration rejected"
            );
            oauth_error_response(error)
        }
    }
}

async fn oauth_authorize_get(
    State(state): State<HttpState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(request): Query<AuthorizeRequest>,
) -> Response {
    if !state.oauth.enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let base = match request_base_url(&state, peer, &headers) {
        Ok(base) => base,
        Err(response) => return response,
    };
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
    let base = match request_base_url(&state, peer, &headers) {
        Ok(base) => base,
        Err(response) => return response,
    };
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
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(request): Form<TokenRequest>,
) -> Response {
    if !state.oauth.enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let base = match request_base_url(&state, peer, &headers) {
        Ok(base) => base,
        Err(response) => return response,
    };
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

    let base = request_base_url(state, peer, headers)?;
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

fn validate_origin(
    state: &HttpState,
    peer: SocketAddr,
    headers: &HeaderMap,
) -> Result<(), Response> {
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
        || (state.oauth.public_base_url().is_none()
            && infer_request_origin(peer, headers)
                .is_ok_and(|base| base.eq_ignore_ascii_case(origin.trim_end_matches('/'))))
    {
        Ok(())
    } else {
        Err(plain_response(StatusCode::FORBIDDEN, "invalid Origin"))
    }
}

fn request_base_url(
    state: &HttpState,
    peer: SocketAddr,
    headers: &HeaderMap,
) -> Result<String, Response> {
    if let Some(value) = state.oauth.public_base_url() {
        return Ok(value.trim_end_matches('/').to_string());
    }

    let base = infer_request_origin(peer, headers).map_err(|reason| {
        warn!(reason, "OAuth public origin could not be inferred");
        plain_response(StatusCode::BAD_REQUEST, format!("OAuth origin: {reason}"))
    })?;
    debug!(origin = %base, "inferred OAuth public origin from request");
    Ok(base)
}

/// In automatic mode the public origin is scoped to the current request.
/// The ingress proxy must enforce the publicly routed Host. Forwarded
/// metadata is used only from local peers, never from arbitrary remote peers.
fn infer_request_origin(peer: SocketAddr, headers: &HeaderMap) -> Result<String, &'static str> {
    let host_header = optional_header(headers, HOST.as_str())
        .ok_or("Host header is required when public_base_url is not configured")?;
    let host = parse_origin_host(host_header)?;
    let initial_loopback = origin_is_loopback(&host);

    let forwarded_host = if peer.ip().is_loopback() {
        optional_header(headers, "x-forwarded-host")
    } else {
        None
    };
    let host = if initial_loopback {
        match forwarded_host {
            Some(value) => parse_origin_host(value)?,
            None => host,
        }
    } else {
        // Prefer the HTTP Host when it already contains the public authority.
        // A forwarded host never overrides a non-local authority.
        host
    };

    let local = origin_is_loopback(&host);
    let forwarded_proto = if peer.ip().is_loopback() {
        optional_header(headers, "x-forwarded-proto")
    } else {
        None
    };

    let scheme = match forwarded_proto {
        Some("https") => "https",
        Some("http") if local => "http",
        Some("http") => return Err("public OAuth endpoints require HTTPS"),
        Some(_) => return Err("unsupported X-Forwarded-Proto value"),
        None if local => "http",
        // HTTPS-terminating tunnels commonly preserve the public Host but
        // do not always provide a forwarded scheme. Avoid emitting a broken
        // http:// URL for a public authority in that case.
        None if peer.ip().is_loopback() => "https",
        None => return Err("public OAuth endpoints require HTTPS; configure a trusted proxy"),
    };

    Ok(format!("{scheme}://{host}"))
}

/// Require an authority only: no scheme, path, userinfo, whitespace, lists,
/// or other values that could poison the discovery URLs and JWT audience.
fn parse_origin_host(raw: &str) -> Result<String, &'static str> {
    if raw.is_empty()
        || raw.contains(|c: char| c.is_whitespace() || c.is_control())
        || raw
            .chars()
            .any(|c| matches!(c, '@' | '/' | '\\' | '?' | '#' | ','))
    {
        return Err("invalid Host authority");
    }

    let url = Url::parse(&format!("http://{raw}")).map_err(|_| "invalid Host authority")?;
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("invalid Host authority");
    }

    Ok(raw.to_ascii_lowercase())
}

fn origin_is_loopback(authority: &str) -> bool {
    let Ok(url) = Url::parse(&format!("http://{authority}")) else {
        return false;
    };
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    })
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
        assert!(
            validate_origin(
                &state(),
                "127.0.0.1:10000".parse().unwrap(),
                &HeaderMap::new()
            )
            .is_ok()
        );
    }

    #[test]
    fn rejects_untrusted_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(ORIGIN, HeaderValue::from_static("https://example.com"));

        assert!(validate_origin(&state(), "127.0.0.1:10000".parse().unwrap(), &headers).is_err());
    }

    #[test]
    fn allows_loopback_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(ORIGIN, HeaderValue::from_static("http://localhost:8765"));

        assert!(validate_origin(&state(), "127.0.0.1:10000".parse().unwrap(), &headers).is_ok());
    }

    #[test]
    fn infers_ngrok_public_origin_without_config() {
        let peer = "127.0.0.1:42833".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            HOST,
            HeaderValue::from_static("quality-femur-booting.ngrok-free.dev"),
        );
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));

        let origin = infer_request_origin(peer, &headers).unwrap();
        assert_eq!(origin, "https://quality-femur-booting.ngrok-free.dev");
        assert!(validate_origin(&state(), peer, &headers_with_origin(&headers, &origin),).is_ok());
    }

    #[test]
    fn infers_https_from_public_host_when_local_tunnel_omits_forwarded_proto() {
        let peer = "127.0.0.1:42833".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("some-tunnel.ngrok-free.dev"));
        assert_eq!(
            infer_request_origin(peer, &headers).unwrap(),
            "https://some-tunnel.ngrok-free.dev"
        );
    }

    #[test]
    fn supports_local_development_and_rewritten_proxy_host() {
        let peer = "127.0.0.1:42833".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("127.0.0.1:8765"));
        assert_eq!(
            infer_request_origin(peer, &headers).unwrap(),
            "http://127.0.0.1:8765"
        );

        headers.insert(
            "x-forwarded-host",
            HeaderValue::from_static("dynamic.example.com"),
        );
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        assert_eq!(
            infer_request_origin(peer, &headers).unwrap(),
            "https://dynamic.example.com"
        );
    }

    #[test]
    fn rejects_host_poisoning_and_insecure_public_origin() {
        let peer = "127.0.0.1:42833".parse().unwrap();
        for host in [
            "evil.com/path",
            "evil.com@localhost",
            "evil.com,other.com",
            "evil.com\\r\\nInjected",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                HOST,
                HeaderValue::from_str(host).unwrap_or(HeaderValue::from_static("bad/host")),
            );
            assert!(infer_request_origin(peer, &headers).is_err(), "{host}");
        }

        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("dynamic.example.com"));
        headers.insert("x-forwarded-proto", HeaderValue::from_static("http"));
        assert!(infer_request_origin(peer, &headers).is_err());
    }

    fn headers_with_origin(headers: &HeaderMap, origin: &str) -> HeaderMap {
        let mut headers = headers.clone();
        headers.insert(ORIGIN, HeaderValue::from_str(origin).unwrap());
        headers
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
