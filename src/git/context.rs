use std::fmt;
use std::path::Path;

use super::run_git;

/// Status of a changed file in the working tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
}

impl fmt::Display for FileStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Added => write!(f, "added"),
            Self::Modified => write!(f, "modified"),
            Self::Deleted => write!(f, "deleted"),
            Self::Renamed => write!(f, "renamed"),
            Self::Untracked => write!(f, "untracked"),
        }
    }
}

/// A file that has changes in the working tree or index.
#[derive(Debug, Clone)]
pub struct ChangedFile {
    pub path: String,
    pub status: FileStatus,
}

impl fmt::Display for ChangedFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.path, self.status)
    }
}

/// Git repository context for the working directory.
///
/// Detected on startup via git commands. All fields are populated
/// best-effort — failures result in `is_git_repo: false` or empty fields.
#[derive(Debug, Clone)]
pub struct GitContext {
    pub is_git_repo: bool,
    pub current_branch: Option<String>,
    pub default_branch: Option<String>,
    pub has_uncommitted_changes: bool,
    pub changed_files: Vec<ChangedFile>,
    pub recent_commits: Vec<String>,
}

impl GitContext {
    /// Detect git context for the given working directory.
    ///
    /// Never panics or returns an error — if git is unavailable or the
    /// directory is not a repo, returns a context with `is_git_repo: false`.
    pub async fn detect(working_dir: &Path) -> Self {
        let is_git_repo = run_git(working_dir, &["rev-parse", "--is-inside-work-tree"])
            .await
            .map(|s| s == "true")
            .unwrap_or(false);

        if !is_git_repo {
            return Self {
                is_git_repo: false,
                current_branch: None,
                default_branch: None,
                has_uncommitted_changes: false,
                changed_files: Vec::new(),
                recent_commits: Vec::new(),
            };
        }

        let current_branch = run_git(working_dir, &["branch", "--show-current"]).await;

        let default_branch = detect_default_branch(working_dir).await;

        let (has_uncommitted_changes, changed_files) = detect_changes(working_dir).await;

        let recent_commits = run_git(working_dir, &["log", "--oneline", "-5"])
            .await
            .map(|s| s.lines().map(String::from).collect())
            .unwrap_or_default();

        Self {
            is_git_repo,
            current_branch,
            default_branch,
            has_uncommitted_changes,
            changed_files,
            recent_commits,
        }
    }

    /// One-line summary for display (e.g., in `/status`).
    ///
    /// Returns `"main (3 uncommitted changes)"` or `"main (clean)"`.
    pub fn summary(&self) -> String {
        if !self.is_git_repo {
            return "not a git repository".to_string();
        }

        let branch = self
            .current_branch
            .as_deref()
            .unwrap_or("(detached HEAD)");

        if self.has_uncommitted_changes {
            let count = self.changed_files.len();
            let noun = if count == 1 { "change" } else { "changes" };
            format!("{} ({} uncommitted {})", branch, count, noun)
        } else {
            format!("{} (clean)", branch)
        }
    }

    /// Multi-line section for injection into the system prompt.
    pub fn system_prompt_section(&self) -> String {
        if !self.is_git_repo {
            return String::new();
        }

        let mut lines = Vec::new();

        let branch = self
            .current_branch
            .as_deref()
            .unwrap_or("(detached HEAD)");

        if self.has_uncommitted_changes {
            let count = self.changed_files.len();
            lines.push(format!(
                "Git context: On branch `{}`, {} uncommitted change{}.",
                branch,
                count,
                if count == 1 { "" } else { "s" }
            ));

            if !self.changed_files.is_empty() {
                let file_list: Vec<String> =
                    self.changed_files.iter().map(|f| f.to_string()).collect();
                lines.push(format!("Changed files: {}", file_list.join(", ")));
            }
        } else {
            lines.push(format!(
                "Git context: On branch `{}`, working tree clean.",
                branch
            ));
        }

        if !self.recent_commits.is_empty() {
            lines.push("Recent commits:".to_string());
            for commit in &self.recent_commits {
                lines.push(format!("  {}", commit));
            }
        }

        lines.join("\n")
    }
}

impl fmt::Display for GitContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.summary())
    }
}

