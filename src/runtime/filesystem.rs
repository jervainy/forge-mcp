use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use globset::{GlobBuilder, GlobMatcher};
use regex::{Regex, RegexBuilder};
use tokio::fs;
use walkdir::WalkDir;

use crate::{
    app::ForgeMcp,
    batch::ToolError,
    tools::{
        FileDeleteItem, FileDeleteResult, FileEditItem, FileEditOperation, FileEditResult,
        FileEntry, FileEntryKind, FileListItem, FileReadItem, FileReadResult, FileSearchItem,
        FileSearchMatch, FileWriteItem, FileWriteMode, FileWriteResult,
    },
};

const MAX_TEXT_SCAN_BYTES: u64 = 16 * 1024 * 1024;
const MAX_LIST_DEPTH: usize = 64;
const DEFAULT_SEARCH_RESULTS: usize = 200;
const MAX_SEARCH_RESULTS: usize = 5000;

pub async fn list(app: &ForgeMcp, item: FileListItem) -> Result<Vec<FileEntry>, ToolError> {
    let root = app.workspace().resolve_existing(&item.path)?;
    if !root.is_dir() {
        return Err(ToolError::new(
            "NOT_A_DIRECTORY",
            format!("{} is not a directory", app.workspace().display_path(&root)),
        ));
    }

    let workspace = app.workspace().clone();
    let depth = item.depth.unwrap_or(1).clamp(1, MAX_LIST_DEPTH);
    let include_hidden = item.include_hidden.unwrap_or(false);

    tokio::task::spawn_blocking(move || {
        let mut entries = Vec::new();
        for entry in WalkDir::new(&root)
            .follow_links(false)
            .min_depth(1)
            .max_depth(depth)
        {
            let entry = entry.map_err(|error| {
                ToolError::new("FILE_LIST_FAILED", format!("directory walk failed: {error}"))
            })?;

            if !include_hidden && is_hidden(entry.path(), &root) {
                if entry.file_type().is_dir() {
                    continue;
                }
                continue;
            }

            let metadata = std::fs::symlink_metadata(entry.path()).map_err(|error| {
                ToolError::new("FILE_LIST_FAILED", format!("metadata failed: {error}"))
            })?;
            let kind = if metadata.file_type().is_symlink() {
                FileEntryKind::Symlink
            } else if metadata.is_file() {
                FileEntryKind::File
            } else if metadata.is_dir() {
                FileEntryKind::Directory
            } else {
                FileEntryKind::Other
            };

            entries.push(FileEntry {
                path: workspace.display_path(entry.path()),
                name: entry.file_name().to_string_lossy().into_owned(),
                kind,
                size: metadata.is_file().then_some(metadata.len()),
            });
        }

        entries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(entries)
    })
    .await
    .map_err(|error| ToolError::new("FILE_LIST_FAILED", error.to_string()))?
}

pub async fn read(app: &ForgeMcp, item: FileReadItem) -> Result<FileReadResult, ToolError> {
    let path = app.workspace().resolve_existing(&item.path)?;
    let metadata = fs::metadata(&path)
        .await
        .map_err(|error| ToolError::new("FILE_READ_FAILED", error.to_string()))?;
    if !metadata.is_file() {
        return Err(ToolError::new("NOT_A_FILE", "file_read requires a file"));
    }
    if metadata.len() > MAX_TEXT_SCAN_BYTES {
        return Err(ToolError::new(
            "FILE_TOO_LARGE",
            format!(
                "text file is {} bytes; maximum scan size is {MAX_TEXT_SCAN_BYTES}",
                metadata.len()
            ),
        ));
    }

    let bytes = fs::read(&path)
        .await
        .map_err(|error| ToolError::new("FILE_READ_FAILED", error.to_string()))?;
    let text = String::from_utf8(bytes).map_err(|_| {
        ToolError::new("NOT_UTF8", "file_read currently supports UTF-8 text files")
    })?;

    let start_line = item.start_line.unwrap_or(1);
    if start_line == 0 {
        return Err(ToolError::new(
            "INVALID_RANGE",
            "start_line is 1-based and must be at least 1",
        ));
    }
    let line_count = item.line_count.unwrap_or(200).clamp(1, 100_000);

    let lines = text.lines().collect::<Vec<_>>();
    let total_lines = lines.len();
    let start_index = start_line.saturating_sub(1).min(total_lines);
    let end_index = start_index.saturating_add(line_count).min(total_lines);
    let selected = lines[start_index..end_index].join("\n");
    let (content, truncated) = truncate_utf8(selected, app.max_read_bytes());

    Ok(FileReadResult {
        path: app.workspace().display_path(&path),
        start_line,
        end_line: if end_index > start_index { end_index } else { 0 },
        total_lines,
        content,
        truncated,
    })
}

