# Phase 9e: Polish, Edge Cases & Comprehensive Testing

> Harden the TUI with error display, mouse support, performance tuning, file-path completion, and a thorough QA pass across all 23+ slash commands.

**Prerequisite:** Phase 9d (overlays: approval, diff viewer, session picker, mode picker).

---

## Table of Contents

1. [Goal & Checkpoint](#1-goal--checkpoint)
2. [Dependency Changes](#2-dependency-changes)
3. [File Overview](#3-file-overview)
4. [Error & Rate-Limit Display](#4-error--rate-limit-display)
5. [tui/file_completion.rs — Input Tab Completion](#5-tuifile_completionrs--input-tab-completion)
6. [Mouse Support](#6-mouse-support)
7. [Status Bar Live Updates](#7-status-bar-live-updates)
8. [Slash Command Audit](#8-slash-command-audit)
9. [Performance](#9-performance)
10. [Modifications to Phase 9a–9d Files](#10-modifications-to-phase-9a9d-files)
11. [Implementation Order](#11-implementation-order)
12. [Tests](#12-tests)
13. [Comprehensive QA Checklist](#13-comprehensive-qa-checklist)

---

## 1. Goal & Checkpoint

**Phase 9e delivers:**

- **Error display** — API errors, rate-limit countdowns, and tool errors render as styled system messages in the chat area with appropriate severity colors
- **Rate-limit countdown** — When hitting API rate limits, a countdown timer shows seconds remaining until retry
- **Sub-agent progress indicators** — Review and commit sub-agents show box-drawing UI with running/completed states in the chat
- **Scroll position indicator** — "↑ N more" badge when scrolled up, "↓ N more" when scrolled away from bottom
- **Context pruning notification** — System message when conversation history is automatically pruned
- **Status bar live updates** — Turn count, git info, and mode badge refresh after every orchestrator interaction
- **Input tab completion** — File/directory path completion triggered by Tab in the input pane
- **Mouse support** — Scroll wheel for chat area, click on input to focus
- **Performance profiling** — Ensure render time stays under 16ms for conversations up to 500 messages
- **Slash command full audit** — Every one of the 23+ commands verified working through TUI
- **Comprehensive manual QA checklist** — Every feature from Phase 9a through 9e

**What is NOT in Phase 9e (post-launch improvements):**

- No image/attachment support in chat display
- No markdown rendering in assistant messages (plain text only)
- No syntax highlighting in diff viewer (colorized by +/- only)
- No split-pane layout
- No custom keybinding configuration

---

## 2. Dependency Changes

No new Cargo.toml dependencies. Phase 9e uses only existing crates (ratatui, crossterm, tokio, similar, chrono).

---

## 3. File Overview

### New Files (1)

| File | Purpose | Est. Lines |
|------|---------|-----------|
| `src/tui/file_completion.rs` | Tab-triggered file/directory path completion for the input pane | ~120 |

### Modified Files (8)

| File | Change |
|------|--------|
| `src/tui/app.rs` | Add completion state, mouse event handling, rate-limit countdown, status refresh logic |
| `src/tui/events.rs` | Add `Mouse(MouseEvent)` variant, `StatusUpdate(StatusSnapshot)` variant |
| `src/tui/keybindings.rs` | Add `Tab` action for completion, mouse scroll mapping |
| `src/tui/chat.rs` | Render "↓ N more" indicator, improve scroll indicator positioning |
| `src/tui/message.rs` | Add error/rate-limit message styles, context pruning message style |
| `src/tui/layout.rs` | Wire mouse capture, render completion popup |
| `src/tui/input.rs` | Expose cursor position for completion, add `word_before_cursor()` |
| `src/tui/mod.rs` | Add `pub mod file_completion;` |

---

## 4. Error & Rate-Limit Display

### Error Message Rendering

Errors from the orchestrator arrive as `AppEvent::Error(String)`. Phase 9c already adds them as `ChatMessage::System`. Phase 9e adds visual distinction for different error severities.

#### Error Categories

```rust
// In message.rs, extend render_system or add a new enum:

/// System message severity for styling.
#[derive(Debug, Clone, Copy)]
pub enum SystemSeverity {
    Info,       // Mode change, compact, general info
    Success,    // Commit success, export success
    Warning,    // Rate limit approaching, context approaching limit
    Error,      // API errors, tool errors, network failures
}

/// Render a system message with severity styling.
fn render_system_styled(
    content: &str,
    severity: SystemSeverity,
    width: u16,
) -> Vec<Line<'static>> {
    let (prefix_icon, color) = match severity {
        SystemSeverity::Info    => ("──", TuiTheme::FG_DIM),
        SystemSeverity::Success => ("✓ ", TuiTheme::SUCCESS),
        SystemSeverity::Warning => ("⚠ ", TuiTheme::WARNING),
        SystemSeverity::Error   => ("✗ ", TuiTheme::ERROR),
    };

    let w = width as usize;
    let prefix = format!(" {} {} ", prefix_icon, content);
    let rest_len = w.saturating_sub(prefix.len());
    let rest = "─".repeat(rest_len);
    vec![Line::from(vec![
        Span::styled(prefix, Style::new().fg(color)),
        Span::styled(rest, Style::new().fg(TuiTheme::FG_DIM)),
    ])]
}
```

#### ChatMessage Extension

```rust
// Extend ChatMessage to carry severity:
#[derive(Debug, Clone)]
pub enum ChatMessage {
    User { content: String },
    Assistant {
        content: String,
        tool_calls: Vec<ToolCallDisplay>,
        is_streaming: bool,
    },
    System {
        content: String,
        severity: SystemSeverity,
    },
}
```

> **Migration note:** All existing `ChatMessage::System { content }` constructors become `ChatMessage::System { content, severity: SystemSeverity::Info }`. Error events use `SystemSeverity::Error`.

### Rate-Limit Countdown

When the API returns a rate-limit error (HTTP 429), the orchestrator event includes retry timing.

```rust
// New AppEvent variant:
AppEvent::RateLimited { retry_after_secs: u64 },

// In app.rs event handler:
AppEvent::RateLimited { retry_after_secs } => {
    app.rate_limit_until = Some(Instant::now() + Duration::from_secs(retry_after_secs));
    app.messages.push(ChatMessage::System {
        content: format!("Rate limited. Retrying in {}s...", retry_after_secs),
        severity: SystemSeverity::Warning,
    });
}

// In tick handler, update countdown:
AppEvent::Tick => {
    app.tick_count = app.tick_count.wrapping_add(1);

    // Update rate limit countdown
    if let Some(until) = app.rate_limit_until {
        if Instant::now() >= until {
            app.rate_limit_until = None;
        } else {
            let remaining = until.duration_since(Instant::now()).as_secs();
            // Update the last system message if it's the rate limit message
            if let Some(ChatMessage::System { content, severity: SystemSeverity::Warning }) =
                app.messages.last_mut()
            {
                if content.starts_with("Rate limited.") {
                    *content = format!("Rate limited. Retrying in {}s...", remaining);
                }
            }
        }
    }
}
```

### Context Pruning Notification

When the orchestrator prunes conversation history to stay within the context window, it sends a system event:

```rust
// New AppEvent variant:
AppEvent::ContextPruned { turns_removed: usize, turns_remaining: usize },

// Handler:
AppEvent::ContextPruned { turns_removed, turns_remaining } => {
    app.messages.push(ChatMessage::System {
        content: format!(
            "Context pruned: removed {} oldest turns ({} remaining)",
            turns_removed, turns_remaining
        ),
        severity: SystemSeverity::Warning,
    });
    // Refresh status to update turn counter
    // app.status.turn_count = turns_remaining;
}
```

---

## 5. `tui/file_completion.rs` — Input Tab Completion

Provides file path completion triggered by Tab in the input pane. Uses the working directory to resolve relative paths.

### Data Model

```rust
use std::path::{Path, PathBuf};

/// State for file path completion.
pub struct FileCompletion {
    /// Matches for the current prefix.
    pub matches: Vec<String>,
    /// Index of the currently shown match.
    pub match_index: usize,
    /// The original text before completion started.
    pub original_prefix: String,
}

impl FileCompletion {
    /// Compute completions for a given prefix relative to working_directory.
    pub fn compute(prefix: &str, working_directory: &Path) -> Option<Self> {
        if prefix.is_empty() {
            return None;
        }

        let (dir, file_prefix) = split_path_prefix(prefix, working_directory);
        let matches = list_matching_entries(&dir, &file_prefix);

        if matches.is_empty() {
            return None;
        }

        Some(Self {
            matches,
            match_index: 0,
            original_prefix: prefix.to_string(),
        })
    }

    /// Get the current completion suggestion.
    pub fn current(&self) -> &str {
        &self.matches[self.match_index]
    }

    /// Cycle to next match.
    pub fn next(&mut self) {
        self.match_index = (self.match_index + 1) % self.matches.len();
    }

    /// Cycle to previous match.
    pub fn prev(&mut self) {
        if self.match_index == 0 {
            self.match_index = self.matches.len() - 1;
        } else {
            self.match_index -= 1;
        }
    }

    /// Number of matches available.
    pub fn count(&self) -> usize {
        self.matches.len()
    }
}
```

### Path Resolution

```rust
/// Split a user-typed prefix into (directory to scan, filename prefix).
fn split_path_prefix(prefix: &str, working_directory: &Path) -> (PathBuf, String) {
    let path = Path::new(prefix);

    if prefix.ends_with('/') || prefix.ends_with(std::path::MAIN_SEPARATOR) {
        // User typed "src/" — list contents of "src/"
        let dir = if path.is_absolute() {
            path.to_path_buf()
        } else {
            working_directory.join(path)
        };
        (dir, String::new())
    } else {
        // User typed "src/ma" — list "src/" entries starting with "ma"
        let parent = path.parent().unwrap_or(Path::new(""));
        let file_prefix = path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();

        let dir = if parent.as_os_str().is_empty() {
            working_directory.to_path_buf()
        } else if parent.is_absolute() {
            parent.to_path_buf()
        } else {
            working_directory.join(parent)
        };

        (dir, file_prefix)
    }
}

/// List directory entries matching a prefix.
fn list_matching_entries(dir: &Path, prefix: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let prefix_lower = prefix.to_lowercase();
    let mut matches: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            // Skip hidden files unless prefix starts with '.'
            if name.starts_with('.') && !prefix.starts_with('.') {
                return None;
            }
            if name.to_lowercase().starts_with(&prefix_lower) {
                let suffix = if entry.path().is_dir() {
                    format!("{}/", name)
                } else {
                    name
                };
                Some(suffix)
            } else {
                None
            }
        })
        .collect();

    matches.sort();
    matches
}
```

### Integration with InputPane

```rust
// In input.rs, add helper method:
impl<'a> InputPane<'a> {
    /// Extract the word (potential file path) before the cursor position.
    /// Returns the word and its start column in the current line.
    pub fn word_before_cursor(&self) -> Option<(String, usize)> {
        let lines = self.textarea.lines();
        let (row, col) = self.textarea.cursor();
        let line = lines.get(row)?;
        let before_cursor = &line[..col.min(line.len())];

        // Find the start of the current "word" (split on whitespace)
        let word_start = before_cursor.rfind(char::is_whitespace)
            .map(|i| i + 1)
            .unwrap_or(0);

        let word = &before_cursor[word_start..];
        if word.is_empty() {
            None
        } else {
            Some((word.to_string(), word_start))
        }
    }

    /// Replace the word before cursor with the completion text.
    pub fn apply_completion(&mut self, word_start: usize, completion: &str) {
        let lines = self.textarea.lines();
        let (row, col) = self.textarea.cursor();
        if let Some(line) = lines.get(row) {
            let new_line = format!(
                "{}{}{}",
                &line[..word_start],
                completion,
                &line[col.min(line.len())..]
            );
            // Reconstruct textarea with modified line
            let mut all_lines: Vec<String> = lines.iter().map(String::from).collect();
            all_lines[row] = new_line;
            let text = all_lines.join("\n");
            self.set_text(&text);
            // Position cursor after the completion
            // (set_text moves to end, need to position correctly)
        }
    }
}
```

### Completion Popup Rendering

When completions are active, show a small popup above the input area:

```
                    ┌──────────────────────────┐
                    │ src/tui/                  │
                    │ src/tui/app.rs            │
                    │ src/tui/chat.rs       ▸   │
                    │ src/tui/commands.rs       │
                    └──── 4 of 12 ─────────────┘
```

```rust
// In layout.rs, render completion popup conditionally:
if let Some(ref completion) = app.file_completion {
    render_completion_popup(frame, completion, input_area);
}

fn render_completion_popup(
    frame: &mut Frame,
    completion: &FileCompletion,
    input_area: Rect,
) {
    let max_visible = 5.min(completion.count());
    let width = 30.min(input_area.width);
    let height = max_visible as u16 + 2; // border

    // Position above the input area
    let x = input_area.x + 2;
    let y = input_area.y.saturating_sub(height);
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(TuiTheme::BORDER))
        .title_bottom(
            Line::from(format!(
                " {} of {} ",
                completion.match_index + 1,
                completion.count()
            ))
            .right_aligned()
            .style(Style::new().fg(TuiTheme::FG_MUTED)),
        );

    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    // Render visible matches
    let start = completion.match_index.saturating_sub(max_visible / 2);
    let visible: Vec<&String> = completion
        .matches
        .iter()
        .skip(start)
        .take(max_visible)
        .collect();

    for (i, entry) in visible.iter().enumerate() {
        let global_idx = start + i;
        let is_selected = global_idx == completion.match_index;
        let row_y = inner.y + i as u16;
        if row_y >= inner.bottom() {
            break;
        }
        let row = Rect::new(inner.x, row_y, inner.width, 1);

        let style = if is_selected {
            Style::new()
                .fg(TuiTheme::PICKER_HIGHLIGHT_FG)
                .bg(TuiTheme::PICKER_HIGHLIGHT_BG)
        } else {
            Style::new().fg(TuiTheme::FG)
        };

        let indicator = if is_selected { " ▸ " } else { "   " };
        let line = Line::from(vec![
            Span::styled(indicator, style),
            Span::styled(entry.as_str(), style),
        ]);
        frame.render_widget(Paragraph::new(line), row);
    }
}
```

### Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "").unwrap();
        fs::write(dir.path().join("src/lib.rs"), "").unwrap();
        fs::create_dir(dir.path().join("src/tui")).unwrap();
        fs::write(dir.path().join("src/tui/app.rs"), "").unwrap();
        fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        fs::write(dir.path().join("README.md"), "").unwrap();
        dir
    }

    #[test]
    fn complete_directory_prefix() {
        let dir = setup_test_dir();
        let completion = FileCompletion::compute("src/", dir.path()).unwrap();
        assert!(completion.count() >= 2); // main.rs, lib.rs, tui/
    }

    #[test]
    fn complete_file_prefix() {
        let dir = setup_test_dir();
        let completion = FileCompletion::compute("src/ma", dir.path()).unwrap();
        assert_eq!(completion.count(), 1);
        assert_eq!(completion.current(), "main.rs");
    }

    #[test]
    fn complete_no_match() {
        let dir = setup_test_dir();
        let result = FileCompletion::compute("nonexistent/", dir.path());
        assert!(result.is_none());
    }

    #[test]
    fn cycle_wraps() {
        let dir = setup_test_dir();
        let mut completion = FileCompletion::compute("src/", dir.path()).unwrap();
        let count = completion.count();
        for _ in 0..count {
            completion.next();
        }
        assert_eq!(completion.match_index, 0); // Wrapped back
    }

    #[test]
    fn hidden_files_excluded_by_default() {
        let dir = setup_test_dir();
        fs::write(dir.path().join(".hidden"), "").unwrap();
        let completion = FileCompletion::compute(".", dir.path());
        // Should include .hidden since prefix starts with '.'
        assert!(completion.is_some());
    }

    #[test]
    fn split_path_prefix_relative() {
        let wd = PathBuf::from("/project");
        let (dir, prefix) = split_path_prefix("src/ma", &wd);
        assert_eq!(dir, PathBuf::from("/project/src"));
        assert_eq!(prefix, "ma");
    }

    #[test]
    fn split_path_prefix_trailing_slash() {
        let wd = PathBuf::from("/project");
        let (dir, prefix) = split_path_prefix("src/", &wd);
        assert_eq!(dir, PathBuf::from("/project/src"));
        assert_eq!(prefix, "");
    }
}
```

---

## 6. Mouse Support

Enable scroll wheel for the chat area. Requires enabling mouse capture in crossterm.

### Event System Changes

```rust
// In events.rs, enable mouse capture in the event loop:
use crossterm::event::{EnableMouseCapture, DisableMouseCapture, MouseEvent, MouseEventKind};

pub fn spawn_event_loop() -> mpsc::UnboundedReceiver<AppEvent> {
    let (tx, rx) = mpsc::unbounded_channel();

    // Enable mouse capture
    crossterm::execute!(std::io::stdout(), EnableMouseCapture).ok();

    tokio::spawn(async move {
        let tick_rate = Duration::from_millis(TICK_RATE_MS);
        let mut tick_interval = tokio::time::interval(tick_rate);

        loop {
            tokio::select! {
                _ = tick_interval.tick() => {
                    if tx.send(AppEvent::Tick).is_err() { break; }
                }
                result = poll_crossterm_event() => {
                    match result {
                        Some(crossterm::event::Event::Key(key)) => {
                            if tx.send(AppEvent::Key(key)).is_err() { break; }
                        }
                        Some(crossterm::event::Event::Mouse(mouse)) => {
                            if tx.send(AppEvent::Mouse(mouse)).is_err() { break; }
                        }
                        Some(crossterm::event::Event::Resize(w, h)) => {
                            if tx.send(AppEvent::Resize(w, h)).is_err() { break; }
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    rx
}

// AppEvent addition:
pub enum AppEvent {
    // ... existing variants ...
    Mouse(MouseEvent),
}
```

### Mouse Event Handling

```rust
// In the main event loop:
AppEvent::Mouse(mouse) => {
    match mouse.kind {
        MouseEventKind::ScrollUp => {
            app.chat_viewport.scroll_up(3);
        }
        MouseEventKind::ScrollDown => {
            app.chat_viewport.scroll_down(3);
        }
        _ => {} // Ignore other mouse events for now
    }
}
```

### Terminal Cleanup

```rust
// In restore_terminal(), disable mouse capture:
fn restore_terminal() {
    crossterm::execute!(std::io::stdout(), DisableMouseCapture).ok();
    ratatui::restore();
}
```

> **Note:** Mouse capture must be disabled before restoring the terminal, otherwise the shell may receive ghost mouse events after exit.

---

## 7. Status Bar Live Updates

The status bar should reflect the current orchestrator state after every interaction.

### Update Points

The `StatusSnapshot` must be refreshed at these events:

| Event | What changes |
|-------|-------------|
| `OrchestratorDone` | Turn count, git info (if file writes happened) |
| `ModeChanged` | Mode badge |
| `ToolComplete { name: "write_file" \| "edit_file" }` | Git change count |
| After `/commit` | Git branch clean/dirty |
| After `/compact` | Turn count |
| After `/new` | Turn count, session ID |

### Implementation

```rust
// New AppEvent variant for status refresh:
AppEvent::StatusUpdate(StatusSnapshot),

// In the orchestrator task, after each interaction:
let _ = event_tx.send(AppEvent::StatusUpdate(
    StatusSnapshot::from_orchestrator(&orchestrator),
));

// In the main event loop:
AppEvent::StatusUpdate(snapshot) => {
    app.status = snapshot;
}
```

For slash commands that modify orchestrator state (dispatched on the main thread or via the orchestrator task), refresh the snapshot immediately after the command completes.

### Git Info Refresh

After file write tools complete, the git change count may have changed. Rather than refreshing on every tool completion (which could be expensive), batch the refresh:

```rust
// In app.rs:
pub struct App<'a> {
    // ...
    /// Flag indicating git info needs refresh on next idle transition.
    git_refresh_pending: bool,
}

// When a file write tool completes:
AppEvent::ToolComplete { ref name, .. } if name == "write_file" || name == "edit_file" => {
    app.git_refresh_pending = true;
    // ... existing tool complete handling ...
}

// When orchestrator goes idle:
AppEvent::OrchestratorDone => {
    app.state = AppState::Idle;
    if app.git_refresh_pending {
        app.git_refresh_pending = false;
        // Trigger async git refresh
        let tx = app.event_tx.clone();
        let wd = app.working_directory.clone();
        tokio::spawn(async move {
            // Refresh git status
            // Send StatusUpdate event
        });
    }
}
```

---

## 8. Slash Command Audit

Every slash command must work correctly through the TUI. This section documents the expected behavior and any special handling needed for each.

### Command Behavior Matrix

| Command | TUI Behavior | Special Notes |
|---------|-------------|---------------|
| `/help` | System message listing all commands | Format as multi-line text |
| `/quit` | AppState::Exiting → clean shutdown | Same as Ctrl+D |
| `/clear` | Clear `app.messages`, orchestrator history | System message "Cleared." |
| `/mode` | Show current mode (no args) or switch mode | System message + status bar update |
| `/explore` | Switch to Explore mode | Swaps handler, rebuilds tools |
| `/plan` | Switch to Plan mode | Swaps handler, rebuilds tools |
| `/guided` | Switch to Guided mode | Installs TuiApprovalHandler |
| `/execute` | Switch to Execute mode | Installs DiffOnlyApprovalHandler |
| `/auto` | Switch to Auto mode | Installs DiffOnlyApprovalHandler |
| `/accept` | Open mode picker overlay (9d) | Only in Plan mode with plan |
| `/diff` | Show diff as system message | Large diffs may need scroll |
| `/review` | Spinner → review result in assistant message | Sub-agent runs asynchronously |
| `/commit` | Generate message → commit → system message | Show SHA prefix |
| `/new` | Clear history, new session | System message with session ID |
| `/fork` | Fork session | System message with new ID |
| `/compact` | Compact history | System message with summary |
| `/history` | Show recent turns as system message | Default: last 10 |
| `/export` | Export transcript | System message with path |
| `/resume` | Open session picker overlay (9d) | Load sessions asynchronously |
| `/model` | Show or switch model | System message |
| `/personality` | Show or switch personality | System message |
| `/status` | Show session info | Multi-line system message |
| `/sandbox` | Show sandbox mode + protected paths | System message |

### Commands Requiring Async Execution

These commands involve I/O and should not block the TUI render loop:

```
/diff       — runs git commands
/review     — runs sub-agent (up to 120s)
/commit     — runs sub-agent + git commands
/compact    — runs LLM call
/export     — writes file to disk
/resume     — loads session files
```

For long-running commands (`/review`, `/commit`, `/compact`), show a thinking spinner while the operation runs:

```rust
// In commands.rs, for async commands:
pub async fn handle_review(args: &str, orchestrator: &mut Orchestrator) -> CommandResult {
    // ... (as defined in Phase 9c) ...
    // The TUI shows AppState::Thinking while this runs
}
```

### `/help` Output Format

```rust
fn format_help_text() -> String {
    let commands = super::command_picker::all_commands();
    let mut text = String::from("Available commands:\n\n");

    let mut current_category = None;
    for cmd in &commands {
        let cat = format!("{:?}", cmd.category);
        if current_category.as_ref() != Some(&cat) {
            if current_category.is_some() {
                text.push('\n');
            }
            text.push_str(&format!("  {}:\n", cat));
            current_category = Some(cat);
        }
        text.push_str(&format!(
            "    {:<20} {}\n",
            cmd.display_name(),
            cmd.description
        ));
    }

    text.push_str("\nType / to open the command picker.");
    text
}
```

---

## 9. Performance

### Render Budget

Target: <16ms per frame for a 60fps feel. At our 100ms tick rate, this is generous, but large conversations can slow rendering.

### Potential Bottlenecks

| Bottleneck | Mitigation |
|------------|-----------|
| Rendering 500+ messages | Pre-compute message line counts; only render visible range |
| Re-computing diff on every frame | Cache diff lines in overlay/view state |
| File completion scanning large directories | Limit to first 100 entries; debounce Tab key |
| Status bar git refresh | Async refresh on background task |

### Message Rendering Optimization

Instead of rendering all messages and using `Paragraph::scroll()`, render only the visible window:

```rust
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    // ... border setup ...

    // Pre-compute line counts per message (cache this between renders)
    if app.message_line_cache.len() != app.messages.len() {
        app.message_line_cache = app
            .messages
            .iter()
            .map(|msg| message::line_count(msg, inner.width))
            .collect();
    }

    let total_lines: usize = app.message_line_cache.iter().sum();
    app.chat_viewport.total_height = total_lines;
    app.chat_viewport.visible_height = inner.height as usize;

    let offset = app.chat_viewport.effective_offset();

    // Find the first message that's visible at this offset
    let mut accumulated = 0;
    let mut start_msg = 0;
    let mut line_offset_in_first_msg = 0;

    for (i, &count) in app.message_line_cache.iter().enumerate() {
        if accumulated + count > offset {
            start_msg = i;
            line_offset_in_first_msg = offset - accumulated;
            break;
        }
        accumulated += count;
    }

    // Render only messages in the visible window
    let mut lines: Vec<Line> = Vec::new();
    for msg in &app.messages[start_msg..] {
        let msg_lines = message::render_message(msg, inner.width, app.tick_count);
        lines.extend(msg_lines);
        lines.push(Line::from("")); // gap

        if lines.len() > inner.height as usize + line_offset_in_first_msg {
            break; // Enough lines rendered
        }
    }

    // Skip the partial first message
    let visible_lines: Vec<Line> = lines
        .into_iter()
        .skip(line_offset_in_first_msg)
        .take(inner.height as usize)
        .collect();

    frame.render_widget(Paragraph::new(visible_lines), inner);
}
```

### Message Line Count Cache

```rust
// In app.rs:
pub struct App<'a> {
    // ...
    /// Cached line counts per message (invalidated when messages change).
    pub message_line_cache: Vec<usize>,
}

// In message.rs, add a fast line count function:
pub fn line_count(msg: &ChatMessage, width: u16) -> usize {
    match msg {
        ChatMessage::User { content, .. } => content.lines().count() + 3, // top + content + bottom
        ChatMessage::Assistant { content, tool_calls, .. } => {
            let tc_lines = tool_calls.len();
            let content_lines = content.lines().count().max(1);
            tc_lines + content_lines + 3 // top + tools + gap? + content + bottom
        }
        ChatMessage::System { .. } => 1,
    }
}
```

---

## 10. Modifications to Phase 9a–9d Files

### `tui/events.rs`

```rust
pub enum AppEvent {
    // Phase 9a
    Key(KeyEvent),
    Resize(u16, u16),
    Tick,

    // Phase 9c
    TextDelta(String),
    StreamDone,
    ToolStart { name: String, args_display: String },
    ToolComplete { name: String, duration: Duration },
    ToolError { name: String, error: String },
    AgentStart { agent_type: String, task: String },
    AgentToolCall { agent_type: String },
    AgentComplete { agent_type: String, duration: Duration, tool_calls: usize },
    SystemMessage(String),
    ModeChanged(Mode),
    OrchestratorDone,
    Error(String),

    // Phase 9d
    ApprovalRequest { change: FileChange, response_tx: oneshot::Sender<ApprovalDecision> },
    SessionListReady(Vec<SessionMeta>),

    // Phase 9e
    Mouse(MouseEvent),
    StatusUpdate(StatusSnapshot),
    RateLimited { retry_after_secs: u64 },
    ContextPruned { turns_removed: usize, turns_remaining: usize },
}
```

### `tui/keybindings.rs` — Tab Completion

```rust
pub enum Action {
    // ... existing variants ...

    // Phase 9e: Completion
    TabComplete,
    TabCompletePrev,   // Shift+Tab
}

fn map_idle(key: KeyEvent) -> Action {
    match (key.code, key.modifiers) {
        // ... existing mappings ...

        // Tab completion
        (KeyCode::Tab, m) if !m.contains(KeyModifiers::SHIFT) => Action::TabComplete,
        (KeyCode::BackTab, _) => Action::TabCompletePrev,

        // ... rest of mappings ...
    }
}
```

### `tui/app.rs` — Completion State

```rust
use super::file_completion::FileCompletion;

pub struct App<'a> {
    // ... existing fields ...

    // Phase 9e
    pub file_completion: Option<FileCompletion>,
    pub message_line_cache: Vec<usize>,
    pub rate_limit_until: Option<Instant>,
    pub git_refresh_pending: bool,
}

// In handle_action:
Action::TabComplete => {
    if let Some(ref mut completion) = self.file_completion {
        // Already completing — cycle to next
        completion.next();
        let current = completion.current().to_string();
        // Apply completion to input
        // ...
    } else {
        // Start new completion
        if let Some((word, _word_start)) = self.input_pane.word_before_cursor() {
            if let Some(completion) = FileCompletion::compute(
                &word,
                &self.working_directory,
            ) {
                let current = completion.current().to_string();
                self.file_completion = Some(completion);
                // Apply first completion to input
                // ...
            }
        }
    }
}

Action::TabCompletePrev => {
    if let Some(ref mut completion) = self.file_completion {
        completion.prev();
        let current = completion.current().to_string();
        // Apply completion to input
    }
}

// Any other input action should dismiss completion:
Action::InsertChar(_) | Action::Backspace | Action::Delete => {
    self.file_completion = None;
    // ... existing handling ...
}
```

### `tui/chat.rs` — Improved Scroll Indicators

```rust
// Add "↓ N more" indicator when not at the bottom and not auto-scrolling:
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    // ... existing rendering ...

    // Scroll-up indicator
    let lines_above = app.chat_viewport.lines_above();
    if lines_above > 0 {
        let indicator = format!(" ↑ {} more ", lines_above);
        let indicator_area = Rect::new(
            inner.right().saturating_sub(indicator.len() as u16 + 1),
            inner.y,
            indicator.len() as u16,
            1,
        );
        frame.render_widget(
            Paragraph::new(indicator)
                .style(Style::new().fg(TuiTheme::FG_MUTED).bg(TuiTheme::BG)),
            indicator_area,
        );
    }

    // Scroll-down indicator (when manually scrolled and more content below)
    if !app.chat_viewport.is_auto_scroll() {
        let lines_below = app.chat_viewport.total_height
            .saturating_sub(app.chat_viewport.effective_offset() + app.chat_viewport.visible_height);
        if lines_below > 0 {
            let indicator = format!(" ↓ {} more ", lines_below);
            let indicator_area = Rect::new(
                inner.right().saturating_sub(indicator.len() as u16 + 1),
                inner.bottom().saturating_sub(1),
                indicator.len() as u16,
                1,
            );
            frame.render_widget(
                Paragraph::new(indicator)
                    .style(Style::new().fg(TuiTheme::FG_MUTED).bg(TuiTheme::BG)),
                indicator_area,
            );
        }
    }
}
```

---

## 11. Implementation Order

Build in this order to keep the project compilable at each step:

| Step | File | Why this order |
|------|------|----------------|
| 1 | `src/tui/message.rs` | Add `SystemSeverity` enum and styled system rendering |
| 2 | `src/tui/chat.rs` | Update ChatMessage to include severity, add "↓ N more" indicator |
| 3 | `src/tui/events.rs` | Add `Mouse`, `StatusUpdate`, `RateLimited`, `ContextPruned` variants |
| 4 | `src/tui/file_completion.rs` | New file: completion logic + tests |
| 5 | `src/tui/mod.rs` | Add `pub mod file_completion;` |
| 6 | `src/tui/input.rs` | Add `word_before_cursor()` and `apply_completion()` methods |
| 7 | `src/tui/keybindings.rs` | Add `TabComplete`, `TabCompletePrev` actions |
| 8 | `src/tui/app.rs` | Add completion state, mouse handling, rate-limit, git refresh, line cache |
| 9 | `src/tui/layout.rs` | Wire completion popup rendering, mouse capture setup |
| 10 | `src/tui/commands.rs` | Ensure `/help` format, audit all command handlers |
| 11 | `src/tui/chat.rs` | Performance: visible-range rendering, line count cache |
| 12 | — | `cargo test && cargo clippy` |
| 13 | — | Manual QA pass (see checklist below) |

---

## 12. Tests

### Test Summary

| File | # Tests | Coverage |
|------|---------|----------|
| `file_completion.rs` | 7 | Directory prefix, file prefix, no match, cycle wrap, hidden files, split_path relative/trailing slash |
| `message.rs` | 3 | System severity Info/Error/Warning rendering |
| `input.rs` | 2 | word_before_cursor basic, word_before_cursor empty |
| `chat.rs` | 2 | Lines below calculation, scroll indicator visibility |
| **Total** | **14** | |

### `message.rs` Severity Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_info_uses_dim_color() {
        let lines = render_system_styled("Mode changed", SystemSeverity::Info, 60);
        assert_eq!(lines.len(), 1);
        // Verify the line contains the content
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("Mode changed"));
    }

    #[test]
    fn system_error_includes_icon() {
        let lines = render_system_styled("API failed", SystemSeverity::Error, 60);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("✗"));
    }

    #[test]
    fn system_success_includes_icon() {
        let lines = render_system_styled("Committed", SystemSeverity::Success, 60);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("✓"));
    }
}
```

### `input.rs` Completion Tests

```rust
#[cfg(test)]
mod tests {
    // ... existing tests ...

    #[test]
    fn word_before_cursor_basic() {
        let mut p = pane();
        for c in "src/main".chars() {
            p.insert_char(c);
        }
        let (word, start) = p.word_before_cursor().unwrap();
        assert_eq!(word, "src/main");
        assert_eq!(start, 0);
    }

    #[test]
    fn word_before_cursor_after_space() {
        let mut p = pane();
        for c in "edit src/main".chars() {
            p.insert_char(c);
        }
        let (word, start) = p.word_before_cursor().unwrap();
        assert_eq!(word, "src/main");
        assert_eq!(start, 5);
    }
}
```

---

## 13. Comprehensive QA Checklist

This checklist covers the entire TUI (Phases 9a through 9e). Every item should be verified manually before the TUI is considered complete.

### Phase 9a — App Shell

- [ ] `cargo run -- --tui` launches the TUI
- [ ] Header shows "closed-code" on left, session ID on right
- [ ] Status bar shows mode badge, model name, turn counter, context gauge, git info
- [ ] Terminal resize reflows layout correctly
- [ ] Terminal too small (<40x10) shows "Terminal too small" message
- [ ] Ctrl+D exits cleanly, terminal restored to normal
- [ ] Ctrl+L redraws the screen
- [ ] Panic hook restores terminal before printing panic

### Phase 9b — Input & Command Picker

- [ ] Typing text appears in the input pane
- [ ] Backspace deletes character before cursor
- [ ] Delete deletes character at cursor
- [ ] Left/Right arrow moves cursor
- [ ] Home/End moves cursor to start/end of line
- [ ] Shift+Enter / Alt+Enter inserts newline (multi-line input)
- [ ] Input pane grows height for multi-line content (up to 8 lines)
- [ ] Enter submits input and clears the pane
- [ ] Ctrl+U clears input
- [ ] Esc clears input
- [ ] Ctrl+G opens $EDITOR with current input content
- [ ] Up arrow cycles through input history (most recent first)
- [ ] Down arrow cycles forward through history
- [ ] History saves current input and restores it on cycling past newest
- [ ] Consecutive duplicate submissions are deduplicated in history
- [ ] Typing `/` when input is empty opens command picker overlay
- [ ] Command picker shows all commands with descriptions
- [ ] Typing filters commands by name
- [ ] Up/Down navigates highlighted entry
- [ ] Enter selects command and inserts into input
- [ ] Esc dismisses picker and clears input
- [ ] Backspace past `/` dismisses picker
- [ ] Command picker scrolls for long lists

### Phase 9c — Chat Area & Streaming

- [ ] Typing a message and pressing Enter shows it as a user message (cyan border)
- [ ] User message shows "You" title
- [ ] After submitting, "Thinking..." spinner appears
- [ ] Spinner animates (braille characters cycle)
- [ ] LLM response streams token-by-token into assistant message (violet border)
- [ ] Assistant message shows "Assistant" title
- [ ] Streaming cursor indicator (▌) blinks
- [ ] When streaming completes, cursor disappears, state returns to Idle
- [ ] Tool calls show spinner during execution
- [ ] Tool calls show ✓ + duration on completion
- [ ] Tool errors show ✗ with red error message
- [ ] Sub-agent shows box-drawing header with agent type
- [ ] Sub-agent shows ✓ + call count + duration on completion
- [ ] Multiple tool calls in a single response render correctly
- [ ] Chat auto-scrolls to bottom during streaming
- [ ] Page Up scrolls chat up (pauses auto-scroll)
- [ ] Page Down scrolls chat down
- [ ] Scrolling to bottom re-enables auto-scroll
- [ ] "↑ N more" indicator appears when scrolled up
- [ ] Home scrolls to top of chat, End scrolls to bottom
- [ ] Ctrl+C during streaming interrupts and shows "Interrupted."
- [ ] Ctrl+C during tool execution interrupts
- [ ] `/help` shows formatted command list
- [ ] `/clear` clears chat and shows confirmation
- [ ] `/mode` shows current mode
- [ ] `/mode explore` switches mode, system message shown, status bar updated
- [ ] `/explore`, `/plan`, `/guided`, `/execute`, `/auto` each switch modes
- [ ] `/diff` shows git diff output in chat
- [ ] `/diff staged` shows staged diff
- [ ] `/review` shows spinner, then review result
- [ ] `/commit` generates message and commits
- [ ] `/commit "message"` commits with provided message
- [ ] `/new` starts new session, system message with ID
- [ ] `/fork` forks session, system message with new ID
- [ ] `/compact` compacts history, shows summary
- [ ] `/compact "focus on X"` compacts with custom prompt
- [ ] `/history` shows last 10 turns
- [ ] `/history 5` shows last 5 turns
- [ ] `/export` exports to timestamped file
- [ ] `/export myfile.md` exports to specified path
- [ ] `/model` shows current model
- [ ] `/model gemini-2` switches model
- [ ] `/personality` shows current personality
- [ ] `/personality pragmatic` switches personality
- [ ] `/status` shows full session status
- [ ] `/sandbox` shows sandbox mode and protected paths
- [ ] `!ls` executes shell command, output shown in chat
- [ ] `!invalid_command` shows error in chat
- [ ] Unknown `/command` shows "Unknown command" message
- [ ] Status bar turn count updates after each interaction
- [ ] Status bar git info refreshes after `/commit`

### Phase 9d — Overlays

- [ ] In Guided mode, file write shows approval overlay
- [ ] Approval overlay shows file path with "CREATE" or "MODIFY" badge
- [ ] Approval overlay shows colorized diff (green adds, red deletes)
- [ ] Approval overlay shows change summary (+N additions, -M deletions)
- [ ] Pressing `y` approves change
- [ ] Pressing `n` rejects change
- [ ] Pressing `Esc` rejects change
- [ ] Pressing `d` opens full-screen diff viewer
- [ ] Diff viewer shows file path and scroll position
- [ ] `j`/`k` scroll one line
- [ ] `Ctrl+d`/`Ctrl+u` scroll half-page
- [ ] `g` scrolls to top, `G` scrolls to bottom
- [ ] `q` returns to approval overlay
- [ ] After approval/rejection, orchestrator continues
- [ ] `/resume` opens session picker overlay
- [ ] Session picker shows ID, time, mode, preview
- [ ] Up/Down navigates sessions
- [ ] Enter resumes selected session
- [ ] Esc cancels
- [ ] Empty session list shows message (no overlay)
- [ ] `/accept` in Plan mode opens mode picker
- [ ] Mode picker shows Guided, Execute, Auto with descriptions
- [ ] Auto shows DANGER indicator
- [ ] Selecting Guided/Execute immediately accepts
- [ ] Selecting Auto shows confirmation warning
- [ ] `y` confirms Auto, `n` returns to selection
- [ ] `/accept` outside Plan mode shows error
- [ ] Mode switch installs correct approval handler

### Phase 9e — Polish

- [ ] API errors show as red system messages with ✗ icon
- [ ] Rate-limit errors show countdown timer
- [ ] Countdown timer decrements each second
- [ ] Context pruning shows warning message
- [ ] "↓ N more" indicator appears when scrolled up from bottom
- [ ] Tab triggers file path completion
- [ ] Completion popup shows matching files/directories
- [ ] Tab cycles through completions
- [ ] Shift+Tab cycles backwards
- [ ] Typing dismisses completion popup
- [ ] Directories shown with trailing `/`
- [ ] Hidden files excluded unless prefix starts with `.`
- [ ] Mouse scroll wheel scrolls chat area
- [ ] Status bar updates after every orchestrator interaction
- [ ] Large conversation (100+ messages) renders without lag
- [ ] All 23+ slash commands work (see Phase 9c checklist above)

### Cross-Cutting

- [ ] Ctrl+C works from every state (Idle, Thinking, Streaming, ToolExecuting, overlays)
- [ ] Ctrl+D exits from every state
- [ ] Resizing terminal during streaming doesn't crash
- [ ] Resizing terminal with overlay open reflows overlay
- [ ] Rapidly pressing Enter doesn't queue conflicting operations
- [ ] Session events are persisted correctly for all TUI interactions
- [ ] `--resume` flag works with TUI mode
- [ ] Starting with `--mode guided --tui` uses TuiApprovalHandler
- [ ] Starting with `--mode execute --tui` uses DiffOnlyApprovalHandler

---

## Estimated Scope

| Metric | Value |
|--------|-------|
| New files | 1 |
| Modified files | 8 |
| New lines (est.) | ~600 |
| New tests | ~14 |
