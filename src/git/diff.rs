use std::path::Path;

use crossterm::style::Stylize;

use crate::error::{ClosedCodeError, Result};
use crate::ui::theme::Theme;

use super::run_git;

/// Run a git diff command, returning the raw diff output.
/// Returns an error if the directory is not a git repository.
async fn run_git_diff(working_dir: &Path, args: &[&str]) -> Result<String> {
    let mut full_args = vec!["diff"];
    full_args.extend_from_slice(args);

    // First verify we're in a git repo
    if run_git(working_dir, &["rev-parse", "--is-inside-work-tree"])
        .await
        .is_none()
    {
        return Err(ClosedCodeError::ToolError {
            name: "git".into(),
            message: "Not a git repository".into(),
        });
    }

    // Run the diff command — git diff returns exit code 0 even with changes
    let output = tokio::process::Command::new("git")
        .args(&full_args)
        .current_dir(working_dir)
        .output()
        .await
        .map_err(|e| ClosedCodeError::ShellError(format!("Failed to run git diff: {}", e)))?;

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Show unstaged changes (working tree vs index).
pub async fn unstaged(working_dir: &Path) -> Result<String> {
    run_git_diff(working_dir, &[]).await
}

/// Show staged changes only (index vs HEAD).
pub async fn staged(working_dir: &Path) -> Result<String> {
    run_git_diff(working_dir, &["--cached"]).await
}

/// Show all uncommitted changes (working tree vs HEAD).
pub async fn all_uncommitted(working_dir: &Path) -> Result<String> {
    run_git_diff(working_dir, &["HEAD"]).await
}

/// Show changes since branching from the given base branch.
pub async fn branch_diff(working_dir: &Path, base_branch: &str) -> Result<String> {
    // Find merge base
    let merge_base = run_git(working_dir, &["merge-base", "HEAD", base_branch]).await;

    match merge_base {
        Some(base) => run_git_diff(working_dir, &[&base, "HEAD"]).await,
        None => Err(ClosedCodeError::ToolError {
            name: "git".into(),
            message: format!("Cannot find merge base between HEAD and '{}'", base_branch),
        }),
    }
}

/// Show diff for a commit range (e.g., "HEAD~3..HEAD" or "HEAD~3").
pub async fn commit_range(working_dir: &Path, range: &str) -> Result<String> {
    // If range doesn't contain "..", assume it's "range..HEAD"
    let actual_range = if range.contains("..") {
        range.to_string()
    } else {
        format!("{}..HEAD", range)
    };

    // Split range on ".." for the diff command
    let parts: Vec<&str> = actual_range.split("..").collect();
    if parts.len() != 2 {
        return Err(ClosedCodeError::ToolError {
            name: "git".into(),
            message: format!("Invalid commit range: {}", range),
        });
    }

    run_git_diff(working_dir, &[parts[0], parts[1]]).await
}

/// Print a raw git diff string with ANSI coloring to stdout.
pub fn colorize_git_diff(raw_diff: &str) {
    for line in raw_diff.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            println!("{}", line.with(Theme::DIFF_ADD));
        } else if line.starts_with('-') && !line.starts_with("---") {
            println!("{}", line.with(Theme::DIFF_DELETE));
        } else if line.starts_with("@@") {
            println!("{}", line.with(Theme::DIFF_HUNK));
        } else if line.starts_with("diff --git")
            || line.starts_with("---")
            || line.starts_with("+++")
            || line.starts_with("index ")
        {
            println!("{}", line.with(Theme::DIFF_CONTEXT));
        } else {
            println!("{}", line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn init_repo_with_commit(dir: &Path) {
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
        std::fs::write(dir.join("initial.txt"), "initial content\n").unwrap();
        tokio::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(dir)
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["commit", "-m", "initial commit"])
            .current_dir(dir)
            .output()
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn unstaged_no_changes() {
        let dir = TempDir::new().unwrap();
        init_repo_with_commit(dir.path()).await;

        let diff = unstaged(dir.path()).await.unwrap();
        assert!(diff.is_empty());
    }

    #[tokio::test]
    async fn unstaged_with_changes() {
        let dir = TempDir::new().unwrap();
        init_repo_with_commit(dir.path()).await;

        // Modify a tracked file
        std::fs::write(dir.path().join("initial.txt"), "modified content\n").unwrap();

        let diff = unstaged(dir.path()).await.unwrap();
        assert!(!diff.is_empty());
        assert!(diff.contains("-initial content"));
        assert!(diff.contains("+modified content"));
    }

    #[tokio::test]
    async fn staged_changes() {
        let dir = TempDir::new().unwrap();
        init_repo_with_commit(dir.path()).await;

        // Create and stage a new file
        std::fs::write(dir.path().join("new.txt"), "new content\n").unwrap();
        tokio::process::Command::new("git")
            .args(["add", "new.txt"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();

        let diff = staged(dir.path()).await.unwrap();
        assert!(!diff.is_empty());
        assert!(diff.contains("+new content"));
    }

    #[tokio::test]
    async fn all_uncommitted_combines() {
        let dir = TempDir::new().unwrap();
        init_repo_with_commit(dir.path()).await;

        // Create staged change
        std::fs::write(dir.path().join("staged.txt"), "staged\n").unwrap();
        tokio::process::Command::new("git")
            .args(["add", "staged.txt"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();

        // Create unstaged change
        std::fs::write(dir.path().join("initial.txt"), "modified\n").unwrap();

        let diff = all_uncommitted(dir.path()).await.unwrap();
        assert!(!diff.is_empty());
        // Should contain both changes
        assert!(diff.contains("staged.txt"));
        assert!(diff.contains("initial.txt"));
    }

    #[tokio::test]
    async fn commit_range_works() {
        let dir = TempDir::new().unwrap();
        init_repo_with_commit(dir.path()).await;

        // Make a second commit
        std::fs::write(dir.path().join("second.txt"), "second\n").unwrap();
        tokio::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["commit", "-m", "second commit"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();

        let diff = commit_range(dir.path(), "HEAD~1").await.unwrap();
        assert!(!diff.is_empty());
        assert!(diff.contains("second.txt"));
    }

    #[tokio::test]
    async fn diff_non_git_dir_returns_error() {
        let dir = TempDir::new().unwrap();
        let result = unstaged(dir.path()).await;
        assert!(result.is_err());
    }

    #[test]
    fn colorize_git_diff_runs_without_panic() {
        let sample = "\
diff --git a/test.rs b/test.rs
index abc..def 100644
--- a/test.rs
+++ b/test.rs
@@ -1,3 +1,4 @@
 unchanged line
-old line
+new line
+added line
 context line";
        // Just verify it doesn't panic (output goes to stdout)
        colorize_git_diff(sample);
    }
}