pub async fn write(app: &ForgeMcp, item: FileWriteItem) -> Result<FileWriteResult, ToolError> {
    if item.content.len() > app.max_write_bytes() {
        return Err(ToolError::new(
            "CONTENT_TOO_LARGE",
            format!(
                "content is {} bytes; maximum is {}",
                item.content.len(),
                app.max_write_bytes()
            ),
        ));
    }

    let path = app.workspace().resolve_target(&item.path)?;
    if app.workspace().is_root(&path) {
        return Err(ToolError::new("INVALID_TARGET", "cannot overwrite workspace root"));
    }

    let existed = path.exists();
    match item.mode.unwrap_or(FileWriteMode::CreateOrOverwrite) {
        FileWriteMode::Create if existed => {
            return Err(ToolError::new("ALREADY_EXISTS", "target already exists"));
        }
        FileWriteMode::Overwrite if !existed => {
            return Err(ToolError::new("PATH_NOT_FOUND", "target does not exist"));
        }
        _ => {}
    }

    if existed && path.is_dir() {
        return Err(ToolError::new("NOT_A_FILE", "target is a directory"));
    }

    if item.create_parent_dirs {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|error| ToolError::new("FILE_WRITE_FAILED", error.to_string()))?;
        }
    } else if path.parent().is_some_and(|parent| !parent.exists()) {
        return Err(ToolError::new(
            "PARENT_NOT_FOUND",
            "parent directory does not exist",
        ));
    }

    fs::write(&path, item.content.as_bytes())
        .await
        .map_err(|error| ToolError::new("FILE_WRITE_FAILED", error.to_string()))?;

    Ok(FileWriteResult {
        path: app.workspace().display_path(&path),
        bytes_written: item.content.len(),
        created: !existed,
    })
}

pub async fn edit(app: &ForgeMcp, item: FileEditItem) -> Result<FileEditResult, ToolError> {
    let path = app.workspace().resolve_existing(&item.path)?;
    let metadata = fs::metadata(&path)
        .await
        .map_err(|error| ToolError::new("FILE_EDIT_FAILED", error.to_string()))?;
    if !metadata.is_file() {
        return Err(ToolError::new("NOT_A_FILE", "file_edit requires a file"));
    }
    if metadata.len() as usize > app.max_write_bytes() {
        return Err(ToolError::new(
            "FILE_TOO_LARGE",
            "file is larger than the configured edit limit",
        ));
    }

    let original = fs::read_to_string(&path)
        .await
        .map_err(|error| ToolError::new("FILE_EDIT_FAILED", error.to_string()))?;

    let (updated, replacements) = match item.operation {
        FileEditOperation::Replace {
            old,
            new,
            replace_all,
        } => {
            if old.is_empty() {
                return Err(ToolError::new(
                    "INVALID_EDIT",
                    "old string must not be empty",
                ));
            }
            let count = original.matches(&old).count();
            if count == 0 {
                return Err(ToolError::new("NO_MATCH", "old string was not found"));
            }
            if replace_all {
                (original.replace(&old, &new), count)
            } else {
                (original.replacen(&old, &new, 1), 1)
            }
        }
        FileEditOperation::ReplaceLines {
            start_line,
            end_line,
            content,
        } => {
            if start_line == 0 || end_line < start_line {
                return Err(ToolError::new(
                    "INVALID_RANGE",
                    "line range must be 1-based with end_line >= start_line",
                ));
            }

            let trailing_newline = original.ends_with('\n');
            let mut lines = original.lines().map(ToString::to_string).collect::<Vec<_>>();
            if end_line > lines.len() {
                return Err(ToolError::new(
                    "INVALID_RANGE",
                    format!("end_line {end_line} exceeds {} lines", lines.len()),
                ));
            }

            let replacement = content.lines().map(ToString::to_string).collect::<Vec<_>>();
            lines.splice(start_line - 1..end_line, replacement);
            let mut updated = lines.join("\n");
            if trailing_newline && !updated.is_empty() {
                updated.push('\n');
            }
            (updated, end_line - start_line + 1)
        }
    };

    if updated.len() > app.max_write_bytes() {
        return Err(ToolError::new(
            "CONTENT_TOO_LARGE",
            "edited file exceeds configured write limit",
        ));
    }

    fs::write(&path, updated.as_bytes())
        .await
        .map_err(|error| ToolError::new("FILE_EDIT_FAILED", error.to_string()))?;

    Ok(FileEditResult {
        path: app.workspace().display_path(&path),
        replacements,
        bytes_written: updated.len(),
    })
}

pub async fn delete(app: &ForgeMcp, item: FileDeleteItem) -> Result<FileDeleteResult, ToolError> {
    let path = app.workspace().resolve_target(&item.path)?;
    if app.workspace().is_root(&path) {
        return Err(ToolError::new(
            "INVALID_TARGET",
            "workspace root cannot be deleted",
        ));
    }

    let metadata = match fs::symlink_metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && item.ignore_missing => {
            return Ok(FileDeleteResult {
                path: app.workspace().display_path(&path),
                deleted: false,
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ToolError::new("PATH_NOT_FOUND", "target does not exist"));
        }
        Err(error) => return Err(ToolError::new("FILE_DELETE_FAILED", error.to_string())),
    };

    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        if item.recursive {
            fs::remove_dir_all(&path)
                .await
                .map_err(|error| ToolError::new("FILE_DELETE_FAILED", error.to_string()))?;
        } else {
            fs::remove_dir(&path)
                .await
                .map_err(|error| ToolError::new("FILE_DELETE_FAILED", error.to_string()))?;
        }
    } else {
        fs::remove_file(&path)
            .await
            .map_err(|error| ToolError::new("FILE_DELETE_FAILED", error.to_string()))?;
    }

    Ok(FileDeleteResult {
        path: app.workspace().display_path(&path),
        deleted: true,
    })
}

