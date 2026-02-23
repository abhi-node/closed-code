use std::fmt::Debug;

use async_trait::async_trait;

use crate::error::{ClosedCodeError, Result};

/// Describes a proposed file change for approval.
#[derive(Debug, Clone)]
pub struct FileChange {
    /// Path relative to working directory (used in diff display).
    pub file_path: String,
    /// Absolute resolved path on disk (shown in approval banner).
    pub resolved_path: String,
    /// Previous file content (empty string for new files).
    pub old_content: String,
    /// Proposed new content.
    pub new_content: String,
    /// Whether this is a new file (no previous content).
    pub is_new_file: bool,
}

/// Approval decision from the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approved,
    Rejected,
}

/// Trait for handling file change approvals.
///
/// Implementations display the proposed change to the user and collect their
/// decision. The trait is async to support blocking I/O (dialoguer) via
/// `spawn_blocking`.
#[async_trait]
pub trait ApprovalHandler: Send + Sync + Debug {
    async fn request_approval(&self, change: &FileChange) -> Result<ApprovalDecision>;
}

/// Terminal-based approval handler.
///
/// Displays a colorized unified diff using `ui::diff::display_diff` and
/// prompts the user with `dialoguer::Confirm` (default: No).
///
/// The dialoguer prompt blocks stdin, so it runs on a blocking thread
/// via `tokio::task::spawn_blocking`.
#[derive(Debug)]
pub struct TerminalApprovalHandler;

impl TerminalApprovalHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TerminalApprovalHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ApprovalHandler for TerminalApprovalHandler {
    async fn request_approval(&self, change: &FileChange) -> Result<ApprovalDecision> {
        use crossterm::style::Stylize;
        use crate::ui::theme::Theme;

        let operation = if change.is_new_file { "CREATE" } else { "MODIFY" };

        // Print header banner with full resolved path
        println!();
        println!(
            "{}",
            format!("── {} {} ──", operation, change.resolved_path)
                .with(Theme::ACCENT)
                .bold()
        );
        println!();

        // Display the colorized diff
        crate::ui::diff::display_diff(
            &change.file_path,
            &change.old_content,
            &change.new_content,
        );

        // Prompt user on a blocking thread (dialoguer blocks stdin)
        let approved = tokio::task::spawn_blocking(|| {
            dialoguer::Confirm::new()
                .with_prompt("Apply this change? (y = write to disk, N = skip)")
                .default(false)
                .interact()
        })
        .await
        .map_err(|e| ClosedCodeError::ApprovalError(format!("spawn_blocking failed: {e}")))?
        .map_err(|e| ClosedCodeError::ApprovalError(format!("prompt failed: {e}")))?;

        if approved {
            println!(
                "  {} {}",
                "\u{2713}".with(Theme::SUCCESS),
                "Change applied.".with(Theme::DIM)
            );
            Ok(ApprovalDecision::Approved)
        } else {
            println!(
                "  {} {}",
                "\u{2717}".with(Theme::ERROR),
                "Change rejected.".with(Theme::DIM)
            );
            Ok(ApprovalDecision::Rejected)
        }
    }
}

/// Auto-approve handler for testing.
///
/// Configurable to always approve or always reject without user interaction.
#[derive(Debug)]
pub struct AutoApproveHandler {
    approve: bool,
}

impl AutoApproveHandler {
    pub fn always_approve() -> Self {
        Self { approve: true }
    }

    pub fn always_reject() -> Self {
        Self { approve: false }
    }
}

#[async_trait]
impl ApprovalHandler for AutoApproveHandler {
    async fn request_approval(&self, _change: &FileChange) -> Result<ApprovalDecision> {
        if self.approve {
            Ok(ApprovalDecision::Approved)
        } else {
            Ok(ApprovalDecision::Rejected)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn auto_approve_handler_approves() {
        let handler = AutoApproveHandler::always_approve();
        let change = FileChange {
            file_path: "test.rs".into(),
            resolved_path: "/tmp/test.rs".into(),
            old_content: String::new(),
            new_content: "fn main() {}".into(),
            is_new_file: true,
        };
        let decision = handler.request_approval(&change).await.unwrap();
        assert_eq!(decision, ApprovalDecision::Approved);
    }

    #[tokio::test]
    async fn auto_approve_handler_rejects() {
        let handler = AutoApproveHandler::always_reject();
        let change = FileChange {
            file_path: "test.rs".into(),
            resolved_path: "/tmp/test.rs".into(),
            old_content: String::new(),
            new_content: "fn main() {}".into(),
            is_new_file: true,
        };
        let decision = handler.request_approval(&change).await.unwrap();
        assert_eq!(decision, ApprovalDecision::Rejected);
    }

    #[test]
    fn file_change_debug() {
        let change = FileChange {
            file_path: "test.rs".into(),
            resolved_path: "/tmp/test.rs".into(),
            old_content: "old".into(),
            new_content: "new".into(),
            is_new_file: false,
        };
        let debug = format!("{:?}", change);
        assert!(debug.contains("test.rs"));
    }

    #[test]
    fn terminal_handler_debug() {
        let handler = TerminalApprovalHandler::new();
        let debug = format!("{:?}", handler);
        assert!(debug.contains("TerminalApprovalHandler"));
    }

}
