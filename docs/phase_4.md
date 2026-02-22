# Phase 4: Execute Mode — Diffs, Approvals, and Mode-Specific Behavior

**Goal**: The model can create and edit files with colorized unified diffs and user approval gates. Each mode has clearly distinct behavior: Explore is strictly read-only, Plan creates reviewable plans with an accept/revise flow, and Execute enables file modification behind approval gates. Plan acceptance transitions directly into Execute mode.

**Deliverable**: `cargo run --mode execute` launches the REPL with `write_file` and `edit_file` tools available. The LLM proposes file changes as colorized diffs; the user approves or rejects each change with a `[y/N]` prompt (default No). In Plan mode, the user can refine a plan with feedback or type `/accept` to transition to Execute mode and begin implementation. Explore mode remains strictly read-only.

**Builds on**: Phase 3 (Orchestrator, Agent trait, ExplorerAgent, PlannerAgent, WebSearchAgent, spawn tools, ToolRegistry factory functions `create_orchestrator_registry` / `create_subagent_registry` / `create_planner_registry`, StreamResult, streaming tool-call loop, mode-specific system prompts, all Phase 2/3 error variants).

**Estimated**: ~2,000 lines of new Rust across ~4 new files + modifications to ~8 existing Phase 3 files.

---

## File Layout

### New Files

```
src/
  ui/
    diff.rs            # Unified diff generation + ANSI colorized display
    approval.rs        # ApprovalHandler trait, TerminalApprovalHandler, AutoApproveHandler
  tool/
    file_write.rs      # WriteFileTool (create/overwrite with approval gate)
    file_edit.rs       # EditFileTool (search/replace with approval gate)
```

### Modified Phase 3 Files

```
Cargo.toml             # + similar, dialoguer
src/
  tool/
    mod.rs             # + pub mod file_write; pub mod file_edit;
    registry.rs        # Update create_orchestrator_registry() to accept ApprovalHandler,
                       #   register write tools in Execute mode
  ui/
    mod.rs             # + pub mod diff; pub mod approval;
    theme.rs           # + DIFF_ADD, DIFF_DELETE, DIFF_HUNK, DIFF_CONTEXT colors
  error.rs             # + ProtectedPath, ApprovalError variants
  agent/
    orchestrator.rs    # + approval_handler field, current_plan field, plan methods,
                       #   enhanced mode-specific system prompts, accept_plan()
  repl.rs              # + TerminalApprovalHandler creation, /accept command,
                       #   SlashResult::ExecutePlan, plan capture after Plan mode responses
```

---

## Dependencies to Add

Add these to the existing `Cargo.toml` from Phase 3:

```toml
# Diff generation (Myers algorithm)
similar = "2"

# Interactive terminal prompts (approval y/N)
dialoguer = "0.11"
```

### New Crate Rationale

| Crate | Why |
|-------|-----|
| `similar` | Unified diff generation using the Myers diff algorithm. Used by `ui/diff.rs` to compute additions, deletions, and context lines between old and new file content. Battle-tested, used widely in the Rust ecosystem. |
| `dialoguer` | Terminal prompt library for interactive `y/N` confirmation. Used by `TerminalApprovalHandler` to prompt users before applying file changes. Handles terminal state properly (raw mode, cursor position), which is important since `rustyline` may have modified terminal settings. |

---

## Mode Behavior Specification

Phase 4 establishes clear behavioral contracts for each mode. These are enforced at two levels: tool availability (registry filtering) and system prompt guidance.

### Mode Summary

| Mode | File Changes | Write Tools | Agent Capabilities | User Flow |
|------|-------------|-------------|-------------------|-----------|
| **Explore** | Never | None | Read files, search, grep, spawn_explorer | Ask questions, get explanations |
| **Plan** | Never | None | Read files, spawn_explorer/planner/web_search | Get plan → feedback/revise → `/accept` |
| **Execute** | With approval | write_file, edit_file | All read tools + write tools + spawn_explorer | LLM proposes changes → user approves/rejects |

### Tool Counts Per Mode

| Mode | Phase 3 Count | Phase 4 Count | Tools Added |
|------|--------------|--------------|-------------|
| Explore | 6 | 6 | — |
| Plan | 8 | 8 | — |
| Execute | 6 | **8** | write_file, edit_file |

### Plan → Execute Transition

```
1. User enters Plan mode (/mode plan or --mode plan)
2. User asks LLM to plan changes
3. LLM researches via sub-agents, produces structured plan
4. REPL captures plan text in orchestrator.current_plan
5. User reviews plan:
   a. Feedback → user types natural language → LLM refines plan → goto 4
   b. Accept → user types /accept:
      i.   Plan injected into history as "[ACCEPTED PLAN]" context
      ii.  Mode switches to Execute (write tools registered)
      iii. Auto-sends "Execute the accepted plan step by step."
      iv.  LLM begins executing with write_file/edit_file tools
```

---

## Phase 3 Modifications

### `src/error.rs` — New Variants

Add these after the existing Phase 3 agent error variants:

```rust
#[derive(Error, Debug)]
pub enum ClosedCodeError {
    // ... existing Phase 1 + Phase 2 + Phase 3 variants ...

    // File modification errors (Phase 4)
    #[error("Cannot modify protected path: {path}")]
    ProtectedPath { path: String },

    #[error("Approval prompt failed: {0}")]
    ApprovalError(String),
}
```

These error variants are not retryable. Update `is_retryable()` — no changes needed since the default branch already returns `false`.

