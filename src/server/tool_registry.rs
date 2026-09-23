use schemars::{JsonSchema, schema_for};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::process::Command;
use uuid::Uuid;

use crate::{
    app::ForgeMcp,
    batch::{BatchRequest, ToolError, execute_read_batch, execute_write_batch},
    protocol::mcp::tools::{CallToolParams, CallToolResult, SecurityScheme, Tool},
    runtime::{filesystem, patch, shell},
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
                "Read ranges from one or more UTF-8 text files.",
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
                "Search workspace file names and UTF-8 text content using glob/literal/regex matching.",
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

        let arguments = params.arguments.unwrap_or_else(|| json!({}));
        let result = match params.name.as_str() {
            "shell_run" => shell_run(app, arguments).await,
            "file_list" => file_list(app, arguments).await,
            "file_read" => file_read(app, arguments).await,
            "file_write" => file_write(app, arguments).await,
            "file_edit" => file_edit(app, arguments).await,
            "file_delete" => file_delete(app, arguments).await,
            "file_search" => file_search(app, arguments).await,
            "file_patch" => file_patch(app, arguments).await,
            "workspace_info" => workspace_info(app).await,
            "audit_list" => audit_list(app, arguments).await,
            _ => unreachable!("tool name was validated"),
        };

        Some(match result {
            Ok(value) => json_result(value),
            Err(error) => CallToolResult::tool_error(format!("{}: {}", error.code, error.message)),
        })
    }
}

async fn shell_run(app: &ForgeMcp, arguments: Value) -> Result<impl Serialize, ToolError> {
    let request = decode::<BatchRequest<ShellRunItem>>(arguments)?;
    let batch_id = Uuid::new_v4().to_string();
    let app = app.clone();

    execute_read_batch(
        request,
        app.max_batch_items(),
        4,
        app.max_concurrency(),
        move |index, item| {
            let app = app.clone();
            let batch_id = batch_id.clone();
            async move {
                let target = item.cwd.clone().or_else(|| Some(".".to_string()));
                let result = shell::run(&app, item).await;
                audit_result(&app, &batch_id, "shell_run", index, target, &result).await;
                result
            }
        },
    )
    .await
}

async fn file_list(app: &ForgeMcp, arguments: Value) -> Result<impl Serialize, ToolError> {
    let request = decode::<BatchRequest<FileListItem>>(arguments)?;
    let batch_id = Uuid::new_v4().to_string();
    let app = app.clone();

    execute_read_batch(
        request,
        app.max_batch_items(),
        8,
        app.max_concurrency(),
        move |index, item| {
            let app = app.clone();
            let batch_id = batch_id.clone();
            async move {
                let target = Some(item.path.clone());
                let result = filesystem::list(&app, item).await;
                audit_result(&app, &batch_id, "file_list", index, target, &result).await;
                result
            }
        },
    )
    .await
}

async fn file_read(app: &ForgeMcp, arguments: Value) -> Result<impl Serialize, ToolError> {
    let request = decode::<BatchRequest<FileReadItem>>(arguments)?;
    let batch_id = Uuid::new_v4().to_string();
    let app = app.clone();

    execute_read_batch(
        request,
        app.max_batch_items(),
        8,
        app.max_concurrency(),
        move |index, item| {
            let app = app.clone();
            let batch_id = batch_id.clone();
            async move {
                let target = Some(item.path.clone());
                let result = filesystem::read(&app, item).await;
                audit_result(&app, &batch_id, "file_read", index, target, &result).await;
                result
            }
        },
    )
    .await
}

async fn file_write(app: &ForgeMcp, arguments: Value) -> Result<impl Serialize, ToolError> {
    let request = decode::<BatchRequest<FileWriteItem>>(arguments)?;
    let batch_id = Uuid::new_v4().to_string();
    let app = app.clone();

    execute_write_batch(request, app.max_batch_items(), move |index, item| {
        let app = app.clone();
        let batch_id = batch_id.clone();
        async move {
            let target = Some(item.path.clone());
            let result = filesystem::write(&app, item).await;
            audit_result(&app, &batch_id, "file_write", index, target, &result).await;
            result
        }
    })
    .await
}

