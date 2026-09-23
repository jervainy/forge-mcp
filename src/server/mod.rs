mod tool_registry;

use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::{
    app::ForgeMcp,
    auth::AuthContext,
    protocol::{
        jsonrpc::{
            INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, JsonRpcId, JsonRpcMessage,
            JsonRpcResponse, METHOD_NOT_FOUND, SERVER_NOT_INITIALIZED,
        },
        mcp::{
            PROTOCOL_VERSION,
            lifecycle::{
                Implementation, InitializeParams, InitializeResult, ServerCapabilities,
                ToolsCapability,
            },
            tools::{CallToolParams, CallToolResult, ListToolsResult},
        },
    },
};

use tool_registry::ToolRegistry;

#[derive(Debug)]
struct SessionState {
    protocol_version: String,
    client_info: Implementation,
    client_capabilities: Value,
    initialized: bool,
}

pub struct ForgeServer {
    app: ForgeMcp,
    session: Option<SessionState>,
}

impl ForgeServer {
    pub fn new(app: ForgeMcp) -> Self {
        Self { app, session: None }
    }

    pub async fn handle(&mut self, message: JsonRpcMessage) -> Option<JsonRpcResponse> {
        self.handle_with_auth(message, &AuthContext::Unrestricted)
            .await
    }

    pub async fn handle_with_auth(
        &mut self,
        message: JsonRpcMessage,
        auth: &AuthContext,
    ) -> Option<JsonRpcResponse> {
        if let Err(reason) = message.validate() {
            if message.is_notification() {
                warn!(reason, "discarding invalid JSON-RPC notification");
                return None;
            }
            return Some(JsonRpcResponse::error(
                message.id,
                INVALID_REQUEST,
                "Invalid Request",
                Some(Value::String(reason.to_string())),
            ));
        }

        if message.is_notification() {
            self.handle_notification(message).await;
            return None;
        }

        Some(self.handle_request(message, auth).await)
    }

    async fn handle_notification(&mut self, message: JsonRpcMessage) {
        match message.method.as_str() {
            "notifications/initialized" => {
                if let Some(session) = &mut self.session {
                    session.initialized = true;
                    info!(
                        protocol_version = %session.protocol_version,
                        client = %session.client_info.name,
                        "MCP session initialized"
                    );
                } else {
                    warn!("received notifications/initialized before initialize");
                }
            }
            method => {
                debug!(method, "ignoring unsupported notification");
            }
        }
    }

    async fn handle_request(
        &mut self,
        message: JsonRpcMessage,
        auth: &AuthContext,
    ) -> JsonRpcResponse {
        let id = message.id.clone();

        match message.method.as_str() {
            "initialize" => self.handle_initialize(id, message.params),
            "ping" => JsonRpcResponse::success(id, json!({})),
            "tools/list" => {
                if let Some(error) = self.require_initialized(id.clone()) {
                    return error;
                }
                JsonRpcResponse::success(
                    id,
                    ListToolsResult {
                        tools: ToolRegistry::list(),
                    },
                )
            }
            "tools/call" => {
                if let Some(error) = self.require_initialized(id.clone()) {
                    return error;
                }

                let params = match decode_params::<CallToolParams>(message.params) {
                    Ok(params) => params,
                    Err(error) => {
                        return JsonRpcResponse::error(
                            id,
                            INVALID_PARAMS,
                            "Invalid params",
                            Some(Value::String(error)),
                        );
                    }
                };

                let Some(required_scopes) = ToolRegistry::required_scopes(&params.name) else {
                    return JsonRpcResponse::error(id, INVALID_PARAMS, "Unknown tool", None);
                };

                if !auth.allows(required_scopes) {
                    let challenge = auth
                        .www_authenticate(required_scopes)
                        .unwrap_or_else(|| "Bearer error=\"insufficient_scope\"".to_string());
                    return JsonRpcResponse::success(
                        id,
                        CallToolResult::authentication_required(challenge),
                    );
                }

                match ToolRegistry::call(&self.app, params).await {
                    Some(result) => JsonRpcResponse::success(id, result),
                    None => JsonRpcResponse::error(id, INVALID_PARAMS, "Unknown tool", None),
                }
            }
            _ => JsonRpcResponse::error(
                id,
                METHOD_NOT_FOUND,
                "Method not found",
                Some(Value::String(message.method)),
            ),
        }
    }

