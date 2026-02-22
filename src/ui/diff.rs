use crossterm::style::Stylize;
use similar::{ChangeTag, TextDiff};

use crate::ui::theme::Theme;

/// Summary of changes in a diff.
#[derive(Debug, Clone)]
pub struct DiffSummary {
    pub additions: usize,
    pub deletions: usize,
}

impl std::fmt::Display for DiffSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} addition{}, {} deletion{}",
            self.additions,
            if self.additions == 1 { "" } else { "s" },
            self.deletions,
            if self.deletions == 1 { "" } else { "s" },
        )
    }
}

/// Generate a unified diff between old and new content and print it
/// to stdout with ANSI colors.
///
/// Returns a `DiffSummary` with addition/deletion counts.
pub fn display_diff(file_path: &str, old_content: &str, new_content: &str) -> DiffSummary {
    let is_new_file = old_content.is_empty();

    // Print file headers
    if is_new_file {
        println!("{}", "--- /dev/null".with(Theme::DIFF_CONTEXT));
        println!(
            "{}",
            format!("+++ b/{}", file_path).with(Theme::DIFF_CONTEXT)
        );
    } else {
        println!(
            "{}",
            format!("--- a/{}", file_path).with(Theme::DIFF_CONTEXT)
        );
        println!(
            "{}",
            format!("+++ b/{}", file_path).with(Theme::DIFF_CONTEXT)
        );
    }

    let diff = TextDiff::from_lines(old_content, new_content);
    let mut additions = 0usize;
    let mut deletions = 0usize;

    // Use unified_diff for standard output with context
    let mut unified = diff.unified_diff();
    let unified = unified.context_radius(3);

    for hunk in unified.iter_hunks() {
        // Print hunk header
        println!(
            "{}",
            hunk.header().to_string().trim().with(Theme::DIFF_HUNK)
        );

        for change in hunk.iter_changes() {
            let sign = match change.tag() {
                ChangeTag::Delete => {
                    deletions += 1;
                    "-"
                }
                ChangeTag::Insert => {
                    additions += 1;
                    "+"
                }
                ChangeTag::Equal => " ",
            };

            let line = format!("{}{}", sign, change);

            let colored = match change.tag() {
                ChangeTag::Delete => line.with(Theme::DIFF_DELETE).to_string(),
                ChangeTag::Insert => line.with(Theme::DIFF_ADD).to_string(),
                ChangeTag::Equal => line.with(Theme::DIFF_CONTEXT).to_string(),
            };

            // change already includes trailing newline from TextDiff
            if change.missing_newline() {
                println!("{}", colored);
            } else {
                print!("{}", colored);
            }
        }
    }

    let summary = DiffSummary {
        additions,
        deletions,
    };

    // Print summary
    println!();
    if is_new_file {
        println!(
            "  File: {} {}",
            file_path,
            "(new)".with(Theme::DIM)
        );
    } else {
        println!("  File: {}", file_path);
    }
    println!("  Changes: {}", summary.to_string().with(Theme::DIM));
    println!();

    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_summary_display_singular() {
        let summary = DiffSummary {
            additions: 1,
            deletions: 1,
        };
        assert_eq!(summary.to_string(), "1 addition, 1 deletion");
    }

    #[test]
    fn diff_summary_display_plural() {
        let summary = DiffSummary {
            additions: 3,
            deletions: 0,
        };
        assert_eq!(summary.to_string(), "3 additions, 0 deletions");
    }

    #[test]
    fn diff_identical_files() {
        let content = "line 1\nline 2\nline 3\n";
        let summary = display_diff("test.rs", content, content);
        assert_eq!(summary.additions, 0);
        assert_eq!(summary.deletions, 0);
    }

    #[test]
    fn diff_new_file() {
        let content = "fn main() {\n    println!(\"hello\");\n}\n";
        let summary = display_diff("hello.rs", "", content);
        assert_eq!(summary.additions, 3);
        assert_eq!(summary.deletions, 0);
    }

    #[test]
    fn diff_added_lines() {
        let old = "line 1\nline 2\n";
        let new = "line 1\nline 2\nline 3\n";
        let summary = display_diff("test.rs", old, new);
        assert_eq!(summary.additions, 1);
        assert_eq!(summary.deletions, 0);
    }

    #[test]
    fn diff_removed_lines() {
        let old = "line 1\nline 2\nline 3\n";
        let new = "line 1\nline 3\n";
        let summary = display_diff("test.rs", old, new);
        assert_eq!(summary.additions, 0);
        assert_eq!(summary.deletions, 1);
    }

    #[test]
    fn diff_modified_lines() {
        let old = "line 1\nold line\nline 3\n";
        let new = "line 1\nnew line\nline 3\n";
        let summary = display_diff("test.rs", old, new);
        assert_eq!(summary.additions, 1);
        assert_eq!(summary.deletions, 1);
    }

    #[test]
    fn diff_empty_to_content() {
        let summary = display_diff("new.txt", "", "hello\nworld\n");
        assert_eq!(summary.additions, 2);
        assert_eq!(summary.deletions, 0);
    }
}
