# Phase 10: Polish, Animations, Multi-Agent, Feature Parity

> The "wow factor" phase. Transform `closed-code` into a fully-featured, production-ready coding assistant with fuzzy file search, parallel multi-agent execution via git worktrees, rich animations, and comprehensive testing. This phase achieves full feature parity with the Codex CLI.

---

## Table of Contents

1. [Goal & Vision](#1-goal--vision)
2. [Architecture Overview](#2-architecture-overview)
3. [Feature 1: Fuzzy File Search & Image Tagging](#3-feature-1-fuzzy-file-search--image-tagging)
4. [Feature 2: Multi-Agent Parallel Execution](#4-feature-2-multi-agent-parallel-execution)
5. [Feature 3: Desktop Notifications](#5-feature-3-desktop-notifications)
6. [Feature 4: Rich TUI Animations](#6-feature-4-rich-tui-animations)
7. [Feature 5: Headless Execution Mode](#7-feature-5-headless-execution-mode)
8. [Feature 6: Error Recovery & Edge Cases](#8-feature-6-error-recovery--edge-cases)
9. [Feature 7: Configuration Completions](#9-feature-7-configuration-completions)
10. [Feature 8: Testing Infrastructure](#10-feature-8-testing-infrastructure)
11. [Dependencies & File Changes](#11-dependencies--file-changes)
12. [Phased Rollout Strategy](#12-phased-rollout-strategy)
13. [Verification & Codex CLI Parity](#13-verification--codex-cli-parity)

---

## 1. Goal & Vision

Phase 10 is the culmination of the `closed-code` project. It takes the solid foundation built in Phases 1-9 and adds the final layer of polish, advanced capabilities, and robustness required for a world-class developer tool.

The primary goals are:
- **Productivity**: Introduce `@` fuzzy search and `@image` tagging to speed up context gathering.
- **Scale**: Enable parallel multi-agent execution using isolated git worktrees.
- **Polish**: Add smooth TUI animations, desktop notifications, and robust error recovery.
- **Reliability**: Establish a comprehensive integration testing suite with mock clients and snapshot testing.

By completing Phase 10, the project reaches full feature parity with the Codex CLI, delivering a robust, polished multi-agent terminal application.

---

## 2. Architecture Overview

Phase 10 touches almost every part of the system, but introduces two major new architectural components:

1. **Parallel Agent Coordinator**: Manages the lifecycle of concurrent agents, their isolated git worktrees, and IPC/state synchronization with the main orchestrator. State synchronization must be non-blocking using `tokio` concurrency primitives.
2. **Testing Harness**: A deterministic environment for integration testing, utilizing `MockGeminiClient` and `MockApprovalHandler` to record and replay interactions.

Existing systems (TUI, Config, Shell, Tools) will be incrementally enhanced to support the new features, strictly adhering to Rust best practices: returning robust custom errors (`crate::error::Result`) and isolating blocking I/O (like fuzzy search indexing) from the main UI thread.

---

## 3. Feature 1: Fuzzy File Search & Image Tagging

**Goal**: Seamlessly include files and images in prompts using the `@` syntax.

### Architecture & Implementation
- **Dependency**: Add `nucleo = "0.5"` for high-performance fuzzy matching (same engine as Helix).
- **File Indexer**: A background Tokio task that recursively scans the workspace (respecting `.gitignore` and the new `.closed-code-ignore`), caching the results so the main UI thread is not blocked.
- **TUI Overlay (`src/tui/file_picker.rs`)**: 
  - Triggered when the user types `@` in the input pane.
  - Displays a floating list of top 10 matches, updated reactively as the user types.
  - Arrow keys navigate, `Tab`/`Enter` inserts the path.
- **Image Tagging (`@image` or `@img`)**:
  - Filters the fuzzy picker to image extensions (`.png`, `.jpg`, `.webp`, etc.).
  - Reuses the `read_image_file()` and `ImageDescriptionAgent` pipeline from Phase 8.
  - Multiple tags are resolved independently before sending the prompt to the LLM.

---

## 4. Feature 2: Multi-Agent Parallel Execution

**Goal**: Spawn independent agents to tackle tasks concurrently without stepping on each other's toes.

### Architecture & Implementation
- **Git Worktrees (`src/agent/parallel.rs`)**:
  - When `/agent spawn <task>` is called, the coordinator creates a new git worktree: `git worktree add .closed-code/worktrees/<agent-id> -b agent/<agent-id>`.
  - The spawned agent operates entirely within this isolated directory. All tool calls execute within this specific worktree.
- **State Management**:
  - Agents run as separate Tokio tasks.
  - They maintain their own conversation history, tool registries, and execution loops.
- **Commands**:
  - `/agent status`: Lists running agents, their tool call counts, and elapsed time.
  - `/agent join <id>`: Merges the agent's branch back into the main branch and cleans up the worktree.
  - `/agent kill <id>`: Terminates the Tokio task and forcefully removes the worktree.
- **Cleanup**: On startup, the system scans `.closed-code/worktrees` for orphaned directories from crashed sessions and safely garbage-collects them.

---

## 5. Feature 3: Desktop Notifications

**Goal**: Alert the user when long-running tasks complete or require attention.

### Architecture & Implementation
- **Triggers**:
  - Agent completes a task taking > 30 seconds.
  - Approval is required (e.g., file write) AND the terminal is not currently focused.
  - Critical session error (e.g., rate limit exhausted).
- **Platform Dispatch (`src/ui/notify.rs`)**:
  - macOS: `osascript -e 'display notification ...'`
  - Linux: `notify-send ...`
- **Focus Detection**: Listen to `crossterm::event::FocusGained` and `FocusLost` to track terminal visibility.
- **Config**: Respects `notifications = true/false` in `config.toml`.

---

## 6. Feature 4: Rich TUI Animations

**Goal**: Make the interface feel responsive, modern, and polished without sacrificing the < 16ms render loop target.

### Architecture & Implementation
- **Typing Indicator**: Animated dots (`Thinking...` -> `Thinking....`) updated on the TUI tick loop.
- **Message Entrance**: Simulate a fade-in by initially rendering new messages with a dimmed style, transitioning to normal color over 200ms.
- **Diff Flash**: When the approval overlay opens, changed lines briefly flash bright green/red before settling to standard diff colors.
- **Status Bar Transitions**: Smoothly interpolate background colors when switching modes (e.g., Explore to Plan).
- **Accessibility**: All animations are gated behind a `reduce_motion = false` config flag.

---

## 7. Feature 5: Headless Execution Mode

**Goal**: Allow `closed-code` to be used in CI/CD pipelines and shell scripts.

### Architecture & Implementation
- **CLI Flag**: `closed-code exec "task"` bypasses the TUI entirely.
- **Approval Policy**: Automatically forced to `FullAuto` (no interactive prompts).
- **Output**:
  - Standard stdout logging.
  - `--output json` flag emits a structured JSON response containing status, files modified, and the final agent report.
- **TTY Detection**: If `atty::isnt(Stream::Stdout)` is detected, auto-switch to headless mode.
- **Exit Codes**: `0` on success, `1` on failure.
- **Error Handling**: Bubble up context-rich errors via `Result` since there's no UI to display warnings to.

---

## 8. Feature 6: Error Recovery & Edge Cases

**Goal**: Ensure the system handles network blips, massive files, and LLM hallucinations gracefully.

### Architecture & Implementation
- **Network Resilience**: If the SSE stream breaks, display "Connection lost. Retrying..." and attempt reconnection using exponential backoff.
- **Large Files**: Cap file reads at 1MB. Truncate with a clear warning appended to the content: `[TRUNCATED: File exceeds 1MB limit]`. Skip binary files entirely.
- **Infinite Loop Prevention**: Track tool calls in the orchestrator. If the exact same tool with the exact same arguments is called 3 times sequentially, inject a system error message forcing the model to change strategy.
- **Context Management**: Warn the user when token usage hits 80% of the context window. Suggest running `/compact`.
- **Parallel Reads**: Optimize the `ExplorerAgent` by executing multiple read-only tool calls in the same turn concurrently using `tokio::join!`.

---

## 9. Feature 7: Configuration Completions

**Goal**: Provide flexible configuration for diverse project environments.

### Architecture & Implementation
- **`.closed-code-ignore`**: Similar syntax to `.gitignore`. Parsed by the `ignore` crate to exclude specific paths from the `@` file picker and `SearchFilesTool`.
- **Environment Variables**: Support `CLOSED_CODE_HOME` to override the default `~/.closed-code` directory.
- **Profiles**:
  - Add `[profiles.<name>]` sections to `config.toml`.
  - Invoke via `closed-code --profile <name>`.
  - Useful for switching between strict review modes and fast hacking modes.

---

## 10. Feature 8: Testing Infrastructure

**Goal**: Guarantee stability and prevent regressions as the codebase evolves, particularly with complex git worktree and agent interactions.

### Architecture & Implementation
- **Mocking (`tests/integration/`)**:
  - `MockGeminiClient`: Implements the same trait as the real client but returns pre-recorded JSON responses.
  - `MockApprovalHandler`: Configured per-test to auto-approve, reject, or delay.
- **Replay Harness**:
  - A utility to record real API interactions into `tests/fixtures/recorded_responses/`.
  - Tests load these fixtures to deterministically simulate complex multi-turn conversations without network calls or API limits.
- **Snapshot Tests**:
  - Use `insta` crate to snapshot the output of diff generation and markdown rendering.
- **CI Integration**: All integration tests run in headless mode.

---

## 11. Dependencies & File Changes

### Dependencies (`Cargo.toml`)
```toml
[dependencies]
nucleo = "0.5"          # High-performance fuzzy matching
ignore = "0.4"          # Parsing .closed-code-ignore files
atty = "0.2"            # TTY detection for headless mode

[dev-dependencies]
insta = "1.34"          # Snapshot testing
mockall = "0.12"        # Mock generation for traits
```

### File Structure Additions & Modifications
```text
src/
  tui/
    file_picker.rs        # NEW: Fuzzy file search overlay widget
    animations.rs         # NEW: Animation state tracking and interpolation
  agent/
    parallel.rs           # NEW: Git worktree management and agent coordination
  ui/
    notify.rs             # NEW: Cross-platform desktop notifications
  config.rs               # MODIFIED: Add profiles, reduce_motion, notifications
  cli.rs                  # MODIFIED: Add exec subcommand, --profile, --output
tests/
  integration/
    mod.rs
    tool_loop_test.rs     # NEW: Deterministic tool loop tests
    approval_test.rs      # NEW: Approval flow tests
    session_test.rs       # NEW: Session persistence round-trips
    diff_test.rs          # NEW: Snapshot tests for diffs
  fixtures/
    recorded_responses/   # NEW: JSON files for MockGeminiClient
```

---

## 12. Phased Rollout Strategy

Given the massive scope of Phase 10 (~3,500 lines, 10-15 files), it must be implemented in strictly ordered sub-phases to maintain system stability and avoid sprawling regressions.

1. **Step 1: Testing Infrastructure (The Safety Net)**
   - Implement `MockGeminiClient`, `MockApprovalHandler`, and the replay harness. Write snapshot tests for diffs and markdown.
   - *Rationale*: We need a robust test suite before adding complex concurrent multi-agent logic.

2. **Step 2: Configuration Enhancements & Headless Mode**
   - Add `.closed-code-ignore`, `--profile`, and the `exec` subcommand. Implement TTY detection.
   - *Rationale*: Easy wins that build on the config system from Phase 5. Headless mode is required to run the new integration tests in CI.

3. **Step 3: Error Recovery & Edge Cases**
   - Implement the 1MB file cap, binary file detection, circular tool-call detection, and parallel read execution.
   - *Rationale*: Hardens the core orchestrator loop before introducing multiple concurrent agents.

4. **Step 4: Desktop Notifications**
   - Create `src/ui/notify.rs` with OS-specific dispatch. Wire up focus detection in the TUI event loop.
   - *Rationale*: Standalone feature, easy to verify independently without affecting core logic.

5. **Step 5: Fuzzy File Search & Image Tagging**
   - Integrate `nucleo`. Build the `FileIndexer` background task. Create the TUI overlay. Wire up `@image` to the existing vision pipeline.
   - *Rationale*: Significantly enhances user experience. Requires careful integration with the TUI event loop to remain performant.

6. **Step 6: Rich TUI Animations**
   - Add fading, flashing, and smooth scrolling to the TUI widgets. Respect the `reduce_motion` flag. Ensure the render loop stays under 16ms.
   - *Rationale*: Pure UI polish. Done late in the phase so it doesn't interfere with functional changes.

7. **Step 7: Multi-Agent Parallel Execution (The Final Boss)**
   - Implement git worktree management. Build the `ParallelCoordinator`. Add `/agent spawn|status|join|kill` commands. Ensure TUI can display status of background agents.
   - *Rationale*: The most complex feature. Relies on the hardened orchestrator (Step 3), notifications (Step 4), and testing infrastructure (Step 1) to be successful.

---

## 13. Verification & Codex CLI Parity

Upon completing Phase 10, run the final verification checklist to ensure production readiness:

- [ ] **Fuzzy Search**: Type `@` in the TUI; ensure the overlay appears instantly and filters correctly.
- [ ] **Multi-Agent**: Spawn two agents simultaneously (`/agent spawn`). Verify they create separate git worktrees, execute tools without blocking the main TUI, and can be joined successfully (`/agent join <id>`).
- [ ] **Headless**: Run `closed-code exec "echo test" --output json` and verify valid JSON output without TUI artifacts.
- [ ] **Resilience**: Force a circular tool-call in a test fixture and verify the orchestrator breaks the loop automatically.
- [ ] **Tests**: Run `cargo test` and ensure all integration tests and snapshot tests pass.