    fn handle_initialize(&mut self, id: JsonRpcId, params: Option<Value>) -> JsonRpcResponse {
        if self.session.is_some() {
            return JsonRpcResponse::error(id, INVALID_REQUEST, "Server already initialized", None);
        }

        let params = match decode_params::<InitializeParams>(params) {
            Ok(params) => params,
            Err(error) => {
                return JsonRpcResponse::error(
                    id,
                    INVALID_PARAMS,
                    "Invalid initialize params",
                    Some(Value::String(error)),
                );
            }
        };

        if params.protocol_version != PROTOCOL_VERSION {
            debug!(
                requested = %params.protocol_version,
                selected = PROTOCOL_VERSION,
                "client requested an unsupported handshake revision; counter-offering supported revision"
            );
        }

        self.session = Some(SessionState {
            protocol_version: PROTOCOL_VERSION.to_string(),
            client_info: params.client_info,
            client_capabilities: params.capabilities,
            initialized: false,
        });

        JsonRpcResponse::success(
            id,
            InitializeResult {
                protocol_version: PROTOCOL_VERSION.to_string(),
                capabilities: ServerCapabilities {
                    tools: ToolsCapability::default(),
                },
                server_info: Implementation::forge_mcp(),
                instructions: Some(
                    "ForgeMCP exposes workspace-scoped shell and file tools. Batch-capable tools accept an items array."
                        .to_string(),
                ),
            },
        )
    }

    fn require_initialized(&self, id: JsonRpcId) -> Option<JsonRpcResponse> {
        match &self.session {
            Some(session) if session.initialized => {
                let _ = &session.client_capabilities;
                None
            }
            _ => Some(JsonRpcResponse::error(
                id,
                SERVER_NOT_INITIALIZED,
                "Server not initialized",
                None,
            )),
        }
    }
}

fn decode_params<T>(params: Option<Value>) -> Result<T, String>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(params.unwrap_or_else(|| json!({}))).map_err(|error| error.to_string())
}

#[allow(dead_code)]
fn internal_error(id: JsonRpcId, error: impl ToString) -> JsonRpcResponse {
    JsonRpcResponse::error(
        id,
        INTERNAL_ERROR,
        "Internal error",
        Some(Value::String(error.to_string())),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use serde_json::Number;

    fn server() -> ForgeServer {
        ForgeServer::new(ForgeMcp::new(AppConfig::test(
            std::env::current_dir().unwrap(),
        )))
    }

    fn request(id: i64, method: &str, params: Value) -> JsonRpcMessage {
        JsonRpcMessage {
            jsonrpc: "2.0".to_string(),
            id: JsonRpcId::Number(Number::from(id)),
            method: method.to_string(),
            params: Some(params),
        }
    }

    #[tokio::test]
    async fn requires_initialized_notification_before_tools() {
        let mut server = server();

        let response = server
            .handle(request(
                1,
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {
                        "name": "test-client",
                        "version": "0.1.0"
                    }
                }),
            ))
            .await;
        assert!(response.is_some());

        let response = server
            .handle(request(2, "tools/list", json!({})))
            .await
            .unwrap();
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["error"]["code"], SERVER_NOT_INITIALIZED);

        server
            .handle(JsonRpcMessage {
                jsonrpc: "2.0".to_string(),
                id: JsonRpcId::Missing,
                method: "notifications/initialized".to_string(),
                params: None,
            })
            .await;

        let response = server
            .handle(request(3, "tools/list", json!({})))
            .await
            .unwrap();
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["result"]["tools"].as_array().unwrap().len(), 10);
        assert_eq!(
            value["result"]["tools"][0]["securitySchemes"][0]["type"],
            "oauth2"
        );
    }
}
