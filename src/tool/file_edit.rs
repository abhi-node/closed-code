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

pub struct EditFileTool {
    working_directory: PathBuf,
    approval_handler: Arc<dyn ApprovalHandler>,
    protected_paths: Vec<String>,
}

impl EditFileTool {
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

impl std::fmt::Debug for EditFileTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditFileTool")
            .field("working_directory", &self.working_directory)
            .finish()
    }
}

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Edit an existing file by replacing a specific text segment. Provide the exact \
         text to find (old_text) and the replacement text (new_text). Shows a unified \
         diff of the changes and requires user approval before applying. The old_text \
         must match exactly — include enough surrounding context lines for a unique match. \
         Always use read_file first to see the current file content before editing."
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name().into(),
            description: self.description().into(),
            parameters: ParamBuilder::new()
                .string("path", "File path relative to working directory", true)
                .string(
                    "old_text",
                    "The exact text to find and replace. Must match exactly, including \
                     whitespace and indentation. Include enough context for a unique match.",
                    true,
                )
                .string(
                    "new_text",
                    "The replacement text. Use an empty string to delete the old_text.",
                    true,
                )
                .build(),
        }
    }

    async fn execute(&self, args: Value) -> Result<Value> {
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| ClosedCodeError::ToolError {
                name: "edit_file".into(),
                message: "Missing required parameter 'path'".into(),
            })?;

        let old_text = args["old_text"]
            .as_str()
            .ok_or_else(|| ClosedCodeError::ToolError {
                name: "edit_file".into(),
                message: "Missing required parameter 'old_text'".into(),
            })?;

        let new_text = args["new_text"]
            .as_str()
            .ok_or_else(|| ClosedCodeError::ToolError {
                name: "edit_file".into(),
                message: "Missing required parameter 'new_text'".into(),
            })?;

        // Check protected paths
        if super::is_protected_path(path_str, &self.protected_paths) {
            return Err(ClosedCodeError::ProtectedPath {
                path: path_str.to_string(),
            });
        }

        let resolved = self.resolve_path(path_str);

        // Read the existing file
        let old_content =
            fs::read_to_string(&resolved)
                .await
                .map_err(|e| ClosedCodeError::ToolError {
                    name: "edit_file".into(),
                    message: format!("Cannot read '{}': {}", path_str, e),
                })?;

        // Find old_text in the file
        let occurrences = old_content.matches(old_text).count();

        if occurrences == 0 {
            return Ok(json!({
                "error": "old_text not found in file",
                "path": path_str,
                "hint": "The exact text was not found. Verify the current file content \
                         with read_file, and ensure old_text matches exactly including \
                         whitespace and indentation."
            }));
        }

        // Replace first occurrence
        let new_content = old_content.replacen(old_text, new_text, 1);

        if occurrences > 1 {
            tracing::warn!(
                "edit_file: {} occurrences of old_text in {}, replacing first only",
                occurrences,
                path_str
            );
        }

        // Skip if no actual change
        if old_content == new_content {
            return Ok(json!({
                "status": "no_change",
                "path": path_str,
                "message": "old_text and new_text are identical; no change needed."
            }));
        }

        let change = FileChange {
            file_path: path_str.to_string(),
            resolved_path: resolved.display().to_string(),
            old_content: old_content.clone(),
            new_content: new_content.clone(),
            is_new_file: false,
        };

        // Request user approval
        let decision = self.approval_handler.request_approval(&change).await?;

        match decision {
            ApprovalDecision::Approved => {
                fs::write(&resolved, &new_content).await.map_err(|e| {
                    ClosedCodeError::ToolError {
                        name: "edit_file".into(),
                        message: format!("Cannot write to '{}': {}", path_str, e),
                    }
                })?;

                tracing::info!("File edited: {}", path_str);

                let mut result = json!({
                    "status": "applied",
                    "path": path_str,
                });

                if occurrences > 1 {
                    result["warning"] = json!(format!(
                        "Found {} occurrences of old_text; replaced the first one only.",
                        occurrences
                    ));
                }

                Ok(result)
            }
            ApprovalDecision::Rejected => {
                tracing::info!("User rejected edit to: {}", path_str);
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

    fn create_file(dir: &TempDir, name: &str, content: &str) {
        std::fs::write(dir.path().join(name), content).unwrap();
    }

    #[tokio::test]
    async fn edit_file_approved() {
        let (dir, handler) = setup();
        create_file(&dir, "test.rs", "fn main() {\n    old_code();\n}\n");

        let tool = EditFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        let result = tool
            .execute(json!({
                "path": "test.rs",
                "old_text": "    old_code();",
                "new_text": "    new_code();"
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "applied");

        let content = std::fs::read_to_string(dir.path().join("test.rs")).unwrap();
        assert!(content.contains("new_code()"));
        assert!(!content.contains("old_code()"));
    }

    #[tokio::test]
    async fn edit_file_rejected() {
        let (dir, handler) = setup_reject();
        let original = "fn main() {\n    original();\n}\n";
        create_file(&dir, "test.rs", original);

        let tool = EditFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        let result = tool
            .execute(json!({
                "path": "test.rs",
                "old_text": "    original();",
                "new_text": "    replaced();"
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "rejected");

        // File should be unchanged
        let content = std::fs::read_to_string(dir.path().join("test.rs")).unwrap();
        assert_eq!(content, original);
    }

    #[tokio::test]
    async fn edit_text_not_found() {
        let (dir, handler) = setup();
        create_file(&dir, "test.rs", "fn main() {}\n");

        let tool = EditFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        let result = tool
            .execute(json!({
                "path": "test.rs",
                "old_text": "nonexistent text",
                "new_text": "replacement"
            }))
            .await
            .unwrap();

        assert!(result["error"].as_str().unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn edit_multiple_occurrences_replaces_first() {
        let (dir, handler) = setup();
        create_file(&dir, "test.rs", "foo\nfoo\nfoo\n");

        let tool = EditFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        let result = tool
            .execute(json!({
                "path": "test.rs",
                "old_text": "foo",
                "new_text": "bar"
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "applied");
        assert!(result["warning"]
            .as_str()
            .unwrap()
            .contains("3 occurrences"));

        let content = std::fs::read_to_string(dir.path().join("test.rs")).unwrap();
        assert_eq!(content, "bar\nfoo\nfoo\n");
    }

    #[tokio::test]
    async fn edit_nonexistent_file() {
        let (dir, handler) = setup();
        let tool = EditFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        let result = tool
            .execute(json!({
                "path": "missing.rs",
                "old_text": "x",
                "new_text": "y"
            }))
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn edit_missing_args() {
        let (dir, handler) = setup();
        let tool = EditFileTool::new(dir.path().to_path_buf(), handler, vec![]);

        // Missing old_text
        let result = tool.execute(json!({"path": "x.rs", "new_text": "y"})).await;
        assert!(result.is_err());

        // Missing new_text
        let result = tool.execute(json!({"path": "x.rs", "old_text": "y"})).await;
        assert!(result.is_err());

        // Missing path
        let result = tool
            .execute(json!({"old_text": "x", "new_text": "y"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn edit_delete_text() {
        let (dir, handler) = setup();
        create_file(&dir, "test.rs", "line 1\ndelete me\nline 3\n");

        let tool = EditFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        let result = tool
            .execute(json!({
                "path": "test.rs",
                "old_text": "delete me\n",
                "new_text": ""
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "applied");
        let content = std::fs::read_to_string(dir.path().join("test.rs")).unwrap();
        assert_eq!(content, "line 1\nline 3\n");
    }

    #[test]
    fn edit_available_modes() {
        let (dir, handler) = setup();
        let tool = EditFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        assert_eq!(
            tool.available_modes(),
            vec![Mode::Guided, Mode::Execute, Mode::Auto]
        );
    }

    #[test]
    fn edit_tool_debug() {
        let (dir, handler) = setup();
        let tool = EditFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        let debug = format!("{:?}", tool);
        assert!(debug.contains("EditFileTool"));
    }

    #[tokio::test]
    async fn edit_protected_git_path_rejected() {
        let (dir, handler) = setup();
        let tool = EditFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        let result = tool
            .execute(json!({
                "path": ".git/config",
                "old_text": "old",
                "new_text": "new"
            }))
            .await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ClosedCodeError::ProtectedPath { .. }
        ));
    }

    #[tokio::test]
    async fn edit_protected_closed_code_rejected() {
        let (dir, handler) = setup();
        let tool = EditFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        let result = tool
            .execute(json!({
                "path": ".closed-code/config.toml",
                "old_text": "old",
                "new_text": "new"
            }))
            .await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ClosedCodeError::ProtectedPath { .. }
        ));
    }

    #[tokio::test]
    async fn edit_protected_env_rejected() {
        let (dir, handler) = setup();
        let tool = EditFileTool::new(dir.path().to_path_buf(), handler, vec![]);
        let result = tool
            .execute(json!({
                "path": ".env",
                "old_text": "old",
                "new_text": "new"
            }))
            .await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ClosedCodeError::ProtectedPath { .. }
        ));
    }
}
