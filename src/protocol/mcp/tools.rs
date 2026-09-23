use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub security_schemes: Vec<SecurityScheme>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum SecurityScheme {
    #[serde(rename = "oauth2")]
    OAuth2 { scopes: Vec<String> },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListToolsResult {
    pub tools: Vec<Tool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CallToolParams {
    pub name: String,
    #[serde(default)]
    pub arguments: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolResult {
    pub content: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<BTreeMap<String, Value>>,
}

impl CallToolResult {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text { text: text.into() }],
            is_error: None,
            meta: None,
        }
    }

    pub fn tool_error(message: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text {
                text: message.into(),
            }],
            is_error: Some(true),
            meta: None,
        }
    }

    pub fn authentication_required(challenge: String) -> Self {
        let mut meta = BTreeMap::new();
        meta.insert(
            "mcp/www_authenticate".to_string(),
            Value::Array(vec![Value::String(challenge)]),
        );

        Self {
            content: vec![Content::Text {
                text: "Authentication requires additional OAuth permission.".to_string(),
            }],
            is_error: Some(true),
            meta: Some(meta),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Content {
    Text { text: String },
}
