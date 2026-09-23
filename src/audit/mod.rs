use std::{
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
    sync::Mutex,
};

use crate::{batch::ToolError, tools::AuditListRequest};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub timestamp_ms: u128,
    pub batch_id: String,
    pub tool: String,
    pub index: usize,
    pub success: bool,
    pub target: Option<String>,
    pub error_code: Option<String>,
}

#[derive(Clone)]
pub struct AuditLog {
    path: Arc<PathBuf>,
    lock: Arc<Mutex<()>>,
}

impl AuditLog {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path: Arc::new(path),
            lock: Arc::new(Mutex::new(())),
        }
    }

    pub async fn record(
        &self,
        batch_id: &str,
        tool: &str,
        index: usize,
        target: Option<String>,
        result: &Result<(), ToolError>,
    ) {
        let _guard = self.lock.lock().await;
        let entry = AuditEntry {
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            batch_id: batch_id.to_string(),
            tool: tool.to_string(),
            index,
            success: result.is_ok(),
            target,
            error_code: result.as_ref().err().map(|error| error.code.clone()),
        };

        if let Some(parent) = self.path.parent() {
            if fs::create_dir_all(parent).await.is_err() {
                return;
            }
        }

        let Ok(line) = serde_json::to_vec(&entry) else {
            return;
        };
        let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path.as_ref())
            .await
        else {
            return;
        };

        let _ = file.write_all(&line).await;
        let _ = file.write_all(b"\n").await;
    }

    pub async fn list(&self, request: &AuditListRequest) -> Result<Vec<AuditEntry>, ToolError> {
        let _guard = self.lock.lock().await;
        let limit = request.limit.unwrap_or(100).clamp(1, 1000);

        let content = match fs::read_to_string(self.path.as_ref()).await {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(ToolError::new(
                    "AUDIT_READ_FAILED",
                    format!("failed to read audit log: {error}"),
                ));
            }
        };

        let mut entries = content
            .lines()
            .filter_map(|line| serde_json::from_str::<AuditEntry>(line).ok())
            .filter(|entry| {
                request
                    .tool
                    .as_deref()
                    .is_none_or(|tool| entry.tool == tool)
                    && request
                        .success
                        .is_none_or(|success| entry.success == success)
            })
            .collect::<Vec<_>>();
        entries.reverse();
        entries.truncate(limit);
        Ok(entries)
    }
}
