use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const TOOL_NAMES: [&str; 10] = [
    "shell_run",
    "file_list",
    "file_read",
    "file_write",
    "file_edit",
    "file_delete",
    "file_search",
    "file_patch",
    "workspace_info",
    "audit_list",
];

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ShellRunItem {
    pub command: String,
    pub cwd: Option<String>,
    pub timeout_ms: Option<u64>,
    pub env: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShellRunResult {
    pub command: String,
    pub cwd: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u128,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FileListItem {
    pub path: String,
    pub depth: Option<usize>,
    pub include_hidden: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    pub path: String,
    pub name: String,
    pub kind: FileEntryKind,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileEntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FileReadItem {
    pub path: String,
    pub start_line: Option<usize>,
    pub line_count: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileReadResult {
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub total_lines: usize,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FileWriteItem {
    pub path: String,
    pub content: String,
    pub mode: Option<FileWriteMode>,
    #[serde(default)]
    pub create_parent_dirs: bool,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileWriteMode {
    Create,
    Overwrite,
    CreateOrOverwrite,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FileEditItem {
    pub path: String,
    pub operation: FileEditOperation,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileEditOperation {
    Replace {
        old: String,
        new: String,
        #[serde(default)]
        replace_all: bool,
    },
    ReplaceLines {
        start_line: usize,
        end_line: usize,
        content: String,
    },
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FileDeleteItem {
    pub path: String,
    #[serde(default)]
    pub recursive: bool,
    #[serde(default)]
    pub ignore_missing: bool,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FileSearchItem {
    pub path: String,
    pub glob: Option<String>,
    pub pattern: Option<String>,
    pub regex: Option<bool>,
    pub case_sensitive: Option<bool>,
    pub max_results: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileSearchMatch {
    pub path: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FilePatchItem {
    pub patch: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceInfo {
    pub root: String,
    pub platform: String,
    pub arch: String,
    pub shell: Option<String>,
    pub git: Option<GitInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitInfo {
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub dirty: bool,
}

#[derive(Debug, Clone, Deserialize, Default, JsonSchema)]
pub struct AuditListRequest {
    pub limit: Option<usize>,
    pub tool: Option<String>,
    pub success: Option<bool>,
}
