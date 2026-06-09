//! File tools (read / write / edit / list) that run unwrapped against a
//! workspace root. There is no sandbox: paths resolve directly on the real
//! filesystem, confined to a configurable root that defaults to the current
//! working directory.

use super::output::{prepare_tool_output, DEFAULT_MAX_OUTPUT_CHARS};
use super::Tool;
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};
use tokio::fs;

/// Resolves a raw path against a workspace root, rejecting escapes.
#[derive(Clone)]
struct FileBoundary {
    root: PathBuf,
}

impl FileBoundary {
    fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn current_dir() -> Self {
        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::new(root)
    }

    fn resolve(&self, raw_path: &str) -> Result<PathBuf> {
        resolve_within(&self.root, raw_path).with_context(|| {
            format!(
                "Could not resolve path `{}`. Retry with a narrower path inside the workspace, such as `src/lib.rs`; avoid `..` escapes and unrelated absolute paths.",
                raw_path
            )
        })
    }
}

macro_rules! file_tool_ctors {
    () => {
        /// Resolve paths against the current working directory.
        pub fn direct() -> Self {
            Self {
                boundary: FileBoundary::current_dir(),
            }
        }

        /// Resolve paths against a specific workspace root.
        pub fn workspace(root: impl Into<PathBuf>) -> Self {
            Self {
                boundary: FileBoundary::new(root.into()),
            }
        }
    };
}

/// Read file tool.
pub struct ReadFileTool {
    boundary: FileBoundary,
}

impl ReadFileTool {
    file_tool_ctors!();
}

impl Default for ReadFileTool {
    fn default() -> Self {
        Self::direct()
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file from the workspace"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start from (1-indexed)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing path parameter"))?;

        let resolved = self.boundary.resolve(path)?;
        let content = fs::read_to_string(&resolved).await.with_context(|| {
            format!(
                "Could not read `{}`. Retry with `list_dir` on its parent directory or a narrower file path inside the workspace.",
                path
            )
        })?;

        let offset = args["offset"].as_u64().unwrap_or(1) as usize;
        let limit = args["limit"].as_u64();

        let lines: Vec<&str> = content.lines().collect();
        let start = offset.saturating_sub(1);

        let selected: Vec<&str> = if let Some(limit) = limit {
            lines.into_iter().skip(start).take(limit as usize).collect()
        } else {
            lines.into_iter().skip(start).collect()
        };

        Ok(prepare_tool_output(
            &selected.join("\n"),
            DEFAULT_MAX_OUTPUT_CHARS,
        ))
    }
}

/// Write file tool.
pub struct WriteFileTool {
    boundary: FileBoundary,
}

impl WriteFileTool {
    file_tool_ctors!();
}

impl Default for WriteFileTool {
    fn default() -> Self {
        Self::direct()
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file in the workspace, creating parent directories if needed"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing path parameter"))?;
        let content = args["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing content parameter"))?;

        let resolved = self.boundary.resolve(path)?;

        // Create parent directories if needed.
        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent).await?;
        }

        fs::write(&resolved, content).await.with_context(|| {
            format!(
                "Could not write `{}`. Retry with a path inside the workspace and content small enough to review.",
                path
            )
        })?;

        Ok(format!(
            "Successfully wrote {} bytes to {}",
            content.len(),
            path
        ))
    }
}

/// Edit file tool.
pub struct EditFileTool {
    boundary: FileBoundary,
}

impl EditFileTool {
    file_tool_ctors!();
}

impl Default for EditFileTool {
    fn default() -> Self {
        Self::direct()
    }
}

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Edit a file in the workspace by replacing exact text"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file"
                },
                "old_text": {
                    "type": "string",
                    "description": "Exact text to find"
                },
                "new_text": {
                    "type": "string",
                    "description": "Text to replace with"
                }
            },
            "required": ["path", "old_text", "new_text"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing path parameter"))?;
        let old_text = args["old_text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing old_text parameter"))?;
        let new_text = args["new_text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing new_text parameter"))?;
        if old_text.is_empty() {
            anyhow::bail!(
                "old_text must not be empty. Retry with a unique exact snippet from the file."
            );
        }

        let resolved = self.boundary.resolve(path)?;
        let content = fs::read_to_string(&resolved).await.with_context(|| {
            format!(
                "Could not read `{}` for editing. Retry with a path inside the workspace.",
                path
            )
        })?;

        let matches = content.matches(old_text).count();
        if matches == 0 {
            anyhow::bail!(
                "old_text was not found in `{}`. Retry after reading the target lines and copy an exact snippet.",
                path
            );
        }
        if matches > 1 {
            anyhow::bail!(
                "old_text matched {} times in `{}`. Retry with a longer unique snippet or split the edit into smaller replacements.",
                matches,
                path
            );
        }

        let new_content = content.replacen(old_text, new_text, 1);
        fs::write(&resolved, &new_content).await.with_context(|| {
            format!(
                "Could not write edited `{}`. Retry with a path inside the workspace.",
                path
            )
        })?;

        Ok(format!("Successfully edited {}", path))
    }
}

