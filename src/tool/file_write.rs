use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::fs;

use crate::error::{ClosedCodeError, Result};
use crate::gemini::types::FunctionDeclaration;
use crate::mode::Mode;
use crate::ui::approval::{ApprovalDecision, ApprovalHandler, FileChange};

use super::{ParamBuilder, Tool};

pub struct WriteFileTool {
    working_directory: PathBuf,
    approval_handler: Arc<dyn ApprovalHandler>,
    protected_paths: Vec<String>,
}

impl WriteFileTool {
    pub fn new(
        working_directory: PathBuf,
        approval_handler: Arc<dyn ApprovalHandler>,
        protected_paths: Vec<String>,
    ) -> Self {
        Self {
            working_directory,
            approval_handler,
            protected_paths,
        }
    }

    fn resolve_path(&self, path: &str) -> PathBuf {
        if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            self.working_directory.join(path)
        }
    }
}

impl std::fmt::Debug for WriteFileTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteFileTool")
            .field("working_directory", &self.working_directory)
            .finish()
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Create a new file or overwrite an existing file with the given content. \
         Shows a unified diff of the changes and requires user approval before writing. \
         Use this to create new files or completely replace file contents. \
         For targeted edits to existing files, prefer edit_file instead."
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name().into(),
            description: self.description().into(),
            parameters: ParamBuilder::new()
                .string("path", "File path relative to working directory", true)
                .string("content", "The complete file content to write", true)
                .build(),
        }
    }

    async fn execute(&self, args: Value) -> Result<Value> {
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| ClosedCodeError::ToolError {
                name: "write_file".into(),
                message: "Missing required parameter 'path'".into(),
            })?;

        let content = args["content"]
            .as_str()
            .ok_or_else(|| ClosedCodeError::ToolError {
                name: "write_file".into(),
                message: "Missing required parameter 'content'".into(),
            })?;

        // Check protected paths
        if super::is_protected_path(path_str, &self.protected_paths) {
            return Err(ClosedCodeError::ProtectedPath {
                path: path_str.to_string(),
            });
        }

        let resolved = self.resolve_path(path_str);

        // Read existing content if file exists
        let (old_content, is_new_file) = if resolved.exists() {
            let existing =
                fs::read_to_string(&resolved)
                    .await
                    .map_err(|e| ClosedCodeError::ToolError {
                        name: "write_file".into(),
                        message: format!("Cannot read existing file '{}': {}", path_str, e),
                    })?;
            (existing, false)
        } else {
            (String::new(), true)
        };

        // Skip if content is identical
        if !is_new_file && old_content == content {
            return Ok(json!({
                "status": "no_change",
                "path": path_str,
                "message": "File content is already identical to the proposed content."
            }));
        }

        let change = FileChange {
            file_path: path_str.to_string(),
            resolved_path: resolved.display().to_string(),
            old_content,
            new_content: content.to_string(),
            is_new_file,
        };

        // Request user approval
        let decision = self.approval_handler.request_approval(&change).await?;

        match decision {
            ApprovalDecision::Approved => {
                // Create parent directories if needed
                if let Some(parent) = resolved.parent() {
                    fs::create_dir_all(parent)
                        .await
                        .map_err(|e| ClosedCodeError::ToolError {
                            name: "write_file".into(),
                            message: format!(
                                "Cannot create directory '{}': {}",
                                parent.display(),
                                e
                            ),
                        })?;
                }

                fs::write(&resolved, content)
                    .await
                    .map_err(|e| ClosedCodeError::ToolError {
                        name: "write_file".into(),
                        message: format!("Cannot write to '{}': {}", path_str, e),
                    })?;

                let action = if is_new_file { "created" } else { "updated" };
                tracing::info!("File {}: {}", action, path_str);

                Ok(json!({
                    "status": "applied",
                    "path": path_str,
                    "action": action,
                }))
            }
            ApprovalDecision::Rejected => {
                tracing::info!("User rejected change to: {}", path_str);
                Ok(json!({
                    "status": "rejected",
                    "reason": "User declined the change",
                    "path": path_str,
                }))
            }
        }
    }

    fn available_modes(&self) -> Vec<Mode> {
        vec![Mode::Guided, Mode::Execute, Mode::Auto]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::approval::AutoApproveHandler;
    use tempfile::TempDir;

    fn setup() -> (TempDir, Arc<dyn ApprovalHandler>) {
        let dir = TempDir::new().unwrap();
        let handler = Arc::new(AutoApproveHandler::always_approve()) as Arc<dyn ApprovalHandler>;
        (dir, handler)
    }

    fn setup_reject() -> (TempDir, Arc<dyn ApprovalHandler>) {
        let dir = TempDir::new().unwrap();
        let handler = Arc::new(AutoApproveHandler::always_reject()) as Arc<dyn ApprovalHandler>;
        (dir, handler)
    }

    #[tokio::test]
    async fn write_new_file_approved() {
        let (dir, handler) = setup();
        let tool = WriteFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        let result = tool
            .execute(json!({
                "path": "hello.rs",
                "content": "fn main() {}"
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "applied");
        assert_eq!(result["action"], "created");

        let written = std::fs::read_to_string(dir.path().join("hello.rs")).unwrap();
        assert_eq!(written, "fn main() {}");
    }

    #[tokio::test]
    async fn write_new_file_rejected() {
        let (dir, handler) = setup_reject();
        let tool = WriteFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        let result = tool
            .execute(json!({
                "path": "hello.rs",
                "content": "fn main() {}"
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "rejected");
        assert!(!dir.path().join("hello.rs").exists());
    }

    #[tokio::test]
    async fn write_existing_file_approved() {
        let (dir, handler) = setup();
        let file_path = dir.path().join("existing.rs");
        std::fs::write(&file_path, "old content").unwrap();

        let tool = WriteFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        let result = tool
            .execute(json!({
                "path": "existing.rs",
                "content": "new content"
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "applied");
        assert_eq!(result["action"], "updated");
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "new content");
    }

    #[tokio::test]
    async fn write_creates_parent_dirs() {
        let (dir, handler) = setup();
        let tool = WriteFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        let result = tool
            .execute(json!({
                "path": "nested/deep/file.rs",
                "content": "content"
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "applied");
        assert!(dir.path().join("nested/deep/file.rs").exists());
    }

    #[tokio::test]
    async fn write_no_change_skips_approval() {
        let (dir, handler) = setup_reject(); // reject handler, but should not be called
        let file_path = dir.path().join("same.rs");
        std::fs::write(&file_path, "same content").unwrap();

        let tool = WriteFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        let result = tool
            .execute(json!({
                "path": "same.rs",
                "content": "same content"
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "no_change");
    }

    #[tokio::test]
    async fn write_missing_path_arg() {
        let (dir, handler) = setup();
        let tool = WriteFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        let result = tool.execute(json!({"content": "x"})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn write_missing_content_arg() {
        let (dir, handler) = setup();
        let tool = WriteFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        let result = tool.execute(json!({"path": "x.rs"})).await;
        assert!(result.is_err());
    }

    #[test]
    fn write_available_modes() {
        let (dir, handler) = setup();
        let tool = WriteFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        assert_eq!(
            tool.available_modes(),
            vec![Mode::Guided, Mode::Execute, Mode::Auto]
        );
    }

    #[test]
    fn write_tool_debug() {
        let (dir, handler) = setup();
        let tool = WriteFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        let debug = format!("{:?}", tool);
        assert!(debug.contains("WriteFileTool"));
    }

    #[tokio::test]
    async fn write_protected_git_path_rejected() {
        let (dir, handler) = setup();
        let tool = WriteFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        let result = tool
            .execute(json!({
                "path": ".git/config",
                "content": "bad"
            }))
            .await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ClosedCodeError::ProtectedPath { .. }
        ));
    }

    #[tokio::test]
    async fn write_protected_closed_code_rejected() {
        let (dir, handler) = setup();
        let tool = WriteFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        let result = tool
            .execute(json!({
                "path": ".closed-code/config.toml",
                "content": "bad"
            }))
            .await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ClosedCodeError::ProtectedPath { .. }
        ));
    }

    #[tokio::test]
    async fn write_protected_env_rejected() {
        let (dir, handler) = setup();
        let tool = WriteFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        let result = tool
            .execute(json!({
                "path": ".env",
                "content": "SECRET=bad"
            }))
            .await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ClosedCodeError::ProtectedPath { .. }
        ));
    }

    #[tokio::test]
    async fn write_protected_pem_rejected() {
        let (dir, handler) = setup();
        let tool = WriteFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        let result = tool
            .execute(json!({
                "path": "secrets/key.pem",
                "content": "bad"
            }))
            .await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ClosedCodeError::ProtectedPath { .. }
        ));
    }

    #[tokio::test]
    async fn write_custom_protected_path() {
        let (dir, handler) = setup();
        let tool = WriteFileTool::new(
            dir.path().to_path_buf(),
            handler,
            vec!["credentials.json".to_string()],
        );
        let result = tool
            .execute(json!({
                "path": "credentials.json",
                "content": "bad"
            }))
            .await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ClosedCodeError::ProtectedPath { .. }
        ));
    }
}
