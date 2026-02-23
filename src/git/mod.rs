pub mod commit;
pub mod context;
pub mod diff;

pub use context::GitContext;

use std::path::Path;

/// Run a git command in the given working directory.
/// Returns `Some(stdout)` on success, `None` on failure.
pub(crate) async fn run_git(working_dir: &Path, args: &[&str]) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(working_dir)
        .output()
        .await
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn run_git_success() {
        let dir = TempDir::new().unwrap();
        // Init a repo so git commands work
        tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();

        let result = run_git(dir.path(), &["rev-parse", "--is-inside-work-tree"]).await;
        assert_eq!(result, Some("true".to_string()));
    }

    #[tokio::test]
    async fn run_git_in_non_repo() {
        let dir = TempDir::new().unwrap();
        let result = run_git(dir.path(), &["status"]).await;
        assert!(result.is_none());
    }
}
