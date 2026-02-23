# Phase 6: Git Integration + Diff Review + Protected Paths

**Goal**: Deep git awareness — branch info in system prompts, `/diff` for reviewing changes, `/review` for LLM code review, `/commit` for auto-commit with generated messages, and protected path enforcement on write tools.

**Depends on**: Phase 5 (Configuration + Enhanced REPL)

---

## Phase Dependency Graph (within Phase 6)

```
6.1 Git Module (context, diff, commit)
  │
  ├──► 6.2 Protected Paths (file_write, file_edit)
  │
  ├──► 6.3 Git-Aware System Prompt (orchestrator)
  │         │
  │         ▼
  └──► 6.4 Git Slash Commands (repl.rs)
              │
              ▼
        6.5 Sub-Agent Powered /commit + /review
```

---

## Files Overview

```
src/
  git/
    mod.rs             # NEW: Module re-exports + shared run_git() helper
    context.rs         # NEW: GitContext struct — branch, changes, recent commits
    diff.rs            # NEW: Git diff functions (unstaged, staged, branch, range)
    commit.rs          # NEW: Commit operations (commit_all, commit_files)
  agent/
    commit_agent.rs    # NEW: CommitAgent — sub-agent for generating commit messages
    review_agent.rs    # NEW: ReviewAgent — sub-agent for structured code reviews
    mod.rs             # MODIFIED: Added pub mod commit_agent, review_agent
    orchestrator.rs    # MODIFIED: git_context field, system prompt injection, sub-agent runners
  tool/
    file_write.rs      # MODIFIED: Protected path check before writes
    file_edit.rs       # MODIFIED: Protected path check before edits
  repl.rs              # MODIFIED: async handle_slash_command, /diff, /review, /commit with sub-agents
  lib.rs               # MODIFIED: Added `pub mod git;`
```

**No new Cargo dependencies** — all git operations use subprocess shell-outs via `tokio::process::Command`. `dialoguer` (already in Cargo.toml) is used for commit confirmation prompts.

---

## Sub-Phase 6.1: Git Module

### New File: `src/git/mod.rs`

Module re-exports and a shared helper used by all git submodules:

```rust
pub mod commit;
pub mod context;
pub mod diff;

pub use context::GitContext;

/// Run a git command in the given working directory.
/// Returns `Some(stdout)` on success, `None` on failure.
pub(crate) async fn run_git(working_dir: &Path, args: &[&str]) -> Option<String>
```

`run_git` never panics or returns errors — it returns `None` on any failure. This makes it safe for best-effort detection where git may not be installed or the directory may not be a repo.

### New File: `src/git/context.rs` — GitContext

A data struct populated by running git commands asynchronously:

```rust
#[derive(Debug, Clone)]
pub struct GitContext {
    pub is_git_repo: bool,
    pub current_branch: Option<String>,
    pub default_branch: Option<String>,
    pub has_uncommitted_changes: bool,
    pub changed_files: Vec<ChangedFile>,
    pub recent_commits: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ChangedFile {
    pub path: String,
    pub status: FileStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
}
```

**Key methods:**

| Method | Description |
|--------|-------------|
| `GitContext::detect(working_dir)` | Async. Runs all detection commands, returns populated struct. Never panics or errors. |
| `GitContext::summary()` | One-line: `"main (3 uncommitted changes)"` or `"main (clean)"` |
| `GitContext::system_prompt_section()` | Multi-line for system prompt injection |
| `Display` impl | Delegates to `summary()` |

**Detection commands:**

- `git rev-parse --is-inside-work-tree` → `is_git_repo`
- `git branch --show-current` → `current_branch`
- `git rev-parse --verify main` (then fallback `master`) → `default_branch`
- `git status --porcelain` → `changed_files` + `has_uncommitted_changes`
- `git log --oneline -5` → `recent_commits`

**Internal helpers:**

- `detect_default_branch()` — tries "main" then "master"
- `detect_changes()` — parses `git status --porcelain` output
- `parse_porcelain_line()` — parses XY status codes (`" M"`, `"A "`, `"??"`, `"D "`, `"R "`)

### New File: `src/git/diff.rs` — Git Diff Functions

Functions that shell out to `git diff` and return raw diff strings:

```rust
/// Unstaged changes (working tree vs index)
pub async fn unstaged(working_dir: &Path) -> Result<String>

/// Staged changes only (index vs HEAD)
pub async fn staged(working_dir: &Path) -> Result<String>

/// All uncommitted changes (working tree vs HEAD)
pub async fn all_uncommitted(working_dir: &Path) -> Result<String>

/// Changes since branching from the given base branch
pub async fn branch_diff(working_dir: &Path, base_branch: &str) -> Result<String>

/// Diff for a commit range (e.g., "HEAD~3..HEAD" or "HEAD~3")
pub async fn commit_range(working_dir: &Path, range: &str) -> Result<String>

/// Print a raw git diff string with ANSI coloring to stdout
pub fn colorize_git_diff(raw_diff: &str)
```

`colorize_git_diff` uses existing `Theme::DIFF_ADD`, `Theme::DIFF_DELETE`, `Theme::DIFF_HUNK`, and `Theme::DIFF_CONTEXT` colors for consistent styling.

All async functions verify the directory is a git repo before running diff commands and return `ClosedCodeError::ToolError` on failure.

### New File: `src/git/commit.rs` — Commit Operations

```rust
/// Stage all changes and commit with the given message.
/// Returns the short SHA of the new commit.
pub async fn commit_all(working_dir: &Path, message: &str) -> Result<String>

/// Stage specific files and commit.
/// Returns the short SHA of the new commit.
pub async fn commit_files(working_dir: &Path, files: &[&str], message: &str) -> Result<String>

/// Get the short SHA of the most recent commit.
pub async fn last_commit_sha(working_dir: &Path) -> Result<String>
```

`commit_all` runs `git add -A` then `git commit -m "{message}"`, returns SHA from `git rev-parse --short HEAD`.

### `src/lib.rs`

Added `pub mod git;` to the module declarations.

---

## Sub-Phase 6.2: Protected Paths

### `src/tool/file_write.rs` and `src/tool/file_edit.rs`

Both files received the same protection mechanism — a standalone `is_protected_path()` function and a check in the `execute()` method:

```rust
fn is_protected_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    normalized == ".git"
        || normalized.starts_with(".git/")
        || normalized == ".closed-code"
        || normalized.starts_with(".closed-code/")
}
```

The check is placed after path extraction, before any file I/O:

```rust
if is_protected_path(path_str) {
    return Err(ClosedCodeError::ProtectedPath {
        path: path_str.to_string(),
    });
}
```

**Protected directories:**

| Path | Reason |
|------|--------|
| `.git/` | Git internal data must never be modified by the LLM |
| `.closed-code/` | Application config must not be modified by tools |

**Not protected** (by design):

- `.github/` — CI/CD workflows are legitimate edit targets
- `.gitignore` — top-level gitignore is a regular file
- `src/.gitignore` — nested gitignore files

The `ClosedCodeError::ProtectedPath` variant (added in Phase 4) is reused here.

---

## Sub-Phase 6.3: Git-Aware System Prompt

### `src/agent/orchestrator.rs`

**New field:**

```rust
pub struct Orchestrator {
    // ... existing fields ...
    // Phase 6
    git_context: Option<GitContext>,
}
```

**Updated `build_system_prompt()`** — now accepts `git_context: Option<&GitContext>`:

```rust
fn build_system_prompt(
    mode: &Mode,
    working_directory: &Path,
    personality: Personality,
    git_context: Option<&GitContext>,
) -> String
```

When git context is present, the system prompt includes a section like:

```
Git context: On branch `main`, 3 uncommitted changes.
Changed files: src/auth.rs (modified), src/lib.rs (modified), tests/auth_test.rs (added)
Recent commits:
  abc1234 Add login endpoint
  def5678 Setup database connection
```

When working tree is clean:

```
Git context: On branch `main`, working tree clean.
```

**Design decision**: `Orchestrator::new()` remains synchronous. Git detection is performed via a separate `detect_git_context()` async method called after construction. This avoids converting 25+ existing synchronous tests to async.

**New methods:**

| Method | Description |
|--------|-------------|
| `detect_git_context(&mut self)` | Async. Detects git context and rebuilds system prompt. |
| `refresh_git_context(&mut self)` | Async. Re-detects (e.g., after a commit). |
| `working_directory(&self) -> &Path` | Returns the working directory. |
| `git_default_branch(&self) -> Option<&str>` | Returns detected default branch ("main" or "master"). |
| `git_summary(&self) -> String` | One-line git summary for `/status` display. |

**Updated methods** — `set_mode()` and `set_personality()` now pass `self.git_context.as_ref()` to `build_system_prompt()`, preserving git context across mode and personality changes.