Note: `ProtectedPath` is included for forward compatibility with Phase 6 (protected path enforcement). It is not used in Phase 4 but establishes the error type early.

### `src/ui/theme.rs` — Diff Colors

Add diff-specific color constants:

```rust
use crossterm::style::Color;

pub struct Theme;

impl Theme {
    // ... existing Phase 1 colors ...
    pub const USER: Color = Color::Cyan;
    pub const ASSISTANT: Color = Color::White;
    pub const ERROR: Color = Color::Red;
    pub const SUCCESS: Color = Color::Green;
    pub const DIM: Color = Color::DarkGrey;
    pub const ACCENT: Color = Color::Yellow;
    pub const PROMPT: Color = Color::Blue;

    // Phase 4: diff display colors
    pub const DIFF_ADD: Color = Color::Green;
    pub const DIFF_DELETE: Color = Color::Red;
    pub const DIFF_HUNK: Color = Color::Cyan;
    pub const DIFF_CONTEXT: Color = Color::DarkGrey;
}
```

### `src/ui/mod.rs` — Module Declarations

```rust
pub mod spinner;
pub mod theme;
pub mod diff;      // Phase 4
pub mod approval;  // Phase 4
```

### `src/tool/mod.rs` — Module Declarations

Add to existing module declarations:

```rust
pub mod filesystem;
pub mod shell;
pub mod spawn;
pub mod report;
pub mod file_write;  // Phase 4
pub mod file_edit;   // Phase 4
```

---

## Implementation Details

### `src/ui/diff.rs`

Generates unified diffs using the `similar` crate and displays them with ANSI coloring.