/// Detect the default branch (main or master).
async fn detect_default_branch(working_dir: &Path) -> Option<String> {
    // Try "main" first, then "master"
    if run_git(working_dir, &["rev-parse", "--verify", "main"])
        .await
        .is_some()
    {
        return Some("main".to_string());
    }
    if run_git(working_dir, &["rev-parse", "--verify", "master"])
        .await
        .is_some()
    {
        return Some("master".to_string());
    }
    None
}

/// Parse `git status --porcelain` output into changed files.
async fn detect_changes(working_dir: &Path) -> (bool, Vec<ChangedFile>) {
    let output = match run_git(working_dir, &["status", "--porcelain"]).await {
        Some(s) => s,
        None => return (false, Vec::new()),
    };

    if output.is_empty() {
        return (false, Vec::new());
    }

    let files: Vec<ChangedFile> = output
        .lines()
        .filter_map(|line| parse_porcelain_line(line))
        .collect();

    let has_changes = !files.is_empty();
    (has_changes, files)
}

/// Parse a single line from `git status --porcelain`.
///
/// Format: `XY path` where X is index status, Y is working tree status.
/// Examples: `" M src/main.rs"`, `"A  new_file.rs"`, `"?? untracked.txt"`
fn parse_porcelain_line(line: &str) -> Option<ChangedFile> {
    if line.len() < 4 {
        return None;
    }

    let xy = &line[..2];
    let path = line[3..].to_string();

    let status = match xy {
        "??" => FileStatus::Untracked,
        s if s.starts_with('A') || s.ends_with('A') => FileStatus::Added,
        s if s.starts_with('D') || s.ends_with('D') => FileStatus::Deleted,
        s if s.starts_with('R') || s.ends_with('R') => FileStatus::Renamed,
        s if s.starts_with('M') || s.ends_with('M') || s.starts_with(' ') => FileStatus::Modified,
        _ => FileStatus::Modified,
    };

    Some(ChangedFile { path, status })
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
        // Set user for commits
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
    async fn detect_not_a_git_repo() {
        let dir = TempDir::new().unwrap();
        let ctx = GitContext::detect(dir.path()).await;
        assert!(!ctx.is_git_repo);
        assert!(ctx.current_branch.is_none());
        assert!(ctx.changed_files.is_empty());
        assert!(!ctx.has_uncommitted_changes);
    }

    #[tokio::test]
    async fn detect_in_git_repo() {
        let dir = TempDir::new().unwrap();
        init_repo(dir.path()).await;

        let ctx = GitContext::detect(dir.path()).await;
        assert!(ctx.is_git_repo);
        // On a fresh init, branch might be "main" or "master" depending on git config
        assert!(ctx.current_branch.is_some());
    }

    #[tokio::test]
    async fn detect_with_uncommitted_changes() {
        let dir = TempDir::new().unwrap();
        init_repo(dir.path()).await;

        // Create an initial commit so we have a branch
        std::fs::write(dir.path().join("initial.txt"), "initial").unwrap();
        tokio::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(dir.path())
            .output()
            .await
            .unwrap();

        // Create an uncommitted file
        std::fs::write(dir.path().join("new.txt"), "hello").unwrap();

        let ctx = GitContext::detect(dir.path()).await;
        assert!(ctx.is_git_repo);
        assert!(ctx.has_uncommitted_changes);
        assert!(!ctx.changed_files.is_empty());
    }

    #[test]
    fn parse_porcelain_modified() {
        let file = parse_porcelain_line(" M src/main.rs").unwrap();
        assert_eq!(file.path, "src/main.rs");
        assert_eq!(file.status, FileStatus::Modified);
    }

    #[test]
    fn parse_porcelain_added() {
        let file = parse_porcelain_line("A  new_file.rs").unwrap();
        assert_eq!(file.path, "new_file.rs");
        assert_eq!(file.status, FileStatus::Added);
    }

    #[test]
    fn parse_porcelain_deleted() {
        let file = parse_porcelain_line("D  old_file.rs").unwrap();
        assert_eq!(file.path, "old_file.rs");
        assert_eq!(file.status, FileStatus::Deleted);
    }

    #[test]
    fn parse_porcelain_untracked() {
        let file = parse_porcelain_line("?? untracked.txt").unwrap();
        assert_eq!(file.path, "untracked.txt");
        assert_eq!(file.status, FileStatus::Untracked);
    }

    #[test]
    fn parse_porcelain_renamed() {
        let file = parse_porcelain_line("R  old.rs -> new.rs").unwrap();
        assert_eq!(file.status, FileStatus::Renamed);
    }

    #[test]
    fn parse_porcelain_short_line() {
        assert!(parse_porcelain_line("AB").is_none());
    }

    #[test]
    fn summary_clean() {
        let ctx = GitContext {
            is_git_repo: true,
            current_branch: Some("main".into()),
            default_branch: Some("main".into()),
            has_uncommitted_changes: false,
            changed_files: Vec::new(),
            recent_commits: Vec::new(),
        };
        assert_eq!(ctx.summary(), "main (clean)");
    }

    #[test]
    fn summary_with_changes() {
        let ctx = GitContext {
            is_git_repo: true,
            current_branch: Some("feature".into()),
            default_branch: Some("main".into()),
            has_uncommitted_changes: true,
            changed_files: vec![
                ChangedFile {
                    path: "a.rs".into(),
                    status: FileStatus::Modified,
                },
                ChangedFile {
                    path: "b.rs".into(),
                    status: FileStatus::Added,
                },
            ],
            recent_commits: Vec::new(),
        };
        assert_eq!(ctx.summary(), "feature (2 uncommitted changes)");
    }

    #[test]
    fn summary_single_change() {
        let ctx = GitContext {
            is_git_repo: true,
            current_branch: Some("main".into()),
            default_branch: None,
            has_uncommitted_changes: true,
            changed_files: vec![ChangedFile {
                path: "a.rs".into(),
                status: FileStatus::Modified,
            }],
            recent_commits: Vec::new(),
        };
        assert_eq!(ctx.summary(), "main (1 uncommitted change)");
    }

    #[test]
    fn summary_not_a_repo() {
        let ctx = GitContext {
            is_git_repo: false,
            current_branch: None,
            default_branch: None,
            has_uncommitted_changes: false,
            changed_files: Vec::new(),
            recent_commits: Vec::new(),
        };
        assert_eq!(ctx.summary(), "not a git repository");
    }

    #[test]
    fn system_prompt_section_with_changes() {
        let ctx = GitContext {
            is_git_repo: true,
            current_branch: Some("main".into()),
            default_branch: Some("main".into()),
            has_uncommitted_changes: true,
            changed_files: vec![ChangedFile {
                path: "src/main.rs".into(),
                status: FileStatus::Modified,
            }],
            recent_commits: vec!["abc1234 Initial commit".into()],
        };
        let section = ctx.system_prompt_section();
        assert!(section.contains("On branch `main`"));
        assert!(section.contains("1 uncommitted change"));
        assert!(section.contains("src/main.rs (modified)"));
        assert!(section.contains("abc1234 Initial commit"));
    }

    #[test]
    fn system_prompt_section_clean() {
        let ctx = GitContext {
            is_git_repo: true,
            current_branch: Some("main".into()),
            default_branch: Some("main".into()),
            has_uncommitted_changes: false,
            changed_files: Vec::new(),
            recent_commits: Vec::new(),
        };
        let section = ctx.system_prompt_section();
        assert!(section.contains("working tree clean"));
    }

    #[test]
    fn system_prompt_section_non_repo() {
        let ctx = GitContext {
            is_git_repo: false,
            current_branch: None,
            default_branch: None,
            has_uncommitted_changes: false,
            changed_files: Vec::new(),
            recent_commits: Vec::new(),
        };
        assert!(ctx.system_prompt_section().is_empty());
    }

    #[test]
    fn display_impl_matches_summary() {
        let ctx = GitContext {
            is_git_repo: true,
            current_branch: Some("dev".into()),
            default_branch: None,
            has_uncommitted_changes: false,
            changed_files: Vec::new(),
            recent_commits: Vec::new(),
        };
        assert_eq!(format!("{}", ctx), ctx.summary());
    }

    #[test]
    fn file_status_display() {
        assert_eq!(FileStatus::Added.to_string(), "added");
        assert_eq!(FileStatus::Modified.to_string(), "modified");
        assert_eq!(FileStatus::Deleted.to_string(), "deleted");
        assert_eq!(FileStatus::Renamed.to_string(), "renamed");
        assert_eq!(FileStatus::Untracked.to_string(), "untracked");
    }

    #[test]
    fn changed_file_display() {
        let f = ChangedFile {
            path: "src/lib.rs".into(),
            status: FileStatus::Modified,
        };
        assert_eq!(f.to_string(), "src/lib.rs (modified)");
    }
}
