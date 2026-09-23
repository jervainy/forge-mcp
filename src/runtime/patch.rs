use tokio::fs;

use crate::{
    app::ForgeMcp,
    batch::ToolError,
    tools::{FilePatchItem, FilePatchResult, PatchFileResult},
};

#[derive(Debug)]
struct ParsedFilePatch {
    old_path: Option<String>,
    new_path: Option<String>,
    hunks: Vec<Hunk>,
}

#[derive(Debug)]
struct Hunk {
    old_start: usize,
    old_count: usize,
    new_count: usize,
    lines: Vec<HunkLine>,
}

#[derive(Debug)]
enum HunkLine {
    Context(String),
    Remove(String),
    Add(String),
}

pub async fn apply(app: &ForgeMcp, item: FilePatchItem) -> Result<FilePatchResult, ToolError> {
    if item.patch.len() > app.max_write_bytes() {
        return Err(ToolError::new(
            "PATCH_TOO_LARGE",
            format!(
                "patch is {} bytes; maximum is {}",
                item.patch.len(),
                app.max_write_bytes()
            ),
        ));
    }

    let patches = parse_patch(&item.patch)?;
    if patches.is_empty() {
        return Err(ToolError::new(
            "INVALID_PATCH",
            "unified diff contains no file patches",
        ));
    }

    let mut files = Vec::with_capacity(patches.len());
    for patch in patches {
        files.push(apply_file_patch(app, patch).await?);
    }

    Ok(FilePatchResult { files })
}

async fn apply_file_patch(
    app: &ForgeMcp,
    patch: ParsedFilePatch,
) -> Result<PatchFileResult, ToolError> {
    let original_path = match patch.old_path.as_deref() {
        Some(path) => Some(app.workspace().resolve_existing(path)?),
        None => None,
    };
    let destination_path = match patch.new_path.as_deref() {
        Some(path) => Some(app.workspace().resolve_target(path)?),
        None => None,
    };

    if original_path
        .as_ref()
        .is_some_and(|path| app.workspace().is_root(path))
        || destination_path
            .as_ref()
            .is_some_and(|path| app.workspace().is_root(path))
    {
        return Err(ToolError::new(
            "INVALID_PATCH_TARGET",
            "patch cannot replace or delete the workspace root",
        ));
    }

    let original = match original_path.as_ref() {
        Some(path) => {
            let metadata = fs::metadata(path)
                .await
                .map_err(|error| ToolError::new("PATCH_READ_FAILED", error.to_string()))?;
            if !metadata.is_file() {
                return Err(ToolError::new(
                    "PATCH_TARGET_NOT_FILE",
                    "patch target is not a regular file",
                ));
            }
            if metadata.len() as usize > app.max_write_bytes() {
                return Err(ToolError::new(
                    "PATCH_TARGET_TOO_LARGE",
                    "patch target exceeds configured write limit",
                ));
            }
            fs::read_to_string(path)
                .await
                .map_err(|error| ToolError::new("PATCH_READ_FAILED", error.to_string()))?
        }
        None => String::new(),
    };

    let updated = apply_hunks(&original, &patch.hunks)?;
    if updated.len() > app.max_write_bytes() {
        return Err(ToolError::new(
            "PATCH_RESULT_TOO_LARGE",
            "patched file exceeds configured write limit",
        ));
    }

    let action = match (original_path.as_ref(), destination_path.as_ref()) {
        (Some(old), None) => {
            fs::remove_file(old)
                .await
                .map_err(|error| ToolError::new("PATCH_DELETE_FAILED", error.to_string()))?;
            files_result(app, old, "deleted")
        }
        (None, Some(new)) => {
            if let Some(parent) = new.parent() {
                fs::create_dir_all(parent)
                    .await
                    .map_err(|error| ToolError::new("PATCH_WRITE_FAILED", error.to_string()))?;
            }
            if new.exists() {
                return Err(ToolError::new(
                    "PATCH_TARGET_EXISTS",
                    format!("{} already exists", app.workspace().display_path(new)),
                ));
            }
            fs::write(new, updated.as_bytes())
                .await
                .map_err(|error| ToolError::new("PATCH_WRITE_FAILED", error.to_string()))?;
            files_result(app, new, "created")
        }
        (Some(old), Some(new)) => {
            if let Some(parent) = new.parent() {
                fs::create_dir_all(parent)
                    .await
                    .map_err(|error| ToolError::new("PATCH_WRITE_FAILED", error.to_string()))?;
            }
            fs::write(new, updated.as_bytes())
                .await
                .map_err(|error| ToolError::new("PATCH_WRITE_FAILED", error.to_string()))?;

            if old != new {
                fs::remove_file(old)
                    .await
                    .map_err(|error| ToolError::new("PATCH_RENAME_FAILED", error.to_string()))?;
                files_result(app, new, "renamed")
            } else {
                files_result(app, new, "modified")
            }
        }
        (None, None) => {
            return Err(ToolError::new(
                "INVALID_PATCH",
                "both old and new paths cannot be /dev/null",
            ));
        }
    };

    Ok(action)
}

