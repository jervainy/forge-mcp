use std::path::{Component, Path, PathBuf};

use crate::batch::ToolError;

#[derive(Debug, Clone)]
pub struct WorkspaceGuard {
    root: PathBuf,
}

impl WorkspaceGuard {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn resolve_existing(&self, value: &str) -> Result<PathBuf, ToolError> {
        let candidate = self.lexical_candidate(value)?;
        let resolved = std::fs::canonicalize(&candidate).map_err(|error| {
            ToolError::new(
                "PATH_NOT_FOUND",
                format!("{}: {error}", self.display_path(&candidate)),
            )
        })?;
        self.ensure_inside(&resolved)?;
        Ok(resolved)
    }

    pub fn resolve_target(&self, value: &str) -> Result<PathBuf, ToolError> {
        let candidate = self.lexical_candidate(value)?;

        if candidate.exists() {
            let resolved = std::fs::canonicalize(&candidate).map_err(|error| {
                ToolError::new(
                    "PATH_RESOLUTION_FAILED",
                    format!("{}: {error}", self.display_path(&candidate)),
                )
            })?;
            self.ensure_inside(&resolved)?;
            return Ok(candidate);
        }

        let mut ancestor = candidate.as_path();
        while !ancestor.exists() {
            ancestor = ancestor.parent().ok_or_else(|| {
                ToolError::new("PATH_OUTSIDE_WORKSPACE", "path escapes workspace")
            })?;
        }

        let resolved_ancestor = std::fs::canonicalize(ancestor)
            .map_err(|error| ToolError::new("PATH_RESOLUTION_FAILED", error.to_string()))?;
        self.ensure_inside(&resolved_ancestor)?;
        Ok(candidate)
    }

    pub fn display_path(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    pub fn is_root(&self, path: &Path) -> bool {
        path == self.root
    }

    fn lexical_candidate(&self, value: &str) -> Result<PathBuf, ToolError> {
        let input = Path::new(value);
        let joined = if input.is_absolute() {
            input.to_path_buf()
        } else {
            self.root.join(input)
        };

        let mut normalized = PathBuf::new();
        for component in joined.components() {
            match component {
                Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
                Component::RootDir => normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
                Component::CurDir => {}
                Component::ParentDir => {
                    if !normalized.pop() {
                        return Err(ToolError::new(
                            "PATH_OUTSIDE_WORKSPACE",
                            "path escapes workspace",
                        ));
                    }
                }
                Component::Normal(part) => normalized.push(part),
            }
        }

        if !normalized.starts_with(&self.root) {
            return Err(ToolError::new(
                "PATH_OUTSIDE_WORKSPACE",
                format!("{value} is outside the configured workspace"),
            ));
        }
        Ok(normalized)
    }

    fn ensure_inside(&self, path: &Path) -> Result<(), ToolError> {
        if path.starts_with(&self.root) {
            Ok(())
        } else {
            Err(ToolError::new(
                "PATH_OUTSIDE_WORKSPACE",
                format!(
                    "{} resolves outside the configured workspace",
                    path.display()
                ),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_parent_escape() {
        let root = std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
        let guard = WorkspaceGuard::new(root);
        assert!(guard.resolve_target("../outside").is_err());
    }
}