### `src/repl.rs` — Startup Changes

Both `run_repl()` and `run_oneshot()` now call `orchestrator.detect_git_context().await` after construction.

The startup banner now includes a git line:

```
closed-code
Mode: explore | Model: gemini-2.5-pro | Tools: 6
Working directory: /Users/me/project
Git: main (3 uncommitted changes)
Type /help for commands, Ctrl+C to interrupt, /quit to exit.
```

---

## Sub-Phase 6.4: Git Slash Commands

### `handle_slash_command` → `async fn`

The function signature changed from `fn` to `async fn` to support the new git commands that call async diff/commit functions:

```rust
async fn handle_slash_command(
    input: &str,
    orchestrator: &mut Orchestrator,
) -> SlashResult
```

The call site in `run_repl()` was updated to `.await` on the result. All existing tests were converted from `#[test]` to `#[tokio::test]` with `.await` on `handle_slash_command` calls.

### `/diff [staged|branch|HEAD~N]`

Shows colorized git diffs:

| Usage | Behavior |
|-------|----------|
| `/diff` or `/diff all` | All uncommitted changes (working tree vs HEAD) |
| `/diff staged` | Only staged changes (index vs HEAD) |
| `/diff branch` | Changes since branching from the default branch |
| `/diff HEAD~N` | Diff for a commit range |

Displays colorized output via `colorize_git_diff()`. Shows "No changes found." when the diff is empty. Shows usage help for unrecognized arguments.

### `/review [HEAD~N]`

Spawns a **ReviewAgent** sub-agent to analyze changes and produce a structured code review:

| Usage | Behavior |
|-------|----------|
| `/review` | Review all uncommitted changes |
| `/review HEAD~N` | Review changes in a commit range |

**Flow:**

1. Get the diff (uncommitted or commit range).
2. Spawn a ReviewAgent sub-agent with the diff as context. The agent can explore files, read related code, and use spawn_explorer for deep research.
3. The sub-agent returns a structured review via `create_report`.
4. Print the review to the terminal.
5. Inject the review into the main conversation history as `[CODE REVIEW — Sub-agent analysis]`, so the user can ask follow-up questions.

Shows "No changes to review." when the diff is empty. A spinner displays while the sub-agent is working.

**Key benefit**: The raw diff never enters the main conversation history — only the compact, structured review does.

### `/commit [message]`

Generates a commit message and commits with user approval:

| Usage | Behavior |
|-------|----------|
| `/commit` | Sub-agent generates commit message from diff, user confirms |
| `/commit fix auth bug` | Uses provided message directly, user confirms |

**Flow:**

1. Get all uncommitted changes via `all_uncommitted()`. Exit early if nothing to commit.
2. If no message argument, spawn a **CommitAgent** sub-agent with the diff as context. The agent can explore files for context and returns the commit message via `create_report`. A spinner displays during generation.
3. If message argument is provided, use it directly (no sub-agent involved).
4. Display the proposed commit message.
5. Prompt for confirmation using `dialoguer::Confirm` (default: No).
6. If approved, run `commit_all()` and display the short SHA.
7. After successful commit, call `orchestrator.refresh_git_context().await` to update the system prompt.

**Key benefit**: The sub-agent can explore related files to understand changes in context, producing better commit messages than a simple prompt-based approach. No diff or LLM response enters the main conversation history.

### `/help` (updated)

Added three new entries:

```
/diff [opts]       — Show git diff (staged, branch, HEAD~N)
/review [HEAD~N]   — Send changes to LLM for code review
/commit [message]  — Generate commit message and commit
```

### `/status` (updated)

Added git summary line:

```
Mode: explore | Model: gemini-2.5-pro | Personality: pragmatic
Git: main (3 uncommitted changes)
Tokens: 1,234 prompt + 567 completion = 1,801 total (3 API calls)
Turns: 4 / 50 | Tools: 6
```

---

## Sub-Phase 6.5: Sub-Agent Powered `/commit` and `/review`

### Problem

The initial `/commit` and `/review` implementations used `orchestrator.handle_user_input_streaming()` to send diffs to the main LLM. This had two problems:

1. **History pollution**: Raw diffs and LLM responses were added to the main conversation history, wasting context window tokens on temporary content.
2. **Limited analysis**: The main LLM could only see the diff text — it couldn't explore related files for context.

### Solution

Specialized sub-agents that follow the existing ExplorerAgent/PlannerAgent pattern:

### New File: `src/agent/commit_agent.rs` — CommitAgent