fn files_result(
    app: &ForgeMcp,
    path: &std::path::Path,
    action: &str,
) -> PatchFileResult {
    PatchFileResult {
        path: app.workspace().display_path(path),
        action: action.to_string(),
    }
}

fn apply_hunks(original: &str, hunks: &[Hunk]) -> Result<String, ToolError> {
    if hunks.is_empty() {
        return Ok(original.to_string());
    }

    let trailing_newline = original.ends_with('\n');
    let original_lines = original.lines().collect::<Vec<_>>();
    let mut output = Vec::<String>::new();
    let mut cursor = 0_usize;

    for hunk in hunks {
        let start = if hunk.old_start == 0 {
            0
        } else {
            hunk.old_start - 1
        };
        if start < cursor || start > original_lines.len() {
            return Err(ToolError::new(
                "PATCH_CONTEXT_MISMATCH",
                "hunk starts outside the expected source range",
            ));
        }

        output.extend(
            original_lines[cursor..start]
                .iter()
                .map(|line| (*line).to_string()),
        );
        cursor = start;

        let mut old_seen = 0_usize;
        let mut new_seen = 0_usize;
        for line in &hunk.lines {
            match line {
                HunkLine::Context(expected) => {
                    let actual = original_lines.get(cursor).ok_or_else(|| {
                        ToolError::new(
                            "PATCH_CONTEXT_MISMATCH",
                            "context extends beyond end of file",
                        )
                    })?;
                    if actual != expected {
                        return Err(ToolError::new(
                            "PATCH_CONTEXT_MISMATCH",
                            format!("expected context {expected:?}, found {actual:?}"),
                        ));
                    }
                    output.push(expected.clone());
                    cursor += 1;
                    old_seen += 1;
                    new_seen += 1;
                }
                HunkLine::Remove(expected) => {
                    let actual = original_lines.get(cursor).ok_or_else(|| {
                        ToolError::new(
                            "PATCH_CONTEXT_MISMATCH",
                            "deletion extends beyond end of file",
                        )
                    })?;
                    if actual != expected {
                        return Err(ToolError::new(
                            "PATCH_CONTEXT_MISMATCH",
                            format!("expected deletion {expected:?}, found {actual:?}"),
                        ));
                    }
                    cursor += 1;
                    old_seen += 1;
                }
                HunkLine::Add(value) => {
                    output.push(value.clone());
                    new_seen += 1;
                }
            }
        }

        if old_seen != hunk.old_count || new_seen != hunk.new_count {
            return Err(ToolError::new(
                "INVALID_PATCH",
                "hunk line counts do not match its header",
            ));
        }
    }

    output.extend(original_lines[cursor..].iter().map(|line| (*line).to_string()));
    let mut result = output.join("\n");
    if (trailing_newline || !result.is_empty()) && !result.is_empty() {
        result.push('\n');
    }
    Ok(result)
}

