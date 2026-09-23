use std::{collections::HashMap, sync::Arc};

use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{ACCEPT, ALLOW, CONTENT_TYPE, ORIGIN},
    },
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info};
use uuid::Uuid;

use crate::{
    app::ForgeMcp,
    protocol::{
        jsonrpc::{INVALID_REQUEST, JsonRpcMessage, JsonRpcResponse},
        mcp::PROTOCOL_VERSION,
    },
    server::ForgeServer,
};

const SESSION_HEADER: &str = "mcp-session-id";
const PROTOCOL_VERSION_HEADER: &str = "mcp-protocol-version";

type Session = Arc<Mutex<ForgeServer>>;

#[derive(Clone)]
struct HttpState {
    app: ForgeMcp,
    sessions: Arc<RwLock<HashMap<String, Session>>>,
    allowed_origins: Arc<Vec<String>>,
}

pub async fn serve(app: ForgeMcp, host: &str, port: u16) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind((host, port)).await?;
    let local_addr = listener.local_addr()?;

    let state = HttpState {
        app,
        sessions: Arc::new(RwLock::new(HashMap::new())),
        allowed_origins: Arc::new(allowed_origins(host, port)),
    };

    let router = Router::new()
        .route(
            "/mcp",
            post(post_mcp).get(get_mcp).delete(delete_mcp),
        )
        .with_state(state);

    info!(
        address = %local_addr,
        endpoint = %format!("http://{local_addr}/mcp"),
        "ForgeMCP Streamable HTTP transport listening"
    );

    axum::serve(listener, router).await?;
    Ok(())
}

async fn post_mcp(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(response) = validate_origin(&state, &headers) {
        return response;
    }

    if let Err(response) = validate_post_headers(&headers) {
        return response;
    }

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
            JsonRpcResponse::invalid_request("message must be a request, notification, or response"),
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
        return initialize_session(state, headers, message).await;
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
        let mut server = session.lock().await;
        server.handle(message).await
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
) -> Response {
    if headers.contains_key(SESSION_HEADER) {
        return plain_response(
            StatusCode::BAD_REQUEST,
            "initialize must not reuse an MCP-Session-Id",
        );
    }

    let mut server = ForgeServer::new(state.app.clone());
    let response = server.handle(message).await;

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
    state
        .sessions
        .write()
        .await
        .insert(session_id.clone(), Arc::new(Mutex::new(server)));

    debug!(session_id, "created MCP HTTP session");

    json_rpc_response(StatusCode::OK, response, Some(&session_id))
}

async fn get_mcp(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = validate_origin(&state, &headers) {
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
    headers: HeaderMap,
) -> Response {
    if let Err(response) = validate_origin(&state, &headers) {
        return response;
    }

    let session_id = match required_header(&headers, SESSION_HEADER) {
        Ok(value) => value.to_string(),
        Err(response) => return response,
    };

    let removed = state.sessions.write().await.remove(&session_id);
    if removed.is_some() {
        debug!(session_id, "terminated MCP HTTP session");
        StatusCode::NO_CONTENT.into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
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
    {
        Ok(())
    } else {
        Err(plain_response(StatusCode::FORBIDDEN, "invalid Origin"))
    }
}

fn required_header<'a>(
    headers: &'a HeaderMap,
    name: &str,
) -> Result<&'a str, Response> {
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
        HttpState {
            app: ForgeMcp::new(AppConfig {
                workspace_root: std::env::current_dir().unwrap(),
                max_batch_items: 100,
                max_concurrency: 16,
            }),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            allowed_origins: Arc::new(allowed_origins("127.0.0.1", 8765)),
        }
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
        headers.insert(
            ORIGIN,
            HeaderValue::from_static("http://localhost:8765"),
        );

        assert!(validate_origin(&state(), &headers).is_ok());
    }
}
