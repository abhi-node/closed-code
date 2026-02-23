# Phase 9a: Foundation — App Shell, Layout & Status Bar

> Create the bare TUI skeleton: full-screen alternate screen, event loop, header bar, status bar with live data, and clean exit. Chat area and input are placeholders.

---

## Table of Contents

1. [Goal & Checkpoint](#1-goal--checkpoint)
2. [Dependency Changes](#2-dependency-changes)
3. [File Overview](#3-file-overview)
4. [tui/theme.rs — Tailwind Palette](#4-tuithemers--tailwind-palette)
5. [tui/events.rs — Event System](#5-tuieventsrs--event-system)
6. [tui/app.rs — Terminal Setup, State Machine & Event Loop](#6-tuiapprs--terminal-setup-state-machine--event-loop)
7. [tui/layout.rs — Root Layout](#7-tuilayoutrs--root-layout)
8. [tui/header.rs — Header Bar](#8-tuiheaderrs--header-bar)
9. [tui/gauge.rs — Context Usage Gauge](#9-tuigaugers--context-usage-gauge)
10. [tui/status_bar.rs — Status Bar](#10-tuistatus_barrs--status-bar)
11. [tui/mod.rs — Module Entry Point](#11-tuimodrs--module-entry-point)
12. [Modified Files](#12-modified-files)
13. [Architecture & Data Flow](#13-architecture--data-flow)
14. [Implementation Order](#14-implementation-order)
15. [Tests](#15-tests)
16. [Verification Checklist](#16-verification-checklist)

---

## 1. Goal & Checkpoint

**Phase 9a delivers:**

- A full-screen TUI in alternate screen mode
- A header bar with app name and session ID
- A status bar showing: mode badge (colored), model name, turn counter, context gauge, git info
- An empty chat area placeholder
- An empty input area placeholder with hint text
- Double-line (`═`) dividers between zones
- `Ctrl+D` exits cleanly (terminal restored)
- Panic hook restores terminal even on crash
- Graceful "terminal too small" message when window is < 40x10

**What is NOT in Phase 9a:**

- No text input (Phase 9b)
- No command picker (Phase 9b)
- No chat messages or streaming (Phase 9c)
- No approval overlays (Phase 9d)

---

## 2. Dependency Changes

### Cargo.toml

```toml
# Add to [dependencies]:
ratatui = { version = "0.29", features = ["crossterm"] }

# Update crossterm to add event-stream feature:
crossterm = { version = "0.28", features = ["event-stream"] }
```

The `event-stream` feature enables `crossterm::event::EventStream` for async-compatible event reading, avoiding blocking the tokio runtime. The existing `futures = "0.3"` dependency provides the `StreamExt` trait needed by `EventStream`.

**No other dependency changes in Phase 9a.** `tui-textarea` is added in Phase 9b.

---

## 3. File Overview

### New Files (8)

| File | Purpose | Est. Lines |
|------|---------|-----------|
| `src/tui/mod.rs` | Module declarations + `run_tui()` entry point | ~20 |
| `src/tui/theme.rs` | Tailwind palette constants + mode helpers | ~80 |
| `src/tui/events.rs` | `AppEvent` enum, crossterm EventStream + tick timer | ~65 |
| `src/tui/app.rs` | `App` struct, `AppState`, `StatusSnapshot`, terminal setup, event loop | ~200 |
| `src/tui/layout.rs` | Root vertical layout with 6 zones + placeholders | ~100 |
| `src/tui/header.rs` | Header bar: app name (left) + session ID (right) | ~45 |
| `src/tui/gauge.rs` | Context usage gauge widget (filled/empty blocks + %) | ~60 |
| `src/tui/status_bar.rs` | Mode badge, model, turns, gauge, git info | ~140 |

### Modified Files (3)

| File | Change |
|------|--------|
| `Cargo.toml` | Add `ratatui`, update `crossterm` features |
| `src/lib.rs` | Add `pub mod tui;` |
| `src/main.rs` | Replace `run_repl(&config)` with `tui::run_tui(&config)` in the `None` arm |

---

## 4. `tui/theme.rs` — Tailwind Palette

Centralized color theme using ratatui's built-in Tailwind CSS palette. This replaces `ui::theme::Theme` (which uses crossterm colors) for all TUI rendering.

```rust
use ratatui::style::Color;
use ratatui::style::palette::tailwind;

pub struct TuiTheme;

impl TuiTheme {
    // ── Base ──
    pub const BG: Color = tailwind::SLATE.c950;
    pub const FG: Color = tailwind::SLATE.c200;
    pub const FG_DIM: Color = tailwind::SLATE.c500;
    pub const FG_MUTED: Color = tailwind::SLATE.c600;

    // ── Borders ──
    pub const BORDER: Color = tailwind::SLATE.c700;
    pub const BORDER_FOCUS: Color = tailwind::BLUE.c400;
    pub const BORDER_DIM: Color = tailwind::SLATE.c800;

    // ── Accents ──
    pub const ACCENT: Color = tailwind::BLUE.c400;

    // ── Semantic ──
    pub const SUCCESS: Color = tailwind::EMERALD.c400;
    pub const WARNING: Color = tailwind::AMBER.c400;
    pub const ERROR: Color = tailwind::RED.c400;

    // ── Message Roles (used in Phase 9c) ──
    pub const USER: Color = tailwind::CYAN.c400;
    pub const USER_BG: Color = tailwind::CYAN.c950;
    pub const ASSISTANT: Color = tailwind::VIOLET.c400;
    pub const TOOL: Color = tailwind::AMBER.c400;
    pub const AGENT: Color = tailwind::TEAL.c400;

    // ── Diff (used in Phase 9d) ──
    pub const DIFF_ADD: Color = tailwind::EMERALD.c400;
    pub const DIFF_ADD_BG: Color = tailwind::EMERALD.c950;
    pub const DIFF_DEL: Color = tailwind::RED.c400;
    pub const DIFF_DEL_BG: Color = tailwind::RED.c950;
    pub const DIFF_HUNK: Color = tailwind::BLUE.c400;

    // ── Mode Colors ──
    pub const MODE_EXPLORE: Color = tailwind::BLUE.c400;
    pub const MODE_PLAN: Color = tailwind::VIOLET.c400;
    pub const MODE_GUIDED: Color = tailwind::AMBER.c400;
    pub const MODE_EXECUTE: Color = tailwind::EMERALD.c400;
    pub const MODE_AUTO: Color = tailwind::RED.c400;

    // ── Gauge ──
    pub const GAUGE_LOW: Color = tailwind::EMERALD.c400;   // 0-60%
    pub const GAUGE_MED: Color = tailwind::AMBER.c400;     // 60-85%
    pub const GAUGE_HIGH: Color = tailwind::RED.c400;       // 85-100%

    // ── Command Picker (used in Phase 9b) ──
    pub const PICKER_HIGHLIGHT_BG: Color = tailwind::BLUE.c800;
    pub const PICKER_HIGHLIGHT_FG: Color = tailwind::SLATE.c100;
    pub const PICKER_MATCH: Color = tailwind::AMBER.c400;

    // ── Spinner Frames ──
    pub const SPINNER_FRAMES: &'static [&'static str] =
        &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
}
```

### Helper Functions

```rust
use crate::mode::Mode;

/// Return the theme color for a given Mode.
pub fn mode_color(mode: &Mode) -> Color {
    match mode {
        Mode::Explore => TuiTheme::MODE_EXPLORE,
        Mode::Plan    => TuiTheme::MODE_PLAN,
        Mode::Guided  => TuiTheme::MODE_GUIDED,
        Mode::Execute => TuiTheme::MODE_EXECUTE,
        Mode::Auto    => TuiTheme::MODE_AUTO,
    }
}

/// Return the uppercase label for a given Mode (status bar badge).
pub fn mode_label(mode: &Mode) -> &'static str {
    match mode {
        Mode::Explore => "EXPLORE",
        Mode::Plan    => "PLAN",
        Mode::Guided  => "GUIDED",
        Mode::Execute => "EXECUTE",
        Mode::Auto    => "AUTO",
    }
}
```

### Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_colors_are_distinct() {
        let colors = [
            mode_color(&Mode::Explore),
            mode_color(&Mode::Plan),
            mode_color(&Mode::Guided),
            mode_color(&Mode::Execute),
            mode_color(&Mode::Auto),
        ];
        for (i, a) in colors.iter().enumerate() {
            for (j, b) in colors.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b);
                }
            }
        }
    }

    #[test]
    fn mode_labels_are_uppercase() {
        for mode in [Mode::Explore, Mode::Plan, Mode::Guided, Mode::Execute, Mode::Auto] {
            let label = mode_label(&mode);
            assert_eq!(label, label.to_uppercase());
        }
    }
}
```

---

## 5. `tui/events.rs` — Event System

Defines `AppEvent` and a spawned task that merges crossterm terminal events with a periodic tick timer into a single `mpsc` channel.

### AppEvent Enum

```rust
use crossterm::event::KeyEvent;

/// Application-level events consumed by the main event loop.
///
/// Phase 9a uses Key, Resize, and Tick.
/// Later phases add TextDelta, ToolStart, ApprovalRequest, etc.
#[derive(Debug)]
pub enum AppEvent {
    /// A key press from the terminal.
    Key(KeyEvent),
    /// Terminal resize (new columns, new rows).
    Resize(u16, u16),
    /// Periodic tick for animations (spinner frame advance).
    Tick,
}
```

### Event Loop Spawner

Uses `crossterm::event::EventStream` (async-compatible) to avoid blocking tokio.

```rust
use std::time::Duration;
use crossterm::event::{Event as CrosstermEvent, EventStream};
use futures::StreamExt;
use tokio::sync::mpsc;

pub const TICK_RATE: Duration = Duration::from_millis(100);

/// Spawn a background task that polls crossterm events and a tick timer,
/// sending `AppEvent` values into the returned receiver.
///
/// The task exits when the receiver is dropped.
pub fn spawn_event_loop() -> mpsc::UnboundedReceiver<AppEvent> {
    let (tx, rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        let mut reader = EventStream::new();
        let mut tick = tokio::time::interval(TICK_RATE);

        loop {
            let event = tokio::select! {
                maybe_event = reader.next() => {
                    match maybe_event {
                        Some(Ok(CrosstermEvent::Key(key))) => Some(AppEvent::Key(key)),
                        Some(Ok(CrosstermEvent::Resize(w, h))) => Some(AppEvent::Resize(w, h)),
                        Some(Ok(_)) => None,   // Mouse events ignored in 9a
                        Some(Err(_)) => None,   // Read error, skip
                        None => break,          // Stream ended
                    }
                }
                _ = tick.tick() => {
                    Some(AppEvent::Tick)
                }
            };

            if let Some(ev) = event {
                if tx.send(ev).is_err() {
                    break; // Receiver dropped
                }
            }
        }
    });

    rx
}
```

### Event Flow Diagram

```
 crossterm::EventStream ─────────┐
     (Key, Resize, Mouse)        │
                                 ├──► mpsc::UnboundedReceiver<AppEvent>
 tokio::time::interval(100ms) ──┘         │
     (Tick)                                ▼
                                   app::run() main loop
                                      │
                            ┌─────────┴──────────┐
                            ▼                    ▼
                     app.update(event)    terminal.draw(render)
```

---

## 6. `tui/app.rs` — Terminal Setup, State Machine & Event Loop

The core of the TUI. Owns `App` state, sets up / restores the terminal, runs the main event loop.

### AppState

```rust
/// Application state machine.
///
/// Phase 9a: only Idle and Exiting.
/// Phase 9b adds: CommandPicker.
/// Phase 9c adds: Thinking, Streaming, ToolExecuting.
/// Phase 9d adds: AwaitingApproval, DiffView, SessionPicker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppState {
    Idle,
    Exiting,
}
```

### StatusSnapshot

Extracted from the Orchestrator before each render to decouple rendering from business logic.

```rust
use crate::mode::Mode;
use crate::session::SessionId;
use crate::ui::usage::SessionUsage;

/// Snapshot of orchestrator state needed for rendering.
pub struct StatusSnapshot {
    pub mode: Mode,
    pub model: String,
    pub turn_count: usize,
    pub context_window_turns: usize,
    pub session_id: Option<SessionId>,
    pub git_branch: Option<String>,
    pub git_change_count: usize,
    pub git_is_clean: bool,
}
```

#### Construction from Orchestrator

The Orchestrator exposes `git_summary() -> String` in one of three formats:
- `"main (clean)"`
- `"feature (3 uncommitted changes)"`
- `"not a git repository"`

We parse this for the status bar fields:

```rust
use crate::agent::orchestrator::Orchestrator;

impl StatusSnapshot {
    pub fn from_orchestrator(orch: &Orchestrator) -> Self {
        let summary = orch.git_summary();
        let (git_branch, git_change_count, git_is_clean) =
            parse_git_summary(&summary);

        Self {
            mode: *orch.mode(),
            model: orch.model().to_string(),
            turn_count: orch.turn_count(),
            context_window_turns: orch.context_window_turns(),
            session_id: orch.session_id().cloned(),
            git_branch,
            git_change_count,
            git_is_clean,
        }
    }
}

/// Parse `GitContext::summary()` into structured fields.
fn parse_git_summary(summary: &str) -> (Option<String>, usize, bool) {
    if summary == "not a git repository" {
        return (None, 0, true);
    }
    let branch = summary.split(' ').next().map(String::from);
    let is_clean = summary.contains("(clean)");
    let change_count = if is_clean {
        0
    } else {
        summary
            .split('(')
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0)
    };
    (branch, change_count, is_clean)
}
```

### App Struct

```rust
pub struct App {
    pub state: AppState,
    pub tick_count: usize,
    pub status: StatusSnapshot,
}
```

### Terminal Setup & Restore

```rust
use ratatui::DefaultTerminal;

fn setup_terminal() -> anyhow::Result<DefaultTerminal> {
    // Install panic hook that restores terminal before printing panic info.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = ratatui::restore();
        original_hook(panic_info);
    }));

    let terminal = ratatui::init();
    Ok(terminal)
}

fn restore_terminal() {
    let _ = ratatui::restore();
}
```

**Panic hook chaining:** `main.rs` installs a hook that calls `disable_raw_mode()`. Our `setup_terminal()` calls `take_hook()` to capture it, then installs a new hook that calls `ratatui::restore()` (which does `disable_raw_mode` + `LeaveAlternateScreen`) and then chains to the original. This means both hooks fire on panic — the ratatui restore first, then the original as a harmless no-op.

### Main `run()` Function

```rust
use std::sync::Arc;
use crossterm::event::{KeyCode, KeyModifiers};
use crate::config::Config;
use crate::gemini::GeminiClient;
use crate::sandbox::create_sandbox;
use crate::session::store::SessionStore;
use crate::ui::approval::DiffOnlyApprovalHandler;

use super::events::{self, AppEvent};
use super::layout;

pub async fn run(config: &Config) -> anyhow::Result<()> {
    // ── Build Orchestrator (mirrors run_repl setup) ──
    let client = Arc::new(GeminiClient::new(
        config.api_key.clone(),
        config.model.clone(),
    ));
    let sandbox = create_sandbox(config.sandbox_mode, config.working_directory.clone());
    let approval_handler: Arc<dyn crate::ui::approval::ApprovalHandler> =
        Arc::new(DiffOnlyApprovalHandler::new());

    let mut orchestrator = Orchestrator::new(
        client, config.mode, config.working_directory.clone(),
        config.max_output_tokens, approval_handler,
        config.personality, config.context_window_turns,
        sandbox, config.protected_paths.clone(),
    );
    orchestrator.detect_git_context().await;

    if config.session_auto_save {
        let store = SessionStore::new(config.sessions_dir.clone());
        orchestrator.start_session(store);
    }

    // ── Terminal setup ──
    let mut terminal = setup_terminal()?;

    // ── App state ──
    let mut app = App {
        state: AppState::Idle,
        tick_count: 0,
        status: StatusSnapshot::from_orchestrator(&orchestrator),
    };

    // ── Event loop ──
    let mut event_rx = events::spawn_event_loop();

    loop {
        terminal.draw(|frame| layout::render(frame, &app))?;

        let Some(event) = event_rx.recv().await else {
            break;
        };

        match event {
            AppEvent::Key(key) => {
                match (key.code, key.modifiers) {
                    (KeyCode::Char('d'), m) if m.contains(KeyModifiers::CONTROL) => {
                        app.state = AppState::Exiting;
                    }
                    (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => {
                        // Phase 9a: Ctrl+C also exits (no input to clear yet)
                        app.state = AppState::Exiting;
                    }
                    _ => {} // No input handling in Phase 9a
                }
            }
            AppEvent::Resize(_w, _h) => {
                // Ratatui handles resize on next draw automatically.
            }
            AppEvent::Tick => {
                app.tick_count = app.tick_count.wrapping_add(1);
            }
        }

        if app.state == AppState::Exiting {
            break;
        }
    }

    // ── Cleanup ──
    orchestrator.emit_event(crate::session::SessionEvent::SessionEnd {
        timestamp: chrono::Utc::now(),
    });
    restore_terminal();
    Ok(())
}
```

### Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_git_summary_clean() {
        let (branch, count, clean) = parse_git_summary("main (clean)");
        assert_eq!(branch, Some("main".to_string()));
        assert_eq!(count, 0);
        assert!(clean);
    }

    #[test]
    fn parse_git_summary_changes() {
        let (branch, count, clean) = parse_git_summary("feature (3 uncommitted changes)");
        assert_eq!(branch, Some("feature".to_string()));
        assert_eq!(count, 3);
        assert!(!clean);
    }

    #[test]
    fn parse_git_summary_single_change() {
        let (branch, count, clean) = parse_git_summary("main (1 uncommitted change)");
        assert_eq!(branch, Some("main".to_string()));
        assert_eq!(count, 1);
        assert!(!clean);
    }

    #[test]
    fn parse_git_summary_not_repo() {
        let (branch, count, clean) = parse_git_summary("not a git repository");
        assert_eq!(branch, None);
        assert_eq!(count, 0);
        assert!(clean);
    }

    #[test]
    fn app_state_idle_is_not_exiting() {
        assert_ne!(AppState::Idle, AppState::Exiting);
    }
}
```

---

## 7. `tui/layout.rs` — Root Layout

Divides the terminal into 6 vertical zones and delegates to sub-widget renderers.

### Visual Structure

```
┌──────────────────────────────────────────────┐  ← header (1 line)
│                                              │
│            chat area (Fill)                   │
│                                              │
╞══════════════════════════════════════════════╡  ← divider (1 line)
│  Type a message, / for commands              │  ← input (3 lines)
╞══════════════════════════════════════════════╡  ← divider (1 line)
│ EXPLORE │ gemini-3.1-pro │ 2/50 │ ██░░ 4% │ │  ← status (1 line)
└──────────────────────────────────────────────┘
```

### Implementation

```rust
use ratatui::prelude::*;
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::Paragraph;

use super::app::App;
use super::{header, status_bar};
use super::theme::TuiTheme;

const MIN_WIDTH: u16 = 40;
const MIN_HEIGHT: u16 = 10;

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let msg = Paragraph::new("Terminal too small.\nResize to at least 40x10.")
            .style(Style::default().fg(TuiTheme::WARNING))
            .alignment(Alignment::Center);
        frame.render_widget(msg, area);
        return;
    }

    let [header_area, chat_area, input_divider, input_area, status_divider, status_area] =
        Layout::vertical([
            Constraint::Length(1),   // Header bar
            Constraint::Fill(1),     // Chat area
            Constraint::Length(1),   // ══ divider
            Constraint::Length(3),   // Input area (Phase 9a: placeholder)
            Constraint::Length(1),   // ══ divider
            Constraint::Length(1),   // Status bar
        ])
        .areas(area);

    header::render(frame, header_area, app);
    render_chat_placeholder(frame, chat_area);
    render_divider(frame, input_divider);
    render_input_placeholder(frame, input_area);
    render_divider(frame, status_divider);
    status_bar::render(frame, status_area, app);
}

/// Empty chat area with subtle side borders.
fn render_chat_placeholder(frame: &mut Frame, area: Rect) {
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::LEFT | ratatui::widgets::Borders::RIGHT)
        .border_style(Style::default().fg(TuiTheme::BORDER_DIM));
    frame.render_widget(block, area);
}

/// Placeholder input with hint text.
fn render_input_placeholder(frame: &mut Frame, area: Rect) {
    let hint = Paragraph::new("  Type a message, / for commands")
        .style(Style::default().fg(TuiTheme::FG_MUTED));
    frame.render_widget(hint, area);
}

/// Double-line horizontal divider.
fn render_divider(frame: &mut Frame, area: Rect) {
    let line = "═".repeat(area.width as usize);
    let divider = Paragraph::new(line)
        .style(Style::default().fg(TuiTheme::BORDER_DIM));
    frame.render_widget(divider, area);
}
```

### Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_size_constants() {
        assert_eq!(MIN_WIDTH, 40);
        assert_eq!(MIN_HEIGHT, 10);
    }
}
```

---

## 8. `tui/header.rs` — Header Bar

Single-line bar: app name on the left, session ID on the right.

```
 closed-code                              session: a1b2c3d4
```

### Implementation

```rust
use ratatui::prelude::*;
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::Paragraph;

use super::app::App;
use super::theme::TuiTheme;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let [left, right] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(22),
    ])
    .areas(area);

    // App name
    let title = Paragraph::new(Span::styled(
        " closed-code",
        Style::default().fg(TuiTheme::ACCENT).add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(title, left);

    // Session ID (first 8 chars)
    if let Some(ref sid) = app.status.session_id {
        let id_str = sid.as_str();
        let short = if id_str.len() >= 8 { &id_str[..8] } else { id_str };
        let session = Paragraph::new(Span::styled(
            format!("session: {} ", short),
            Style::default().fg(TuiTheme::FG_DIM),
        ))
        .alignment(Alignment::Right);
        frame.render_widget(session, right);
    }
}
```

---

## 9. `tui/gauge.rs` — Context Usage Gauge

Visual progress bar showing context window usage as filled/empty block characters with a percentage label.

```
 ████████░░ 78%
```

### Implementation

```rust
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use super::theme::TuiTheme;

const BAR_WIDTH: usize = 10;

pub fn render(frame: &mut Frame, area: Rect, used: usize, total: usize) {
    let (ratio, percent) = if total > 0 {
        let r = used as f64 / total as f64;
        (r, (r * 100.0).round().min(100.0) as u16)
    } else {
        (0.0, 0)
    };

    let filled = ((ratio * BAR_WIDTH as f64).round() as usize).min(BAR_WIDTH);
    let empty = BAR_WIDTH - filled;
    let color = gauge_color(ratio);

    let line = Line::from(vec![
        Span::raw(" "),
        Span::styled("█".repeat(filled), Style::default().fg(color)),
        Span::styled("░".repeat(empty), Style::default().fg(TuiTheme::BORDER_DIM)),
        Span::styled(format!(" {}%", percent), Style::default().fg(color)),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}

/// Determine gauge color based on fill ratio.
pub fn gauge_color(ratio: f64) -> Color {
    if ratio >= 0.85 {
        TuiTheme::GAUGE_HIGH
    } else if ratio >= 0.60 {
        TuiTheme::GAUGE_MED
    } else {
        TuiTheme::GAUGE_LOW
    }
}
```

### Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gauge_color_thresholds() {
        assert_eq!(gauge_color(0.0), TuiTheme::GAUGE_LOW);
        assert_eq!(gauge_color(0.59), TuiTheme::GAUGE_LOW);
        assert_eq!(gauge_color(0.60), TuiTheme::GAUGE_MED);
        assert_eq!(gauge_color(0.84), TuiTheme::GAUGE_MED);
        assert_eq!(gauge_color(0.85), TuiTheme::GAUGE_HIGH);
        assert_eq!(gauge_color(1.0), TuiTheme::GAUGE_HIGH);
    }
}
```

---

## 10. `tui/status_bar.rs` — Status Bar

The most complex widget in Phase 9a. Single-line, 9-segment horizontal layout:

```
 EXPLORE │ gemini-3.1-pro │ 42/50 turns │ ████████░░ 78% │ main ▲3
```

### Implementation

```rust
use ratatui::prelude::*;
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::Paragraph;

use super::app::App;
use super::gauge;
use super::theme::{self, TuiTheme};

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let status = &app.status;

    let [mode_area, s1, model_area, s2, turns_area, s3, gauge_area, s4, git_area] =
        Layout::horizontal([
            Constraint::Length(10),
            Constraint::Length(1),
            Constraint::Length(18),
            Constraint::Length(1),
            Constraint::Length(14),
            Constraint::Length(1),
            Constraint::Length(18),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(area);

    // Mode badge
    render_mode_badge(frame, mode_area, &status.mode);

    // Separators
    let sep = Paragraph::new("│").style(Style::default().fg(TuiTheme::BORDER_DIM));
    for area in [s1, s2, s3, s4] {
        frame.render_widget(sep.clone(), area);
    }

    // Model name (truncated to 16 chars)
    let model_display = truncate(&status.model, 16);
    frame.render_widget(
        Paragraph::new(format!(" {}", model_display))
            .style(Style::default().fg(TuiTheme::FG_DIM)),
        model_area,
    );

    // Turn counter
    render_turn_counter(frame, turns_area, status.turn_count, status.context_window_turns);

    // Context gauge
    gauge::render(frame, gauge_area, status.turn_count, status.context_window_turns);

    // Git info
    render_git_info(frame, git_area, status);
}

fn render_mode_badge(frame: &mut Frame, area: Rect, mode: &crate::mode::Mode) {
    let color = theme::mode_color(mode);
    let label = theme::mode_label(mode);
    let padded = format!(" {:^8}", label);
    frame.render_widget(
        Paragraph::new(padded).style(
            Style::default()
                .fg(TuiTheme::BG)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ),
        area,
    );
}

fn render_turn_counter(frame: &mut Frame, area: Rect, turns: usize, max: usize) {
    let ratio = if max > 0 { turns as f64 / max as f64 } else { 0.0 };
    let color = if ratio >= 0.95 {
        TuiTheme::ERROR
    } else if ratio >= 0.80 {
        TuiTheme::WARNING
    } else {
        TuiTheme::FG
    };
    frame.render_widget(
        Paragraph::new(format!(" {}/{} turns", turns, max))
            .style(Style::default().fg(color)),
        area,
    );
}

fn render_git_info(frame: &mut Frame, area: Rect, status: &super::app::StatusSnapshot) {
    let line = match &status.git_branch {
        Some(branch) if status.git_is_clean => Line::from(vec![
            Span::styled(format!(" {}", branch), Style::default().fg(TuiTheme::FG_DIM)),
            Span::styled(" ✓", Style::default().fg(TuiTheme::SUCCESS)),
        ]),
        Some(branch) => Line::from(vec![
            Span::styled(format!(" {}", branch), Style::default().fg(TuiTheme::FG_DIM)),
            Span::styled(format!(" ▲{}", status.git_change_count), Style::default().fg(TuiTheme::WARNING)),
        ]),
        None => Line::from(""),
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else if max > 3 {
        format!("{}...", &s[..max - 3])
    } else {
        s[..max].to_string()
    }
}
```

### Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_exact() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn truncate_long() {
        assert_eq!(truncate("gemini-3.1-pro-preview", 16), "gemini-3.1-pr...");
    }
}
```

---

## 11. `tui/mod.rs` — Module Entry Point

```rust
pub mod app;
pub mod events;
pub mod gauge;
pub mod header;
pub mod layout;
pub mod status_bar;
pub mod theme;

use crate::config::Config;

/// Launch the full-screen TUI application.
///
/// Replaces `run_repl()` as the default interactive entry point.
pub async fn run_tui(config: &Config) -> anyhow::Result<()> {
    app::run(config).await
}
```

---

## 12. Modified Files

### `src/lib.rs`

Add one line after `pub mod tool;`:

```rust
pub mod tui;
```

### `src/main.rs`

Change the `None` arm to call `run_tui`:

```rust
// Before:
None => {
    run_repl(&config).await?;
}

// After:
None => {
    closed_code::tui::run_tui(&config).await?;
}
```

Update the import at the top — add `use closed_code::tui;` or use the fully-qualified path. The `run_repl` import stays for the `Ask` and `Resume` commands.

### `Cargo.toml`

```toml
# Change:
crossterm = "0.28"
# To:
crossterm = { version = "0.28", features = ["event-stream"] }

# Add:
ratatui = { version = "0.29", features = ["crossterm"] }
```

---

## 13. Architecture & Data Flow

### Render Pipeline

```
layout::render(frame, &app)
  ├── terminal too small? → warning message, return
  ├── Layout::vertical → 6 areas
  ├── header::render(frame, header_area, &app)
  │     ├── Layout::horizontal → [left, right]
  │     ├── "closed-code" (ACCENT, bold) → left
  │     └── "session: XXXXXXXX" (FG_DIM) → right
  ├── render_chat_placeholder (side borders only)
  ├── render_divider ("═══...═══")
  ├── render_input_placeholder ("Type a message...")
  ├── render_divider ("═══...═══")
  └── status_bar::render(frame, status_area, &app)
        ├── Layout::horizontal → 9 segments
        ├── mode badge (colored bg, bold text)
        ├── "│" separators
        ├── model name (truncated)
        ├── turn counter (color warns near limit)
        ├── gauge::render (filled/empty blocks + %)
        └── git info (branch + ✓/▲N)
```

### State → Render Decoupling

```
Orchestrator
    │ (accessor methods)
    ▼
StatusSnapshot::from_orchestrator()
    │ (struct with plain data)
    ▼
App { state, tick_count, status }
    │ (passed by &App reference)
    ▼
layout::render() → header, status_bar, etc.
```

Key principle: **renderers never touch the Orchestrator directly.** They only read `&App`, which contains a `StatusSnapshot` with plain data. This makes rendering pure and testable.

---

## 14. Implementation Order

Build in this order to keep the project compilable at each step:

| Step | File | Why this order |
|------|------|----------------|
| 1 | `Cargo.toml` | Add ratatui dependency, enable event-stream |
| 2 | `src/tui/theme.rs` | No dependencies on other new files |
| 3 | `src/tui/events.rs` | Depends only on crossterm + tokio |
| 4 | `src/tui/gauge.rs` | Depends only on theme.rs |
| 5 | `src/tui/app.rs` (structs only) | Define AppState, StatusSnapshot, App — no `run()` yet |
| 6 | `src/tui/header.rs` | Depends on theme.rs, app.rs (StatusSnapshot) |
| 7 | `src/tui/status_bar.rs` | Depends on theme.rs, gauge.rs, app.rs |
| 8 | `src/tui/layout.rs` | Depends on header.rs, status_bar.rs, theme.rs |
| 9 | `src/tui/app.rs` (complete) | Add `run()` — depends on events.rs, layout.rs |
| 10 | `src/tui/mod.rs` | Wire all modules + `run_tui()` |
| 11 | `src/lib.rs` | Add `pub mod tui;` |
| 12 | `src/main.rs` | Switch `None` arm to `run_tui` |
| 13 | `cargo test` | All existing + new tests pass |
| 14 | `cargo run` | Manual verification (see checklist) |

---

## 15. Tests

### Test Summary

| File | # Tests | Coverage |
|------|---------|----------|
| `theme.rs` | 2 | Mode colors distinct, labels uppercase |
| `events.rs` | 1 | Tick rate constant value |
| `app.rs` | 5 | Git summary parsing (clean, changes, 1 change, not repo), state inequality |
| `gauge.rs` | 1 (6 asserts) | Color thresholds at boundaries |
| `status_bar.rs` | 3 | Truncation: short, exact, long |
| **Total** | **12** | |

All tests are pure unit tests using `#[cfg(test)]` in-module. No integration tests require a terminal.

---

## 16. Verification Checklist

### Automated

```bash
cargo test          # All existing tests pass, 12 new tests pass
cargo clippy        # No warnings
cargo fmt --check   # Formatted
```

### Manual

- [ ] `cargo run` launches full-screen TUI in alternate screen
- [ ] Header shows "closed-code" in blue, bold
- [ ] Header shows "session: XXXXXXXX" on the right (if session auto-save enabled)
- [ ] Status bar shows mode badge with correct color background
- [ ] Status bar shows model name (truncated if needed)
- [ ] Status bar shows turn counter (0/50 at start)
- [ ] Status bar shows context gauge (0% at start, green)
- [ ] Status bar shows git branch + ✓ or ▲N
- [ ] Chat area is empty with subtle side borders
- [ ] Input area shows "Type a message, / for commands" in muted color
- [ ] Double-line dividers (═) separate zones
- [ ] `Ctrl+D` exits cleanly, terminal fully restored
- [ ] `Ctrl+C` exits cleanly
- [ ] Resize terminal → layout reflows correctly
- [ ] Resize to < 40x10 → shows "Terminal too small" warning
- [ ] Force a panic (e.g., via test code) → terminal is still restored

---

## Estimated Scope

| Metric | Value |
|--------|-------|
| New files | 8 |
| Modified files | 3 |
| New lines (est.) | ~710 |
| New tests | 12 |