fn parse_patch(value: &str) -> Result<Vec<ParsedFilePatch>, ToolError> {
    let lines = value.lines().collect::<Vec<_>>();
    let mut index = 0_usize;
    let mut files = Vec::new();

    while index < lines.len() {
        if !lines[index].starts_with("--- ") {
            index += 1;
            continue;
        }

        let old_path = parse_path(lines[index].trim_start_matches("--- "))?;
        index += 1;
        let new_header = lines.get(index).ok_or_else(|| {
            ToolError::new("INVALID_PATCH", "missing +++ header after --- header")
        })?;
        if !new_header.starts_with("+++ ") {
            return Err(ToolError::new(
                "INVALID_PATCH",
                "missing +++ header after --- header",
            ));
        }
        let new_path = parse_path(new_header.trim_start_matches("+++ "))?;
        index += 1;

        let mut hunks = Vec::new();
        while index < lines.len() && !lines[index].starts_with("--- ") {
            if !lines[index].starts_with("@@ ") {
                index += 1;
                continue;
            }

            let (old_start, old_count, new_count) = parse_hunk_header(lines[index])?;
            index += 1;
            let mut hunk_lines = Vec::new();
            let mut old_seen = 0_usize;
            let mut new_seen = 0_usize;

            while index < lines.len() && (old_seen < old_count || new_seen < new_count) {
                let line = lines[index];
                index += 1;

                if line.starts_with("\\ No newline at end of file") {
                    continue;
                }

                let (kind, text) = line.split_at_checked(1).ok_or_else(|| {
                    ToolError::new("INVALID_PATCH", "empty line inside hunk")
                })?;
                match kind {
                    " " => {
                        hunk_lines.push(HunkLine::Context(text.to_string()));
                        old_seen += 1;
                        new_seen += 1;
                    }
                    "-" => {
                        hunk_lines.push(HunkLine::Remove(text.to_string()));
                        old_seen += 1;
                    }
                    "+" => {
                        hunk_lines.push(HunkLine::Add(text.to_string()));
                        new_seen += 1;
                    }
                    _ => {
                        return Err(ToolError::new(
                            "INVALID_PATCH",
                            format!("invalid hunk line prefix: {kind:?}"),
                        ));
                    }
                }
            }

            if old_seen != old_count || new_seen != new_count {
                return Err(ToolError::new(
                    "INVALID_PATCH",
                    "hunk ended before declared line counts were satisfied",
                ));
            }

            hunks.push(Hunk {
                old_start,
                old_count,
                new_count,
                lines: hunk_lines,
            });
        }

        files.push(ParsedFilePatch {
            old_path,
            new_path,
            hunks,
        });
    }

    Ok(files)
}

fn parse_path(value: &str) -> Result<Option<String>, ToolError> {
    let raw = value.split('\t').next().unwrap_or(value).trim();
    if raw == "/dev/null" {
        return Ok(None);
    }
    if raw.is_empty() {
        return Err(ToolError::new("INVALID_PATCH", "patch path is empty"));
    }

    let path = raw
        .strip_prefix("a/")
        .or_else(|| raw.strip_prefix("b/"))
        .unwrap_or(raw);
    Ok(Some(path.to_string()))
}

fn parse_hunk_header(value: &str) -> Result<(usize, usize, usize), ToolError> {
    let body = value
        .strip_prefix("@@ ")
        .and_then(|value| value.split(" @@").next())
        .ok_or_else(|| ToolError::new("INVALID_PATCH", "invalid hunk header"))?;
    let mut ranges = body.split_whitespace();
    let old = ranges
        .next()
        .ok_or_else(|| ToolError::new("INVALID_PATCH", "missing old hunk range"))?;
    let new = ranges
        .next()
        .ok_or_else(|| ToolError::new("INVALID_PATCH", "missing new hunk range"))?;

    let (old_start, old_count) = parse_range(old, '-')?;
    let (_, new_count) = parse_range(new, '+')?;
    Ok((old_start, old_count, new_count))
}

fn parse_range(value: &str, prefix: char) -> Result<(usize, usize), ToolError> {
    let value = value.strip_prefix(prefix).ok_or_else(|| {
        ToolError::new("INVALID_PATCH", format!("range must start with {prefix}"))
    })?;
    let (start, count) = value.split_once(',').unwrap_or((value, "1"));
    let start = start
        .parse::<usize>()
        .map_err(|_| ToolError::new("INVALID_PATCH", "invalid hunk start"))?;
    let count = count
        .parse::<usize>()
        .map_err(|_| ToolError::new("INVALID_PATCH", "invalid hunk count"))?;
    Ok((start, count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_simple_unified_diff() {
        let patch = parse_patch(
            "--- a/test.txt\n+++ b/test.txt\n@@ -1,2 +1,2 @@\n hello\n-world\n+rust\n",
        )
        .unwrap();
        let updated = apply_hunks("hello\nworld\n", &patch[0].hunks).unwrap();
        assert_eq!(updated, "hello\nrust\n");
    }
}