An explorer-like agent specialized for commit message generation:

- **System prompt**: Focused on analyzing code changes and generating conventional commit messages
- **Tools**: `read_file`, `list_directory`, `search_files`, `grep`, `shell` (read-only), `create_report` — same as ExplorerAgent
- **Max iterations**: 10 (lighter than Explorer's 15)
- **Timeout**: 90s (shorter than Explorer's 120s)
- **Output**: The `summary` field of `create_report` contains the commit message text

The agent receives the diff in its initial context and can optionally explore related files to understand the changes better before generating the message.

### New File: `src/agent/review_agent.rs` — ReviewAgent

A planner-like agent specialized for code review:

- **System prompt**: Focused on thorough code review (bugs, quality, suggestions)
- **Tools**: `read_file`, `list_directory`, `search_files`, `grep`, `shell`, `spawn_explorer`, `create_report` — same as PlannerAgent
- **Max iterations**: 15
- **Timeout**: 120s
- **Output**: The `detailed_report` field of `create_report` contains the structured review

The agent receives the diff in its initial context and can explore the codebase, check existing patterns, read tests, and even spawn explorer sub-agents for deep research.

### `src/agent/orchestrator.rs` — Sub-Agent Runner Methods

Two new methods run sub-agents directly from slash commands (not via tool calls):

```rust
/// Run a commit agent to generate a commit message from a diff.
/// Returns the commit message string. Does not modify conversation history.
pub async fn run_commit_agent(&self, diff: &str) -> Result<String>

/// Run a review agent to produce a structured code review.
/// Returns the review text and injects it into conversation history
/// so the main LLM has the review as context for follow-up questions.
pub async fn run_review_agent(&mut self, diff: &str) -> Result<String>
```

Key difference: `run_commit_agent` takes `&self` (no history mutation), while `run_review_agent` takes `&mut self` (injects review into history as `[CODE REVIEW — Sub-agent analysis of recent changes]`).

### `src/repl.rs` — Updated Handlers

Both `/commit` and `/review` now use `Spinner` for progress indication while the sub-agent runs. The `/review` handler prints a hint after the review: `"(Review added to context — ask follow-up questions if needed)"`.

Updated `/help` descriptions:
```
/review [HEAD~N]   — Review changes with sub-agent (adds to context)
/commit [message]  — Generate commit message via sub-agent and commit
```

---

## Test Summary

| File | New Tests | Category |
|------|-----------|----------|
| `src/git/mod.rs` | 2 | `run_git` success/failure |
| `src/git/context.rs` | 18 | Detection, porcelain parsing, summary, system prompt section, display |
| `src/git/diff.rs` | 7 | Unstaged, staged, all uncommitted, branch diff, commit range, colorize, non-repo error |
| `src/git/commit.rs` | 6 | commit_all, commit_files, nothing to commit, SHA format, message preserved, non-repo error |
| `src/tool/file_write.rs` | 3 | Protected .git/ path, protected .closed-code/ path, is_protected_path variants |
| `src/tool/file_edit.rs` | 2 | Protected .git/ path, protected .closed-code/ path |
| `src/agent/commit_agent.rs` | 4 | Agent properties, extract_report, missing fields, constants |
| `src/agent/review_agent.rs` | 4 | Agent properties, extract_report with artifact/snippets, constants |
| `src/agent/orchestrator.rs` | 9 | System prompt with/without git context, git_summary, git_default_branch, working_directory, detect in repo/non-repo, set_mode preserves git context, commit/review agent accessible |
| `src/repl.rs` | 6 | /diff, /diff staged, /diff branch, /diff bad arg, /review, /commit |
| **Total** | **61 new tests** | |

All existing tests were updated where needed (repl.rs tests converted to `#[tokio::test]` for the async `handle_slash_command`).

**Final count**: 335 tests passing, clean compilation with no warnings.

---

## Orchestrator Changes Summary

The `Orchestrator` struct after Phase 6:

```rust
pub struct Orchestrator {
    client: Arc<GeminiClient>,
    mode: Mode,
    working_directory: PathBuf,
    history: Vec<Content>,
    registry: ToolRegistry,
    system_prompt: String,
    max_output_tokens: u32,
    approval_handler: Arc<dyn ApprovalHandler>,
    current_plan: Option<String>,
    cancelled: Arc<AtomicBool>,
    // Phase 5
    personality: Personality,
    context_window_turns: usize,
    session_usage: SessionUsage,
    model_name: String,
    // Phase 6
    git_context: Option<GitContext>,
}
```

**New public methods:**

| Method | Description |
|--------|-------------|
| `detect_git_context(&mut self)` | Async. Detects git context, rebuilds system prompt. |
| `refresh_git_context(&mut self)` | Async. Re-detects after changes (e.g., commit). |
| `working_directory(&self) -> &Path` | Returns the working directory path. |
| `git_default_branch(&self) -> Option<&str>` | Returns detected default branch name. |
| `git_summary(&self) -> String` | One-line git summary for display. |
| `run_commit_agent(&self, diff) -> Result<String>` | Async. Runs CommitAgent sub-agent, returns commit message. No history mutation. |
| `run_review_agent(&mut self, diff) -> Result<String>` | Async. Runs ReviewAgent sub-agent, returns review text, injects into history. |

**Updated methods:**

| Method | Change |
|--------|--------|
| `build_system_prompt()` | New `git_context: Option<&GitContext>` parameter |
| `set_mode()` | Passes `self.git_context.as_ref()` to `build_system_prompt()` |
| `set_personality()` | Passes `self.git_context.as_ref()` to `build_system_prompt()` |

---

## Milestone

```bash
# Git-aware startup
cargo run
# closed-code
# Mode: explore | Model: gemini-2.5-pro | Tools: 6
# Working directory: /Users/me/project
# Git: main (3 uncommitted changes)

# Diff review
# > /diff
# --- a/src/main.rs
# +++ b/src/main.rs
# @@ -10,3 +10,5 @@
# ...colorized diff...

# > /diff staged
# ...only staged changes...

# > /diff branch
# ...changes since branching from main...

# Code review (sub-agent powered)
# > /review
# ⠋ Reviewing changes...
# (ReviewAgent explores codebase, analyzes diff)
# ## Summary
# The changes add git integration with diff review...
# ## Issues
# ...
# (Review added to context — ask follow-up questions if needed)

# Auto-commit (sub-agent powered)
# > /commit
# ⠋ Generating commit message...
# (CommitAgent explores files, analyzes diff)
# Proposed commit message: "Add git integration with diff review"
# Commit with this message? [y/N] y
# ✓ Committed: abc1234

# Manual commit message (no sub-agent)
# > /commit fix auth bug
# Proposed commit message: "fix auth bug"
# Commit with this message? [y/N] y
# ✓ Committed: def5678

# Status includes git info
# > /status
# Mode: explore | Model: gemini-2.5-pro | Personality: pragmatic
# Git: main (clean)
# Tokens: 1,234 prompt + 567 completion = 1,801 total (3 API calls)
# Turns: 4 / 50 | Tools: 6

# Protected paths
# (in execute mode, LLM tries to write .git/config)
# Error: Protected path: .git/config

# Non-git directory
cargo run -- --working-directory /tmp
# Git: not a git repository
```

---

## Implementation Order

1. `src/git/mod.rs` + `src/git/context.rs` — foundation
2. `src/git/diff.rs` — diff helpers
3. `src/git/commit.rs` — commit helpers
4. `src/lib.rs` — add `pub mod git;`
5. `cargo test` checkpoint — git module tests pass
6. `src/tool/file_write.rs` + `src/tool/file_edit.rs` — protected paths
7. `cargo test` checkpoint
8. `src/agent/orchestrator.rs` — git context in system prompt, new accessors
9. `src/repl.rs` — async `handle_slash_command`, `/diff`, `/review`, `/commit`, updated `/help` + `/status`
10. `cargo test` — 325 tests pass
11. `src/agent/commit_agent.rs` + `src/agent/review_agent.rs` — sub-agents
12. `src/agent/mod.rs` — register new modules
13. `src/agent/orchestrator.rs` — add `run_commit_agent()` and `run_review_agent()`
14. `src/repl.rs` — replace `handle_user_input_streaming` with sub-agent calls in `/commit` and `/review`
15. `cargo test` — all 335 tests pass

---

## Complexity: **Medium**

No new Cargo dependencies. All git operations are subprocess shell-outs via `tokio::process::Command`. The main complexity is in making `handle_slash_command` async (requiring all repl tests to be converted to `#[tokio::test]`), threading git context through the orchestrator's system prompt rebuild methods, and creating the CommitAgent/ReviewAgent sub-agents that follow the existing ExplorerAgent/PlannerAgent pattern. ~6 new files, ~8 modified files, ~61 new tests.
