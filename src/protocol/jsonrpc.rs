use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Number, Value};

pub const JSONRPC_VERSION: &str = "2.0";

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
pub const SERVER_NOT_INITIALIZED: i64 = -32002;

#[derive(Debug, Clone, PartialEq)]
pub enum JsonRpcId {
    Missing,
    Null,
    Number(Number),
    String(String),
}

impl JsonRpcId {
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }

    pub fn response_id(self) -> Self {
        if self.is_missing() {
            Self::Null
        } else {
            self
        }
    }
}

impl Default for JsonRpcId {
    fn default() -> Self {
        Self::Missing
    }
}

impl Serialize for JsonRpcId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Missing | Self::Null => serializer.serialize_none(),
            Self::Number(value) => value.serialize(serializer),
            Self::String(value) => serializer.serialize_str(value),
        }
    }
}

impl<'de> Deserialize<'de> for JsonRpcId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        match value {
            Value::Null => Ok(Self::Null),
            Value::Number(value) => Ok(Self::Number(value)),
            Value::String(value) => Ok(Self::String(value)),
            _ => Err(serde::de::Error::custom(
                "JSON-RPC id must be a string, number, or null",
            )),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcMessage {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: JsonRpcId,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

impl JsonRpcMessage {
    pub fn is_notification(&self) -> bool {
        self.id.is_missing()
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.jsonrpc != JSONRPC_VERSION {
            return Err("jsonrpc must be exactly \"2.0\"");
        }
        if self.method.is_empty() {
            return Err("method must not be empty");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcSuccess {
    pub jsonrpc: &'static str,
    pub id: JsonRpcId,
    pub result: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcFailure {
    pub jsonrpc: &'static str,
    pub id: JsonRpcId,
    pub error: JsonRpcError,
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum JsonRpcResponse {
    Success(JsonRpcSuccess),
    Error(JsonRpcFailure),
}

impl JsonRpcResponse {
    pub fn success(id: JsonRpcId, result: impl Serialize) -> Self {
        let result = serde_json::to_value(result)
            .unwrap_or_else(|_| Value::Object(Default::default()));
        Self::Success(JsonRpcSuccess {
            jsonrpc: JSONRPC_VERSION,
            id: id.response_id(),
            result,
        })
    }

    pub fn error(
        id: JsonRpcId,
        code: i64,
        message: impl Into<String>,
        data: Option<Value>,
    ) -> Self {
        Self::Error(JsonRpcFailure {
            jsonrpc: JSONRPC_VERSION,
            id: id.response_id(),
            error: JsonRpcError {
                code,
                message: message.into(),
                data,
            },
        })
    }

    pub fn parse_error() -> Self {
        Self::error(JsonRpcId::Null, PARSE_ERROR, "Parse error", None)
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::error(
            JsonRpcId::Null,
            INVALID_REQUEST,
            "Invalid Request",
            Some(Value::String(message.into())),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_request_notification_and_null_id() {
        let request: JsonRpcMessage = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
        )
        .unwrap();
        assert!(!request.is_notification());

        let notification: JsonRpcMessage = serde_json::from_str(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        )
        .unwrap();
        assert!(notification.is_notification());

        let null_id: JsonRpcMessage = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#,
        )
        .unwrap();
        assert!(!null_id.is_notification());
        assert_eq!(null_id.id, JsonRpcId::Null);
    }

    #[test]
    fn serializes_success_response() {
        let response = JsonRpcResponse::success(
            JsonRpcId::Number(Number::from(7)),
            serde_json::json!({"ok": true}),
        );
        let value = serde_json::to_value(response).unwrap();

        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], 7);
        assert_eq!(value["result"]["ok"], true);
    }
}