/// List directory tool.
pub struct ListDirTool {
    boundary: FileBoundary,
}

impl ListDirTool {
    file_tool_ctors!();
}

impl Default for ListDirTool {
    fn default() -> Self {
        Self::direct()
    }
}

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "List the contents of a directory from the workspace"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the directory"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing path parameter"))?;

        let resolved = self.boundary.resolve(path)?;
        let mut entries = fs::read_dir(&resolved).await.with_context(|| {
            format!(
                "Could not list `{}`. Retry with a directory inside the workspace.",
                path
            )
        })?;
        let mut results = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            let prefix = if file_type.is_dir() { "d " } else { "f " };
            results.push(format!("{}{}", prefix, entry.file_name().to_string_lossy()));
        }

        results.sort();
        Ok(prepare_tool_output(
            &results.join("\n"),
            DEFAULT_MAX_OUTPUT_CHARS,
        ))
    }
}

fn resolve_within(root: &Path, raw_path: &str) -> Result<PathBuf> {
    let root = root
        .canonicalize()
        .unwrap_or_else(|_| normalize_lexical(root));
    let requested = Path::new(raw_path);
    let mapped = if requested.is_absolute() {
        normalize_existing_or_lexical(requested)
    } else {
        normalize_lexical(&root.join(requested))
    };

    if !is_within_or_same(&mapped, &root) {
        anyhow::bail!(
            "path {} escapes the workspace {}",
            requested.display(),
            root.display()
        );
    }

    Ok(mapped)
}

fn normalize_existing_or_lexical(path: &Path) -> PathBuf {
    path.canonicalize()
        .unwrap_or_else(|_| normalize_lexical(path))
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn is_within_or_same(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs as std_fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn write_read_edit_list_round_trip() {
        let root = tempdir().unwrap();

        // Write.
        let writer = WriteFileTool::workspace(root.path());
        writer
            .execute(json!({"path": "sub/note.txt", "content": "one\ntwo\n"}))
            .await
            .unwrap();
        assert_eq!(
            std_fs::read_to_string(root.path().join("sub/note.txt")).unwrap(),
            "one\ntwo\n"
        );

        // Read.
        let reader = ReadFileTool::workspace(root.path());
        let read = reader
            .execute(json!({"path": "sub/note.txt"}))
            .await
            .unwrap();
        assert_eq!(read, "one\ntwo");

        // Edit.
        let editor = EditFileTool::workspace(root.path());
        editor
            .execute(json!({
                "path": "sub/note.txt",
                "old_text": "one",
                "new_text": "ONE"
            }))
            .await
            .unwrap();
        assert_eq!(
            std_fs::read_to_string(root.path().join("sub/note.txt")).unwrap(),
            "ONE\ntwo\n"
        );

        // List.
        let lister = ListDirTool::workspace(root.path());
        let listing = lister.execute(json!({"path": "sub"})).await.unwrap();
        assert!(listing.contains("f note.txt"));
    }

    #[tokio::test]
    async fn read_offset_and_limit() {
        let root = tempdir().unwrap();
        std_fs::write(root.path().join("lines.txt"), "a\nb\nc\nd\n").unwrap();

        let reader = ReadFileTool::workspace(root.path());
        let out = reader
            .execute(json!({"path": "lines.txt", "offset": 2, "limit": 2}))
            .await
            .unwrap();
        assert_eq!(out, "b\nc");
    }

    #[tokio::test]
    async fn edit_rejects_path_escape() {
        let root = tempdir().unwrap();
        let editor = EditFileTool::workspace(root.path());

        let err = editor
            .execute(json!({
                "path": "../outside.rs",
                "old_text": "one",
                "new_text": "two"
            }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Could not resolve path"));
    }

    #[tokio::test]
    async fn edit_requires_unique_match() {
        let root = tempdir().unwrap();
        std_fs::write(root.path().join("dup.txt"), "one\none\n").unwrap();

        let editor = EditFileTool::workspace(root.path());
        let err = editor
            .execute(json!({
                "path": "dup.txt",
                "old_text": "one",
                "new_text": "two"
            }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("matched 2 times"));
    }
}
