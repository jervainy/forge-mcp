use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct BatchRequest<T> {
    pub items: Vec<T>,
    #[serde(default)]
    pub options: BatchOptions,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BatchOptions {
    #[serde(default)]
    pub fail_fast: bool,
    pub concurrency: Option<usize>,
}

impl Default for BatchOptions {
    fn default() -> Self {
        Self {
            fail_fast: false,
            concurrency: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchResponse<T> {
    pub results: Vec<BatchItemResult<T>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchItemResult<T> {
    pub index: usize,
    pub success: bool,
    pub skipped: bool,
    pub data: Option<T>,
    pub error: Option<ToolError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolError {
    pub code: String,
    pub message: String,
}
