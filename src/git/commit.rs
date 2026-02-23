use std::path::Path;

use crate::error::{ClosedCodeError, Result};

use super::run_git;

/// Stage all changes and commit with the given message.
///
/// Runs `git add -A` followed by `git commit -m "message"`.
/// Returns the short SHA of the new commit.
pub async fn commit_all(working_dir: &Path, message: &str) -> Result<String> {
    // Verify git repo
    if run_git(working_dir, &["rev-parse", "--is-inside-work-tree"])
        .await
        .is_none()
    {
        return Err(ClosedCodeError::ToolError {
            name: "git".into(),
            message: "Not a git repository".into(),
        });
    }

    // Stage all
    let add_output = tokio::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(working_dir)
        .output()
        .await
        .map_err(|e| ClosedCodeError::ShellError(format!("git add failed: {}", e)))?;

    if !add_output.status.success() {
        let stderr = String::from_utf8_lossy(&add_output.stderr);
        return Err(ClosedCodeError::ShellError(format!(
            "git add -A failed: {}",
            stderr
        )));
    }

    // Commit
    let commit_output = tokio::process::Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(working_dir)
        .output()
        .await
        .map_err(|e| ClosedCodeError::ShellError(format!("git commit failed: {}", e)))?;

    if !commit_output.status.success() {
        let stderr = String::from_utf8_lossy(&commit_output.stderr);
        return Err(ClosedCodeError::ShellError(format!(
            "git commit failed: {}",
            stderr.trim()
        )));
    }

    // Get the short SHA
    last_commit_sha(working_dir).await
}

/// Stage specific files and commit with the given message.
///
/// Returns the short SHA of the new commit.
pub async fn commit_files(working_dir: &Path, files: &[&str], message: &str) -> Result<String> {
    // Verify git repo
    if run_git(working_dir, &["rev-parse", "--is-inside-work-tree"])
        .await
        .is_none()
    {
        return Err(ClosedCodeError::ToolError {
            name: "git".into(),
            message: "Not a git repository".into(),
        });
    }

    // Stage specific files
    let mut add_args = vec!["add"];
    add_args.extend(files);

    let add_output = tokio::process::Command::new("git")
        .args(&add_args)
        .current_dir(working_dir)
        .output()
        .await
        .map_err(|e| ClosedCodeError::ShellError(format!("git add failed: {}", e)))?;

    if !add_output.status.success() {
        let stderr = String::from_utf8_lossy(&add_output.stderr);
        return Err(ClosedCodeError::ShellError(format!(
            "git add failed: {}",
            stderr
        )));
    }

    // Commit
    let commit_output = tokio::process::Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(working_dir)
        .output()
        .await
        .map_err(|e| ClosedCodeError::ShellError(format!("git commit failed: {}", e)))?;

    if !commit_output.status.success() {
        let stderr = String::from_utf8_lossy(&commit_output.stderr);
        return Err(ClosedCodeError::ShellError(format!(
            "git commit failed: {}",
            stderr.trim()
        )));
    }

    last_commit_sha(working_dir).await
}

/// Get the short SHA of the most recent commit.
pub async fn last_commit_sha(working_dir: &Path) -> Result<String> {
    run_git(working_dir, &["rev-parse", "--short", "HEAD"])
        .await
        .ok_or_else(|| ClosedCodeError::ToolError {
            name: "git".into(),
            message: "Failed to get commit SHA".into(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn init_repo(dir: &Path) {
        tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(dir)
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir)
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir)
            .output()
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn commit_all_success() {
        let dir = TempDir::new().unwrap();
        init_repo(dir.path()).await;

        std::fs::write(dir.path().join("hello.txt"), "hello world").unwrap();
        let sha = commit_all(dir.path(), "initial commit").await.unwrap();
        assert!(!sha.is_empty());
        // SHA should be hex characters
        assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn commit_files_specific() {
        let dir = TempDir::new().unwrap();
        init_repo(dir.path()).await;

        // Create initial commit
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        commit_all(dir.path(), "initial").await.unwrap();

        // Create two new files
        std::fs::write(dir.path().join("b.txt"), "b").unwrap();
        std::fs::write(dir.path().join("c.txt"), "c").unwrap();

        // Only commit b.txt
        let sha = commit_files(dir.path(), &["b.txt"], "add b only")
            .await
            .unwrap();
        assert!(!sha.is_empty());

        // c.txt should still be untracked
        let status = run_git(dir.path(), &["status", "--porcelain"]).await.unwrap();
        assert!(status.contains("c.txt"));
        assert!(!status.contains("b.txt"));
    }

    #[tokio::test]
    async fn commit_nothing_to_commit_errors() {
        let dir = TempDir::new().unwrap();
        init_repo(dir.path()).await;

        // Create and commit a file so we have a HEAD
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        commit_all(dir.path(), "initial").await.unwrap();

        // Try to commit again with no changes
        let result = commit_all(dir.path(), "empty").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn last_commit_sha_format() {
        let dir = TempDir::new().unwrap();
        init_repo(dir.path()).await;

        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        commit_all(dir.path(), "initial").await.unwrap();

        let sha = last_commit_sha(dir.path()).await.unwrap();
        assert!(sha.len() >= 7);
        assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn commit_message_preserved() {
        let dir = TempDir::new().unwrap();
        init_repo(dir.path()).await;

        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        commit_all(dir.path(), "my specific message").await.unwrap();

        let log = run_git(dir.path(), &["log", "-1", "--format=%s"]).await.unwrap();
        assert_eq!(log, "my specific message");
    }

    #[tokio::test]
    async fn commit_non_git_dir_errors() {
        let dir = TempDir::new().unwrap();
        let result = commit_all(dir.path(), "test").await;
        assert!(result.is_err());
    }
}
