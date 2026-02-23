# Phase 9b: Input Pane & Command Picker

> Enable user input with a multi-line text editor, input history, external editor support, and a filterable command picker overlay triggered by `/`.

**Prerequisite:** Phase 9a (app shell, layout, header, status bar, event loop).

---

## Table of Contents

1. [Goal & Checkpoint](#1-goal--checkpoint)
2. [Dependency Changes](#2-dependency-changes)
3. [File Overview](#3-file-overview)
4. [tui/keybindings.rs — Action Enum & Key Mapping](#4-tuikeybindingsrs--action-enum--key-mapping)
5. [tui/input.rs — Input Pane](#5-tuiinputrs--input-pane)
6. [tui/command_picker.rs — Command Registry & Overlay](#6-tuicommand_pickerrs--command-registry--overlay)
7. [Modifications to Phase 9a Files](#7-modifications-to-phase-9a-files)
8. [Interaction Model](#8-interaction-model)
9. [Implementation Order](#9-implementation-order)
10. [Tests](#10-tests)
11. [Verification Checklist](#11-verification-checklist)

---

## 1. Goal & Checkpoint

**Phase 9b delivers:**

- A multi-line input pane powered by `tui-textarea`
- Placeholder text ("Type a message, / for commands") when empty
- `Enter` submits input, `Shift+Enter` / `Alt+Enter` inserts newline
- `Ctrl+U` clears input, `Escape` clears input
- `Ctrl+G` opens `$EDITOR` for composing long prompts
- Arrow `Up`/`Down` cycles through input history when input is empty
- Dynamic input height (3-8 lines) that grows with content
- A floating command picker overlay:
  - Opens when `/` is typed as the first character in empty input
  - Shows all 24 slash commands with descriptions
  - Filters as the user types (case-insensitive substring match)
  - Arrow key navigation, `Enter` to select, `Escape` to dismiss
  - Scrollable with "N of M" footer
- `AppState::CommandPicker` state added to the state machine
- Centralized key-to-action mapping via `keybindings.rs`

**What submitted input does in Phase 9b:**

Input is captured into `app.pending_input: Option<String>` but is **not yet sent to the Orchestrator** (that's Phase 9c). Slash commands are also captured but not dispatched. The focus is on the input UX itself.

---

## 2. Dependency Changes

### Cargo.toml

```toml
# Add to [dependencies]:
tui-textarea = "0.7"
```

The `tui-textarea` crate provides a full-featured text editor widget compatible with `ratatui 0.29`. It handles cursor movement, text insertion/deletion, line wrapping, and scroll internally.

> **Compatibility note:** Verify `tui-textarea 0.7` works with `ratatui 0.29`. If there is a version mismatch, use `ratatui-textarea` (a fork maintained by the ratatui team) instead.

---

## 3. File Overview

### New Files (3)

| File | Purpose | Est. Lines |
|------|---------|-----------|
| `src/tui/keybindings.rs` | `Action` enum + `map_key()` per AppState | ~130 |
| `src/tui/input.rs` | `InputPane` wrapping `TextArea`, history, editor | ~220 |
| `src/tui/command_picker.rs` | `CommandPicker`, command registry, overlay rendering | ~250 |

### Modified Files (4)

| File | Change |
|------|--------|
| `Cargo.toml` | Add `tui-textarea` dependency |
| `src/tui/mod.rs` | Add `pub mod input; pub mod keybindings; pub mod command_picker;` |
| `src/tui/app.rs` | Add `InputPane`, `CommandPicker`, `pending_input` to `App`; add `CommandPicker` variant to `AppState`; wire `handle_action()` dispatch |
| `src/tui/layout.rs` | Use dynamic input height from `InputPane`; render `InputPane` and `CommandPicker` overlay |

---

## 4. `tui/keybindings.rs` — Action Enum & Key Mapping

Centralizes all keyboard handling. Maps raw `crossterm::event::KeyEvent` to semantic `Action` values based on the current `AppState`.

### Action Enum

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use super::app::AppState;

/// Semantic actions dispatched by the event loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    // ── Global ──
    Cancel,
    Exit,
    Redraw,

    // ── Input ──
    Submit,
    InsertNewline,
    InsertChar(char),
    Backspace,
    Delete,
    ClearInput,
    OpenEditor,
    CursorLeft,
    CursorRight,
    CursorHome,
    CursorEnd,

    // ── History ──
    HistoryPrev,
    HistoryNext,

    // ── Chat Scrolling (wired in Phase 9c) ──
    ScrollUp,
    ScrollDown,
    PageUp,
    PageDown,
    ScrollToTop,
    ScrollToBottom,

    // ── Command Picker ──
    PickerUp,
    PickerDown,
    PickerSelect,
    PickerDismiss,
    PickerBackspace,
    PickerFilter(char),

    Noop,
}
```

### Mapping Function

```rust
/// Map a key event to an action based on the current state.
pub fn map_key(key: KeyEvent, state: &AppState) -> Action {
    // Global keys — always handled
    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => return Action::Cancel,
        (KeyCode::Char('d'), m) if m.contains(KeyModifiers::CONTROL) => return Action::Exit,
        (KeyCode::Char('l'), m) if m.contains(KeyModifiers::CONTROL) => return Action::Redraw,
        _ => {}
    }

    match state {
        AppState::Idle => map_idle(key),
        AppState::CommandPicker { .. } => map_picker(key),
        _ => Action::Noop,
    }
}

fn map_idle(key: KeyEvent) -> Action {
    match (key.code, key.modifiers) {
        // Submit
        (KeyCode::Enter, m) if !m.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) =>
            Action::Submit,

        // Newline
        (KeyCode::Enter, m) if m.contains(KeyModifiers::SHIFT) => Action::InsertNewline,
        (KeyCode::Enter, m) if m.contains(KeyModifiers::ALT)   => Action::InsertNewline,

        // Editor / Clear
        (KeyCode::Char('g'), m) if m.contains(KeyModifiers::CONTROL) => Action::OpenEditor,
        (KeyCode::Char('u'), m) if m.contains(KeyModifiers::CONTROL) => Action::ClearInput,

        // Cursor movement
        (KeyCode::Left, _)  => Action::CursorLeft,
        (KeyCode::Right, _) => Action::CursorRight,
        (KeyCode::Home, _)  => Action::CursorHome,
        (KeyCode::End, _)   => Action::CursorEnd,

        // History / scroll
        (KeyCode::Up, _)   => Action::HistoryPrev,
        (KeyCode::Down, _) => Action::HistoryNext,
        (KeyCode::PageUp, _)   => Action::PageUp,
        (KeyCode::PageDown, _) => Action::PageDown,

        // Edit
        (KeyCode::Backspace, _) => Action::Backspace,
        (KeyCode::Delete, _)    => Action::Delete,
        (KeyCode::Esc, _)       => Action::ClearInput,

        // Printable characters
        (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) =>
            Action::InsertChar(c),

        _ => Action::Noop,
    }
}

fn map_picker(key: KeyEvent) -> Action {
    match (key.code, key.modifiers) {
        (KeyCode::Up, _)        => Action::PickerUp,
        (KeyCode::Down, _)      => Action::PickerDown,
        (KeyCode::Enter, _)     => Action::PickerSelect,
        (KeyCode::Esc, _)       => Action::PickerDismiss,
        (KeyCode::Backspace, _) => Action::PickerBackspace,
        (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) =>
            Action::PickerFilter(c),
        _ => Action::Noop,
    }
}
```

### Platform Notes

- **`Shift+Enter`**: Some terminals cannot distinguish `Shift+Enter` from `Enter`. `Alt+Enter` is the reliable cross-platform alternative. Both are supported.
- **`Ctrl+G`**: Universally available across terminals.

---

## 5. `tui/input.rs` — Input Pane

Wraps `tui_textarea::TextArea` with application-specific behavior: placeholder, history, submit, dynamic height, and external editor integration.

### Constants

```rust
const INPUT_MIN_HEIGHT: u16 = 3;
const INPUT_MAX_HEIGHT: u16 = 8;
const HISTORY_MAX: usize = 200;
```

### Struct

```rust
use std::path::PathBuf;
use tui_textarea::TextArea;

pub struct InputPane<'a> {
    textarea: TextArea<'a>,
    history: Vec<String>,
    history_index: Option<usize>,
    saved_input: Option<String>,
    working_directory: PathBuf,
}
```

### Construction

```rust
impl<'a> InputPane<'a> {
    pub fn new(working_directory: PathBuf) -> Self {
        let mut textarea = TextArea::default();
        textarea.set_placeholder_text("Type a message, / for commands");
        textarea.set_placeholder_style(Style::new().fg(TuiTheme::FG_MUTED));
        textarea.set_cursor_style(Style::new().reversed());
        textarea.set_style(Style::new().fg(TuiTheme::FG));

        Self {
            textarea,
            history: Vec::new(),
            history_index: None,
            saved_input: None,
            working_directory,
        }
    }
}
```

### Text Operations

```rust
impl<'a> InputPane<'a> {
    pub fn insert_char(&mut self, c: char) {
        self.textarea.insert_char(c);
        self.reset_history_cycling();
    }

    pub fn insert_newline(&mut self) {
        self.textarea.insert_newline();
        self.reset_history_cycling();
    }

    pub fn delete_char_before(&mut self) {
        self.textarea.delete_char();
    }

    pub fn delete_char_at(&mut self) {
        self.textarea.delete_next_char();
    }

    pub fn move_cursor_left(&mut self) {
        self.textarea.move_cursor(tui_textarea::CursorMove::Back);
    }

    pub fn move_cursor_right(&mut self) {
        self.textarea.move_cursor(tui_textarea::CursorMove::Forward);
    }

    pub fn move_cursor_home(&mut self) {
        self.textarea.move_cursor(tui_textarea::CursorMove::Head);
    }

    pub fn move_cursor_end(&mut self) {
        self.textarea.move_cursor(tui_textarea::CursorMove::End);
    }

    pub fn text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.textarea.is_empty()
    }

    pub fn first_char(&self) -> Option<char> {
        self.textarea.lines().first()?.chars().next()
    }
}
```

### Clear & Submit

```rust
impl<'a> InputPane<'a> {
    pub fn clear(&mut self) {
        // Replace with a fresh TextArea, re-apply configuration
        self.textarea = TextArea::default();
        self.apply_config();
        self.reset_history_cycling();
    }

    /// Submit input: extract text, push to history, clear.
    /// Returns None if empty/whitespace-only.
    pub fn submit(&mut self) -> Option<String> {
        let text = self.text().trim().to_string();
        if text.is_empty() {
            return None;
        }
        // Avoid consecutive duplicates in history
        if self.history.last().map_or(true, |last| last != &text) {
            self.history.push(text.clone());
            if self.history.len() > HISTORY_MAX {
                self.history.remove(0);
            }
        }
        self.clear();
        Some(text)
    }

    fn apply_config(&mut self) {
        self.textarea.set_placeholder_text("Type a message, / for commands");
        self.textarea.set_placeholder_style(Style::new().fg(TuiTheme::FG_MUTED));
        self.textarea.set_cursor_style(Style::new().reversed());
        self.textarea.set_style(Style::new().fg(TuiTheme::FG));
    }
}
```

### Dynamic Height

```rust
impl<'a> InputPane<'a> {
    /// Calculate the desired height in terminal rows.
    pub fn desired_height(&self) -> u16 {
        let lines = self.textarea.lines().len().max(1) as u16;
        lines.clamp(INPUT_MIN_HEIGHT, INPUT_MAX_HEIGHT)
    }
}
```

| Content Lines | Displayed Height | Notes |
|---|---|---|
| 0-3 | 3 | Minimum |
| 4-8 | N | Grows with content |
| 9+ | 8 | Max; TextArea scrolls internally |

### Input History

```rust
impl<'a> InputPane<'a> {
    /// Whether we are currently cycling through history.
    pub fn is_cycling_history(&self) -> bool {
        self.history_index.is_some()
    }

    /// Cycle to the previous (older) history entry.
    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = match self.history_index {
            None => {
                self.saved_input = Some(self.text());
                self.history.len() - 1
            }
            Some(0) => return, // already at oldest
            Some(i) => i - 1,
        };
        self.history_index = Some(idx);
        self.set_text(&self.history[idx].clone());
    }

    /// Cycle to the next (newer) history entry.
    pub fn history_next(&mut self) {
        let idx = match self.history_index {
            None => return,
            Some(i) => i + 1,
        };
        if idx >= self.history.len() {
            // Past the newest — restore saved input
            self.history_index = None;
            let saved = self.saved_input.take().unwrap_or_default();
            self.set_text(&saved);
        } else {
            self.history_index = Some(idx);
            self.set_text(&self.history[idx].clone());
        }
    }

    fn reset_history_cycling(&mut self) {
        self.history_index = None;
        self.saved_input = None;
    }

    fn set_text(&mut self, text: &str) {
        let lines: Vec<String> = text.lines().map(String::from).collect();
        let lines = if lines.is_empty() { vec![String::new()] } else { lines };
        self.textarea = TextArea::new(lines);
        self.apply_config();
        // Move cursor to end of content
        self.textarea.move_cursor(tui_textarea::CursorMove::Bottom);
        self.textarea.move_cursor(tui_textarea::CursorMove::End);
    }
}
```

### External Editor (Ctrl+G)

```rust
impl<'a> InputPane<'a> {
    /// Open $EDITOR with current input. Returns Ok(true) if content was updated.
    pub fn open_editor(&mut self) -> anyhow::Result<bool> {
        let editor = std::env::var("EDITOR")
            .or_else(|_| std::env::var("VISUAL"))
            .unwrap_or_else(|_| "vi".to_string());

        let temp_path = std::env::temp_dir()
            .join(format!("closed-code-input-{}.txt", std::process::id()));
        std::fs::write(&temp_path, self.text())?;

        // Leave alternate screen for the editor
        crossterm::terminal::disable_raw_mode()?;
        crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen)?;

        let status = std::process::Command::new(&editor)
            .arg(&temp_path)
            .status();

        // Re-enter alternate screen (always, even on editor failure)
        crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
        crossterm::terminal::enable_raw_mode()?;

        let status = status?;
        if !status.success() {
            let _ = std::fs::remove_file(&temp_path);
            return Ok(false);
        }

        let content = std::fs::read_to_string(&temp_path)?;
        let _ = std::fs::remove_file(&temp_path);

        let content = content.trim_end().to_string();
        if !content.is_empty() {
            self.set_text(&content);
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
```

### Rendering

The `InputPane` renders by delegating to `TextArea::widget()`:

```rust
impl<'a> InputPane<'a> {
    pub fn widget(&'a self) -> impl ratatui::widgets::Widget + 'a {
        self.textarea.widget()
    }
}
```

In the layout render function:

```rust
frame.render_widget(app.input_pane.widget(), input_area);
```

---

## 6. `tui/command_picker.rs` — Command Registry & Overlay

### Command Registry

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandCategory {
    Navigation,
    Mode,
    Git,
    Session,
    Config,
}

#[derive(Debug, Clone)]
pub struct CommandEntry {
    pub name: &'static str,
    pub args: &'static str,
    pub description: &'static str,
    pub category: CommandCategory,
}

impl CommandEntry {
    pub fn display_name(&self) -> String {
        if self.args.is_empty() {
            self.name.to_string()
        } else {
            format!("{} {}", self.name, self.args)
        }
    }
}
```

### Full Command List (24 Commands)

All commands match the `/help` output in `src/repl.rs:640-665`:

```rust
pub fn all_commands() -> Vec<CommandEntry> {
    use CommandCategory::*;
    vec![
        // Navigation
        CommandEntry { name: "/help",        args: "",         description: "Show this help",                                  category: Navigation },
        CommandEntry { name: "/quit",        args: "",         description: "Exit closed-code",                                category: Navigation },
        CommandEntry { name: "/clear",       args: "",         description: "Clear conversation history",                      category: Navigation },

        // Mode
        CommandEntry { name: "/mode",        args: "[name]",   description: "Show or switch mode",                             category: Mode },
        CommandEntry { name: "/explore",     args: "",         description: "Switch to Explore mode",                          category: Mode },
        CommandEntry { name: "/plan",        args: "",         description: "Switch to Plan mode",                             category: Mode },
        CommandEntry { name: "/guided",      args: "",         description: "Switch to Guided mode (writes require approval)", category: Mode },
        CommandEntry { name: "/execute",     args: "",         description: "Switch to Execute mode",                          category: Mode },
        CommandEntry { name: "/auto",        args: "",         description: "Switch to Auto mode (unrestricted shell)",        category: Mode },
        CommandEntry { name: "/accept",      args: "",         description: "Accept plan and choose execution mode",           category: Mode },

        // Git
        CommandEntry { name: "/diff",        args: "[opts]",   description: "Show git diff (staged, branch, HEAD~N)",          category: Git },
        CommandEntry { name: "/review",      args: "[HEAD~N]", description: "Review changes with sub-agent",                   category: Git },
        CommandEntry { name: "/commit",      args: "[message]",description: "Generate commit message and commit",              category: Git },

        // Session
        CommandEntry { name: "/new",         args: "",         description: "Start a new session (clears history)",            category: Session },
        CommandEntry { name: "/fork",        args: "",         description: "Fork current session into a new one",             category: Session },
        CommandEntry { name: "/compact",     args: "[prompt]", description: "Compact conversation history via LLM",            category: Session },
        CommandEntry { name: "/history",     args: "[N]",      description: "Show last N conversation turns",                  category: Session },
        CommandEntry { name: "/export",      args: "[file]",   description: "Export session transcript to markdown",           category: Session },
        CommandEntry { name: "/resume",      args: "",         description: "List recent sessions",                            category: Session },

        // Config
        CommandEntry { name: "/model",       args: "[name]",   description: "Show or switch model",                            category: Config },
        CommandEntry { name: "/personality",  args: "[style]",  description: "Show or change personality",                      category: Config },
        CommandEntry { name: "/status",      args: "",         description: "Show session status and token usage",             category: Config },
        CommandEntry { name: "/sandbox",     args: "",         description: "Show sandbox mode and protected paths",           category: Config },
    ]
}
```

**Note:** `/exit` and `/q` are aliases for `/quit` in the REPL handler. We omit them from the picker to keep it clean. They still work when typed directly. Count: **23 entries** in the picker.

### CommandPicker Struct

```rust
pub struct CommandPicker {
    commands: Vec<CommandEntry>,
    max_visible: usize,
    scroll_offset: usize,
}

impl CommandPicker {
    pub fn new() -> Self {
        Self {
            commands: all_commands(),
            max_visible: 10,
            scroll_offset: 0,
        }
    }

    /// Filter commands by case-insensitive substring match on name.
    /// `filter` should NOT include the leading `/`.
    pub fn filtered(&self, filter: &str) -> Vec<&CommandEntry> {
        if filter.is_empty() {
            return self.commands.iter().collect();
        }
        let filter_lower = filter.to_lowercase();
        self.commands
            .iter()
            .filter(|cmd| {
                let name = cmd.name.strip_prefix('/').unwrap_or(cmd.name);
                name.to_lowercase().contains(&filter_lower)
            })
            .collect()
    }

    pub fn filtered_count(&self, filter: &str) -> usize {
        self.filtered(filter).len()
    }

    pub fn get_selected(&self, filter: &str, index: usize) -> Option<&CommandEntry> {
        self.filtered(filter).get(index).copied()
    }

    pub fn ensure_visible(&mut self, selected: usize) {
        if selected < self.scroll_offset {
            self.scroll_offset = selected;
        } else if selected >= self.scroll_offset + self.max_visible {
            self.scroll_offset = selected - self.max_visible + 1;
        }
    }
}
```

### Overlay Rendering

The picker renders as a floating overlay anchored to the bottom of the chat area, centered horizontally.

```rust
use ratatui::prelude::*;
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};
use super::theme::TuiTheme;

impl CommandPicker {
    pub fn render(
        &mut self,
        frame: &mut Frame,
        filter: &str,
        selected: usize,
        terminal_area: Rect,
        chat_area: Rect,
    ) {
        let filtered = self.filtered(filter);
        let matched = filtered.len();
        let total = self.commands.len();

        if matched == 0 {
            return;
        }

        self.ensure_visible(selected);

        // Overlay dimensions
        let width = terminal_area.width.saturating_sub(4).min(60);
        let visible_items = matched.min(self.max_visible);
        let height = (visible_items as u16) + 4; // border + search + gap + footer

        // Position: anchored to bottom of chat area, centered horizontally
        let x = (terminal_area.width.saturating_sub(width)) / 2;
        let y = chat_area.bottom().saturating_sub(height);
        let overlay = Rect::new(x, y, width, height);

        // Clear background
        frame.render_widget(Clear, overlay);

        // Border
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(TuiTheme::ACCENT))
            .title(
                Line::from(" Commands ")
                    .style(Style::new().fg(TuiTheme::ACCENT).bold()),
            )
            .title_bottom(
                Line::from(format!(" {} of {} ", matched, total))
                    .right_aligned()
                    .style(Style::new().fg(TuiTheme::FG_MUTED)),
            )
            .padding(Padding::horizontal(1));

        let inner = block.inner(overlay);
        frame.render_widget(block, overlay);

        // Layout: search line + gap + command list
        let [search_area, _gap, list_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(inner);

        // Search line
        let search = Line::from(vec![
            Span::styled("> ", Style::new().fg(TuiTheme::ACCENT)),
            Span::styled(format!("/{}", filter), Style::new().fg(TuiTheme::FG)),
        ]);
        frame.render_widget(Paragraph::new(search), search_area);

        // Command list
        let visible: Vec<&CommandEntry> = filtered
            .iter()
            .skip(self.scroll_offset)
            .take(self.max_visible)
            .copied()
            .collect();

        let name_width = 20.min(list_area.width as usize / 2);

        for (i, cmd) in visible.iter().enumerate() {
            let row_y = list_area.y + i as u16;
            if row_y >= list_area.bottom() {
                break;
            }
            let row = Rect::new(list_area.x, row_y, list_area.width, 1);

            let is_selected = (self.scroll_offset + i) == selected;
            let display = cmd.display_name();
            let padded = format!("{:<width$}", display, width = name_width);

            let (indicator, name_style, desc_style) = if is_selected {
                (
                    " ▸ ",
                    Style::new().fg(TuiTheme::PICKER_HIGHLIGHT_FG).bg(TuiTheme::PICKER_HIGHLIGHT_BG).bold(),
                    Style::new().fg(TuiTheme::PICKER_HIGHLIGHT_FG).bg(TuiTheme::PICKER_HIGHLIGHT_BG),
                )
            } else {
                (
                    "   ",
                    Style::new().fg(TuiTheme::ACCENT).bold(),
                    Style::new().fg(TuiTheme::FG_DIM),
                )
            };

            let line = Line::from(vec![
                Span::styled(indicator, if is_selected {
                    Style::new().bg(TuiTheme::PICKER_HIGHLIGHT_BG)
                } else {
                    Style::default()
                }),
                Span::styled(padded, name_style),
                Span::styled(cmd.description, desc_style),
            ]);
            frame.render_widget(Paragraph::new(line), row);
        }
    }
}
```

### Visual Mockup

**Empty filter:**
```
╭─ Commands ─────────────────────────────────────────────╮
│ > /                                                     │
│                                                         │
│  ▸ /help              Show this help                    │
│    /quit              Exit closed-code                  │
│    /clear             Clear conversation history        │
│    /mode [name]       Show or switch mode               │
│    /explore           Switch to Explore mode            │
│    /plan              Switch to Plan mode               │
│    /guided            Switch to Guided mode (writes...  │
│    /execute           Switch to Execute mode            │
│    /auto              Switch to Auto mode (unrestric... │
│    /accept            Accept plan and choose executio...│
│                                       ── 10 of 23 ──   │
╰─────────────────────────────────────────────────────────╯
```

**With filter `/com`:**
```
╭─ Commands ─────────────────────────────────────────────╮
│ > /com                                                  │
│                                                         │
│  ▸ /commit [message]  Generate commit message and co... │
│    /compact [prompt]  Compact conversation history vi...│
│                                        ── 2 of 23 ──   │
╰─────────────────────────────────────────────────────────╯
```

---

## 7. Modifications to Phase 9a Files

### `tui/mod.rs`

Add three new module declarations:

```rust
pub mod command_picker;
pub mod input;
pub mod keybindings;
```

### `tui/app.rs`

#### AppState — Add CommandPicker Variant

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppState {
    Idle,
    CommandPicker { filter: String, selected: usize },
    Exiting,
}
```

#### App Struct — Add New Fields

```rust
use super::input::InputPane;
use super::command_picker::CommandPicker;

pub struct App<'a> {
    pub state: AppState,
    pub tick_count: usize,
    pub status: StatusSnapshot,
    pub input_pane: InputPane<'a>,
    pub command_picker: CommandPicker,
    pub pending_input: Option<String>,
}
```

#### Event Loop — Route Through keybindings

Replace the inline key matching with:

```rust
AppEvent::Key(key) => {
    let action = keybindings::map_key(key, &app.state);
    app.handle_action(action);
}
```

#### Action Handler

```rust
use super::keybindings::Action;

impl<'a> App<'a> {
    pub fn handle_action(&mut self, action: Action) {
        match action {
            Action::Exit => {
                self.state = AppState::Exiting;
            }
            Action::Cancel => {
                if matches!(self.state, AppState::CommandPicker { .. }) {
                    self.state = AppState::Idle;
                    self.input_pane.clear();
                } else {
                    self.input_pane.clear();
                }
            }
            Action::Redraw => {} // Next frame will redraw

            // ── Input actions ──
            Action::Submit => {
                if let Some(text) = self.input_pane.submit() {
                    self.pending_input = Some(text);
                    // Phase 9c: dispatch to orchestrator
                }
            }
            Action::InsertNewline => {
                self.input_pane.insert_newline();
            }
            Action::InsertChar(c) => {
                if c == '/' && self.input_pane.is_empty() {
                    // Trigger command picker
                    self.input_pane.insert_char('/');
                    self.state = AppState::CommandPicker {
                        filter: String::new(),
                        selected: 0,
                    };
                } else {
                    self.input_pane.insert_char(c);
                }
            }
            Action::Backspace => {
                self.input_pane.delete_char_before();
            }
            Action::Delete => {
                self.input_pane.delete_char_at();
            }
            Action::ClearInput => {
                self.input_pane.clear();
                if matches!(self.state, AppState::CommandPicker { .. }) {
                    self.state = AppState::Idle;
                }
            }
            Action::OpenEditor => {
                match self.input_pane.open_editor() {
                    Ok(_) => {}
                    Err(e) => tracing::warn!("Editor error: {}", e),
                }
            }
            Action::CursorLeft  => self.input_pane.move_cursor_left(),
            Action::CursorRight => self.input_pane.move_cursor_right(),
            Action::CursorHome  => self.input_pane.move_cursor_home(),
            Action::CursorEnd   => self.input_pane.move_cursor_end(),

            // ── History ──
            Action::HistoryPrev => {
                if self.input_pane.is_empty() || self.input_pane.is_cycling_history() {
                    self.input_pane.history_prev();
                }
                // If input has content and not cycling, this could scroll chat (Phase 9c)
            }
            Action::HistoryNext => {
                if self.input_pane.is_cycling_history() {
                    self.input_pane.history_next();
                }
            }

            // ── Chat scrolling (Phase 9c) ──
            Action::PageUp | Action::PageDown |
            Action::ScrollUp | Action::ScrollDown |
            Action::ScrollToTop | Action::ScrollToBottom => {
                // No-op in Phase 9b; wired in Phase 9c
            }

            // ── Command picker ──
            Action::PickerUp => {
                if let AppState::CommandPicker { ref mut selected, .. } = self.state {
                    *selected = selected.saturating_sub(1);
                }
            }
            Action::PickerDown => {
                if let AppState::CommandPicker { ref filter, ref mut selected, .. } = self.state {
                    let count = self.command_picker.filtered_count(filter);
                    if *selected + 1 < count {
                        *selected += 1;
                    }
                }
            }
            Action::PickerSelect => {
                if let AppState::CommandPicker { ref filter, selected, .. } = self.state {
                    if let Some(cmd) = self.command_picker.get_selected(filter, selected) {
                        let text = if cmd.args.is_empty() {
                            cmd.name.to_string()
                        } else {
                            format!("{} ", cmd.name)
                        };
                        self.input_pane.clear();
                        self.input_pane.set_text(&text);
                    }
                }
                self.state = AppState::Idle;
            }
            Action::PickerDismiss => {
                self.state = AppState::Idle;
                self.input_pane.clear();
            }
            Action::PickerBackspace => {
                let text = self.input_pane.text();
                if text.len() <= 1 {
                    // Backspace past `/` — close picker
                    self.state = AppState::Idle;
                    self.input_pane.clear();
                } else {
                    self.input_pane.delete_char_before();
                    if let AppState::CommandPicker { ref mut filter, ref mut selected, .. } = self.state {
                        let new_text = self.input_pane.text();
                        *filter = new_text.strip_prefix('/').unwrap_or("").to_string();
                        *selected = 0;
                    }
                }
            }
            Action::PickerFilter(c) => {
                self.input_pane.insert_char(c);
                if let AppState::CommandPicker { ref mut filter, ref mut selected, .. } = self.state {
                    filter.push(c);
                    *selected = 0;
                }
            }

            Action::Noop => {}
        }
    }
}
```

### `tui/layout.rs`

#### Dynamic Input Height

Replace the hardcoded `Constraint::Length(3)` for input with:

```rust
let input_height = app.input_pane.desired_height();

let [header_area, chat_area, input_divider, input_area, status_divider, status_area] =
    Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(input_height),  // Dynamic!
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
```

#### Render Input Pane (Replace Placeholder)

```rust
// Replace render_input_placeholder(frame, input_area) with:
frame.render_widget(app.input_pane.widget(), input_area);
```

#### Render Command Picker Overlay

After rendering all base widgets, add:

```rust
// Overlay: command picker
if let AppState::CommandPicker { ref filter, selected } = app.state {
    app.command_picker.render(frame, filter, selected, area, chat_area);
}
```

**Note:** `app` must be `&mut App` for this call since `CommandPicker::render` takes `&mut self` (to update `scroll_offset`). Adjust `layout::render` signature accordingly, or make `scroll_offset` a `Cell<usize>` for interior mutability, or pass `scroll_offset` as a separate parameter.

The cleanest approach: make `render()` take `&mut App`:

```rust
pub fn render(frame: &mut Frame, app: &mut App) {
    // ... all rendering ...

    if let AppState::CommandPicker { ref filter, selected } = app.state {
        app.command_picker.render(frame, filter, selected, area, chat_area);
    }
}
```

---

## 8. Interaction Model

### Command Picker Flow

```
 User types '/' (input is empty)
     │
     ▼
 InsertChar('/') → app detects: first char is '/', input was empty
     │
     ▼
 AppState::Idle → AppState::CommandPicker { filter: "", selected: 0 }
     │
     ▼
 Picker overlay renders showing all 23 commands
     │
     ├── User types 'c', 'o', 'm' → PickerFilter('c'), PickerFilter('o'), PickerFilter('m')
     │       Input shows: "/com"
     │       Filter: "com"
     │       Picker shows: /commit, /compact (2 matches)
     │
     ├── User presses ↓ → PickerDown: selected = 1 (/compact)
     │
     ├── User presses Enter → PickerSelect:
     │       Input replaced with "/compact "
     │       State → Idle
     │
     ├── User presses Escape → PickerDismiss:
     │       Input cleared
     │       State → Idle
     │
     └── User presses Backspace past '/' → PickerBackspace:
             Input cleared
             State → Idle
```

### History Cycling Flow

```
 User submits "hello" then "world"
     history = ["hello", "world"]

 Input is empty, user presses ↑
     saved_input = ""
     history_index = 1 (most recent)
     Input shows: "world"

 User presses ↑ again
     history_index = 0
     Input shows: "hello"

 User presses ↓
     history_index = 1
     Input shows: "world"

 User presses ↓ again
     history_index = None
     Input shows: "" (restored saved input)
```

### $EDITOR Flow

```
 User presses Ctrl+G
     │
     ▼
 Write current input to /tmp/closed-code-input-{pid}.txt
     │
     ▼
 Leave alternate screen + disable raw mode
     │
     ▼
 Spawn $EDITOR (or $VISUAL or vi) with temp file
     │
     ▼
 Wait for editor to exit
     │
     ▼
 Re-enter alternate screen + enable raw mode
     │
     ▼
 Read temp file → replace input content
     │
     ▼
 Delete temp file
```

---

## 9. Implementation Order

| Step | File | Why this order |
|------|------|----------------|
| 1 | `Cargo.toml` | Add `tui-textarea = "0.7"` |
| 2 | `src/tui/command_picker.rs` | No dependencies on other 9b files; write registry + filtering + tests |
| 3 | `src/tui/input.rs` | Depends on theme.rs only; write InputPane + tests |
| 4 | `src/tui/keybindings.rs` | Depends on app.rs (AppState); write Action + mapping + tests |
| 5 | `src/tui/mod.rs` | Add three new module declarations |
| 6 | `src/tui/app.rs` | Add CommandPicker variant to AppState, add fields to App, write handle_action |
| 7 | `src/tui/layout.rs` | Dynamic height, render InputPane + overlay |
| 8 | `cargo test` | All new + existing tests pass |
| 9 | `cargo run` | Manual verification |

---

## 10. Tests

### `tui/keybindings.rs` Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    fn alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
    }

    #[test]
    fn global_ctrl_c() {
        assert_eq!(map_key(ctrl('c'), &AppState::Idle), Action::Cancel);
    }

    #[test]
    fn global_ctrl_d() {
        assert_eq!(map_key(ctrl('d'), &AppState::Idle), Action::Exit);
    }

    #[test]
    fn idle_enter_submits() {
        assert_eq!(map_key(key(KeyCode::Enter), &AppState::Idle), Action::Submit);
    }

    #[test]
    fn idle_shift_enter_newline() {
        assert_eq!(map_key(shift(KeyCode::Enter), &AppState::Idle), Action::InsertNewline);
    }

    #[test]
    fn idle_alt_enter_newline() {
        assert_eq!(map_key(alt(KeyCode::Enter), &AppState::Idle), Action::InsertNewline);
    }

    #[test]
    fn idle_ctrl_g_opens_editor() {
        assert_eq!(map_key(ctrl('g'), &AppState::Idle), Action::OpenEditor);
    }

    #[test]
    fn idle_ctrl_u_clears() {
        assert_eq!(map_key(ctrl('u'), &AppState::Idle), Action::ClearInput);
    }

    #[test]
    fn idle_printable_char() {
        assert_eq!(map_key(key(KeyCode::Char('a')), &AppState::Idle), Action::InsertChar('a'));
    }

    #[test]
    fn idle_escape_clears() {
        assert_eq!(map_key(key(KeyCode::Esc), &AppState::Idle), Action::ClearInput);
    }

    #[test]
    fn idle_arrow_up_history() {
        assert_eq!(map_key(key(KeyCode::Up), &AppState::Idle), Action::HistoryPrev);
    }

    #[test]
    fn picker_enter_selects() {
        let state = AppState::CommandPicker { filter: String::new(), selected: 0 };
        assert_eq!(map_key(key(KeyCode::Enter), &state), Action::PickerSelect);
    }

    #[test]
    fn picker_escape_dismisses() {
        let state = AppState::CommandPicker { filter: String::new(), selected: 0 };
        assert_eq!(map_key(key(KeyCode::Esc), &state), Action::PickerDismiss);
    }

    #[test]
    fn picker_char_filters() {
        let state = AppState::CommandPicker { filter: String::new(), selected: 0 };
        assert_eq!(map_key(key(KeyCode::Char('h')), &state), Action::PickerFilter('h'));
    }

    #[test]
    fn picker_arrows_navigate() {
        let state = AppState::CommandPicker { filter: String::new(), selected: 0 };
        assert_eq!(map_key(key(KeyCode::Up), &state), Action::PickerUp);
        assert_eq!(map_key(key(KeyCode::Down), &state), Action::PickerDown);
    }
}
```

### `tui/input.rs` Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn pane() -> InputPane<'static> {
        InputPane::new(PathBuf::from("/tmp"))
    }

    #[test]
    fn new_is_empty() {
        let p = pane();
        assert!(p.is_empty());
        assert_eq!(p.text(), "");
    }

    #[test]
    fn insert_and_read() {
        let mut p = pane();
        p.insert_char('h');
        p.insert_char('i');
        assert_eq!(p.text(), "hi");
        assert!(!p.is_empty());
    }

    #[test]
    fn submit_returns_text_and_clears() {
        let mut p = pane();
        p.insert_char('x');
        assert_eq!(p.submit(), Some("x".to_string()));
        assert!(p.is_empty());
    }

    #[test]
    fn submit_empty_returns_none() {
        let mut p = pane();
        assert_eq!(p.submit(), None);
    }

    #[test]
    fn submit_pushes_to_history() {
        let mut p = pane();
        p.insert_char('a');
        p.submit();
        p.insert_char('b');
        p.submit();
        assert_eq!(p.history.len(), 2);
    }

    #[test]
    fn submit_deduplicates_consecutive() {
        let mut p = pane();
        p.insert_char('a');
        p.submit();
        p.insert_char('a');
        p.submit();
        assert_eq!(p.history.len(), 1);
    }

    #[test]
    fn clear_empties() {
        let mut p = pane();
        p.insert_char('x');
        p.clear();
        assert!(p.is_empty());
    }

    #[test]
    fn first_char_detection() {
        let mut p = pane();
        assert_eq!(p.first_char(), None);
        p.insert_char('/');
        assert_eq!(p.first_char(), Some('/'));
    }

    #[test]
    fn desired_height_empty() {
        let p = pane();
        assert_eq!(p.desired_height(), INPUT_MIN_HEIGHT);
    }

    #[test]
    fn history_cycling() {
        let mut p = pane();
        p.insert_char('a');
        p.submit();
        p.insert_char('b');
        p.submit();

        p.history_prev(); // shows "b"
        assert_eq!(p.text(), "b");
        p.history_prev(); // shows "a"
        assert_eq!(p.text(), "a");
        p.history_next(); // back to "b"
        assert_eq!(p.text(), "b");
        p.history_next(); // restores empty
        assert_eq!(p.text(), "");
    }

    #[test]
    fn history_saves_current_input() {
        let mut p = pane();
        p.insert_char('x');
        p.submit();

        p.insert_char('y');
        p.history_prev(); // saves "y", shows "x"
        assert_eq!(p.text(), "x");
        p.history_next(); // restores "y"
        assert_eq!(p.text(), "y");
    }
}
```

### `tui/command_picker.rs` Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_commands_start_with_slash() {
        for cmd in all_commands() {
            assert!(cmd.name.starts_with('/'), "{} should start with /", cmd.name);
        }
    }

    #[test]
    fn all_commands_have_descriptions() {
        for cmd in all_commands() {
            assert!(!cmd.description.is_empty(), "{} has empty description", cmd.name);
        }
    }

    #[test]
    fn filter_empty_returns_all() {
        let picker = CommandPicker::new();
        assert_eq!(picker.filtered("").len(), all_commands().len());
    }

    #[test]
    fn filter_narrows_results() {
        let picker = CommandPicker::new();
        let matches = picker.filtered("com");
        assert!(matches.len() >= 2); // /commit and /compact at minimum
        assert!(matches.iter().any(|c| c.name == "/commit"));
        assert!(matches.iter().any(|c| c.name == "/compact"));
    }

    #[test]
    fn filter_exact_match() {
        let picker = CommandPicker::new();
        let matches = picker.filtered("help");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "/help");
    }

    #[test]
    fn filter_case_insensitive() {
        let picker = CommandPicker::new();
        assert_eq!(
            picker.filtered("quit").len(),
            picker.filtered("QUIT").len(),
        );
    }

    #[test]
    fn filter_no_match() {
        let picker = CommandPicker::new();
        assert_eq!(picker.filtered("zzzzz").len(), 0);
    }

    #[test]
    fn get_selected_valid() {
        let picker = CommandPicker::new();
        assert!(picker.get_selected("", 0).is_some());
    }

    #[test]
    fn get_selected_out_of_bounds() {
        let picker = CommandPicker::new();
        assert!(picker.get_selected("", 999).is_none());
    }

    #[test]
    fn display_name_with_args() {
        let cmd = CommandEntry {
            name: "/mode", args: "[name]",
            description: "test", category: CommandCategory::Mode,
        };
        assert_eq!(cmd.display_name(), "/mode [name]");
    }

    #[test]
    fn display_name_without_args() {
        let cmd = CommandEntry {
            name: "/help", args: "",
            description: "test", category: CommandCategory::Navigation,
        };
        assert_eq!(cmd.display_name(), "/help");
    }

    #[test]
    fn ensure_visible_scrolls() {
        let mut picker = CommandPicker::new();
        picker.max_visible = 3;
        picker.scroll_offset = 0;
        picker.ensure_visible(5);
        assert_eq!(picker.scroll_offset, 3); // 5 - 3 + 1
    }
}
```

### Test Summary

| File | # Tests | Coverage |
|------|---------|----------|
| `keybindings.rs` | 14 | Global keys, idle mappings, picker mappings |
| `input.rs` | 11 | Empty, insert, submit, history, clear, height, first_char |
| `command_picker.rs` | 11 | Registry validation, filtering, selection, scrolling |
| **Total** | **36** | |

---

## 11. Verification Checklist

### Automated

```bash
cargo test          # All existing tests pass, 36 new tests pass
cargo clippy        # No warnings
cargo fmt --check   # Formatted
```

### Manual

- [ ] Input area shows placeholder when empty
- [ ] Typing text appears in the input area
- [ ] `Enter` submits and clears input (text appears in `pending_input`)
- [ ] `Shift+Enter` inserts a newline (input grows taller)
- [ ] `Alt+Enter` also inserts a newline
- [ ] Input height grows from 3 to 8 lines, then scrolls internally
- [ ] `Ctrl+U` clears input completely
- [ ] `Escape` clears input completely
- [ ] `Backspace` deletes character before cursor
- [ ] Arrow keys move cursor within input
- [ ] Arrow `Up` when input is empty shows previous history entry
- [ ] Arrow `Down` cycles forward through history, then restores original input
- [ ] `Ctrl+G` opens `$EDITOR`, content returns to input
- [ ] Typing `/` in empty input opens command picker overlay
- [ ] Picker shows all commands with descriptions
- [ ] Typing filters commands (e.g., `/com` shows `/commit` and `/compact`)
- [ ] Arrow keys navigate picker highlight
- [ ] `Enter` selects command and inserts it into input (with trailing space if args expected)
- [ ] `Escape` dismisses picker and clears input
- [ ] `Backspace` past `/` dismisses picker
- [ ] Picker scrolls when more than 10 matches
- [ ] Footer shows "N of M" count
- [ ] `Ctrl+D` still exits the app cleanly
- [ ] `Ctrl+C` clears input (or exits if empty — Phase 9b behavior TBD)
- [ ] Terminal resize reflows layout correctly

---

## Estimated Scope

| Metric | Value |
|--------|-------|
| New files | 3 |
| Modified files | 4 |
| New lines (est.) | ~600 |
| New tests | 36 |
