use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

use crate::error::{ClosedCodeError, Result};
use crate::gemini::types::FunctionDeclaration;

use super::{ParamBuilder, Tool};

/// Commands allowed to be executed.
/// Read-only and informational commands only.
const ALLOWED_COMMANDS: &[&str] = &[
    "ls", "cat", "head", "tail", "find", "grep", "rg", "wc", "file", "tree", "pwd", "which",
    "git", "cargo", "rustc", "echo", "sort", "uniq", "diff",
];

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub struct ShellCommandTool {
    working_directory: PathBuf,
}

impl ShellCommandTool {
    pub fn new(working_directory: PathBuf) -> Self {
        Self { working_directory }
    }

    /// Parse a command string into (command, args) using shlex.
    /// Validates the command against the allowlist.
    pub fn parse_and_validate(command_str: &str) -> Result<(String, Vec<String>)> {
        let parts = shlex::split(command_str).ok_or_else(|| {
            ClosedCodeError::ShellError("Invalid command syntax (mismatched quotes)".into())
        })?;

        if parts.is_empty() {
            return Err(ClosedCodeError::ShellError("Empty command".into()));
        }

        let cmd = &parts[0];

        // Extract base command name (handle paths like /usr/bin/git)
        let base_cmd = std::path::Path::new(cmd)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(cmd);

        if !ALLOWED_COMMANDS.contains(&base_cmd) {
            return Err(ClosedCodeError::ShellNotAllowed {
                command: base_cmd.to_string(),
                allowed: ALLOWED_COMMANDS.join(", "),
            });
        }

        Ok((parts[0].clone(), parts[1..].to_vec()))
    }
}

#[async_trait]
impl Tool for ShellCommandTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute a shell command. Only allowlisted commands are permitted: \
         ls, cat, head, tail, find, grep, rg, wc, file, tree, pwd, which, \
         git, cargo, rustc, echo, sort, uniq, diff. \
         Commands have a 30-second timeout. Use this for operations \
         not covered by other tools."
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration {
            name: self.name().into(),
            description: self.description().into(),
            parameters: ParamBuilder::new()
                .string(
                    "command",
                    "The shell command to execute (e.g., 'git log --oneline -10')",
                    true,
                )
                .build(),
        }
    }

    async fn execute(&self, args: Value) -> Result<Value> {
        let command_str =
            args["command"]
                .as_str()
                .ok_or_else(|| ClosedCodeError::ToolError {
                    name: "shell".into(),
                    message: "Missing required parameter 'command'".into(),
                })?;

        let (cmd, cmd_args) = Self::parse_and_validate(command_str)?;

        tracing::info!("Executing shell command: {} {:?}", cmd, cmd_args);

        let output = tokio::time::timeout(
            COMMAND_TIMEOUT,
            Command::new(&cmd)
                .args(&cmd_args)
                .current_dir(&self.working_directory)
                .output(),
        )
        .await
        .map_err(|_| ClosedCodeError::ShellTimeout { seconds: 30 })?
        .map_err(|e| ClosedCodeError::ShellError(format!("Failed to execute '{}': {}", cmd, e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        // Truncate very long output
        let max_output = 50_000; // 50KB
        let stdout_truncated = if stdout.len() > max_output {
            format!(
                "{}...\n[Output truncated: {} bytes total]",
                &stdout[..max_output],
                stdout.len()
            )
        } else {
            stdout
        };

        Ok(json!({
            "stdout": stdout_truncated,
            "stderr": stderr,
            "exit_code": exit_code,
            "command": command_str,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::Mode;

    #[tokio::test]
    async fn shell_allowed_command() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ShellCommandTool::new(dir.path().to_path_buf());
        let result = tool
            .execute(json!({"command": "echo hello"}))
            .await
            .unwrap();
        assert!(result["stdout"].as_str().unwrap().contains("hello"));
        assert_eq!(result["exit_code"], 0);
    }

    #[tokio::test]
    async fn shell_blocked_command() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ShellCommandTool::new(dir.path().to_path_buf());
        let result = tool.execute(json!({"command": "rm -rf /"})).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not in the allowlist"));
    }

    #[test]
    fn shell_parse_and_validate_basic() {
        let (cmd, args) = ShellCommandTool::parse_and_validate("ls -la").unwrap();
        assert_eq!(cmd, "ls");
        assert_eq!(args, vec!["-la"]);
    }

    #[test]
    fn shell_parse_and_validate_quoted() {
        let (cmd, args) =
            ShellCommandTool::parse_and_validate("git log --format='%H %s'").unwrap();
        assert_eq!(cmd, "git");
        assert_eq!(args, vec!["log", "--format=%H %s"]);
    }

    #[test]
    fn shell_parse_and_validate_blocked() {
        let result = ShellCommandTool::parse_and_validate("rm file.txt");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("not in the allowlist"));
    }

    #[test]
    fn shell_parse_and_validate_empty() {
        let result = ShellCommandTool::parse_and_validate("");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Empty command"));
    }

    #[test]
    fn shell_parse_and_validate_mismatched_quotes() {
        let result = ShellCommandTool::parse_and_validate("echo 'hello");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("mismatched quotes"));
    }

    #[tokio::test]
    async fn shell_missing_command_arg() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ShellCommandTool::new(dir.path().to_path_buf());
        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
    }

    #[test]
    fn shell_path_command() {
        let (cmd, args) = ShellCommandTool::parse_and_validate("/usr/bin/git status").unwrap();
        assert_eq!(cmd, "/usr/bin/git");
        assert_eq!(args, vec!["status"]);
    }

    #[test]
    fn shell_tool_trait_methods() {
        let tool = ShellCommandTool::new(PathBuf::from("/tmp"));
        assert_eq!(tool.name(), "shell");
        assert!(!tool.description().is_empty());
        assert_eq!(tool.declaration().name, "shell");
        assert_eq!(
            tool.available_modes(),
            vec![Mode::Explore, Mode::Plan, Mode::Execute]
        );
    }
}
