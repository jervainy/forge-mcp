use schemars::{JsonSchema, schema_for};
use serde_json::{Value, json};
use tokio::process::Command;

use crate::{
    app::ForgeMcp,
    batch::BatchRequest,
    protocol::mcp::tools::{CallToolParams, CallToolResult, SecurityScheme, Tool},
    tools::{
        AuditListRequest, FileDeleteItem, FileEditItem, FileListItem, FilePatchItem, FileReadItem,
        FileSearchItem, FileWriteItem, GitInfo, ShellRunItem, TOOL_NAMES, WorkspaceInfo,
    },
};

pub struct ToolRegistry;

impl ToolRegistry {
    pub fn list() -> Vec<Tool> {
        vec![
            batch_tool::<ShellRunItem>(
                "shell_run",
                "Run one or more one-shot shell commands in the workspace.",
                &["forge:execute"],
            ),
            batch_tool::<FileListItem>(
                "file_list",
                "List one or more workspace directories.",
                &["forge:read"],
            ),
            batch_tool::<FileReadItem>(
                "file_read",
                "Read ranges from one or more text files.",
                &["forge:read"],
            ),
            batch_tool::<FileWriteItem>(
                "file_write",
                "Create or overwrite one or more files.",
                &["forge:write"],
            ),
            batch_tool::<FileEditItem>(
                "file_edit",
                "Apply string or line-range edits to one or more files.",
                &["forge:write"],
            ),
            batch_tool::<FileDeleteItem>(
                "file_delete",
                "Delete one or more files or directories.",
                &["forge:write"],
            ),
            batch_tool::<FileSearchItem>(
                "file_search",
                "Search workspace file names and text content.",
                &["forge:read"],
            ),
            batch_tool::<FilePatchItem>(
                "file_patch",
                "Apply one or more unified diffs in order.",
                &["forge:write"],
            ),
            Tool {
                name: "workspace_info".to_string(),
                description: "Return workspace, platform, shell, and Git information.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "additionalProperties": false
                }),
                security_schemes: oauth(&["forge:read"]),
            },
            Tool {
                name: "audit_list".to_string(),
                description: "List recent ForgeMCP audit records.".to_string(),
                input_schema: schema::<AuditListRequest>(),
                security_schemes: oauth(&["forge:audit"]),
            },
        ]
    }

    pub fn required_scopes(name: &str) -> Option<&'static [&'static str]> {
        match name {
            "shell_run" => Some(&["forge:execute"]),
            "file_list" | "file_read" | "file_search" | "workspace_info" => Some(&["forge:read"]),
            "file_write" | "file_edit" | "file_delete" | "file_patch" => Some(&["forge:write"]),
            "audit_list" => Some(&["forge:audit"]),
            _ => None,
        }
    }

    pub async fn call(app: &ForgeMcp, params: CallToolParams) -> Option<CallToolResult> {
        if !TOOL_NAMES.contains(&params.name.as_str()) {
            return None;
        }

        if params.name == "workspace_info" {
            return Some(workspace_info(app).await);
        }

        Some(CallToolResult::tool_error(format!(
            "{} is registered but its runtime implementation is not part of the protocol milestone yet",
            params.name
        )))
    }
}

fn batch_tool<T>(name: &str, description: &str, scopes: &[&str]) -> Tool
where
    T: JsonSchema,
{
    Tool {
        name: name.to_string(),
        description: description.to_string(),
        input_schema: schema::<BatchRequest<T>>(),
        security_schemes: oauth(scopes),
    }
}

fn oauth(scopes: &[&str]) -> Vec<SecurityScheme> {
    vec![SecurityScheme::OAuth2 {
        scopes: scopes.iter().map(|scope| (*scope).to_string()).collect(),
    }]
}

fn schema<T>() -> Value
where
    T: JsonSchema,
{
    serde_json::to_value(schema_for!(T)).expect("generated JSON schema must serialize")
}

async fn workspace_info(app: &ForgeMcp) -> CallToolResult {
    let info = WorkspaceInfo {
        root: app.workspace_root().display().to_string(),
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        shell: std::env::var("SHELL").ok(),
        git: git_info(app).await,
    };

    match serde_json::to_string_pretty(&info) {
        Ok(text) => CallToolResult::text(text),
        Err(error) => CallToolResult::tool_error(format!(
            "failed to serialize workspace information: {error}"
        )),
    }
}

async fn git_info(app: &ForgeMcp) -> Option<GitInfo> {
    let root = app.workspace_root();

    let inside = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .await
        .ok()?;

    if !inside.status.success() {
        return None;
    }

    let branch = git_stdout(root, &["branch", "--show-current"]).await;
    let commit = git_stdout(root, &["rev-parse", "HEAD"]).await;

    let dirty = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain"])
        .output()
        .await
        .ok()
        .is_some_and(|output| output.status.success() && !output.stdout.is_empty());

    Some(GitInfo {
        branch,
        commit,
        dirty,
    })
}

async fn git_stdout(root: &std::path::Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}
