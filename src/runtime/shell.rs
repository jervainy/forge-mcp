use std::{
    collections::HashMap,
    process::Stdio,
    time::{Duration, Instant},
};

use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    time::timeout,
};

use crate::{
    app::ForgeMcp,
    batch::ToolError,
    tools::{ShellRunItem, ShellRunResult},
};

pub async fn run(app: &ForgeMcp, item: ShellRunItem) -> Result<ShellRunResult, ToolError> {
    if item.command.trim().is_empty() {
        return Err(ToolError::new(
            "INVALID_COMMAND",
            "command must not be empty",
        ));
    }

    let cwd = match item.cwd.as_deref() {
        Some(path) => app.workspace().resolve_existing(path)?,
        None => app.workspace_root().to_path_buf(),
    };
    if !cwd.is_dir() {
        return Err(ToolError::new(
            "INVALID_CWD",
            format!("{} is not a directory", app.workspace().display_path(&cwd)),
        ));
    }

    let timeout_ms = item
        .timeout_ms
        .unwrap_or(app.default_shell_timeout_ms())
        .clamp(1, 24 * 60 * 60 * 1000);
    let command_text = item.command.clone();

    let mut command = shell_command(&item.command);
    command
        .current_dir(&cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    apply_env(&mut command, item.env)?;

    let started = Instant::now();
    let mut child = command.spawn().map_err(|error| {
        ToolError::new("SHELL_SPAWN_FAILED", format!("failed to start command: {error}"))
    })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ToolError::new("SHELL_IO_FAILED", "stdout pipe was not created"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ToolError::new("SHELL_IO_FAILED", "stderr pipe was not created"))?;

    let output_limit = app.max_shell_output_bytes();
    let stdout_task = tokio::spawn(read_limited(stdout, output_limit));
    let stderr_task = tokio::spawn(read_limited(stderr, output_limit));

    let (status, timed_out) = match timeout(Duration::from_millis(timeout_ms), child.wait()).await {
        Ok(result) => (
            result.map_err(|error| {
                ToolError::new("SHELL_WAIT_FAILED", format!("failed waiting for command: {error}"))
            })?,
            false,
        ),
        Err(_) => {
            let _ = child.kill().await;
            let status = child.wait().await.map_err(|error| {
                ToolError::new(
                    "SHELL_KILL_FAILED",
                    format!("failed waiting after timeout: {error}"),
                )
            })?;
            (status, true)
        }
    };

    let (stdout, stdout_truncated) = stdout_task
        .await
        .map_err(|error| ToolError::new("SHELL_IO_FAILED", error.to_string()))?
        .map_err(|error| ToolError::new("SHELL_IO_FAILED", error.to_string()))?;
    let (stderr, stderr_truncated) = stderr_task
        .await
        .map_err(|error| ToolError::new("SHELL_IO_FAILED", error.to_string()))?
        .map_err(|error| ToolError::new("SHELL_IO_FAILED", error.to_string()))?;

    Ok(ShellRunResult {
        command: command_text,
        cwd: app.workspace().display_path(&cwd),
        exit_code: status.code(),
        stdout,
        stderr,
        duration_ms: started.elapsed().as_millis(),
        timed_out,
        stdout_truncated,
        stderr_truncated,
    })
}

#[cfg(unix)]
fn shell_command(command: &str) -> Command {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let mut process = Command::new(shell);
    process.arg("-lc").arg(command);
    process
}

#[cfg(windows)]
fn shell_command(command: &str) -> Command {
    let mut process = Command::new("cmd.exe");
    process.arg("/D").arg("/S").arg("/C").arg(command);
    process
}

fn apply_env(
    command: &mut Command,
    env: Option<HashMap<String, String>>,
) -> Result<(), ToolError> {
    if let Some(env) = env {
        for (key, value) in env {
            if key.is_empty()
                || key.contains('=')
                || key.contains('\0')
                || value.contains('\0')
            {
                return Err(ToolError::new(
                    "INVALID_ENV",
                    format!("invalid environment variable name: {key:?}"),
                ));
            }
            command.env(key, value);
        }
    }
    Ok(())
}

async fn read_limited<R>(mut reader: R, limit: usize) -> std::io::Result<(String, bool)>
where
    R: AsyncRead + Unpin,
{
    let mut stored = Vec::with_capacity(limit.min(64 * 1024));
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;

    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            break;
        }

        if stored.len() < limit {
            let remaining = limit - stored.len();
            let keep = remaining.min(count);
            stored.extend_from_slice(&buffer[..keep]);
            if keep < count {
                truncated = true;
            }
        } else {
            truncated = true;
        }
    }

    Ok((String::from_utf8_lossy(&stored).into_owned(), truncated))
}