pub async fn search(
    app: &ForgeMcp,
    item: FileSearchItem,
) -> Result<Vec<FileSearchMatch>, ToolError> {
    let root = app.workspace().resolve_existing(&item.path)?;
    let workspace = app.workspace().clone();
    let max_results = item
        .max_results
        .unwrap_or(DEFAULT_SEARCH_RESULTS)
        .clamp(1, MAX_SEARCH_RESULTS);
    let glob = build_glob(item.glob.as_deref())?;
    let matcher = build_matcher(
        item.pattern.as_deref(),
        item.regex.unwrap_or(false),
        item.case_sensitive.unwrap_or(true),
    )?;

    tokio::task::spawn_blocking(move || {
        let mut results = Vec::new();
        let walker = if root.is_file() {
            WalkDir::new(&root).max_depth(0)
        } else {
            WalkDir::new(&root).follow_links(false)
        };

        for entry in walker {
            if results.len() >= max_results {
                break;
            }

            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            if !entry.file_type().is_file() {
                continue;
            }

            let display_path = workspace.display_path(entry.path());
            if glob
                .as_ref()
                .is_some_and(|matcher| !matcher.is_match(&display_path))
            {
                continue;
            }

            let Some(matcher) = matcher.as_ref() else {
                results.push(FileSearchMatch {
                    path: display_path,
                    line: None,
                    column: None,
                    text: None,
                });
                continue;
            };

            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.len() > MAX_TEXT_SCAN_BYTES {
                continue;
            }

            let Ok(file) = File::open(entry.path()) else {
                continue;
            };
            for (line_index, line) in BufReader::new(file).lines().enumerate() {
                if results.len() >= max_results {
                    break;
                }
                let Ok(line) = line else {
                    break;
                };

                if let Some(column) = matcher.find_column(&line) {
                    results.push(FileSearchMatch {
                        path: display_path.clone(),
                        line: Some(line_index + 1),
                        column: Some(column),
                        text: Some(truncate_chars(&line, 1000)),
                    });
                }
            }
        }

        Ok(results)
    })
    .await
    .map_err(|error| ToolError::new("FILE_SEARCH_FAILED", error.to_string()))?
}

fn is_hidden(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root)
        .ok()
        .is_some_and(|relative| {
            relative
                .components()
                .any(|component| component.as_os_str().to_string_lossy().starts_with('.'))
        })
}

fn build_glob(value: Option<&str>) -> Result<Option<GlobMatcher>, ToolError> {
    value
        .map(|pattern| {
            GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
                .map(|glob| glob.compile_matcher())
                .map_err(|error| ToolError::new("INVALID_GLOB", error.to_string()))
        })
        .transpose()
}

enum TextMatcher {
    Regex(Regex),
    Literal {
        needle: String,
        case_sensitive: bool,
    },
}

impl TextMatcher {
    fn find_column(&self, line: &str) -> Option<usize> {
        let byte_index = match self {
            Self::Regex(regex) => regex.find(line)?.start(),
            Self::Literal {
                needle,
                case_sensitive: true,
            } => line.find(needle)?,
            Self::Literal {
                needle,
                case_sensitive: false,
            } => line.to_lowercase().find(needle)?,
        };
        Some(line[..byte_index].chars().count() + 1)
    }
}

fn build_matcher(
    pattern: Option<&str>,
    regex: bool,
    case_sensitive: bool,
) -> Result<Option<TextMatcher>, ToolError> {
    let Some(pattern) = pattern else {
        return Ok(None);
    };

    if regex {
        RegexBuilder::new(pattern)
            .case_insensitive(!case_sensitive)
            .build()
            .map(TextMatcher::Regex)
            .map(Some)
            .map_err(|error| ToolError::new("INVALID_REGEX", error.to_string()))
    } else {
        Ok(Some(TextMatcher::Literal {
            needle: if case_sensitive {
                pattern.to_string()
            } else {
                pattern.to_lowercase()
            },
            case_sensitive,
        }))
    }
}

fn truncate_utf8(mut value: String, limit: usize) -> (String, bool) {
    if value.len() <= limit {
        return (value, false);
    }

    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    (value, true)
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub fn target_for_path(item_path: &str) -> Option<String> {
    Some(item_path.to_string())
}

pub fn path_parent(path: &Path) -> Option<PathBuf> {
    path.parent().map(Path::to_path_buf)
}