```rust
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
///
/// Display format:
/// ```text
/// --- a/src/main.rs
/// +++ b/src/main.rs
/// @@ -1,3 +1,5 @@
///  fn main() {
/// -    println!("old");
/// +    println!("new");
/// +    extra_line();
///  }
///
///   File: src/main.rs
///   Changes: 2 additions, 1 deletion
/// ```
pub fn display_diff(file_path: &str, old_content: &str, new_content: &str) -> DiffSummary {
    let is_new_file = old_content.is_empty();

    // Print file headers
    if is_new_file {
        println!(
            "{}",
            "--- /dev/null".with(Theme::DIFF_CONTEXT)
        );
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
    let unified = diff.unified_diff();
    let unified = unified.context_radius(3);

    for hunk in unified.iter_hunks() {
        // Print hunk header
        println!("{}", hunk.header().to_string().trim().with(Theme::DIFF_HUNK));

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
    println!(
        "  Changes: {}",
        summary.to_string().with(Theme::DIM)
    );
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
```

### `src/ui/approval.rs`

Defines the approval trait and implementations.

```rust
use std::fmt::Debug;

use async_trait::async_trait;

use crate::error::{ClosedCodeError, Result};

/// Describes a proposed file change for approval.
#[derive(Debug, Clone)]
pub struct FileChange {
    /// Path relative to working directory.
    pub file_path: String,
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

#[async_trait]
impl ApprovalHandler for TerminalApprovalHandler {
    async fn request_approval(&self, change: &FileChange) -> Result<ApprovalDecision> {
        // Display the colorized diff
        crate::ui::diff::display_diff(
            &change.file_path,
            &change.old_content,
            &change.new_content,
        );

        // Prompt user on a blocking thread (dialoguer blocks stdin)
        let approved = tokio::task::spawn_blocking(|| {
            dialoguer::Confirm::new()
                .with_prompt("Apply this change?")
                .default(false)
                .interact()
        })
        .await
        .map_err(|e| ClosedCodeError::ApprovalError(format!("spawn_blocking failed: {e}")))?
        .map_err(|e| ClosedCodeError::ApprovalError(format!("prompt failed: {e}")))?;

        if approved {
            Ok(ApprovalDecision::Approved)
        } else {
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
```

### `src/tool/file_write.rs`

Creates new files or overwrites existing files with an approval gate.

```rust
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
}

impl WriteFileTool {
    pub fn new(
        working_directory: PathBuf,
        approval_handler: Arc<dyn ApprovalHandler>,
    ) -> Self {
        Self {
            working_directory,
            approval_handler,
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
                .string(
                    "path",
                    "File path relative to working directory",
                    true,
                )
                .string(
                    "content",
                    "The complete file content to write",
                    true,
                )
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

        let resolved = self.resolve_path(path_str);

        // Read existing content if file exists
        let (old_content, is_new_file) = if resolved.exists() {
            let existing = fs::read_to_string(&resolved).await.map_err(|e| {
                ClosedCodeError::ToolError {
                    name: "write_file".into(),
                    message: format!("Cannot read existing file '{}': {}", path_str, e),
                }
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
                    fs::create_dir_all(parent).await.map_err(|e| {
                        ClosedCodeError::ToolError {
                            name: "write_file".into(),
                            message: format!(
                                "Cannot create directory '{}': {}",
                                parent.display(),
                                e
                            ),
                        }
                    })?;
                }

                fs::write(&resolved, content).await.map_err(|e| {
                    ClosedCodeError::ToolError {
                        name: "write_file".into(),
                        message: format!("Cannot write to '{}': {}", path_str, e),
                    }
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
        vec![Mode::Execute]
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
        let tool = WriteFileTool::new(dir.path().to_path_buf(), handler);
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
        let tool = WriteFileTool::new(dir.path().to_path_buf(), handler);
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

        let tool = WriteFileTool::new(dir.path().to_path_buf(), handler);
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
        let tool = WriteFileTool::new(dir.path().to_path_buf(), handler);
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

        let tool = WriteFileTool::new(dir.path().to_path_buf(), handler);
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
        let tool = WriteFileTool::new(dir.path().to_path_buf(), handler);
        let result = tool.execute(json!({"content": "x"})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn write_missing_content_arg() {
        let (dir, handler) = setup();
        let tool = WriteFileTool::new(dir.path().to_path_buf(), handler);
        let result = tool.execute(json!({"path": "x.rs"})).await;
        assert!(result.is_err());
    }

    #[test]
    fn write_available_modes() {
        let (dir, handler) = setup();
        let tool = WriteFileTool::new(dir.path().to_path_buf(), handler);
        assert_eq!(tool.available_modes(), vec![Mode::Execute]);
    }

    #[test]
    fn write_tool_debug() {
        let (dir, handler) = setup();
        let tool = WriteFileTool::new(dir.path().to_path_buf(), handler);
        let debug = format!("{:?}", tool);
        assert!(debug.contains("WriteFileTool"));
    }
}
```

### `src/tool/file_edit.rs`

Edits existing files via search/replace with an approval gate.

```rust
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
}

impl EditFileTool {
    pub fn new(
        working_directory: PathBuf,
        approval_handler: Arc<dyn ApprovalHandler>,
    ) -> Self {
        Self {
            working_directory,
            approval_handler,
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
                .string(
                    "path",
                    "File path relative to working directory",
                    true,
                )
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

        let resolved = self.resolve_path(path_str);

        // Read the existing file
        let old_content = fs::read_to_string(&resolved).await.map_err(|e| {
            ClosedCodeError::ToolError {
                name: "edit_file".into(),
                message: format!("Cannot read '{}': {}", path_str, e),
            }
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
        vec![Mode::Execute]
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

        let tool = EditFileTool::new(dir.path().to_path_buf(), handler);
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

        let tool = EditFileTool::new(dir.path().to_path_buf(), handler);
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

        let tool = EditFileTool::new(dir.path().to_path_buf(), handler);
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

        let tool = EditFileTool::new(dir.path().to_path_buf(), handler);
        let result = tool
            .execute(json!({
                "path": "test.rs",
                "old_text": "foo",
                "new_text": "bar"
            }))
            .await
            .unwrap();

        assert_eq!(result["status"], "applied");
        assert!(result["warning"].as_str().unwrap().contains("3 occurrences"));

        let content = std::fs::read_to_string(dir.path().join("test.rs")).unwrap();
        assert_eq!(content, "bar\nfoo\nfoo\n");
    }

    #[tokio::test]
    async fn edit_nonexistent_file() {
        let (dir, handler) = setup();
        let tool = EditFileTool::new(dir.path().to_path_buf(), handler);
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
        let tool = EditFileTool::new(dir.path().to_path_buf(), handler);

        // Missing old_text
        let result = tool.execute(json!({"path": "x.rs", "new_text": "y"})).await;
        assert!(result.is_err());

        // Missing new_text
        let result = tool
            .execute(json!({"path": "x.rs", "old_text": "y"}))
            .await;
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

        let tool = EditFileTool::new(dir.path().to_path_buf(), handler);
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
        let tool = EditFileTool::new(dir.path().to_path_buf(), handler);
        assert_eq!(tool.available_modes(), vec![Mode::Execute]);
    }

    #[test]
    fn edit_tool_debug() {
        let (dir, handler) = setup();
        let tool = EditFileTool::new(dir.path().to_path_buf(), handler);
        let debug = format!("{:?}", tool);
        assert!(debug.contains("EditFileTool"));
    }
}
```

### `src/tool/registry.rs` — Updated `create_orchestrator_registry`

Change the factory function signature to accept an `ApprovalHandler` for Execute mode write tools:

```rust
use crate::ui::approval::ApprovalHandler;

/// Create a ToolRegistry for the orchestrator.
/// Includes filesystem + spawn tools based on mode.
///
/// In Execute mode, `approval_handler` must be `Some` to register write tools.
/// In other modes, `approval_handler` is ignored (write tools are not registered).
pub fn create_orchestrator_registry(
    working_directory: PathBuf,
    mode: &Mode,
    client: Arc<GeminiClient>,
    approval_handler: Option<Arc<dyn ApprovalHandler>>,
) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    register_filesystem_tools(&mut registry, working_directory.clone());

    match mode {
        Mode::Explore => {
            registry.register(Box::new(super::spawn::SpawnExplorerTool::new(
                client,
                working_directory,
            )));
        }
        Mode::Plan => {
            registry.register(Box::new(super::spawn::SpawnExplorerTool::new(
                client.clone(),
                working_directory.clone(),
            )));
            registry.register(Box::new(super::spawn::SpawnPlannerTool::new(
                client.clone(),
                working_directory.clone(),
            )));
            registry.register(Box::new(super::spawn::SpawnWebSearchTool::new(
                client,
                working_directory,
            )));
        }
        Mode::Execute => {
            registry.register(Box::new(super::spawn::SpawnExplorerTool::new(
                client,
                working_directory.clone(),
            )));
            // Write tools — Execute mode only
            if let Some(handler) = approval_handler {
                registry.register(Box::new(
                    super::file_write::WriteFileTool::new(
                        working_directory.clone(),
                        handler.clone(),
                    ),
                ));
                registry.register(Box::new(
                    super::file_edit::EditFileTool::new(
                        working_directory,
                        handler,
                    ),
                ));
            }
        }
    }

    registry
}
```

**All callers must be updated**:
- `Orchestrator::new()` — pass `Some(approval_handler)`
- `Orchestrator::set_mode()` — pass `Some(self.approval_handler.clone())`
- Existing tests that call `create_orchestrator_registry` — pass `None`

**New tests**:

```rust
#[test]
fn create_orchestrator_registry_execute_mode_with_handler() {
    let client = Arc::new(crate::gemini::GeminiClient::new(
        "key".into(),
        "model".into(),
    ));
    let handler = Arc::new(
        crate::ui::approval::AutoApproveHandler::always_approve()
    ) as Arc<dyn crate::ui::approval::ApprovalHandler>;
    let registry = create_orchestrator_registry(
        PathBuf::from("/tmp"),
        &Mode::Execute,
        client,
        Some(handler),
    );
    // 5 filesystem/shell + spawn_explorer + write_file + edit_file = 8
    assert_eq!(registry.len(), 8);
    assert!(registry.get("write_file").is_some());
    assert!(registry.get("edit_file").is_some());
    assert!(registry.get("spawn_explorer").is_some());
}

#[test]
fn create_orchestrator_registry_execute_mode_without_handler() {
    let client = Arc::new(crate::gemini::GeminiClient::new(
        "key".into(),
        "model".into(),
    ));
    let registry = create_orchestrator_registry(
        PathBuf::from("/tmp"),
        &Mode::Execute,
        client,
        None,
    );
    // No write tools registered without handler
    assert_eq!(registry.len(), 6);
    assert!(registry.get("write_file").is_none());
    assert!(registry.get("edit_file").is_none());
}

#[test]
fn create_orchestrator_registry_explore_ignores_handler() {
    let client = Arc::new(crate::gemini::GeminiClient::new(
        "key".into(),
        "model".into(),
    ));
    let handler = Arc::new(
        crate::ui::approval::AutoApproveHandler::always_approve()
    ) as Arc<dyn crate::ui::approval::ApprovalHandler>;
    let registry = create_orchestrator_registry(
        PathBuf::from("/tmp"),
        &Mode::Explore,
        client,
        Some(handler),
    );
    // Explore mode: no write tools regardless of handler
    assert_eq!(registry.len(), 6);
    assert!(registry.get("write_file").is_none());
}
```

**Update existing tests** that call `create_orchestrator_registry` to pass `None`:

```rust
// Before (Phase 3):
create_orchestrator_registry(PathBuf::from("/tmp"), &Mode::Explore, client)

// After (Phase 4):
create_orchestrator_registry(PathBuf::from("/tmp"), &Mode::Explore, client, None)
```

### `src/agent/orchestrator.rs` — Orchestrator Updates

**Changes to the Orchestrator struct**:

```rust
use crate::ui::approval::ApprovalHandler;

pub struct Orchestrator {
    client: Arc<GeminiClient>,
    mode: Mode,
    working_directory: PathBuf,
    history: Vec<Content>,
    registry: ToolRegistry,
    system_prompt: String,
    max_output_tokens: u32,
    approval_handler: Arc<dyn ApprovalHandler>,  // Phase 4
    current_plan: Option<String>,                // Phase 4
}
```

**Updated constructor**:

```rust
impl Orchestrator {
    pub fn new(
        client: Arc<GeminiClient>,
        mode: Mode,
        working_directory: PathBuf,
        max_output_tokens: u32,
        approval_handler: Arc<dyn ApprovalHandler>,
    ) -> Self {
        let registry = create_orchestrator_registry(
            working_directory.clone(),
            &mode,
            client.clone(),
            Some(approval_handler.clone()),
        );
        let system_prompt = Self::build_system_prompt(&mode, &working_directory);

        Self {
            client,
            mode,
            working_directory,
            history: Vec::new(),
            registry,
            system_prompt,
            max_output_tokens,
            approval_handler,
            current_plan: None,
        }
    }
```

**Updated `set_mode`**:

```rust
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.registry = create_orchestrator_registry(
            self.working_directory.clone(),
            &self.mode,
            self.client.clone(),
            Some(self.approval_handler.clone()),
        );
        self.system_prompt = Self::build_system_prompt(&self.mode, &self.working_directory);
    }
```

**Plan tracking methods** (new):

```rust
    /// Store the current plan text.
    /// Called by the REPL after each Plan mode response.
    pub fn set_current_plan(&mut self, plan: String) {
        self.current_plan = Some(plan);
    }

    /// Get the current plan, if any.
    pub fn current_plan(&self) -> Option<&str> {
        self.current_plan.as_deref()
    }

    /// Accept the current plan and switch to Execute mode.
    ///
    /// Injects the accepted plan into conversation history as context,
    /// then switches mode to Execute (which registers write tools).
    /// Returns the plan text if one was set, or None.
    pub fn accept_plan(&mut self) -> Option<String> {
        if let Some(plan) = self.current_plan.take() {
            self.history.push(Content::user(&format!(
                "[ACCEPTED PLAN — Execute this plan step by step]\n\n{}",
                plan
            )));
            self.set_mode(Mode::Execute);
            Some(plan)
        } else {
            None
        }
    }
```

**Enhanced system prompts**:

```rust
    fn build_system_prompt(mode: &Mode, working_directory: &std::path::Path) -> String {
        let base = format!(
            "You are closed-code, an AI coding assistant operating in {} mode.\n\
             Working directory: {}",
            mode,
            working_directory.display()
        );

        let mode_section = match mode {
            Mode::Explore => {
                "\n\nYou are in EXPLORE mode. You are strictly READ-ONLY.\n\
                 You CANNOT create, modify, or delete any files.\n\
                 \n\
                 Your role is to help the user understand the codebase:\n\
                 - Read and analyze files using read_file\n\
                 - Search for patterns with search_files and grep\n\
                 - List directory contents with list_directory\n\
                 - Run read-only shell commands (git log, cargo check, etc.)\n\
                 - Use spawn_explorer for deep codebase research\n\
                 \n\
                 Explain code architecture, patterns, data flow, and answer questions.\n\
                 NEVER suggest creating or modifying files in this mode."
            }
            Mode::Plan => {
                "\n\nYou are in PLAN mode. You create implementation plans for review.\n\
                 You CANNOT modify files. Your job is to:\n\
                 1. Understand the user's requirements\n\
                 2. Research the codebase using filesystem tools and sub-agents\n\
                 3. Produce a clear, structured implementation plan with:\n\
                    - Step-by-step implementation order\n\
                    - Files to create or modify (with specific changes)\n\
                    - Code patterns to follow from the existing codebase\n\
                    - Potential risks or trade-offs\n\
                 \n\
                 Available tools:\n\
                 - spawn_explorer: Deep codebase research\n\
                 - spawn_planner: Create detailed implementation plans\n\
                 - spawn_web_search: Research topics online\n\
                 - All filesystem read tools\n\
                 \n\
                 The user will either:\n\
                 - Give feedback to refine the plan (continue the conversation)\n\
                 - Accept the plan with /accept (transitions to Execute mode)"
            }
            Mode::Execute => {
                "\n\nYou are in EXECUTE mode. You can create and edit files.\n\
                 \n\
                 Available tools:\n\
                 - write_file: Create new files or overwrite existing ones\n\
                 - edit_file: Make targeted changes using search/replace\n\
                 - spawn_explorer: Research code before making changes\n\
                 - All filesystem read tools (read_file, list_directory, search_files, grep, shell)\n\
                 \n\
                 IMPORTANT workflow:\n\
                 1. Always read the file first (read_file) before editing it\n\
                 2. Use edit_file for targeted changes (preferred over write_file for existing files)\n\
                 3. Use write_file for new files or complete rewrites\n\
                 4. Every file change shows a diff and requires user approval\n\
                 5. If a change is rejected, ask the user what they want instead\n\
                 \n\
                 Make changes methodically: one file at a time, with clear purpose."
            }
        };

        format!("{}{}", base, mode_section)
    }
```

**Updated `Debug` impl**:

```rust
impl std::fmt::Debug for Orchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Orchestrator")
            .field("mode", &self.mode)
            .field("tools", &self.registry.len())
            .field("history_len", &self.history.len())
            .field("has_plan", &self.current_plan.is_some())
            .finish()
    }
}
```

**Updated tests** (existing tests need the new `approval_handler` parameter):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::approval::AutoApproveHandler;

    fn test_client() -> Arc<GeminiClient> {
        Arc::new(GeminiClient::new("key".into(), "model".into()))
    }

    fn test_handler() -> Arc<dyn crate::ui::approval::ApprovalHandler> {
        Arc::new(AutoApproveHandler::always_approve())
    }

    #[test]
    fn orchestrator_new_explore_mode() {
        let orch = Orchestrator::new(
            test_client(),
            Mode::Explore,
            PathBuf::from("/tmp"),
            8192,
            test_handler(),
        );
        assert_eq!(orch.tool_count(), 6);
        assert_eq!(*orch.mode(), Mode::Explore);
        assert!(orch.system_prompt().contains("READ-ONLY"));
        assert!(!orch.system_prompt().contains("write_file"));
    }

    #[test]
    fn orchestrator_new_plan_mode() {
        let orch = Orchestrator::new(
            test_client(),
            Mode::Plan,
            PathBuf::from("/tmp"),
            8192,
            test_handler(),
        );
        assert_eq!(orch.tool_count(), 8);
        assert!(orch.system_prompt().contains("PLAN"));
        assert!(orch.system_prompt().contains("/accept"));
    }

    #[test]
    fn orchestrator_new_execute_mode() {
        let orch = Orchestrator::new(
            test_client(),
            Mode::Execute,
            PathBuf::from("/tmp"),
            8192,
            test_handler(),
        );
        // 5 filesystem/shell + spawn_explorer + write_file + edit_file = 8
        assert_eq!(orch.tool_count(), 8);
        assert!(orch.system_prompt().contains("EXECUTE"));
        assert!(orch.system_prompt().contains("write_file"));
        assert!(orch.system_prompt().contains("edit_file"));
    }

    #[test]
    fn orchestrator_set_current_plan() {
        let mut orch = Orchestrator::new(
            test_client(),
            Mode::Plan,
            PathBuf::from("/tmp"),
            8192,
            test_handler(),
        );

        assert!(orch.current_plan().is_none());
        orch.set_current_plan("Step 1: Add feature X".into());
        assert_eq!(orch.current_plan(), Some("Step 1: Add feature X"));
    }

    #[test]
    fn orchestrator_accept_plan_switches_to_execute() {
        let mut orch = Orchestrator::new(
            test_client(),
            Mode::Plan,
            PathBuf::from("/tmp"),
            8192,
            test_handler(),
        );

        orch.set_current_plan("The plan content".into());
        let plan = orch.accept_plan();

        assert!(plan.is_some());
        assert_eq!(plan.unwrap(), "The plan content");
        assert_eq!(*orch.mode(), Mode::Execute);
        assert_eq!(orch.tool_count(), 8); // Now has write tools
        assert!(orch.current_plan().is_none()); // Plan consumed

        // Plan should be in history
        let last_user_msg = orch.history.last().unwrap();
        let text = last_user_msg.parts.first().unwrap();
        if let crate::gemini::types::Part::Text(t) = text {
            assert!(t.contains("[ACCEPTED PLAN"));
            assert!(t.contains("The plan content"));
        } else {
            panic!("Expected text part in history");
        }
    }

    #[test]
    fn orchestrator_accept_plan_no_plan() {
        let mut orch = Orchestrator::new(
            test_client(),
            Mode::Plan,
            PathBuf::from("/tmp"),
            8192,
            test_handler(),
        );

        let plan = orch.accept_plan();
        assert!(plan.is_none());
        assert_eq!(*orch.mode(), Mode::Plan); // Mode unchanged
    }

    // ... keep all existing tests, updating constructors to include test_handler()
}
```

### `src/repl.rs` — REPL Updates

**Updated REPL setup** to create `TerminalApprovalHandler` and pass to `Orchestrator`:

```rust
use crate::ui::approval::TerminalApprovalHandler;

pub async fn run_oneshot(config: &Config, question: &str) -> anyhow::Result<()> {
    let client = Arc::new(GeminiClient::new(
        config.api_key.clone(),
        config.model.clone(),
    ));
    let approval_handler = Arc::new(TerminalApprovalHandler::new());
    let mut orchestrator = Orchestrator::new(
        client,
        config.mode,
        config.working_directory.clone(),
        config.max_output_tokens,
        approval_handler,
    );
    // ... rest unchanged
}

pub async fn run_repl(config: &Config) -> anyhow::Result<()> {
    let client = Arc::new(GeminiClient::new(
        config.api_key.clone(),
        config.model.clone(),
    ));
    let approval_handler = Arc::new(TerminalApprovalHandler::new());
    let mut orchestrator = Orchestrator::new(
        client,
        config.mode,
        config.working_directory.clone(),
        config.max_output_tokens,
        approval_handler,
    );
    // ... rest continues below
```

**New `SlashResult` variant**:

```rust
enum SlashResult {
    Continue,
    Quit,
    ExecutePlan,  // Phase 4: triggers plan execution after /accept
}
```

**Updated REPL loop** to handle `ExecutePlan` and capture plans:

```rust
    loop {
        let prompt = format!("{} > ", orchestrator.mode());
        match editor.readline(&prompt) {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let _ = editor.add_history_entry(line);

                if line.starts_with('/') {
                    match handle_slash_command(line, &mut orchestrator) {
                        SlashResult::Continue => continue,
                        SlashResult::Quit => break,
                        SlashResult::ExecutePlan => {
                            // Auto-trigger plan execution
                            match orchestrator
                                .handle_user_input_streaming(
                                    "Execute the accepted plan step by step.",
                                    default_stream_handler,
                                )
                                .await
                            {
                                Ok(_) => {}
                                Err(e) => {
                                    eprintln!(
                                        "\n{}: {}",
                                        styled_text("Error", Theme::ERROR),
                                        e
                                    );
                                }
                            }
                            println!();
                            continue;
                        }
                    }
                }

                match orchestrator
                    .handle_user_input_streaming(line, default_stream_handler)
                    .await
                {
                    Ok(ref text) => {
                        // Phase 4: Capture plan text in Plan mode
                        if *orchestrator.mode() == crate::mode::Mode::Plan && !text.is_empty() {
                            orchestrator.set_current_plan(text.clone());
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "\n{}: {}",
                            styled_text("Error", Theme::ERROR),
                            e
                        );
                    }
                }
                println!();
            }
            // ... Ctrl+C, EOF handlers unchanged
        }
    }
```

**Updated `handle_slash_command`** with `/accept`:

```rust
fn handle_slash_command(input: &str, orchestrator: &mut Orchestrator) -> SlashResult {
    match input {
        "/quit" | "/exit" | "/q" => SlashResult::Quit,
        "/clear" => {
            orchestrator.clear_history();
            println!("Conversation history cleared.");
            SlashResult::Continue
        }
        "/accept" | "/a" => {
            if *orchestrator.mode() != crate::mode::Mode::Plan {
                println!(
                    "{}: /accept is only available in Plan mode. Current mode: {}",
                    styled_text("Error", Theme::ERROR),
                    orchestrator.mode()
                );
                return SlashResult::Continue;
            }
            match orchestrator.accept_plan() {
                Some(_) => {
                    println!(
                        "{} Plan accepted. Switched to Execute mode (tools: {}).",
                        styled_text("✓", Theme::SUCCESS),
                        orchestrator.tool_count()
                    );
                    SlashResult::ExecutePlan
                }
                None => {
                    println!(
                        "No plan to accept. Ask the assistant to create a plan first."
                    );
                    SlashResult::Continue
                }
            }
        }
        "/help" => {
            println!("Commands:");
            println!("  /help          — Show this help");
            println!("  /mode [name]   — Show or switch mode (explore, plan, execute)");
            println!("  /accept        — Accept the current plan and switch to Execute mode");
            println!("  /clear         — Clear conversation history");
            println!("  /quit          — Exit");
            SlashResult::Continue
        }
        input if input.starts_with("/mode") => {
            // ... existing mode handling, unchanged
        }
        _ => {
            println!(
                "Unknown command: {}. Type /help for available commands.",
                input
            );
            SlashResult::Continue
        }
    }
}
```

**Updated tests**:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::approval::AutoApproveHandler;
    use std::path::PathBuf;

    fn test_orchestrator() -> Orchestrator {
        let client = Arc::new(GeminiClient::new("key".into(), "model".into()));
        let handler = Arc::new(AutoApproveHandler::always_approve())
            as Arc<dyn crate::ui::approval::ApprovalHandler>;
        Orchestrator::new(
            client,
            crate::mode::Mode::Explore,
            PathBuf::from("/tmp"),
            8192,
            handler,
        )
    }

    fn test_plan_orchestrator() -> Orchestrator {
        let client = Arc::new(GeminiClient::new("key".into(), "model".into()));
        let handler = Arc::new(AutoApproveHandler::always_approve())
            as Arc<dyn crate::ui::approval::ApprovalHandler>;
        Orchestrator::new(
            client,
            crate::mode::Mode::Plan,
            PathBuf::from("/tmp"),
            8192,
            handler,
        )
    }

    // ... existing tests updated to use test_orchestrator() ...

    #[test]
    fn slash_accept_in_plan_mode_with_plan() {
        let mut orch = test_plan_orchestrator();
        orch.set_current_plan("My implementation plan".into());
        let result = handle_slash_command("/accept", &mut orch);
        assert!(matches!(result, SlashResult::ExecutePlan));
        assert_eq!(*orch.mode(), crate::mode::Mode::Execute);
    }

    #[test]
    fn slash_accept_in_plan_mode_no_plan() {
        let mut orch = test_plan_orchestrator();
        let result = handle_slash_command("/accept", &mut orch);
        assert!(matches!(result, SlashResult::Continue));
        assert_eq!(*orch.mode(), crate::mode::Mode::Plan); // unchanged
    }

    #[test]
    fn slash_accept_in_explore_mode() {
        let mut orch = test_orchestrator(); // Explore mode
        let result = handle_slash_command("/accept", &mut orch);
        assert!(matches!(result, SlashResult::Continue));
        assert_eq!(*orch.mode(), crate::mode::Mode::Explore); // unchanged
    }

    #[test]
    fn slash_accept_shorthand() {
        let mut orch = test_plan_orchestrator();
        orch.set_current_plan("plan".into());
        let result = handle_slash_command("/a", &mut orch);
        assert!(matches!(result, SlashResult::ExecutePlan));
    }
}
```

---

## Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| **`similar` crate** for diffs | Battle-tested Myers diff algorithm. Used widely in Rust ecosystem. Generates clean unified diffs with context lines. |
| **`dialoguer` for approval prompt** | Properly handles terminal state (raw mode, cursor). `rustyline` may have modified terminal settings between readline calls, and dialoguer interoperates cleanly. |
| **`ApprovalHandler` as `Arc<dyn>`** | Shared reference across WriteFileTool and EditFileTool. Persists across mode switches (Plan → Execute). Allows different implementations (terminal, auto, future TUI overlay). |
| **`spawn_blocking` for approval** | `dialoguer::Confirm` blocks stdin synchronously. Since `Tool::execute()` is async, we use `tokio::task::spawn_blocking` to avoid blocking the tokio runtime. |
| **Default `No` on approval** | Safety-first: the user must explicitly type `y` to apply changes. Accidental Enter does not apply changes. |
| **Rejected changes return JSON, not errors** | When a user rejects a change, the LLM receives `{"status": "rejected"}` as a function response. This lets the LLM adapt (try a different approach, ask for clarification) rather than crashing the tool loop. |
| **Auto-trigger after `/accept`** | After plan acceptance, the REPL auto-sends "Execute the accepted plan step by step." This provides the best UX — no need for the user to type anything to start execution. |
| **Plan captured from last response** | Every Plan mode response is stored as `current_plan`. This means `/accept` always acts on the most recent plan. If the user gives feedback and the LLM refines, the refined version becomes the plan. |
| **Write tools gated by `available_modes()`** | WriteFileTool and EditFileTool return `[Mode::Execute]` from `available_modes()`. The ToolRegistry filters them out in Explore and Plan modes. This is the same pattern used by SpawnPlannerTool. |
| **Defer `ui/markdown.rs`** | Markdown rendering with `syntect` is orthogonal to the core Phase 4 deliverables (write tools, diffs, approvals, mode behavior). It belongs in Phase 5 alongside the enhanced REPL. |
| **`ApprovalHandler: Debug` bound** | The Tool trait requires `Debug`. By requiring `Debug` on `ApprovalHandler`, we can derive Debug on tools rather than needing manual implementations. Both concrete handlers are trivially Debug. |
| **`no_change` status** | When write_file is called with content identical to the existing file, or edit_file is called with identical old_text/new_text, we skip the approval prompt and return `{"status": "no_change"}`. This avoids unnecessary user interaction. |

---

## Implementation Sequence

Each step produces a compilable, testable increment:

| Step | Files | Description |
|------|-------|-------------|
| 1 | `Cargo.toml`, `src/tool/mod.rs`, `src/ui/mod.rs` | Add dependencies, module declarations |
| 2 | `src/ui/theme.rs` | Add diff color constants |
| 3 | `src/error.rs` | Add `ProtectedPath`, `ApprovalError` variants |
| 4 | `src/ui/diff.rs` | Implement diff generation and colorized display |
| 5 | `src/ui/approval.rs` | Implement ApprovalHandler trait and handlers |
| 6 | `src/tool/file_write.rs` | Implement WriteFileTool with tests |
| 7 | `src/tool/file_edit.rs` | Implement EditFileTool with tests |
| 8 | `src/tool/registry.rs` | Wire write tools into Execute mode, update callers |
| 9 | `src/agent/orchestrator.rs` | Add approval handler, plan tracking, enhanced prompts |
| 10 | `src/repl.rs` | Add /accept command, plan capture, ExecutePlan flow |

---

## Milestone / Verification

After implementing Phase 4, verify each capability:

```bash
# 1. Explore mode is strictly read-only
cargo run --mode explore
# explore > Create a file called test.rs
# (LLM has no write tools, explains it cannot modify files)
# (System prompt says "READ-ONLY", no write_file in tool declarations)

# 2. Plan mode creates plans
cargo run --mode plan
# plan > Add error handling to the API layer
# ⠋ Spawning planner...
# ⠋ Spawning explorer...
# Here's my implementation plan:
# 1. Create an error module...
# 2. Add Result types...
# (structured plan output)

# 3. Plan refinement with feedback
# plan > I'd prefer to use anyhow instead of thiserror
# Updated plan:
# 1. Add anyhow to Cargo.toml...
# (refined plan)

# 4. Plan acceptance transitions to Execute mode
# plan > /accept
# ✓ Plan accepted. Switched to Execute mode (tools: 8).
# ⠋ Thinking...
# I'll start executing the plan step by step.
# First, let me read the current Cargo.toml...
# ⠋ [tool] read_file(path: "Cargo.toml")
# ✓ [tool] read_file(path: "Cargo.toml")
# Now I'll add anyhow as a dependency.
# ⠋ [tool] edit_file(path: "Cargo.toml", ...)
#
# --- a/Cargo.toml
# +++ b/Cargo.toml
# @@ -12,6 +12,7 @@
#  [dependencies]
#  tokio = { version = "1", features = ["full"] }
# +anyhow = "1"
#
#   File: Cargo.toml
#   Changes: 1 addition, 0 deletions
#
# Apply this change? [y/N] y
# ✓ [tool] edit_file(path: "Cargo.toml")
# ...continues with next steps...

# 5. Execute mode direct entry
cargo run --mode execute
# execute > Create a hello.rs with a hello world program
#
# --- /dev/null
# +++ b/hello.rs
# @@ -0,0 +1,3 @@
# +fn main() {
# +    println!("Hello, world!");
# +}
#
#   File: hello.rs (new)
#   Changes: 3 additions, 0 deletions
#
# Apply this change? [y/N] y
# ✓ File created: hello.rs

# 6. Edit an existing file
# execute > Add a goodbye function to hello.rs
# ⠋ [tool] read_file(path: "hello.rs")
# ✓ [tool] read_file(path: "hello.rs")
#
# --- a/hello.rs
# +++ b/hello.rs
# @@ -1,3 +1,8 @@
#  fn main() {
#      println!("Hello, world!");
# +    goodbye();
# +}
# +
# +fn goodbye() {
# +    println!("Goodbye!");
#  }
#
#   File: hello.rs
#   Changes: 5 additions, 0 deletions
#
# Apply this change? [y/N] n
# Change rejected. What would you prefer?

# 7. Rejection feedback
# execute > Just add a comment instead
# ⠋ [tool] edit_file(path: "hello.rs", ...)
# (shows diff with just a comment added)
# Apply this change? [y/N] y
# ✓ File edited: hello.rs

# 8. Tool count verification
# execute > /mode
# Current mode: execute. Usage: /mode <explore|plan|execute>
# (should show 8 tools)

# 9. Mode switching preserves history
# execute > /mode explore
# Switched to explore mode. Tools: 6
# explore > (no write tools available)
# explore > /mode execute
# Switched to execute mode. Tools: 8
# (write tools restored)

# 10. /accept only works in Plan mode
# explore > /accept
# Error: /accept is only available in Plan mode. Current mode: explore

# 11. Tests pass
cargo test
# test ui::diff::tests::diff_identical_files ... ok
# test ui::diff::tests::diff_new_file ... ok
# test ui::approval::tests::auto_approve_handler_approves ... ok
# test tool::file_write::tests::write_new_file_approved ... ok
# test tool::file_edit::tests::edit_file_approved ... ok
# test tool::registry::tests::create_orchestrator_registry_execute_mode_with_handler ... ok
# test agent::orchestrator::tests::orchestrator_accept_plan_switches_to_execute ... ok
# test repl::tests::slash_accept_in_plan_mode_with_plan ... ok
# ...
# test result: ok. ~25 passed; 0 failed
```

---

## What This Phase Does NOT Include

These are explicitly deferred to later phases:

- **Markdown rendering** (`ui/markdown.rs` with `syntect`) — Phase 5. Orthogonal to write tools and approvals.
- **TOML configuration** — Phase 5. Approval policies (Suggest, AutoEdit, FullAuto) require the config system.
- **Protected paths** (`.git/`, `.env`, etc.) — Phase 6. The `ProtectedPath` error variant is defined but not enforced.
- **Sandboxing** — Phase 7. Shell commands run unsandboxed; the allowlist is the only safety mechanism.
- **Session persistence** — Phase 8. File changes are not logged to persistent sessions.
- **TUI approval overlay** — Phase 9. Approvals use inline terminal prompts, not modal overlays.
- **Parallel tool execution** — Phase 10. Multiple write tools execute sequentially.
- **Undo/revert** — Not in scope. The user can manually revert using git.

---

*See [phase_3.md](phase_3.md) for the sub-agent architecture this phase builds on, and [phase_spec.md](phase_spec.md) for the full 10-phase roadmap.*
