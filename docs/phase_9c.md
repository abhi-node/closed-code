# Phase 9c: Chat Area, Streaming & Command Dispatch

> Render conversation messages in a scrollable chat area, stream LLM responses in real-time, execute slash commands, and run shell commands — connecting the TUI to the orchestrator.

**Prerequisite:** Phase 9a (app shell) + Phase 9b (input + command picker).

---

## Table of Contents

1. [Goal & Checkpoint](#1-goal--checkpoint)
2. [Dependency Changes](#2-dependency-changes)
3. [File Overview](#3-file-overview)
4. [tui/chat.rs — Chat Area & Scrollable Viewport](#4-tuichatrs--chat-area--scrollable-viewport)
5. [tui/message.rs — Message Rendering](#5-tuimessagers--message-rendering)
6. [tui/spinner.rs — Spinner Widget](#6-tuispinnerrs--spinner-widget)
7. [tui/commands.rs — Slash Command Dispatch](#7-tuicommandsrs--slash-command-dispatch)
8. [Modifications to Phase 9a/9b Files](#8-modifications-to-phase-9a9b-files)
9. [Streaming Integration](#9-streaming-integration)
10. [Interaction Model](#10-interaction-model)
11. [Implementation Order](#11-implementation-order)
12. [Tests](#12-tests)
13. [Verification Checklist](#13-verification-checklist)

---

## 1. Goal & Checkpoint

**Phase 9c delivers:**

- A scrollable chat area displaying conversation messages:
  - **User messages** — cyan-bordered blocks with "You" title
  - **Assistant messages** — violet-bordered blocks, streamed token-by-token with cursor indicator
  - **Tool call indicators** — inline spinner during execution, checkmark on completion with duration
  - **Sub-agent indicators** — box-drawing header/footer with progress
  - **System messages** — centered horizontal rules (mode changes, compact, clear, errors)
- Real-time LLM streaming via orchestrator → TUI event channel
- Spinner animation for "Thinking..." and tool execution states
- Slash command dispatch (all 23+ commands work through TUI)
- Shell command execution (`!` prefix) with output captured in chat
- Chat scrolling: Arrow Up/Down, Page Up/Down, Home/End, mouse wheel
- Auto-scroll (follows new content when at bottom, pauses on manual scroll-up)
- Scroll position indicator ("↑ N more" when scrolled up)
- Cancellation: Ctrl+C during streaming stops the orchestrator
- New `AppState` variants: `Thinking`, `Streaming`, `ToolExecuting`

**What is NOT in Phase 9c:**

- No approval overlays (Phase 9d)
- No diff viewer (Phase 9d)
- No session picker overlay (Phase 9d)
- `/resume` shows a system message directing users to use `--resume` CLI flag (Phase 9d adds overlay)
- `/accept` shows a system message directing users (Phase 9d adds overlay)

---

## 2. Dependency Changes

No new Cargo.toml dependencies. Phase 9c uses only ratatui, crossterm, tokio, and existing crates.

---

## 3. File Overview

### New Files (4)

| File | Purpose | Est. Lines |
|------|---------|-----------|
| `src/tui/chat.rs` | Chat viewport: scroll state, message list, rendering | ~250 |
| `src/tui/message.rs` | Individual message rendering (user, assistant, tool, system) | ~300 |
| `src/tui/spinner.rs` | Animated spinner widget for thinking/tool states | ~50 |
| `src/tui/commands.rs` | Slash command dispatch, mirroring `repl.rs` handlers | ~400 |

### Modified Files (5)

| File | Change |
|------|--------|
| `src/tui/app.rs` | Add `Thinking`/`Streaming`/`ToolExecuting` states, `messages` vec, orchestrator dispatch task, scroll state, event_tx channel |
| `src/tui/events.rs` | Add `TextDelta`, `StreamDone`, `ToolStart`, `ToolComplete`, `ToolError`, `AgentStart`, `AgentComplete`, `ModeChanged`, `SystemMessage`, `Error` variants |
| `src/tui/keybindings.rs` | Wire scroll actions, add `map_thinking`/`map_streaming` handlers |
| `src/tui/layout.rs` | Replace `render_chat_placeholder` with `chat::render`, add scroll indicator |
| `src/tui/mod.rs` | Add `pub mod chat; pub mod commands; pub mod message; pub mod spinner;` |

---

## 4. `tui/chat.rs` — Chat Area & Scrollable Viewport

### Chat Message Model

```rust
use std::time::{Duration, Instant};

/// A single message in the chat history, ready for rendering.
#[derive(Debug, Clone)]
pub enum ChatMessage {
    /// User's input message.
    User { content: String },

    /// Assistant response (may be streaming).
    Assistant {
        content: String,
        tool_calls: Vec<ToolCallDisplay>,
        is_streaming: bool,
    },

    /// System notification (mode change, compact, error, etc.).
    System { content: String },
}

/// Display state for a tool call within an assistant message.
#[derive(Debug, Clone)]
pub enum ToolCallDisplay {
    /// Tool is currently executing.
    Running {
        name: String,
        args_display: String,
        started_at: Instant,
    },

    /// Tool completed successfully.
    Completed {
        name: String,
        args_display: String,
        duration: Duration,
    },

    /// Tool failed with an error.
    Failed {
        name: String,
        args_display: String,
        error: String,
    },

    /// Sub-agent is running.
    AgentRunning {
        agent_type: String,
        task: String,
        tool_calls: usize,
    },

    /// Sub-agent completed.
    AgentCompleted {
        agent_type: String,
        duration: Duration,
        tool_calls: usize,
    },
}
```

### Scroll State

```rust
pub struct ChatViewport {
    /// Offset from the top of all content. When `None`, auto-scroll is active
    /// (viewport follows the bottom of content).
    scroll_offset: Option<usize>,

    /// Cached total content height from last render (in terminal rows).
    total_height: usize,

    /// Visible height of the chat area (in terminal rows).
    visible_height: usize,
}

impl ChatViewport {
    pub fn new() -> Self {
        Self {
            scroll_offset: None,
            total_height: 0,
            visible_height: 0,
        }
    }

    /// Whether auto-scroll is active (viewport follows bottom).
    pub fn is_auto_scroll(&self) -> bool {
        self.scroll_offset.is_none()
    }

    /// Scroll up by `n` lines. Activates manual scroll mode.
    pub fn scroll_up(&mut self, n: usize) {
        let current = self.effective_offset();
        self.scroll_offset = Some(current.saturating_sub(n));
    }

    /// Scroll down by `n` lines. Re-enables auto-scroll if at bottom.
    pub fn scroll_down(&mut self, n: usize) {
        let current = self.effective_offset();
        let new_offset = current + n;
        let max_offset = self.total_height.saturating_sub(self.visible_height);
        if new_offset >= max_offset {
            self.scroll_offset = None; // Re-enable auto-scroll
        } else {
            self.scroll_offset = Some(new_offset);
        }
    }

    /// Jump to top.
    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = Some(0);
    }

    /// Jump to bottom (re-enable auto-scroll).
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = None;
    }

    /// Page up (half viewport height).
    pub fn page_up(&mut self) {
        let half = self.visible_height / 2;
        self.scroll_up(half.max(1));
    }

    /// Page down (half viewport height).
    pub fn page_down(&mut self) {
        let half = self.visible_height / 2;
        self.scroll_down(half.max(1));
    }

    /// Current effective scroll offset for rendering.
    fn effective_offset(&self) -> usize {
        match self.scroll_offset {
            Some(offset) => offset,
            None => self.total_height.saturating_sub(self.visible_height),
        }
    }

    /// How many lines are above the visible viewport.
    pub fn lines_above(&self) -> usize {
        self.effective_offset()
    }
}
```

### Rendering

The chat area renders into the `chat_area` Rect from `layout.rs`. The approach:

1. Pre-render all messages into a `Vec<Line>` (ratatui Lines)
2. Calculate total height
3. Determine visible slice based on scroll offset
4. Render only the visible lines using `Paragraph::scroll()`

```rust
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::app::App;
use super::message;
use super::theme::TuiTheme;

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    // Side borders for visual containment
    let block = Block::default()
        .borders(Borders::LEFT | Borders::RIGHT)
        .border_style(Style::default().fg(TuiTheme::BORDER_DIM));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Build all lines from messages
    let mut lines: Vec<Line> = Vec::new();
    for msg in &app.messages {
        let msg_lines = message::render_message(msg, inner.width, app.tick_count);
        lines.extend(msg_lines);
        lines.push(Line::from("")); // Blank line between messages
    }

    // Update viewport dimensions
    app.chat_viewport.total_height = lines.len();
    app.chat_viewport.visible_height = inner.height as usize;

    // Calculate scroll offset
    let offset = app.chat_viewport.effective_offset();

    // Render with scroll
    let paragraph = Paragraph::new(lines)
        .scroll((offset as u16, 0));
    frame.render_widget(paragraph, inner);

    // Scroll indicator
    let lines_above = app.chat_viewport.lines_above();
    if lines_above > 0 {
        let indicator = format!("↑ {} more ", lines_above);
        let indicator_area = Rect::new(
            inner.right().saturating_sub(indicator.len() as u16 + 1),
            inner.y,
            indicator.len() as u16,
            1,
        );
        frame.render_widget(
            Paragraph::new(indicator)
                .style(Style::default().fg(TuiTheme::FG_MUTED)),
            indicator_area,
        );
    }
}
```

### Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_auto_scroll_default() {
        let vp = ChatViewport::new();
        assert!(vp.is_auto_scroll());
    }

    #[test]
    fn viewport_scroll_up_activates_manual() {
        let mut vp = ChatViewport::new();
        vp.total_height = 100;
        vp.visible_height = 20;
        vp.scroll_up(5);
        assert!(!vp.is_auto_scroll());
    }

    #[test]
    fn viewport_scroll_to_bottom_re_enables_auto() {
        let mut vp = ChatViewport::new();
        vp.total_height = 100;
        vp.visible_height = 20;
        vp.scroll_up(5);
        assert!(!vp.is_auto_scroll());
        vp.scroll_to_bottom();
        assert!(vp.is_auto_scroll());
    }

    #[test]
    fn viewport_scroll_down_past_bottom_re_enables_auto() {
        let mut vp = ChatViewport::new();
        vp.total_height = 100;
        vp.visible_height = 20;
        vp.scroll_offset = Some(70); // 10 lines from bottom
        vp.scroll_down(20); // Past bottom
        assert!(vp.is_auto_scroll());
    }

    #[test]
    fn viewport_scroll_to_top() {
        let mut vp = ChatViewport::new();
        vp.total_height = 100;
        vp.visible_height = 20;
        vp.scroll_to_top();
        assert_eq!(vp.lines_above(), 0);
    }

    #[test]
    fn viewport_lines_above_at_bottom() {
        let mut vp = ChatViewport::new();
        vp.total_height = 100;
        vp.visible_height = 20;
        // Auto-scroll → at bottom
        assert_eq!(vp.lines_above(), 80);
    }
}
```

---

## 5. `tui/message.rs` — Message Rendering

Converts a `ChatMessage` into `Vec<Line>` for the chat viewport.

### User Message

```
╭─ You ──────────────────────────────────────────────────────────╮
│ What files are in this project?                                 │
╰─────────────────────────────────────────────────────────────────╯
```

```rust
use ratatui::prelude::*;
use super::chat::{ChatMessage, ToolCallDisplay};
use super::theme::TuiTheme;

/// Render a message into Lines for the chat viewport.
pub fn render_message(msg: &ChatMessage, width: u16, tick: usize) -> Vec<Line<'static>> {
    match msg {
        ChatMessage::User { content } => render_user(content, width),
        ChatMessage::Assistant { content, tool_calls, is_streaming } =>
            render_assistant(content, tool_calls, *is_streaming, width, tick),
        ChatMessage::System { content } => render_system(content, width),
    }
}

fn render_user(content: &str, width: u16) -> Vec<Line<'static>> {
    let inner_width = (width as usize).saturating_sub(4); // 2 border + 2 padding
    let mut lines = Vec::new();

    // Top border with title
    let title = "─ You ";
    let rest = "─".repeat(inner_width.saturating_sub(title.len() + 1));
    lines.push(Line::from(vec![
        Span::styled("╭", Style::new().fg(TuiTheme::USER)),
        Span::styled(title, Style::new().fg(TuiTheme::USER).bold()),
        Span::styled(rest, Style::new().fg(TuiTheme::USER)),
        Span::styled("╮", Style::new().fg(TuiTheme::USER)),
    ]));

    // Content lines
    for line in content.lines() {
        let padded = format!("│ {:<width$}│", line, width = inner_width);
        lines.push(Line::styled(padded, Style::new().fg(TuiTheme::FG)));
    }

    // Bottom border
    let bottom = "─".repeat(inner_width + 2);
    lines.push(Line::from(vec![
        Span::styled("╰", Style::new().fg(TuiTheme::USER)),
        Span::styled(bottom, Style::new().fg(TuiTheme::USER)),
        Span::styled("╯", Style::new().fg(TuiTheme::USER)),
    ]));

    lines
}
```

### Assistant Message

```
╭─ Assistant ─────────────────────────────────────────────────────╮
│  ✓ read_file(path: "src/main.rs")                        0.2s  │
│                                                                  │
│ The main function does the following:                            │
│ 1. Sets up the terminal▌                                        │
╰──────────────────────────────────────────────────────────────────╯
```

```rust
fn render_assistant(
    content: &str,
    tool_calls: &[ToolCallDisplay],
    is_streaming: bool,
    width: u16,
    tick: usize,
) -> Vec<Line<'static>> {
    let inner_width = (width as usize).saturating_sub(4);
    let mut lines = Vec::new();

    // Top border with title
    let title = "─ Assistant ";
    let rest = "─".repeat(inner_width.saturating_sub(title.len() + 1));
    lines.push(Line::from(vec![
        Span::styled("╭", Style::new().fg(TuiTheme::ASSISTANT)),
        Span::styled(title, Style::new().fg(TuiTheme::ASSISTANT).bold()),
        Span::styled(rest, Style::new().fg(TuiTheme::ASSISTANT)),
        Span::styled("╮", Style::new().fg(TuiTheme::ASSISTANT)),
    ]));

    // Tool calls (before content)
    for tc in tool_calls {
        lines.push(render_tool_call(tc, inner_width, tick));
    }
    if !tool_calls.is_empty() && !content.is_empty() {
        lines.push(Line::styled(
            format!("│{:width$}│", "", width = inner_width + 2),
            Style::new().fg(TuiTheme::ASSISTANT),
        ));
    }

    // Content lines
    for line in content.lines() {
        let padded = format!("│ {:<width$}│", line, width = inner_width);
        lines.push(Line::styled(padded, Style::new().fg(TuiTheme::FG)));
    }

    // Streaming cursor indicator
    if is_streaming && !content.is_empty() {
        // Blink cursor every 500ms (5 ticks at 100ms)
        if (tick / 5) % 2 == 0 {
            if let Some(last) = lines.last_mut() {
                // Append blinking cursor to the last content line
                let cursor_line = format!("│ {}▌{:>width$}│",
                    content.lines().last().unwrap_or(""),
                    "",
                    width = inner_width.saturating_sub(content.lines().last().map_or(0, |l| l.len()) + 1)
                );
                *last = Line::styled(cursor_line, Style::new().fg(TuiTheme::FG));
            }
        }
    }

    // Bottom border
    let bottom = "─".repeat(inner_width + 2);
    lines.push(Line::from(vec![
        Span::styled("╰", Style::new().fg(TuiTheme::ASSISTANT)),
        Span::styled(bottom, Style::new().fg(TuiTheme::ASSISTANT)),
        Span::styled("╯", Style::new().fg(TuiTheme::ASSISTANT)),
    ]));

    lines
}
```

### Tool Call Display

```rust
fn render_tool_call(tc: &ToolCallDisplay, inner_width: usize, tick: usize) -> Line<'static> {
    match tc {
        ToolCallDisplay::Running { name, args_display, .. } => {
            let frame_char = TuiTheme::SPINNER_FRAMES[tick % TuiTheme::SPINNER_FRAMES.len()];
            let display = format!("{name}({args_display})");
            let truncated = truncate_display(&display, inner_width.saturating_sub(6));
            Line::from(vec![
                Span::styled("│ ", Style::new().fg(TuiTheme::ASSISTANT)),
                Span::styled(frame_char, Style::new().fg(TuiTheme::TOOL)),
                Span::styled(format!(" {}", truncated), Style::new().fg(TuiTheme::FG_DIM)),
            ])
        }
        ToolCallDisplay::Completed { name, args_display, duration } => {
            let display = format!("{name}({args_display})");
            let dur_str = format!("{:.1}s", duration.as_secs_f64());
            let available = inner_width.saturating_sub(dur_str.len() + 6);
            let truncated = truncate_display(&display, available);
            let padding = available.saturating_sub(truncated.len());
            Line::from(vec![
                Span::styled("│ ", Style::new().fg(TuiTheme::ASSISTANT)),
                Span::styled("✓ ", Style::new().fg(TuiTheme::SUCCESS)),
                Span::styled(truncated, Style::new().fg(TuiTheme::FG_DIM)),
                Span::raw(" ".repeat(padding)),
                Span::styled(dur_str, Style::new().fg(TuiTheme::FG_MUTED)),
                Span::styled(" │", Style::new().fg(TuiTheme::ASSISTANT)),
            ])
        }
        ToolCallDisplay::Failed { name, error, .. } => {
            let display = format!("{name}: {error}");
            let truncated = truncate_display(&display, inner_width.saturating_sub(6));
            Line::from(vec![
                Span::styled("│ ", Style::new().fg(TuiTheme::ASSISTANT)),
                Span::styled("✗ ", Style::new().fg(TuiTheme::ERROR)),
                Span::styled(truncated, Style::new().fg(TuiTheme::ERROR)),
            ])
        }
        ToolCallDisplay::AgentRunning { agent_type, task, tool_calls } => {
            let frame_char = TuiTheme::SPINNER_FRAMES[tick % TuiTheme::SPINNER_FRAMES.len()];
            let task_display = truncate_display(task, inner_width.saturating_sub(20));
            Line::from(vec![
                Span::styled("│ ┌ ", Style::new().fg(TuiTheme::AGENT)),
                Span::styled(format!("[{agent_type}] "), Style::new().fg(TuiTheme::AGENT).bold()),
                Span::styled(frame_char, Style::new().fg(TuiTheme::AGENT)),
                Span::styled(format!(" {task_display} ({tool_calls} calls)"), Style::new().fg(TuiTheme::FG_DIM)),
            ])
        }
        ToolCallDisplay::AgentCompleted { agent_type, duration, tool_calls } => {
            let dur_str = format!("{:.1}s", duration.as_secs_f64());
            Line::from(vec![
                Span::styled("│ └ ", Style::new().fg(TuiTheme::AGENT)),
                Span::styled(format!("[{agent_type}] "), Style::new().fg(TuiTheme::AGENT).bold()),
                Span::styled("✓ ", Style::new().fg(TuiTheme::SUCCESS)),
                Span::styled(format!("{tool_calls} calls, {dur_str}"), Style::new().fg(TuiTheme::FG_DIM)),
            ])
        }
    }
}

fn truncate_display(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else if max > 3 {
        format!("{}...", &s[..max - 3])
    } else {
        s[..max].to_string()
    }
}
```

### System Message

```
 ── Switched to execute mode (9 tools) ──────────────────────────
```

```rust
fn render_system(content: &str, width: u16) -> Vec<Line<'static>> {
    let w = width as usize;
    let prefix = format!(" ── {} ", content);
    let rest_len = w.saturating_sub(prefix.len());
    let rest = "─".repeat(rest_len);
    vec![Line::from(vec![
        Span::styled(prefix, Style::new().fg(TuiTheme::FG_DIM)),
        Span::styled(rest, Style::new().fg(TuiTheme::FG_DIM)),
    ])]
}
```

### Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_message_has_borders() {
        let lines = render_user("hello", 40);
        assert!(lines.len() >= 3); // top + content + bottom
    }

    #[test]
    fn system_message_single_line() {
        let lines = render_system("Mode changed", 60);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn truncate_display_short() {
        assert_eq!(truncate_display("hello", 10), "hello");
    }

    #[test]
    fn truncate_display_long() {
        assert_eq!(truncate_display("very long text here", 10), "very lo...");
    }
}
```

---

## 6. `tui/spinner.rs` — Spinner Widget

Standalone widget for rendering an animated braille spinner with a message.

```rust
use ratatui::prelude::*;
use super::theme::TuiTheme;

pub struct SpinnerWidget {
    pub message: String,
    pub tick: usize,
}

impl Widget for &SpinnerWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 4 {
            return;
        }
        let frame_char = TuiTheme::SPINNER_FRAMES[self.tick % TuiTheme::SPINNER_FRAMES.len()];
        let line = Line::from(vec![
            Span::styled(format!("  {}", frame_char), Style::new().fg(TuiTheme::ACCENT)),
            Span::styled(format!(" {}", self.message), Style::new().fg(TuiTheme::FG_DIM)),
        ]);
        line.render(area, buf);
    }
}
```

### Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_frames_cycle() {
        assert_eq!(
            TuiTheme::SPINNER_FRAMES[0 % TuiTheme::SPINNER_FRAMES.len()],
            "⠋"
        );
        assert_eq!(
            TuiTheme::SPINNER_FRAMES[10 % TuiTheme::SPINNER_FRAMES.len()],
            "⠋" // Wraps around
        );
    }
}
```

---

## 7. `tui/commands.rs` — Slash Command Dispatch

Handles all slash commands within the TUI context. Mirrors `repl.rs::handle_slash_command()` but sends output to the chat area instead of stdout.

### Command Result

```rust
use crate::agent::orchestrator::Orchestrator;
use super::chat::ChatMessage;

/// Result of a slash command execution.
pub enum CommandResult {
    /// Display message(s) in chat and continue.
    Continue(Vec<ChatMessage>),
    /// Quit the application.
    Quit,
    /// Execute the accepted plan (send instruction to orchestrator).
    ExecutePlan(String),
    /// Switch to a specific mode (may need handler swap).
    SwitchMode {
        mode: crate::mode::Mode,
        messages: Vec<ChatMessage>,
    },
    /// Show session picker overlay (Phase 9d).
    ShowSessionPicker,
    /// Show mode picker overlay for /accept (Phase 9d).
    ShowModePicker,
}
```

### Dispatcher

```rust
/// Dispatch a slash command. The `input` includes the leading `/`.
pub async fn dispatch(
    input: &str,
    orchestrator: &mut Orchestrator,
) -> CommandResult {
    let (cmd, args) = parse_command(input);

    match cmd {
        // Navigation
        "quit" | "exit" | "q" => CommandResult::Quit,
        "clear" => {
            orchestrator.clear_history();
            CommandResult::Continue(vec![
                ChatMessage::System { content: "Conversation history cleared.".to_string() },
            ])
        }
        "help" => {
            CommandResult::Continue(vec![
                ChatMessage::System { content: format_help_text() },
            ])
        }

        // Mode switching
        "mode" => handle_mode(args, orchestrator),
        "explore" => switch_mode(orchestrator, crate::mode::Mode::Explore),
        "plan" => switch_mode(orchestrator, crate::mode::Mode::Plan),
        "guided" => switch_mode(orchestrator, crate::mode::Mode::Guided),
        "execute" => switch_mode(orchestrator, crate::mode::Mode::Execute),
        "auto" => switch_mode(orchestrator, crate::mode::Mode::Auto),
        "accept" | "a" => {
            // Phase 9d: show mode picker overlay
            // Phase 9c: placeholder
            if orchestrator.mode() != &crate::mode::Mode::Plan {
                return CommandResult::Continue(vec![
                    ChatMessage::System { content: "Not in Plan mode.".to_string() },
                ]);
            }
            if orchestrator.current_plan().is_none() {
                return CommandResult::Continue(vec![
                    ChatMessage::System { content: "No plan to accept.".to_string() },
                ]);
            }
            CommandResult::ShowModePicker
        }

        // Git
        "diff" => handle_diff(args, orchestrator).await,
        "review" => handle_review(args, orchestrator).await,
        "commit" => handle_commit(args, orchestrator).await,

        // Session
        "new" => handle_new(orchestrator),
        "fork" => handle_fork(orchestrator),
        "compact" => handle_compact(args, orchestrator).await,
        "history" => handle_history(args, orchestrator),
        "export" => handle_export(args, orchestrator),
        "resume" => {
            // Phase 9d: show session picker overlay
            CommandResult::ShowSessionPicker
        }

        // Config
        "model" => handle_model(args, orchestrator),
        "personality" => handle_personality(args, orchestrator),
        "status" => handle_status(orchestrator),
        "sandbox" => handle_sandbox(orchestrator),

        _ => CommandResult::Continue(vec![
            ChatMessage::System {
                content: format!("Unknown command: /{}. Type /help for available commands.", cmd),
            },
        ]),
    }
}

fn parse_command(input: &str) -> (&str, &str) {
    let trimmed = input.trim().strip_prefix('/').unwrap_or(input);
    match trimmed.split_once(char::is_whitespace) {
        Some((cmd, args)) => (cmd, args.trim()),
        None => (trimmed, ""),
    }
}
```

### Mode Switching Helper

```rust
fn switch_mode(
    orchestrator: &mut Orchestrator,
    mode: crate::mode::Mode,
) -> CommandResult {
    let tool_count = orchestrator.tool_count_for_mode(&mode);
    orchestrator.set_mode(mode);
    let label = super::theme::mode_label(&mode);
    CommandResult::SwitchMode {
        mode,
        messages: vec![ChatMessage::System {
            content: format!("Switched to {} mode ({} tools)", label, tool_count),
        }],
    }
}
```

### Git Command Handlers

```rust
async fn handle_diff(args: &str, orchestrator: &Orchestrator) -> CommandResult {
    let working_dir = orchestrator.working_directory();
    let diff_result = match args {
        "staged" => crate::git::diff::staged(working_dir).await,
        s if s.starts_with("HEAD~") => crate::git::diff::commit_range(working_dir, s).await,
        "" => crate::git::diff::all_uncommitted(working_dir).await,
        branch => crate::git::diff::branch_diff(working_dir, branch).await,
    };

    match diff_result {
        Ok(diff) if diff.is_empty() => {
            CommandResult::Continue(vec![
                ChatMessage::System { content: "No changes found.".to_string() },
            ])
        }
        Ok(diff) => {
            // Show diff as a code-block-like system message
            CommandResult::Continue(vec![
                ChatMessage::System { content: diff },
            ])
        }
        Err(e) => {
            CommandResult::Continue(vec![
                ChatMessage::System { content: format!("Diff error: {}", e) },
            ])
        }
    }
}

async fn handle_review(args: &str, orchestrator: &mut Orchestrator) -> CommandResult {
    let working_dir = orchestrator.working_directory().to_path_buf();
    let diff = if args.is_empty() {
        crate::git::diff::all_uncommitted(&working_dir).await
    } else {
        crate::git::diff::commit_range(&working_dir, args).await
    };

    match diff {
        Ok(diff) if diff.is_empty() => {
            CommandResult::Continue(vec![
                ChatMessage::System { content: "No changes to review.".to_string() },
            ])
        }
        Ok(diff) => {
            // The review agent runs asynchronously — dispatched from app.rs
            // Return a system message; actual review runs via orchestrator task
            match orchestrator.run_review_agent(&diff).await {
                Ok(review) => CommandResult::Continue(vec![
                    ChatMessage::System { content: "Code review complete:".to_string() },
                    ChatMessage::Assistant {
                        content: review,
                        tool_calls: vec![],
                        is_streaming: false,
                    },
                ]),
                Err(e) => CommandResult::Continue(vec![
                    ChatMessage::System { content: format!("Review failed: {}", e) },
                ]),
            }
        }
        Err(e) => CommandResult::Continue(vec![
            ChatMessage::System { content: format!("Diff error: {}", e) },
        ]),
    }
}

async fn handle_commit(args: &str, orchestrator: &mut Orchestrator) -> CommandResult {
    let working_dir = orchestrator.working_directory().to_path_buf();
    let diff = match crate::git::diff::all_uncommitted(&working_dir).await {
        Ok(d) if d.is_empty() => {
            return CommandResult::Continue(vec![
                ChatMessage::System { content: "Nothing to commit.".to_string() },
            ]);
        }
        Ok(d) => d,
        Err(e) => {
            return CommandResult::Continue(vec![
                ChatMessage::System { content: format!("Git error: {}", e) },
            ]);
        }
    };

    let message = if !args.is_empty() {
        args.to_string()
    } else {
        match orchestrator.run_commit_agent(&diff).await {
            Ok(msg) => msg.trim_matches(|c| c == '"' || c == '`').to_string(),
            Err(e) => {
                return CommandResult::Continue(vec![
                    ChatMessage::System { content: format!("Commit agent error: {}", e) },
                ]);
            }
        }
    };

    // Phase 9d: show confirmation overlay
    // Phase 9c: auto-commit with message displayed
    match crate::git::commit::commit_all(&working_dir, &message).await {
        Ok(sha) => {
            orchestrator.refresh_git_context().await;
            CommandResult::Continue(vec![
                ChatMessage::System {
                    content: format!("✓ Committed: {} — {}", &sha[..8.min(sha.len())], message),
                },
            ])
        }
        Err(e) => CommandResult::Continue(vec![
            ChatMessage::System { content: format!("Commit failed: {}", e) },
        ]),
    }
}
```

### Session Command Handlers

```rust
fn handle_new(orchestrator: &mut Orchestrator) -> CommandResult {
    orchestrator.clear_history();
    let msg = if let Some(sid) = orchestrator.session_id() {
        format!("New session started: {}", &sid.as_str()[..8])
    } else {
        "New session started.".to_string()
    };
    CommandResult::Continue(vec![ChatMessage::System { content: msg }])
}

fn handle_fork(orchestrator: &mut Orchestrator) -> CommandResult {
    orchestrator.fork_session();
    let msg = if let Some(sid) = orchestrator.session_id() {
        format!("Forked to new session: {}", &sid.as_str()[..8])
    } else {
        "Session forked.".to_string()
    };
    CommandResult::Continue(vec![ChatMessage::System { content: msg }])
}

async fn handle_compact(args: &str, orchestrator: &mut Orchestrator) -> CommandResult {
    let prompt = if args.is_empty() { None } else { Some(args) };
    match orchestrator.compact_history(prompt).await {
        Ok(summary) => {
            let turns_after = orchestrator.turn_count();
            CommandResult::Continue(vec![
                ChatMessage::System {
                    content: format!("Compacted to {} turns. Summary: {}", turns_after, summary),
                },
            ])
        }
        Err(e) => CommandResult::Continue(vec![
            ChatMessage::System { content: format!("Compact failed: {}", e) },
        ]),
    }
}

fn handle_history(args: &str, orchestrator: &Orchestrator) -> CommandResult {
    let n: usize = args.parse().unwrap_or(10);
    let history = orchestrator.recent_history_display(n);
    CommandResult::Continue(vec![
        ChatMessage::System { content: history },
    ])
}

fn handle_export(args: &str, orchestrator: &Orchestrator) -> CommandResult {
    let path = if args.is_empty() {
        format!("session-{}.md", chrono::Utc::now().format("%Y%m%d-%H%M%S"))
    } else {
        args.to_string()
    };
    match orchestrator.export_session(&path) {
        Ok(_) => CommandResult::Continue(vec![
            ChatMessage::System { content: format!("Exported to {}", path) },
        ]),
        Err(e) => CommandResult::Continue(vec![
            ChatMessage::System { content: format!("Export failed: {}", e) },
        ]),
    }
}
```

### Config Command Handlers

```rust
fn handle_model(args: &str, orchestrator: &mut Orchestrator) -> CommandResult {
    if args.is_empty() {
        CommandResult::Continue(vec![
            ChatMessage::System { content: format!("Current model: {}", orchestrator.model()) },
        ])
    } else {
        orchestrator.set_model(args);
        CommandResult::Continue(vec![
            ChatMessage::System { content: format!("Model switched to: {}", args) },
        ])
    }
}

fn handle_personality(args: &str, orchestrator: &mut Orchestrator) -> CommandResult {
    if args.is_empty() {
        CommandResult::Continue(vec![
            ChatMessage::System {
                content: format!("Current personality: {:?}", orchestrator.personality()),
            },
        ])
    } else {
        match args.to_lowercase().as_str() {
            "friendly" => orchestrator.set_personality(crate::personality::Personality::Friendly),
            "pragmatic" => orchestrator.set_personality(crate::personality::Personality::Pragmatic),
            "none" => orchestrator.set_personality(crate::personality::Personality::None),
            _ => {
                return CommandResult::Continue(vec![
                    ChatMessage::System {
                        content: "Valid personalities: friendly, pragmatic, none".to_string(),
                    },
                ]);
            }
        }
        CommandResult::Continue(vec![
            ChatMessage::System { content: format!("Personality set to: {}", args) },
        ])
    }
}

fn handle_status(orchestrator: &Orchestrator) -> CommandResult {
    let status = format!(
        "Mode: {:?} | Model: {} | Turns: {}/{} | Personality: {:?}",
        orchestrator.mode(),
        orchestrator.model(),
        orchestrator.turn_count(),
        orchestrator.context_window_turns(),
        orchestrator.personality(),
    );
    CommandResult::Continue(vec![
        ChatMessage::System { content: status },
    ])
}

fn handle_sandbox(orchestrator: &Orchestrator) -> CommandResult {
    let info = format!(
        "Sandbox: {:?} | Protected: {:?}",
        orchestrator.sandbox_mode(),
        orchestrator.protected_paths(),
    );
    CommandResult::Continue(vec![
        ChatMessage::System { content: info },
    ])
}
```

---

## 8. Modifications to Phase 9a/9b Files

### `tui/events.rs` — Add App Event Variants

```rust
use std::time::Duration;

#[derive(Debug)]
pub enum AppEvent {
    // ── Terminal Events (existing) ──
    Key(KeyEvent),
    Resize(u16, u16),
    Tick,

    // ── LLM Streaming Events (Phase 9c) ──
    TextDelta(String),
    StreamDone,

    // ── Tool Events (Phase 9c) ──
    ToolStart { name: String, args_display: String },
    ToolComplete { name: String, duration: Duration },
    ToolError { name: String, error: String },

    // ── Sub-Agent Events (Phase 9c) ──
    AgentStart { agent_type: String, task: String },
    AgentToolCall { agent_type: String },
    AgentComplete { agent_type: String, duration: Duration, tool_calls: usize },

    // ── System Events (Phase 9c) ──
    SystemMessage(String),
    ModeChanged(crate::mode::Mode),
    OrchestratorDone,
    Error(String),
}
```

### `tui/app.rs` — Major Additions

#### AppState — New Variants

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppState {
    Idle,
    CommandPicker { filter: String, selected: usize },
    Thinking,                              // Waiting for first LLM token
    Streaming,                             // LLM tokens arriving
    ToolExecuting { tool_name: String },   // A tool call is in progress
    Exiting,
}
```

#### App Struct — New Fields

```rust
use super::chat::{ChatMessage, ChatViewport};
use tokio::sync::mpsc;

pub struct App<'a> {
    pub state: AppState,
    pub tick_count: usize,
    pub status: StatusSnapshot,
    pub input_pane: InputPane<'a>,
    pub command_picker: CommandPicker,
    pub pending_input: Option<String>,

    // Phase 9c additions:
    pub messages: Vec<ChatMessage>,
    pub chat_viewport: ChatViewport,
    pub event_tx: mpsc::UnboundedSender<AppEvent>,
}
```

#### Orchestrator Dispatch

When `pending_input` is set after `Submit`, the event loop dispatches to the orchestrator on a separate tokio task:

```rust
// In the main event loop, after handling events:
if let Some(input) = app.pending_input.take() {
    if input.starts_with('/') {
        // Slash command — dispatch synchronously or on task
        let result = commands::dispatch(&input, &mut orchestrator).await;
        handle_command_result(&mut app, &mut orchestrator, result);
    } else if input.starts_with('!') {
        // Shell command
        let cmd = input[1..].trim();
        app.messages.push(ChatMessage::User { content: input.clone() });
        let output = execute_shell_command(cmd).await;
        app.messages.push(ChatMessage::System { content: output });
    } else {
        // Normal user input → send to orchestrator
        app.messages.push(ChatMessage::User { content: input.clone() });
        app.messages.push(ChatMessage::Assistant {
            content: String::new(),
            tool_calls: vec![],
            is_streaming: true,
        });
        app.state = AppState::Thinking;

        let tx = app.event_tx.clone();
        let cancel_flag = orchestrator.cancel_flag();

        // Spawn orchestrator work on a separate task
        tokio::spawn(async move {
            // The orchestrator is behind an Arc<Mutex> or similar
            // to allow concurrent access from the spawned task.
            // See "Orchestrator Access Pattern" below.
        });
    }
}
```

#### Orchestrator Access Pattern

The orchestrator needs to be accessible from both the main event loop (for slash commands, status updates) and the spawned streaming task. Two approaches:

**Option A: Arc<Mutex<Orchestrator>>** — Simple but blocks on mutex during streaming.

**Option B: Move orchestrator into spawned task, communicate via channels** — Recommended:

```rust
// The main loop sends user input to the orchestrator task via a channel.
// The orchestrator task sends AppEvents back.

let (input_tx, mut input_rx) = mpsc::unbounded_channel::<String>();

// Orchestrator task (long-lived):
tokio::spawn(async move {
    while let Some(input) = input_rx.recv().await {
        orchestrator.reset_cancel();

        match orchestrator.handle_user_input_streaming(&input, |event| {
            match event {
                StreamEvent::TextDelta(text) => {
                    let _ = event_tx.send(AppEvent::TextDelta(text));
                }
                StreamEvent::Done { .. } => {
                    let _ = event_tx.send(AppEvent::StreamDone);
                }
                StreamEvent::FunctionCall(_) => {
                    // Tool calls handled internally by orchestrator;
                    // we send ToolStart/ToolComplete events from
                    // execute_and_display_tool (which needs modification)
                }
            }
        }).await {
            Ok(_text) => {
                let _ = event_tx.send(AppEvent::OrchestratorDone);
            }
            Err(e) => {
                let _ = event_tx.send(AppEvent::Error(e.to_string()));
            }
        }

        // Send updated status after completion
        let status = StatusSnapshot::from_orchestrator(&orchestrator);
        // ... send status update event
    }
});
```

#### Event Handler Additions

```rust
// In the main event loop match:
AppEvent::TextDelta(text) => {
    app.state = AppState::Streaming;
    // Append to the last assistant message
    if let Some(ChatMessage::Assistant { content, .. }) = app.messages.last_mut() {
        content.push_str(&text);
    }
}
AppEvent::StreamDone => {
    if let Some(ChatMessage::Assistant { is_streaming, .. }) = app.messages.last_mut() {
        *is_streaming = false;
    }
}
AppEvent::ToolStart { name, args_display } => {
    app.state = AppState::ToolExecuting { tool_name: name.clone() };
    if let Some(ChatMessage::Assistant { tool_calls, .. }) = app.messages.last_mut() {
        tool_calls.push(ToolCallDisplay::Running {
            name,
            args_display,
            started_at: Instant::now(),
        });
    }
}
AppEvent::ToolComplete { name, duration } => {
    app.state = AppState::Thinking; // Back to thinking, waiting for next token
    if let Some(ChatMessage::Assistant { tool_calls, .. }) = app.messages.last_mut() {
        // Find the Running entry and replace with Completed
        if let Some(tc) = tool_calls.iter_mut().rev().find(|tc| matches!(tc,
            ToolCallDisplay::Running { name: n, .. } if n == &name
        )) {
            if let ToolCallDisplay::Running { args_display, .. } = tc {
                *tc = ToolCallDisplay::Completed {
                    name: name.clone(),
                    args_display: args_display.clone(),
                    duration,
                };
            }
        }
    }
}
AppEvent::OrchestratorDone => {
    app.state = AppState::Idle;
    // Refresh status snapshot
    // app.status = StatusSnapshot::from_orchestrator(&orchestrator);
}
AppEvent::Error(msg) => {
    app.state = AppState::Idle;
    app.messages.push(ChatMessage::System {
        content: format!("Error: {}", msg),
    });
}
AppEvent::SystemMessage(msg) => {
    app.messages.push(ChatMessage::System { content: msg });
}
```

#### Cancellation

```rust
// In handle_action:
Action::Cancel => {
    match &self.state {
        AppState::Thinking | AppState::Streaming | AppState::ToolExecuting { .. } => {
            // Set orchestrator cancel flag
            // cancel_flag.store(true, Ordering::SeqCst);
            self.state = AppState::Idle;
            if let Some(ChatMessage::Assistant { is_streaming, .. }) = self.messages.last_mut() {
                *is_streaming = false;
            }
            self.messages.push(ChatMessage::System {
                content: "Interrupted.".to_string(),
            });
        }
        AppState::CommandPicker { .. } => {
            self.state = AppState::Idle;
            self.input_pane.clear();
        }
        _ => {
            self.input_pane.clear();
        }
    }
}
```

### `tui/keybindings.rs` — New State Handlers

```rust
fn map_thinking(key: KeyEvent) -> Action {
    match (key.code, key.modifiers) {
        // Only Cancel is allowed during thinking
        _ => Action::Noop,
    }
}

fn map_streaming(key: KeyEvent) -> Action {
    match (key.code, key.modifiers) {
        // Allow scrolling during streaming
        (KeyCode::PageUp, _)   => Action::PageUp,
        (KeyCode::PageDown, _) => Action::PageDown,
        _ => Action::Noop,
    }
}

// Update map_key to route new states:
pub fn map_key(key: KeyEvent, state: &AppState) -> Action {
    // Global keys ...

    match state {
        AppState::Idle => map_idle(key),
        AppState::CommandPicker { .. } => map_picker(key),
        AppState::Thinking => map_thinking(key),
        AppState::Streaming => map_streaming(key),
        AppState::ToolExecuting { .. } => map_thinking(key), // Same as thinking
        _ => Action::Noop,
    }
}
```

### `tui/layout.rs` — Wire Chat Area

```rust
// Replace render_chat_placeholder with:
chat::render(frame, chat_area, &mut app);

// The full render function becomes:
pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    // ... size guard ...

    let input_height = app.input_pane.desired_height();
    let [header_area, chat_area, input_divider, input_area, status_divider, status_area] =
        Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Length(input_height),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);

    header::render(frame, header_area, app);
    chat::render(frame, chat_area, app);       // ← NEW
    render_divider(frame, input_divider);
    frame.render_widget(app.input_pane.textarea(), input_area);
    render_divider(frame, status_divider);
    status_bar::render(frame, status_area, app);

    // Thinking/streaming spinner in chat area
    if matches!(app.state, AppState::Thinking) {
        let spinner = SpinnerWidget {
            message: "Thinking...".to_string(),
            tick: app.tick_count,
        };
        // Render at the bottom of the chat area, above the last message
        let spinner_area = Rect::new(
            chat_area.x + 1,
            chat_area.bottom().saturating_sub(2),
            chat_area.width.saturating_sub(2),
            1,
        );
        frame.render_widget(&spinner, spinner_area);
    }

    // Command picker overlay (existing)
    if let AppState::CommandPicker { ref filter, selected } = app.state {
        let filter = filter.clone();
        app.command_picker.render(frame, &filter, selected, area, chat_area);
    }
}
```

### Shell Command Execution

```rust
/// Execute a shell command and capture output.
async fn execute_shell_command(cmd: &str) -> String {
    match tokio::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .await
    {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let mut result = String::new();
            if !stdout.is_empty() {
                result.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str(&stderr);
            }
            if result.is_empty() {
                "(no output)".to_string()
            } else {
                result
            }
        }
        Err(e) => format!("Shell error: {}", e),
    }
}
```

---

## 9. Streaming Integration

### Event Flow Diagram

```
                     User Input (Enter)
                           │
                           ▼
                    ┌──────────────┐
                    │  App::run()  │
                    │  main loop   │
                    └──────┬───────┘
                           │ input_tx.send(input)
                           ▼
              ┌──────────────────────────┐
              │ Orchestrator Task (tokio) │
              │                          │
              │  handle_user_input_      │
              │    streaming(input,      │
              │      |event| {           │
              │        tx.send(event)    │──► AppEvent::TextDelta
              │      })                  │──► AppEvent::ToolStart
              │                          │──► AppEvent::ToolComplete
              │  execute_and_display_    │──► AppEvent::AgentStart
              │    tool(name, args)      │──► AppEvent::AgentComplete
              │                          │──► AppEvent::StreamDone
              └──────────┬───────────────┘──► AppEvent::OrchestratorDone
                         │
                         ▼
              ┌──────────────────────────┐
              │ App main loop receives   │
              │ events via event_rx      │
              │                          │
              │ TextDelta → append to    │
              │   last assistant message │
              │                          │
              │ ToolStart → add Running  │
              │   to tool_calls vec      │
              │                          │
              │ ToolComplete → replace   │
              │   Running with Completed │
              │                          │
              │ OrchestratorDone →       │
              │   state = Idle           │
              └──────────────────────────┘
```

### Orchestrator Modification Required

The orchestrator's `execute_and_display_tool` method currently prints to stdout. For Phase 9c, it needs to send `AppEvent` values instead. The cleanest approach is to accept an event sender:

```rust
// In orchestrator, add a method or modify existing:
pub fn set_event_sender(&mut self, tx: mpsc::UnboundedSender<AppEvent>) {
    self.event_tx = Some(tx);
}

// In execute_and_display_tool, replace println! with:
if let Some(ref tx) = self.event_tx {
    let _ = tx.send(AppEvent::ToolStart {
        name: name.to_string(),
        args_display: format_tool_call(name, args),
    });
}
// ... execute tool ...
if let Some(ref tx) = self.event_tx {
    let _ = tx.send(AppEvent::ToolComplete {
        name: name.to_string(),
        duration: started.elapsed(),
    });
}
```

Alternatively, inject a callback trait that the TUI implements.

---

## 10. Interaction Model

### Normal User Input Flow

```
User types "What files are here?" and presses Enter
    │
    ▼
Submit → pending_input = "What files are here?"
    │
    ▼
Input sent to orchestrator task via channel
    │
    ▼
AppState::Thinking — spinner shown in chat
    │
    ▼
Orchestrator calls Gemini API (streaming)
    │
    ├── TextDelta("Here") → append to assistant message
    │   State → Streaming
    │
    ├── TextDelta(" are the") → append
    │
    ├── FunctionCall detected → orchestrator runs tool internally
    │   ├── ToolStart { name: "list_directory", args: "." }
    │   │   State → ToolExecuting
    │   │   Spinner shows in tool call line
    │   │
    │   └── ToolComplete { name: "list_directory", duration: 0.2s }
    │       State → Thinking (waiting for next response)
    │
    ├── TextDelta(" files:\n") → append
    │   State → Streaming
    │
    ├── StreamDone → mark assistant message as not streaming
    │
    └── OrchestratorDone → State = Idle
        Refresh StatusSnapshot (turn count updated)
```

### Slash Command Flow

```
User types "/compact" and presses Enter
    │
    ▼
Submit → pending_input = "/compact"
    │
    ▼
Detected as slash command (starts with '/')
    │
    ▼
commands::dispatch("/compact", &mut orchestrator)
    │
    ▼
handle_compact() → orchestrator.compact_history(None)
    │
    ▼
Returns CommandResult::Continue([SystemMessage("Compacted to 5 turns...")])
    │
    ▼
System message added to app.messages
    │
    ▼
StatusSnapshot refreshed (turn count updated)
```

### Shell Command Flow

```
User types "!ls -la" and presses Enter
    │
    ▼
Submit → pending_input = "!ls -la"
    │
    ▼
Detected as shell command (starts with '!')
    │
    ▼
User message added to chat: "!ls -la"
    │
    ▼
execute_shell_command("ls -la").await
    │
    ▼
Output captured → SystemMessage with output added to chat
```

### Cancellation Flow

```
Orchestrator is streaming (AppState::Streaming)
    │
    ▼
User presses Ctrl+C
    │
    ▼
Action::Cancel matched
    │
    ▼
cancel_flag.store(true) — orchestrator will stop at next check point
    │
    ▼
State → Idle
Last assistant message marked as not streaming
SystemMessage("Interrupted.") added to chat
```

---

## 11. Implementation Order

| Step | File | Why this order |
|------|------|----------------|
| 1 | `src/tui/events.rs` | Add new AppEvent variants (no compile errors, just enum additions) |
| 2 | `src/tui/spinner.rs` | Standalone widget, no dependencies on other new files |
| 3 | `src/tui/chat.rs` | ChatMessage model + ChatViewport + scroll logic + tests |
| 4 | `src/tui/message.rs` | Message rendering functions, depends on chat.rs types + theme |
| 5 | `src/tui/commands.rs` | Slash command dispatch, depends on ChatMessage type |
| 6 | `src/tui/mod.rs` | Add 4 new module declarations |
| 7 | `src/tui/keybindings.rs` | Wire scroll actions, add new state handlers |
| 8 | `src/tui/app.rs` | New states, messages vec, viewport, event handling, orchestrator dispatch |
| 9 | `src/tui/layout.rs` | Wire chat::render, spinner overlay |
| 10 | `src/agent/orchestrator.rs` | Add event sender for tool call notifications (optional: can use callback) |
| 11 | — | `cargo test && cargo clippy` |

---

## 12. Tests

### Test Summary

| File | # Tests | Coverage |
|------|---------|----------|
| `chat.rs` | 6 | Viewport: auto-scroll, manual scroll, scroll-to-top/bottom, lines_above |
| `message.rs` | 4 | User borders, system single-line, truncate short/long |
| `spinner.rs` | 1 | Frame cycling |
| `commands.rs` | 6 | parse_command, unknown command, /clear, /help, mode switch, quit |
| **Total** | **17** | |

### `commands.rs` Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_command_simple() {
        assert_eq!(parse_command("/help"), ("help", ""));
    }

    #[test]
    fn parse_command_with_args() {
        assert_eq!(parse_command("/model gemini-2"), ("model", "gemini-2"));
    }

    #[test]
    fn parse_command_with_extra_whitespace() {
        assert_eq!(parse_command("/compact  some prompt "), ("compact", "some prompt"));
    }

    #[test]
    fn parse_command_no_slash() {
        assert_eq!(parse_command("help"), ("help", ""));
    }
}
```

---

## 13. Verification Checklist

### Automated

```bash
cargo test          # All existing + ~17 new tests pass
cargo clippy        # No warnings
cargo fmt --check   # Formatted
```

### Manual

- [ ] Typing a message and pressing Enter shows it as a user message in the chat area
- [ ] After submitting, "Thinking..." spinner appears
- [ ] LLM response streams token-by-token into an assistant message block
- [ ] Streaming cursor indicator (▌) blinks at end of text
- [ ] When streaming completes, cursor disappears and state returns to Idle
- [ ] Tool calls show as spinning indicators during execution
- [ ] Tool calls show checkmark + duration when complete
- [ ] Tool errors show red ✗ with error message
- [ ] Sub-agent activity shows box-drawing header + progress + footer
- [ ] Multiple tool calls in a single response render correctly
- [ ] Chat auto-scrolls to bottom during streaming
- [ ] Page Up scrolls chat up (pauses auto-scroll)
- [ ] Page Down scrolls chat back down
- [ ] Scrolling to bottom re-enables auto-scroll
- [ ] "↑ N more" indicator appears when scrolled up
- [ ] Home scrolls to top, End scrolls to bottom
- [ ] Ctrl+C during streaming interrupts and shows "Interrupted."
- [ ] `/help` shows command list as system message
- [ ] `/clear` clears chat and shows confirmation
- [ ] `/mode explore` switches mode, system message shown, status bar updated
- [ ] `/diff` shows git diff in chat
- [ ] `/status` shows session info as system message
- [ ] `/compact` compacts history, shows result
- [ ] `/new` starts new session
- [ ] `!ls` executes shell command, output shown in chat
- [ ] Status bar turn count updates after each orchestrator interaction
- [ ] Status bar git info refreshes after `/commit`
- [ ] Multiple rapid submissions queue correctly

---

## Estimated Scope

| Metric | Value |
|--------|-------|
| New files | 4 |
| Modified files | 5 (+1 optional orchestrator) |
| New lines (est.) | ~1,400 |
| New tests | ~17 |