async fn file_edit(app: &ForgeMcp, arguments: Value) -> Result<impl Serialize, ToolError> {
    let request = decode::<BatchRequest<FileEditItem>>(arguments)?;
    let batch_id = Uuid::new_v4().to_string();
    let app = app.clone();

    execute_write_batch(request, app.max_batch_items(), move |index, item| {
        let app = app.clone();
        let batch_id = batch_id.clone();
        async move {
            let target = Some(item.path.clone());
            let result = filesystem::edit(&app, item).await;
            audit_result(&app, &batch_id, "file_edit", index, target, &result).await;
            result
        }
    })
    .await
}

async fn file_delete(app: &ForgeMcp, arguments: Value) -> Result<impl Serialize, ToolError> {
    let request = decode::<BatchRequest<FileDeleteItem>>(arguments)?;
    let batch_id = Uuid::new_v4().to_string();
    let app = app.clone();

    execute_write_batch(request, app.max_batch_items(), move |index, item| {
        let app = app.clone();
        let batch_id = batch_id.clone();
        async move {
            let target = Some(item.path.clone());
            let result = filesystem::delete(&app, item).await;
            audit_result(&app, &batch_id, "file_delete", index, target, &result).await;
            result
        }
    })
    .await
}

async fn file_search(app: &ForgeMcp, arguments: Value) -> Result<impl Serialize, ToolError> {
    let request = decode::<BatchRequest<FileSearchItem>>(arguments)?;
    let batch_id = Uuid::new_v4().to_string();
    let app = app.clone();

    execute_read_batch(
        request,
        app.max_batch_items(),
        8,
        app.max_concurrency(),
        move |index, item| {
            let app = app.clone();
            let batch_id = batch_id.clone();
            async move {
                let target = Some(item.path.clone());
                let result = filesystem::search(&app, item).await;
                audit_result(&app, &batch_id, "file_search", index, target, &result).await;
                result
            }
        },
    )
    .await
}

async fn file_patch(app: &ForgeMcp, arguments: Value) -> Result<impl Serialize, ToolError> {
    let request = decode::<BatchRequest<FilePatchItem>>(arguments)?;
    let batch_id = Uuid::new_v4().to_string();
    let app = app.clone();

    execute_write_batch(request, app.max_batch_items(), move |index, item| {
        let app = app.clone();
        let batch_id = batch_id.clone();
        async move {
            let result = patch::apply(&app, item).await;
            audit_result(
                &app,
                &batch_id,
                "file_patch",
                index,
                Some("unified-diff".to_string()),
                &result,
            )
            .await;
            result
        }
    })
    .await
}

async fn workspace_info(app: &ForgeMcp) -> Result<WorkspaceInfo, ToolError> {
    let info = WorkspaceInfo {
        root: app.workspace_root().display().to_string(),
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        shell: std::env::var("SHELL").ok(),
        git: git_info(app).await,
    };

    let batch_id = Uuid::new_v4().to_string();
    let success: Result<(), ToolError> = Ok(());
    app.audit()
        .record(
            &batch_id,
            "workspace_info",
            0,
            Some("workspace".to_string()),
            &success,
        )
        .await;
    Ok(info)
}

async fn audit_list(app: &ForgeMcp, arguments: Value) -> Result<impl Serialize, ToolError> {
    let request = decode::<AuditListRequest>(arguments)?;
    app.audit().list(&request).await
}

async fn audit_result<T>(
    app: &ForgeMcp,
    batch_id: &str,
    tool: &str,
    index: usize,
    target: Option<String>,
    result: &Result<T, ToolError>,
) {
    let audit_result = result.as_ref().map(|_| ()).map_err(|error| error.clone());
    app.audit()
        .record(batch_id, tool, index, target, &audit_result)
        .await;
}

fn decode<T>(arguments: Value) -> Result<T, ToolError>
where
    T: DeserializeOwned,
{
    serde_json::from_value(arguments).map_err(|error| {
        ToolError::new(
            "INVALID_ARGUMENTS",
            format!("tool arguments do not match the input schema: {error}"),
        )
    })
}

fn json_result<T>(value: T) -> CallToolResult
where
    T: Serialize,
{
    match serde_json::to_string_pretty(&value) {
        Ok(text) => CallToolResult::text(text),
        Err(error) => CallToolResult::tool_error(format!("RESULT_SERIALIZATION_FAILED: {error}")),
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
