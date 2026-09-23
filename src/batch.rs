use std::{future::Future, sync::Arc};

use futures::{StreamExt, stream};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct BatchRequest<T> {
    pub items: Vec<T>,
    #[serde(default)]
    pub options: BatchOptions,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
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

impl ToolError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

pub async fn execute_read_batch<I, O, F, Fut>(
    request: BatchRequest<I>,
    max_items: usize,
    default_concurrency: usize,
    max_concurrency: usize,
    operation: F,
) -> Result<BatchResponse<O>, ToolError>
where
    I: Send + 'static,
    O: Send + 'static,
    F: Fn(usize, I) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<O, ToolError>> + Send,
{
    validate_batch(&request, max_items)?;

    if request.options.fail_fast {
        return Ok(execute_sequential(request, operation).await);
    }

    let concurrency = request
        .options
        .concurrency
        .unwrap_or(default_concurrency)
        .clamp(1, max_concurrency.max(1));
    let operation = Arc::new(operation);

    let mut results = stream::iter(request.items.into_iter().enumerate())
        .map(|(index, item)| {
            let operation = operation.clone();
            async move { item_result(index, operation(index, item).await) }
        })
        .buffer_unordered(concurrency)
        .collect::<Vec<_>>()
        .await;

    results.sort_by_key(|result| result.index);
    Ok(BatchResponse { results })
}

pub async fn execute_write_batch<I, O, F, Fut>(
    request: BatchRequest<I>,
    max_items: usize,
    operation: F,
) -> Result<BatchResponse<O>, ToolError>
where
    F: Fn(usize, I) -> Fut,
    Fut: Future<Output = Result<O, ToolError>>,
{
    validate_batch(&request, max_items)?;
    Ok(execute_sequential(request, operation).await)
}

fn validate_batch<T>(request: &BatchRequest<T>, max_items: usize) -> Result<(), ToolError> {
    if request.items.is_empty() {
        return Err(ToolError::new("EMPTY_BATCH", "items must not be empty"));
    }
    if request.items.len() > max_items {
        return Err(ToolError::new(
            "BATCH_TOO_LARGE",
            format!(
                "batch contains {} items, maximum is {max_items}",
                request.items.len()
            ),
        ));
    }
    Ok(())
}

async fn execute_sequential<I, O, F, Fut>(
    request: BatchRequest<I>,
    operation: F,
) -> BatchResponse<O>
where
    F: Fn(usize, I) -> Fut,
    Fut: Future<Output = Result<O, ToolError>>,
{
    let total = request.items.len();
    let fail_fast = request.options.fail_fast;
    let mut results = Vec::with_capacity(total);
    let mut aborted = false;

    for (index, item) in request.items.into_iter().enumerate() {
        if aborted {
            results.push(BatchItemResult {
                index,
                success: false,
                skipped: true,
                data: None,
                error: Some(ToolError::new(
                    "BATCH_ABORTED",
                    "skipped because fail_fast stopped the batch",
                )),
            });
            continue;
        }

        let result = item_result(index, operation(index, item).await);
        if fail_fast && !result.success {
            aborted = true;
        }
        results.push(result);
    }

    BatchResponse { results }
}

fn item_result<T>(index: usize, result: Result<T, ToolError>) -> BatchItemResult<T> {
    match result {
        Ok(data) => BatchItemResult {
            index,
            success: true,
            skipped: false,
            data: Some(data),
            error: None,
        },
        Err(error) => BatchItemResult {
            index,
            success: false,
            skipped: false,
            data: None,
            error: Some(error),
        },
    }
}
