# Phase 9d: Overlays — Approval, Diff Viewer, Session Picker, Mode Picker

> Replace terminal-blocking `dialoguer` prompts with non-blocking TUI overlays for file approval, diff review, session resumption, and plan acceptance.

**Prerequisite:** Phase 9c (chat area, streaming & command dispatch).

---

## Table of Contents

1. [Goal & Checkpoint](#1-goal--checkpoint)
2. [Dependency Changes](#2-dependency-changes)
3. [File Overview](#3-file-overview)
4. [tui/approval_overlay.rs — Approval Overlay](#4-tuiapproval_overlayrs--approval-overlay)
5. [tui/diff_view.rs — Full-Screen Diff Viewer](#5-tuidiff_viewrs--full-screen-diff-viewer)
6. [tui/session_picker.rs — Session Picker Overlay](#6-tuisession_pickerrs--session-picker-overlay)
7. [tui/mode_picker.rs — Mode Picker Overlay](#7-tuimode_pickerrs--mode-picker-overlay)
8. [tui/tui_approval_handler.rs — TUI ApprovalHandler](#8-tuitui_approval_handlerrs--tui-approvalhandler)
9. [Modifications to Phase 9a/9b/9c Files](#9-modifications-to-phase-9a9b9c-files)
10. [Interaction Model](#10-interaction-model)
11. [Implementation Order](#11-implementation-order)
12. [Tests](#12-tests)
13. [Verification Checklist](#13-verification-checklist)

---

## 1. Goal & Checkpoint

**Phase 9d delivers:**

- **Approval overlay** — When the LLM writes/edits a file in Guided mode, a modal overlay shows the proposed change with colorized inline diff, file path, change summary (N additions, M deletions), and key hints (y/n/d/Esc)
- **Full-screen diff viewer** — Pressing `d` from the approval overlay opens a full-screen scrollable diff with vim-style navigation (j/k, Ctrl+d/u, gg/G, q to return)
- **TuiApprovalHandler** — Implements `ApprovalHandler` trait for the TUI context; sends approval requests to the UI thread via channel, waits for response via `oneshot`
- **Session picker overlay** — `/resume` opens a scrollable list of recent sessions with ID prefix, relative time, mode, and preview text; replaces `dialoguer::Select`
- **Mode picker overlay** — `/accept` opens a selection overlay with Guided/Execute/Auto options (with descriptions and Auto danger warning); replaces `dialoguer::Select`
- **New `AppState` variants** — `AwaitingApproval`, `DiffView`, `SessionPicker`, `ModePicker`
- **New `Action` variants** — Approval (approve/reject/view-diff), diff navigation (scroll, quit), list navigation (up/down/select/dismiss)
- **Handler swapping** — When mode changes via overlay, the approval handler is swapped in the orchestrator before rebuilding the tool registry

**What is NOT in Phase 9d:**

- No mouse support for overlays (Phase 9e)
- No file path tab-completion in input (Phase 9e)
- No sub-agent progress bars (Phase 9e)
- No error rate-limit countdown display (Phase 9e)

---

## 2. Dependency Changes

### Cargo.toml

```toml
# Add to [dependencies]:
similar = "2.6"
```

The `similar` crate provides `TextDiff` for computing unified diffs. It is already used by `src/ui/diff.rs` for the terminal diff display; if it is already in `Cargo.toml`, no change is needed. The TUI diff viewer needs it directly for computing diff hunks and rendering them with ratatui styles.

> **Note:** The existing `src/ui/diff.rs` uses `similar` and prints directly to stdout. Phase 9d creates a new TUI-native diff rendering pipeline that returns styled `Line` objects instead of printing.

---

## 3. File Overview

### New Files (5)

| File | Purpose | Est. Lines |
|------|---------|-----------|
| `src/tui/approval_overlay.rs` | Modal overlay for file write/edit approval with inline diff | ~250 |
| `src/tui/diff_view.rs` | Full-screen scrollable diff viewer with vim navigation | ~200 |
| `src/tui/session_picker.rs` | Session selection overlay for `/resume` | ~200 |
| `src/tui/mode_picker.rs` | Mode selection overlay for `/accept` | ~180 |
| `src/tui/tui_approval_handler.rs` | `ApprovalHandler` impl that communicates with TUI via channels | ~60 |

### Modified Files (6)

| File | Change |
|------|--------|
| `src/tui/app.rs` | Add `AwaitingApproval`, `DiffView`, `SessionPicker`, `ModePicker` states; add approval channel fields; handle new events |
| `src/tui/events.rs` | Add `ApprovalRequest`, `SessionList`, `ApprovalResponse` event variants |
| `src/tui/keybindings.rs` | Add `map_approval`, `map_diff_view`, `map_session_picker`, `map_mode_picker` handlers; add new Action variants |
| `src/tui/layout.rs` | Render overlay widgets conditionally based on AppState |
| `src/tui/theme.rs` | Add overlay-specific color constants (OVERLAY_BG, OVERLAY_BORDER) |
| `src/tui/mod.rs` | Add 5 new module declarations |

---

## 4. `tui/approval_overlay.rs` — Approval Overlay

A modal overlay that appears when the LLM proposes a file change in Guided mode. Shows the file path, a colorized inline diff, a change summary, and key hints.

### Data Model

```rust
use std::time::Duration;
use crate::ui::approval::{ApprovalDecision, FileChange};
use tokio::sync::oneshot;

/// State for the approval overlay.
pub struct ApprovalOverlay {
    /// The file change being reviewed.
    pub change: FileChange,
    /// Pre-computed diff lines for rendering.
    pub diff_lines: Vec<DiffLine>,
    /// Summary statistics.
    pub additions: usize,
    pub deletions: usize,
    /// Scroll offset for the diff (if it exceeds visible area).
    pub scroll_offset: usize,
    /// Channel to send the user's decision back to the approval handler.
    pub response_tx: Option<oneshot::Sender<ApprovalDecision>>,
}

/// A single line in the diff display.
#[derive(Debug, Clone)]
pub enum DiffLine {
    /// Hunk header: @@ -start,count +start,count @@
    Hunk(String),
    /// Added line (green).
    Add(String),
    /// Deleted line (red).
    Del(String),
    /// Context line (unchanged).
    Context(String),
}
```

### Construction

```rust
impl ApprovalOverlay {
    pub fn new(
        change: FileChange,
        response_tx: oneshot::Sender<ApprovalDecision>,
    ) -> Self {
        let (diff_lines, additions, deletions) = compute_diff(&change);
        Self {
            change,
            diff_lines,
            additions,
            deletions,
            scroll_offset: 0,
            response_tx: Some(response_tx),
        }
    }

    /// Approve the change and send the decision.
    pub fn approve(&mut self) {
        if let Some(tx) = self.response_tx.take() {
            let _ = tx.send(ApprovalDecision::Approved);
        }
    }

    /// Reject the change and send the decision.
    pub fn reject(&mut self) {
        if let Some(tx) = self.response_tx.take() {
            let _ = tx.send(ApprovalDecision::Rejected);
        }
    }

    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    pub fn scroll_down(&mut self, n: usize, max: usize) {
        self.scroll_offset = (self.scroll_offset + n).min(max);
    }
}
```

### Diff Computation

```rust
use similar::TextDiff;

fn compute_diff(change: &FileChange) -> (Vec<DiffLine>, usize, usize) {
    let diff = TextDiff::from_lines(&change.old_content, &change.new_content);
    let mut lines = Vec::new();
    let mut additions = 0;
    let mut deletions = 0;

    for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
        lines.push(DiffLine::Hunk(format!("{}", hunk.header())));

        for change in hunk.iter_changes() {
            match change.tag() {
                similar::ChangeTag::Insert => {
                    lines.push(DiffLine::Add(format!("+{}", change.value().trim_end())));
                    additions += 1;
                }
                similar::ChangeTag::Delete => {
                    lines.push(DiffLine::Del(format!("-{}", change.value().trim_end())));
                    deletions += 1;
                }
                similar::ChangeTag::Equal => {
                    lines.push(DiffLine::Context(format!(" {}", change.value().trim_end())));
                }
            }
        }
    }

    (lines, additions, deletions)
}
```

### Visual Layout

```
┌─ File Change ──────────────────────────────────────────────────────┐
│                                                                     │
│  MODIFY  src/tui/app.rs                                            │
│                                                                     │
│  @@ -25,6 +25,8 @@                                                │
│   pub enum AppState {                                               │
│       Idle,                                                         │
│       CommandPicker { filter: String, selected: usize },            │
│  +    AwaitingApproval,                                             │
│  +    DiffView,                                                     │
│       Exiting,                                                      │
│   }                                                                 │
│                                                                     │
│  +3 additions, -0 deletions                                        │
│                                                                     │
│  [y] approve  [n] reject  [d] full diff  [Esc] reject              │
└─────────────────────────────────────────────────────────────────────┘
```

### Rendering

```rust
use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};

use super::theme::TuiTheme;

pub fn render(
    frame: &mut Frame,
    overlay: &ApprovalOverlay,
    terminal_area: Rect,
) {
    // Overlay dimensions: 80% width, 70% height, centered
    let width = (terminal_area.width as f32 * 0.8) as u16;
    let height = (terminal_area.height as f32 * 0.7) as u16;
    let x = (terminal_area.width.saturating_sub(width)) / 2;
    let y = (terminal_area.height.saturating_sub(height)) / 2;
    let overlay_area = Rect::new(x, y, width, height);

    // Clear background
    frame.render_widget(Clear, overlay_area);

    // Border
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(TuiTheme::ACCENT))
        .title(
            Line::from(" File Change ")
                .style(Style::new().fg(TuiTheme::ACCENT).bold()),
        )
        .title_bottom(
            Line::from(format!(
                " [y] approve  [n] reject  [d] full diff  [Esc] reject "
            ))
            .centered()
            .style(Style::new().fg(TuiTheme::FG_MUTED)),
        )
        .padding(Padding::new(2, 2, 1, 1));

    let inner = block.inner(overlay_area);
    frame.render_widget(block, overlay_area);

    // Build content lines
    let mut lines: Vec<Line> = Vec::new();

    // File header: "MODIFY  src/tui/app.rs" or "CREATE  src/tui/new.rs"
    let action = if overlay.change.is_new_file { "CREATE" } else { "MODIFY" };
    let action_color = if overlay.change.is_new_file {
        TuiTheme::SUCCESS
    } else {
        TuiTheme::WARNING
    };
    lines.push(Line::from(vec![
        Span::styled(
            format!(" {} ", action),
            Style::new().fg(TuiTheme::BG).bg(action_color).bold(),
        ),
        Span::raw("  "),
        Span::styled(
            &overlay.change.file_path,
            Style::new().fg(TuiTheme::FG).bold(),
        ),
    ]));
    lines.push(Line::from(""));

    // Diff lines
    for diff_line in overlay
        .diff_lines
        .iter()
        .skip(overlay.scroll_offset)
    {
        let line = match diff_line {
            DiffLine::Hunk(text) => Line::styled(
                text.clone(),
                Style::new().fg(TuiTheme::DIFF_HUNK),
            ),
            DiffLine::Add(text) => Line::styled(
                text.clone(),
                Style::new().fg(TuiTheme::DIFF_ADD).bg(TuiTheme::DIFF_ADD_BG),
            ),
            DiffLine::Del(text) => Line::styled(
                text.clone(),
                Style::new().fg(TuiTheme::DIFF_DEL).bg(TuiTheme::DIFF_DEL_BG),
            ),
            DiffLine::Context(text) => Line::styled(
                text.clone(),
                Style::new().fg(TuiTheme::FG_DIM),
            ),
        };
        lines.push(line);
    }

    // Summary line
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            format!("+{}", overlay.additions),
            Style::new().fg(TuiTheme::DIFF_ADD),
        ),
        Span::styled(
            format!(" additions, -{} deletions", overlay.deletions),
            Style::new().fg(TuiTheme::DIFF_DEL),
        ),
    ]));

    let paragraph = Paragraph::new(lines)
        .scroll((0, 0)); // scroll_offset already applied via skip()
    frame.render_widget(paragraph, inner);
}
```

### Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::approval::FileChange;

    fn sample_change() -> FileChange {
        FileChange {
            file_path: "src/main.rs".to_string(),
            resolved_path: "/tmp/src/main.rs".to_string(),
            old_content: "fn main() {\n    println!(\"hello\");\n}\n".to_string(),
            new_content: "fn main() {\n    println!(\"hello world\");\n}\n".to_string(),
            is_new_file: false,
        }
    }

    #[test]
    fn compute_diff_counts() {
        let (lines, additions, deletions) = compute_diff(&sample_change());
        assert!(additions >= 1);
        assert!(deletions >= 1);
        assert!(!lines.is_empty());
    }

    #[test]
    fn compute_diff_new_file() {
        let change = FileChange {
            file_path: "new.rs".to_string(),
            resolved_path: "/tmp/new.rs".to_string(),
            old_content: String::new(),
            new_content: "fn new() {}\n".to_string(),
            is_new_file: true,
        };
        let (lines, additions, deletions) = compute_diff(&change);
        assert!(additions >= 1);
        assert_eq!(deletions, 0);
        assert!(lines.iter().any(|l| matches!(l, DiffLine::Add(_))));
    }

    #[test]
    fn scroll_clamps() {
        let (tx, _rx) = oneshot::channel();
        let mut overlay = ApprovalOverlay::new(sample_change(), tx);
        overlay.scroll_up(100);
        assert_eq!(overlay.scroll_offset, 0);
    }

    #[test]
    fn approve_sends_decision() {
        let (tx, rx) = oneshot::channel();
        let mut overlay = ApprovalOverlay::new(sample_change(), tx);
        overlay.approve();
        assert_eq!(rx.try_recv().unwrap(), ApprovalDecision::Approved);
    }

    #[test]
    fn reject_sends_decision() {
        let (tx, rx) = oneshot::channel();
        let mut overlay = ApprovalOverlay::new(sample_change(), tx);
        overlay.reject();
        assert_eq!(rx.try_recv().unwrap(), ApprovalDecision::Rejected);
    }
}
```

---

## 5. `tui/diff_view.rs` — Full-Screen Diff Viewer

A full-screen scrollable view of the diff, activated by pressing `d` from the approval overlay. Uses vim-style navigation.

### Data Model

```rust
use super::approval_overlay::DiffLine;

/// Full-screen diff viewer state.
pub struct DiffView {
    /// File path being viewed.
    pub file_path: String,
    /// Pre-computed diff lines (shared with ApprovalOverlay).
    pub diff_lines: Vec<DiffLine>,
    /// Scroll offset (in lines from top).
    pub scroll_offset: usize,
    /// Total content height.
    pub total_lines: usize,
    /// Visible area height (updated each render).
    pub visible_height: usize,
}

impl DiffView {
    pub fn new(file_path: String, diff_lines: Vec<DiffLine>) -> Self {
        let total_lines = diff_lines.len();
        Self {
            file_path,
            diff_lines,
            scroll_offset: 0,
            total_lines,
            visible_height: 0,
        }
    }

    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    pub fn scroll_down(&mut self, n: usize) {
        let max = self.total_lines.saturating_sub(self.visible_height);
        self.scroll_offset = (self.scroll_offset + n).min(max);
    }

    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.total_lines.saturating_sub(self.visible_height);
    }

    pub fn page_up(&mut self) {
        self.scroll_up(self.visible_height / 2);
    }

    pub fn page_down(&mut self) {
        self.scroll_down(self.visible_height / 2);
    }
}
```

### Visual Layout

```
╭─ Diff: src/tui/app.rs ─────────────────────────────────────── 1/45 ─╮
│ @@ -25,6 +25,8 @@                                                    │
│  pub enum AppState {                                                  │
│      Idle,                                                            │
│      CommandPicker { filter: String, selected: usize },               │
│ +    AwaitingApproval,                                                │
│ +    DiffView,                                                        │
│      Exiting,                                                         │
│  }                                                                    │
│                                                                       │
│ @@ -82,6 +84,8 @@                                                    │
│  pub struct App<'a> {                                                 │
│      pub state: AppState,                                             │
│ +    pub approval_overlay: Option<ApprovalOverlay>,                   │
│ +    pub diff_view: Option<DiffView>,                                 │
│  }                                                                    │
╰── j/k scroll  Ctrl+d/u page  gg top  G bottom  q back ──────────────╯
```

### Rendering

```rust
use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Paragraph, Padding};
use super::theme::TuiTheme;

pub fn render(frame: &mut Frame, view: &mut DiffView, area: Rect) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(TuiTheme::ACCENT))
        .title(
            Line::from(format!(" Diff: {} ", view.file_path))
                .style(Style::new().fg(TuiTheme::ACCENT).bold()),
        )
        .title_top(
            Line::from(format!(
                " {}/{} ",
                view.scroll_offset + 1,
                view.total_lines
            ))
            .right_aligned()
            .style(Style::new().fg(TuiTheme::FG_MUTED)),
        )
        .title_bottom(
            Line::from(" j/k scroll  Ctrl+d/u page  gg top  G bottom  q back ")
                .centered()
                .style(Style::new().fg(TuiTheme::FG_MUTED)),
        )
        .padding(Padding::horizontal(1));

    let inner = block.inner(area);
    view.visible_height = inner.height as usize;
    frame.render_widget(block, area);

    // Build visible lines
    let lines: Vec<Line> = view
        .diff_lines
        .iter()
        .skip(view.scroll_offset)
        .take(view.visible_height)
        .map(|dl| match dl {
            DiffLine::Hunk(text) => Line::styled(
                text.clone(),
                Style::new().fg(TuiTheme::DIFF_HUNK).bold(),
            ),
            DiffLine::Add(text) => Line::styled(
                text.clone(),
                Style::new().fg(TuiTheme::DIFF_ADD).bg(TuiTheme::DIFF_ADD_BG),
            ),
            DiffLine::Del(text) => Line::styled(
                text.clone(),
                Style::new().fg(TuiTheme::DIFF_DEL).bg(TuiTheme::DIFF_DEL_BG),
            ),
            DiffLine::Context(text) => Line::styled(
                text.clone(),
                Style::new().fg(TuiTheme::FG_DIM),
            ),
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}
```

### Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn sample_lines() -> Vec<DiffLine> {
        vec![
            DiffLine::Hunk("@@ -1,3 +1,4 @@".to_string()),
            DiffLine::Context(" line1".to_string()),
            DiffLine::Del("-old".to_string()),
            DiffLine::Add("+new".to_string()),
            DiffLine::Context(" line3".to_string()),
        ]
    }

    #[test]
    fn scroll_to_top_resets() {
        let mut view = DiffView::new("test.rs".to_string(), sample_lines());
        view.scroll_offset = 3;
        view.scroll_to_top();
        assert_eq!(view.scroll_offset, 0);
    }

    #[test]
    fn scroll_to_bottom_clamps() {
        let mut view = DiffView::new("test.rs".to_string(), sample_lines());
        view.visible_height = 3;
        view.scroll_to_bottom();
        assert_eq!(view.scroll_offset, 2); // 5 lines - 3 visible = 2
    }

    #[test]
    fn scroll_up_clamps_at_zero() {
        let mut view = DiffView::new("test.rs".to_string(), sample_lines());
        view.scroll_up(100);
        assert_eq!(view.scroll_offset, 0);
    }

    #[test]
    fn scroll_down_clamps_at_max() {
        let mut view = DiffView::new("test.rs".to_string(), sample_lines());
        view.visible_height = 3;
        view.scroll_down(100);
        assert_eq!(view.scroll_offset, 2);
    }
}
```

---

## 6. `tui/session_picker.rs` — Session Picker Overlay

A modal overlay that displays recent sessions for `/resume`. Replaces `dialoguer::Select`.

### Data Model

```rust
use crate::session::store::SessionMeta;

/// Session picker overlay state.
pub struct SessionPicker {
    /// List of available sessions (most recent first).
    pub sessions: Vec<SessionMeta>,
    /// Currently highlighted index.
    pub selected: usize,
    /// Scroll offset for long lists.
    pub scroll_offset: usize,
    /// Maximum visible items.
    pub max_visible: usize,
}

impl SessionPicker {
    pub fn new(sessions: Vec<SessionMeta>) -> Self {
        Self {
            sessions,
            selected: 0,
            scroll_offset: 0,
            max_visible: 10,
        }
    }

    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
        self.ensure_visible();
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.sessions.len() {
            self.selected += 1;
        }
        self.ensure_visible();
    }

    pub fn selected_session(&self) -> Option<&SessionMeta> {
        self.sessions.get(self.selected)
    }

    fn ensure_visible(&mut self) {
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + self.max_visible {
            self.scroll_offset = self.selected - self.max_visible + 1;
        }
    }
}
```

### Visual Layout

```
╭─ Resume Session ────────────────────────────────────────── 5 sessions ─╮
│                                                                         │
│  > Select a session to resume                                          │
│                                                                         │
│  ▸ a1b2c3d4  5 minutes ago    plan     "Add auth to the login..."      │
│    e5f6g7h8  2 hours ago      execute  "Fix the failing test in..."     │
│    i9j0k1l2  yesterday        guided   "Refactor the session man..."    │
│    m3n4o5p6  2 days ago       explore  "What does the orchestrat..."    │
│    q7r8s9t0  3 days ago       auto     "Implement the entire CI/..."    │
│                                                                         │
│  [Enter] resume  [Esc] cancel                                          │
╰─────────────────────────────────────────────────────────────────────────╯
```

### Rendering

```rust
use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};
use super::theme::TuiTheme;

pub fn render(
    frame: &mut Frame,
    picker: &SessionPicker,
    terminal_area: Rect,
) {
    let count = picker.sessions.len();
    if count == 0 {
        return;
    }

    // Overlay dimensions
    let width = terminal_area.width.saturating_sub(8).min(80);
    let visible_items = count.min(picker.max_visible);
    let height = (visible_items as u16) + 6; // border + prompt + gap + items + gap + footer

    let x = (terminal_area.width.saturating_sub(width)) / 2;
    let y = (terminal_area.height.saturating_sub(height)) / 2;
    let overlay = Rect::new(x, y, width, height);

    frame.render_widget(Clear, overlay);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(TuiTheme::ACCENT))
        .title(
            Line::from(" Resume Session ")
                .style(Style::new().fg(TuiTheme::ACCENT).bold()),
        )
        .title_top(
            Line::from(format!(" {} sessions ", count))
                .right_aligned()
                .style(Style::new().fg(TuiTheme::FG_MUTED)),
        )
        .title_bottom(
            Line::from(" [Enter] resume  [Esc] cancel ")
                .centered()
                .style(Style::new().fg(TuiTheme::FG_MUTED)),
        )
        .padding(Padding::new(2, 2, 1, 1));

    let inner = block.inner(overlay);
    frame.render_widget(block, overlay);

    // Build content lines
    let mut lines: Vec<Line> = Vec::new();

    // Prompt line
    lines.push(Line::styled(
        "> Select a session to resume",
        Style::new().fg(TuiTheme::ACCENT),
    ));
    lines.push(Line::from(""));

    // Column widths
    let id_width = 10;
    let time_width = 18;
    let mode_width = 10;

    // Session rows
    let visible: Vec<&SessionMeta> = picker
        .sessions
        .iter()
        .skip(picker.scroll_offset)
        .take(picker.max_visible)
        .collect();

    for (i, meta) in visible.iter().enumerate() {
        let global_idx = picker.scroll_offset + i;
        let is_selected = global_idx == picker.selected;

        let id_prefix = &meta.session_id.as_str()[..8.min(meta.session_id.as_str().len())];
        let time_str = meta.relative_time();
        let mode_str = meta.mode.to_string();
        let preview = meta.truncated_preview();

        let (indicator, id_style, text_style) = if is_selected {
            (
                " ▸ ",
                Style::new()
                    .fg(TuiTheme::PICKER_HIGHLIGHT_FG)
                    .bg(TuiTheme::PICKER_HIGHLIGHT_BG)
                    .bold(),
                Style::new()
                    .fg(TuiTheme::PICKER_HIGHLIGHT_FG)
                    .bg(TuiTheme::PICKER_HIGHLIGHT_BG),
            )
        } else {
            (
                "   ",
                Style::new().fg(TuiTheme::ACCENT),
                Style::new().fg(TuiTheme::FG_DIM),
            )
        };

        let line = Line::from(vec![
            Span::styled(
                indicator,
                if is_selected {
                    Style::new().bg(TuiTheme::PICKER_HIGHLIGHT_BG)
                } else {
                    Style::default()
                },
            ),
            Span::styled(format!("{:<w$}", id_prefix, w = id_width), id_style),
            Span::styled(format!("{:<w$}", time_str, w = time_width), text_style),
            Span::styled(format!("{:<w$}", mode_str, w = mode_width), text_style),
            Span::styled(format!("\"{}\"", preview), text_style),
        ]);
        lines.push(line);
    }

    frame.render_widget(Paragraph::new(lines), inner);
}
```

### Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn mock_sessions(n: usize) -> Vec<SessionMeta> {
        // Create n mock SessionMeta entries for testing.
        // In practice, SessionMeta is constructed from real session data.
        // For unit tests, use a helper that creates minimal instances.
        (0..n).map(|i| {
            // Minimal mock — actual construction depends on SessionMeta fields
            create_mock_session_meta(i)
        }).collect()
    }

    #[test]
    fn empty_sessions() {
        let picker = SessionPicker::new(vec![]);
        assert!(picker.selected_session().is_none());
    }

    #[test]
    fn move_up_clamps_at_zero() {
        let mut picker = SessionPicker::new(mock_sessions(5));
        picker.move_up();
        assert_eq!(picker.selected, 0);
    }

    #[test]
    fn move_down_clamps_at_end() {
        let mut picker = SessionPicker::new(mock_sessions(3));
        picker.move_down();
        picker.move_down();
        picker.move_down(); // Already at end
        assert_eq!(picker.selected, 2);
    }

    #[test]
    fn ensure_visible_scrolls_down() {
        let mut picker = SessionPicker::new(mock_sessions(20));
        picker.max_visible = 5;
        // Move to item 7 (beyond visible range)
        for _ in 0..7 {
            picker.move_down();
        }
        assert!(picker.scroll_offset > 0);
        assert!(picker.selected >= picker.scroll_offset);
        assert!(picker.selected < picker.scroll_offset + picker.max_visible);
    }
}
```

---

## 7. `tui/mode_picker.rs` — Mode Picker Overlay

A modal overlay for the `/accept` command. Shows execution mode options with descriptions and a danger warning for Auto mode.

### Data Model

```rust
use crate::mode::Mode;

/// Mode picker option with description.
struct ModeOption {
    mode: Mode,
    label: &'static str,
    description: &'static str,
    is_dangerous: bool,
}

/// Mode picker overlay state.
pub struct ModePicker {
    options: Vec<ModeOption>,
    pub selected: usize,
    /// When true, show the Auto mode confirmation prompt.
    pub confirming_auto: bool,
}

impl ModePicker {
    pub fn new() -> Self {
        Self {
            options: vec![
                ModeOption {
                    mode: Mode::Guided,
                    label: "Guided",
                    description: "Write files with approval for each change",
                    is_dangerous: false,
                },
                ModeOption {
                    mode: Mode::Execute,
                    label: "Execute",
                    description: "Auto-approve writes, allowlisted shell commands",
                    is_dangerous: false,
                },
                ModeOption {
                    mode: Mode::Auto,
                    label: "Auto",
                    description: "Full autonomy, unrestricted shell access",
                    is_dangerous: true,
                },
            ],
            selected: 0,
            confirming_auto: false,
        }
    }

    pub fn move_up(&mut self) {
        if !self.confirming_auto {
            self.selected = self.selected.saturating_sub(1);
        }
    }

    pub fn move_down(&mut self) {
        if !self.confirming_auto {
            if self.selected + 1 < self.options.len() {
                self.selected += 1;
            }
        }
    }

    /// Returns the selected mode. If Auto is selected, transitions to
    /// confirmation state first. Returns None if in confirmation state.
    pub fn try_select(&mut self) -> Option<Mode> {
        let opt = &self.options[self.selected];
        if opt.is_dangerous && !self.confirming_auto {
            self.confirming_auto = true;
            None
        } else {
            Some(opt.mode)
        }
    }

    /// Confirm Auto mode selection.
    pub fn confirm_auto(&self) -> Mode {
        Mode::Auto
    }

    /// Cancel Auto confirmation, return to selection.
    pub fn cancel_auto(&mut self) {
        self.confirming_auto = false;
    }

    pub fn selected_mode(&self) -> Mode {
        self.options[self.selected].mode
    }
}
```

### Visual Layout — Selection State

```
╭─ Accept Plan ────────────────────────────────────────────────────────╮
│                                                                       │
│  > Choose execution mode for the plan                                │
│                                                                       │
│  ▸ Guided   Write files with approval for each change                │
│    Execute  Auto-approve writes, allowlisted shell commands          │
│    Auto     Full autonomy, unrestricted shell access      ⚠ DANGER  │
│                                                                       │
│  [Enter] select  [Esc] cancel                                        │
╰───────────────────────────────────────────────────────────────────────╯
```

### Visual Layout — Auto Confirmation State

```
╭─ Accept Plan ────────────────────────────────────────────────────────╮
│                                                                       │
│  ⚠  WARNING: Auto mode grants unrestricted shell access.             │
│                                                                       │
│  The agent can execute arbitrary commands, modify any file,           │
│  and make network requests without approval.                          │
│                                                                       │
│  [y] confirm Auto mode  [n] go back                                  │
╰───────────────────────────────────────────────────────────────────────╯
```

### Rendering

```rust
use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};
use super::theme::TuiTheme;

pub fn render(
    frame: &mut Frame,
    picker: &ModePicker,
    terminal_area: Rect,
) {
    let width = terminal_area.width.saturating_sub(8).min(72);
    let height = if picker.confirming_auto { 10 } else { 9 };
    let x = (terminal_area.width.saturating_sub(width)) / 2;
    let y = (terminal_area.height.saturating_sub(height)) / 2;
    let overlay = Rect::new(x, y, width, height);

    frame.render_widget(Clear, overlay);

    if picker.confirming_auto {
        render_auto_confirmation(frame, overlay);
    } else {
        render_mode_selection(frame, picker, overlay);
    }
}

fn render_mode_selection(frame: &mut Frame, picker: &ModePicker, area: Rect) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(TuiTheme::ACCENT))
        .title(
            Line::from(" Accept Plan ")
                .style(Style::new().fg(TuiTheme::ACCENT).bold()),
        )
        .title_bottom(
            Line::from(" [Enter] select  [Esc] cancel ")
                .centered()
                .style(Style::new().fg(TuiTheme::FG_MUTED)),
        )
        .padding(Padding::new(2, 2, 1, 1));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::styled(
        "> Choose execution mode for the plan",
        Style::new().fg(TuiTheme::ACCENT),
    ));
    lines.push(Line::from(""));

    let label_width = 10;
    for (i, opt) in picker.options.iter().enumerate() {
        let is_selected = i == picker.selected;
        let (indicator, label_style, desc_style) = if is_selected {
            (
                " ▸ ",
                Style::new()
                    .fg(TuiTheme::PICKER_HIGHLIGHT_FG)
                    .bg(TuiTheme::PICKER_HIGHLIGHT_BG)
                    .bold(),
                Style::new()
                    .fg(TuiTheme::PICKER_HIGHLIGHT_FG)
                    .bg(TuiTheme::PICKER_HIGHLIGHT_BG),
            )
        } else {
            (
                "   ",
                Style::new().fg(TuiTheme::ACCENT).bold(),
                Style::new().fg(TuiTheme::FG_DIM),
            )
        };

        let mut spans = vec![
            Span::styled(
                indicator,
                if is_selected {
                    Style::new().bg(TuiTheme::PICKER_HIGHLIGHT_BG)
                } else {
                    Style::default()
                },
            ),
            Span::styled(format!("{:<w$}", opt.label, w = label_width), label_style),
            Span::styled(opt.description, desc_style),
        ];

        if opt.is_dangerous {
            spans.push(Span::styled(
                "  ⚠ DANGER",
                Style::new().fg(TuiTheme::ERROR).bold(),
            ));
        }

        lines.push(Line::from(spans));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_auto_confirmation(frame: &mut Frame, area: Rect) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(TuiTheme::ERROR))
        .title(
            Line::from(" Accept Plan ")
                .style(Style::new().fg(TuiTheme::ERROR).bold()),
        )
        .title_bottom(
            Line::from(" [y] confirm Auto mode  [n] go back ")
                .centered()
                .style(Style::new().fg(TuiTheme::FG_MUTED)),
        )
        .padding(Padding::new(2, 2, 1, 1));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = vec![
        Line::from(vec![
            Span::styled("⚠  WARNING: ", Style::new().fg(TuiTheme::ERROR).bold()),
            Span::styled(
                "Auto mode grants unrestricted shell access.",
                Style::new().fg(TuiTheme::WARNING),
            ),
        ]),
        Line::from(""),
        Line::styled(
            "The agent can execute arbitrary commands, modify any file,",
            Style::new().fg(TuiTheme::FG),
        ),
        Line::styled(
            "and make network requests without approval.",
            Style::new().fg(TuiTheme::FG),
        ),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}
```

### Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_selection_is_guided() {
        let picker = ModePicker::new();
        assert_eq!(picker.selected_mode(), Mode::Guided);
    }

    #[test]
    fn move_up_clamps() {
        let mut picker = ModePicker::new();
        picker.move_up();
        assert_eq!(picker.selected, 0);
    }

    #[test]
    fn move_down_clamps() {
        let mut picker = ModePicker::new();
        picker.move_down();
        picker.move_down();
        picker.move_down(); // Past end
        assert_eq!(picker.selected, 2);
    }

    #[test]
    fn select_guided_returns_immediately() {
        let mut picker = ModePicker::new();
        assert_eq!(picker.try_select(), Some(Mode::Guided));
    }

    #[test]
    fn select_auto_requires_confirmation() {
        let mut picker = ModePicker::new();
        picker.move_down();
        picker.move_down(); // Auto
        assert_eq!(picker.try_select(), None);
        assert!(picker.confirming_auto);
    }

    #[test]
    fn cancel_auto_returns_to_selection() {
        let mut picker = ModePicker::new();
        picker.move_down();
        picker.move_down();
        picker.try_select(); // Enter confirmation
        picker.cancel_auto();
        assert!(!picker.confirming_auto);
    }

    #[test]
    fn confirm_auto_returns_auto() {
        let picker = ModePicker::new();
        assert_eq!(picker.confirm_auto(), Mode::Auto);
    }
}
```

---

## 8. `tui/tui_approval_handler.rs` — TUI ApprovalHandler

An implementation of the `ApprovalHandler` trait that communicates with the TUI event loop via channels instead of blocking on `dialoguer`.

### Architecture

```
Orchestrator Task          TUI Event Loop
     │                          │
     │  request_approval()      │
     │─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─>│
     │  AppEvent::ApprovalRequest(change, oneshot::Sender)
     │                          │
     │                          │  ── User sees overlay ──
     │                          │  ── User presses y/n ──
     │                          │
     │  oneshot::Receiver       │
     │<─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─│
     │  ApprovalDecision        │
     │                          │
     ▼                          ▼
```

### Implementation

```rust
use std::fmt::Debug;
use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};

use crate::ui::approval::{ApprovalDecision, ApprovalHandler, FileChange};
use super::events::AppEvent;

/// Approval handler that sends requests to the TUI overlay.
#[derive(Clone)]
pub struct TuiApprovalHandler {
    event_tx: mpsc::UnboundedSender<AppEvent>,
}

impl Debug for TuiApprovalHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TuiApprovalHandler").finish()
    }
}

impl TuiApprovalHandler {
    pub fn new(event_tx: mpsc::UnboundedSender<AppEvent>) -> Self {
        Self { event_tx }
    }
}

#[async_trait]
impl ApprovalHandler for TuiApprovalHandler {
    async fn request_approval(&self, change: &FileChange) -> anyhow::Result<ApprovalDecision> {
        let (response_tx, response_rx) = oneshot::channel();

        // Send request to TUI event loop
        self.event_tx
            .send(AppEvent::ApprovalRequest {
                change: change.clone(),
                response_tx,
            })
            .map_err(|_| anyhow::anyhow!("TUI event loop closed"))?;

        // Wait for the user's decision
        response_rx
            .await
            .map_err(|_| anyhow::anyhow!("Approval response channel closed"))
    }
}
```

### Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn handler_sends_and_receives() {
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let handler = TuiApprovalHandler::new(event_tx);

        let change = FileChange {
            file_path: "test.rs".to_string(),
            resolved_path: "/tmp/test.rs".to_string(),
            old_content: "old".to_string(),
            new_content: "new".to_string(),
            is_new_file: false,
        };

        // Spawn handler request
        let handle = tokio::spawn(async move {
            handler.request_approval(&change).await
        });

        // Receive the event and respond
        if let Some(AppEvent::ApprovalRequest { response_tx, .. }) = event_rx.recv().await {
            response_tx.send(ApprovalDecision::Approved).unwrap();
        }

        let result = handle.await.unwrap().unwrap();
        assert_eq!(result, ApprovalDecision::Approved);
    }
}
```

---

## 9. Modifications to Phase 9a/9b/9c Files

### `tui/mod.rs` — Add Module Declarations

```rust
pub mod approval_overlay;
pub mod diff_view;
pub mod mode_picker;
pub mod session_picker;
pub mod tui_approval_handler;
```

### `tui/events.rs` — Add Event Variants

```rust
use crate::ui::approval::FileChange;
use crate::session::store::SessionMeta;
use tokio::sync::oneshot;

#[derive(Debug)]
pub enum AppEvent {
    // ── Terminal Events (Phase 9a) ──
    Key(KeyEvent),
    Resize(u16, u16),
    Tick,

    // ── Streaming Events (Phase 9c) ──
    TextDelta(String),
    StreamDone,
    ToolStart { name: String, args_display: String },
    ToolComplete { name: String, duration: std::time::Duration },
    ToolError { name: String, error: String },
    AgentStart { agent_type: String, task: String },
    AgentToolCall { agent_type: String },
    AgentComplete { agent_type: String, duration: std::time::Duration, tool_calls: usize },
    SystemMessage(String),
    ModeChanged(crate::mode::Mode),
    OrchestratorDone,
    Error(String),

    // ── Overlay Events (Phase 9d) ──
    /// An approval request from the orchestrator.
    ApprovalRequest {
        change: FileChange,
        response_tx: oneshot::Sender<crate::ui::approval::ApprovalDecision>,
    },
    /// Session list loaded for /resume.
    SessionListReady(Vec<SessionMeta>),
}
```

> **Note:** `AppEvent` cannot derive `Debug` directly because `oneshot::Sender` doesn't implement `Debug`. Implement `Debug` manually, or wrap the sender in a newtype.

```rust
impl std::fmt::Debug for AppEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Key(k) => write!(f, "Key({:?})", k),
            Self::Resize(w, h) => write!(f, "Resize({}, {})", w, h),
            Self::Tick => write!(f, "Tick"),
            Self::ApprovalRequest { change, .. } => {
                write!(f, "ApprovalRequest({})", change.file_path)
            }
            Self::SessionListReady(sessions) => {
                write!(f, "SessionListReady({} sessions)", sessions.len())
            }
            // ... other variants ...
            _ => write!(f, "{}", std::any::type_name::<Self>()),
        }
    }
}
```

### `tui/keybindings.rs` — Add Actions and Handlers

#### New Action Variants

```rust
pub enum Action {
    // ── Existing (Phase 9a/9b/9c) ──
    // ...

    // ── Approval Overlay (Phase 9d) ──
    ApprovalApprove,
    ApprovalReject,
    ApprovalViewDiff,

    // ── Diff View (Phase 9d) ──
    DiffScrollUp,
    DiffScrollDown,
    DiffPageUp,
    DiffPageDown,
    DiffScrollToTop,
    DiffScrollToBottom,
    DiffQuit,

    // ── List Picker — shared by SessionPicker and ModePicker (Phase 9d) ──
    ListUp,
    ListDown,
    ListSelect,
    ListDismiss,

    // ── Mode Picker (Phase 9d) ──
    ModeConfirmYes,
    ModeConfirmNo,

    Noop,
}
```

#### State-Based Key Mapping

```rust
pub fn map_key(key: KeyEvent, state: &AppState) -> Action {
    // Global keys (Ctrl+C, Ctrl+D, Ctrl+L) — always handled
    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => return Action::Cancel,
        (KeyCode::Char('d'), m) if m.contains(KeyModifiers::CONTROL) => return Action::Exit,
        (KeyCode::Char('l'), m) if m.contains(KeyModifiers::CONTROL) => return Action::Redraw,
        _ => {}
    }

    match state {
        AppState::Idle => map_idle(key),
        AppState::CommandPicker { .. } => map_picker(key),
        AppState::Thinking => map_thinking(key),
        AppState::Streaming => map_streaming(key),
        AppState::ToolExecuting { .. } => map_thinking(key),
        AppState::AwaitingApproval { confirming_auto: false } => map_approval(key),
        AppState::DiffView => map_diff_view(key),
        AppState::SessionPicker => map_list_picker(key),
        AppState::ModePicker { confirming_auto } => {
            if *confirming_auto {
                map_mode_confirm(key)
            } else {
                map_list_picker(key)
            }
        }
        AppState::Exiting => Action::Noop,
    }
}

fn map_approval(key: KeyEvent) -> Action {
    match (key.code, key.modifiers) {
        (KeyCode::Char('y'), _) => Action::ApprovalApprove,
        (KeyCode::Char('n'), _) | (KeyCode::Esc, _) => Action::ApprovalReject,
        (KeyCode::Char('d'), _) => Action::ApprovalViewDiff,
        (KeyCode::Up, _) => Action::ScrollUp,
        (KeyCode::Down, _) => Action::ScrollDown,
        _ => Action::Noop,
    }
}

fn map_diff_view(key: KeyEvent) -> Action {
    match (key.code, key.modifiers) {
        // Vim-style navigation
        (KeyCode::Char('j'), _) | (KeyCode::Down, _) => Action::DiffScrollDown,
        (KeyCode::Char('k'), _) | (KeyCode::Up, _) => Action::DiffScrollUp,
        (KeyCode::Char('d'), m) if m.contains(KeyModifiers::CONTROL) => Action::DiffPageDown,
        (KeyCode::Char('u'), m) if m.contains(KeyModifiers::CONTROL) => Action::DiffPageUp,
        (KeyCode::Char('g'), _) => Action::DiffScrollToTop,    // gg (simplified: single g)
        (KeyCode::Char('G'), _) => Action::DiffScrollToBottom,
        (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => Action::DiffQuit,
        (KeyCode::PageUp, _) => Action::DiffPageUp,
        (KeyCode::PageDown, _) => Action::DiffPageDown,
        (KeyCode::Home, _) => Action::DiffScrollToTop,
        (KeyCode::End, _) => Action::DiffScrollToBottom,
        _ => Action::Noop,
    }
}

fn map_list_picker(key: KeyEvent) -> Action {
    match (key.code, key.modifiers) {
        (KeyCode::Up, _) | (KeyCode::Char('k'), _) => Action::ListUp,
        (KeyCode::Down, _) | (KeyCode::Char('j'), _) => Action::ListDown,
        (KeyCode::Enter, _) => Action::ListSelect,
        (KeyCode::Esc, _) => Action::ListDismiss,
        _ => Action::Noop,
    }
}

fn map_mode_confirm(key: KeyEvent) -> Action {
    match (key.code, key.modifiers) {
        (KeyCode::Char('y'), _) => Action::ModeConfirmYes,
        (KeyCode::Char('n'), _) | (KeyCode::Esc, _) => Action::ModeConfirmNo,
        _ => Action::Noop,
    }
}
```

### `tui/app.rs` — Major Additions

#### AppState — New Variants

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppState {
    // Phase 9a/9b
    Idle,
    CommandPicker { filter: String, selected: usize },

    // Phase 9c
    Thinking,
    Streaming,
    ToolExecuting { tool_name: String },

    // Phase 9d
    AwaitingApproval,
    DiffView,
    SessionPicker,
    ModePicker { confirming_auto: bool },

    Exiting,
}
```

#### App Struct — New Fields

```rust
use super::approval_overlay::ApprovalOverlay;
use super::diff_view::DiffView as DiffViewState;
use super::session_picker::SessionPicker;
use super::mode_picker::ModePicker;

pub struct App<'a> {
    // Phase 9a/9b fields ...
    pub state: AppState,
    pub tick_count: usize,
    pub status: StatusSnapshot,
    pub input_pane: InputPane<'a>,
    pub command_picker: CommandPicker,
    pub pending_input: Option<String>,

    // Phase 9c fields ...
    pub messages: Vec<ChatMessage>,
    pub chat_viewport: ChatViewport,
    pub event_tx: mpsc::UnboundedSender<AppEvent>,

    // Phase 9d fields:
    pub approval_overlay: Option<ApprovalOverlay>,
    pub diff_view_state: Option<DiffViewState>,
    pub session_picker: Option<SessionPicker>,
    pub mode_picker: Option<ModePicker>,
}
```

#### Event Handlers — New Overlay Events

```rust
// In the main event loop match:
AppEvent::ApprovalRequest { change, response_tx } => {
    app.approval_overlay = Some(ApprovalOverlay::new(change, response_tx));
    app.state = AppState::AwaitingApproval;
}

AppEvent::SessionListReady(sessions) => {
    if sessions.is_empty() {
        app.messages.push(ChatMessage::System {
            content: "No sessions found.".to_string(),
        });
    } else {
        app.session_picker = Some(SessionPicker::new(sessions));
        app.state = AppState::SessionPicker;
    }
}
```

#### Action Handlers — New Overlay Actions

```rust
// In handle_action:
Action::ApprovalApprove => {
    if let Some(mut overlay) = self.approval_overlay.take() {
        overlay.approve();
        self.messages.push(ChatMessage::System {
            content: format!("✓ Approved: {}", overlay.change.file_path),
        });
    }
    self.state = AppState::Thinking; // Return to waiting for orchestrator
}

Action::ApprovalReject => {
    if let Some(mut overlay) = self.approval_overlay.take() {
        overlay.reject();
        self.messages.push(ChatMessage::System {
            content: format!("✗ Rejected: {}", overlay.change.file_path),
        });
    }
    self.state = AppState::Thinking;
}

Action::ApprovalViewDiff => {
    if let Some(ref overlay) = self.approval_overlay {
        let diff_view = DiffViewState::new(
            overlay.change.file_path.clone(),
            overlay.diff_lines.clone(),
        );
        self.diff_view_state = Some(diff_view);
        self.state = AppState::DiffView;
    }
}

Action::DiffScrollUp => {
    if let Some(ref mut view) = self.diff_view_state {
        view.scroll_up(1);
    }
}
Action::DiffScrollDown => {
    if let Some(ref mut view) = self.diff_view_state {
        view.scroll_down(1);
    }
}
Action::DiffPageUp => {
    if let Some(ref mut view) = self.diff_view_state {
        view.page_up();
    }
}
Action::DiffPageDown => {
    if let Some(ref mut view) = self.diff_view_state {
        view.page_down();
    }
}
Action::DiffScrollToTop => {
    if let Some(ref mut view) = self.diff_view_state {
        view.scroll_to_top();
    }
}
Action::DiffScrollToBottom => {
    if let Some(ref mut view) = self.diff_view_state {
        view.scroll_to_bottom();
    }
}
Action::DiffQuit => {
    self.diff_view_state = None;
    self.state = AppState::AwaitingApproval; // Return to approval overlay
}

Action::ListUp => {
    match &self.state {
        AppState::SessionPicker => {
            if let Some(ref mut picker) = self.session_picker {
                picker.move_up();
            }
        }
        AppState::ModePicker { .. } => {
            if let Some(ref mut picker) = self.mode_picker {
                picker.move_up();
            }
        }
        _ => {}
    }
}
Action::ListDown => {
    match &self.state {
        AppState::SessionPicker => {
            if let Some(ref mut picker) = self.session_picker {
                picker.move_down();
            }
        }
        AppState::ModePicker { .. } => {
            if let Some(ref mut picker) = self.mode_picker {
                picker.move_down();
            }
        }
        _ => {}
    }
}
Action::ListSelect => {
    match &self.state {
        AppState::SessionPicker => {
            if let Some(picker) = self.session_picker.take() {
                if let Some(meta) = picker.selected_session() {
                    let session_id = meta.session_id.clone();
                    // Resume this session (load events and reconstruct history)
                    // Dispatched to orchestrator task
                    self.messages.push(ChatMessage::System {
                        content: format!("Resuming session: {}", &session_id.as_str()[..8]),
                    });
                    // ... trigger session resume via event_tx ...
                }
            }
            self.state = AppState::Idle;
        }
        AppState::ModePicker { .. } => {
            if let Some(ref mut picker) = self.mode_picker {
                match picker.try_select() {
                    Some(mode) => {
                        self.mode_picker = None;
                        self.state = AppState::Idle;
                        // Accept plan with selected mode
                        // ... trigger plan acceptance via orchestrator channel ...
                        let label = super::theme::mode_label(&mode);
                        self.messages.push(ChatMessage::System {
                            content: format!("✓ Plan accepted. Switched to {} mode.", label),
                        });
                    }
                    None => {
                        // Entered Auto confirmation state
                        self.state = AppState::ModePicker { confirming_auto: true };
                    }
                }
            }
        }
        _ => {}
    }
}
Action::ListDismiss => {
    self.session_picker = None;
    self.mode_picker = None;
    self.state = AppState::Idle;
    self.messages.push(ChatMessage::System {
        content: "Cancelled.".to_string(),
    });
}

Action::ModeConfirmYes => {
    if let Some(picker) = self.mode_picker.take() {
        let mode = picker.confirm_auto();
        self.state = AppState::Idle;
        let label = super::theme::mode_label(&mode);
        self.messages.push(ChatMessage::System {
            content: format!("✓ Plan accepted. Switched to {} mode.", label),
        });
        // ... trigger plan acceptance with Auto mode via orchestrator ...
    }
}

Action::ModeConfirmNo => {
    if let Some(ref mut picker) = self.mode_picker {
        picker.cancel_auto();
        self.state = AppState::ModePicker { confirming_auto: false };
    }
}
```

### `tui/layout.rs` — Render Overlays

```rust
use super::{approval_overlay, diff_view, session_picker, mode_picker};

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    // ... size guard ...
    // ... header, chat, input, status bar rendering (Phase 9a-9c) ...

    // Phase 9c: command picker overlay
    if let AppState::CommandPicker { ref filter, selected } = app.state {
        let filter = filter.clone();
        app.command_picker.render(frame, &filter, selected, area, chat_area);
    }

    // Phase 9d overlays — render on top of everything
    match &app.state {
        AppState::AwaitingApproval => {
            if let Some(ref overlay) = app.approval_overlay {
                approval_overlay::render(frame, overlay, area);
            }
        }
        AppState::DiffView => {
            if let Some(ref mut view) = app.diff_view_state {
                diff_view::render(frame, view, area);
            }
        }
        AppState::SessionPicker => {
            if let Some(ref picker) = app.session_picker {
                session_picker::render(frame, picker, area);
            }
        }
        AppState::ModePicker { .. } => {
            if let Some(ref picker) = app.mode_picker {
                mode_picker::render(frame, picker, area);
            }
        }
        _ => {}
    }
}
```

### `tui/theme.rs` — Add Overlay Colors

```rust
impl TuiTheme {
    // ── Overlays (Phase 9d) ──
    pub const OVERLAY_BG: Color = tailwind::SLATE.c900;
    pub const OVERLAY_BORDER: Color = tailwind::SLATE.c600;
}
```

### `tui/commands.rs` — Wire ShowSessionPicker and ShowModePicker

The `CommandResult::ShowSessionPicker` and `CommandResult::ShowModePicker` variants defined in Phase 9c are now handled:

```rust
// In app.rs, handle_command_result:
fn handle_command_result(app: &mut App, orchestrator: &mut Orchestrator, result: CommandResult) {
    match result {
        CommandResult::Continue(msgs) => {
            app.messages.extend(msgs);
        }
        CommandResult::Quit => {
            app.state = AppState::Exiting;
        }
        CommandResult::SwitchMode { mode, messages } => {
            // Swap handler based on mode
            let handler = handler_for_mode(&mode, &app.event_tx);
            orchestrator.set_mode_with_handler(mode, Some(handler));
            app.messages.extend(messages);
            app.status = StatusSnapshot::from_orchestrator(orchestrator);
        }
        CommandResult::ShowSessionPicker => {
            // Load sessions asynchronously
            let tx = app.event_tx.clone();
            let sessions_dir = orchestrator.sessions_dir().to_path_buf();
            tokio::spawn(async move {
                let store = SessionStore::new(sessions_dir);
                match store.list_sessions() {
                    Ok(sessions) => {
                        let _ = tx.send(AppEvent::SessionListReady(sessions));
                    }
                    Err(e) => {
                        let _ = tx.send(AppEvent::SystemMessage(
                            format!("Error loading sessions: {}", e),
                        ));
                    }
                }
            });
        }
        CommandResult::ShowModePicker => {
            app.mode_picker = Some(ModePicker::new());
            app.state = AppState::ModePicker { confirming_auto: false };
        }
        CommandResult::ExecutePlan(plan) => {
            // Send plan to orchestrator for execution
            app.messages.push(ChatMessage::System {
                content: "Executing plan...".to_string(),
            });
            // ... dispatch plan execution via orchestrator channel ...
        }
    }
}

/// Create the appropriate handler for a mode.
fn handler_for_mode(
    mode: &Mode,
    event_tx: &mpsc::UnboundedSender<AppEvent>,
) -> Arc<dyn ApprovalHandler> {
    match mode {
        Mode::Guided => Arc::new(TuiApprovalHandler::new(event_tx.clone())),
        _ => Arc::new(DiffOnlyApprovalHandler::new()),
    }
}
```

---

## 10. Interaction Model

### Approval Flow (Guided Mode)

```
Orchestrator executing a tool call that writes a file
    │
    ▼
TuiApprovalHandler::request_approval() called
    │
    ├── Creates oneshot channel (response_tx, response_rx)
    ├── Sends AppEvent::ApprovalRequest { change, response_tx }
    └── Awaits response_rx
                │
                ▼
    TUI event loop receives ApprovalRequest
        │
        ├── Creates ApprovalOverlay::new(change, response_tx)
        ├── Sets AppState::AwaitingApproval
        └── Overlay renders diff with key hints
                    │
                    ├── User presses 'y'
                    │   ├── overlay.approve() → sends Approved via oneshot
                    │   ├── System message: "✓ Approved: path"
                    │   └── State → Thinking (orchestrator continues)
                    │
                    ├── User presses 'n' or Esc
                    │   ├── overlay.reject() → sends Rejected via oneshot
                    │   ├── System message: "✗ Rejected: path"
                    │   └── State → Thinking (orchestrator continues with rejection)
                    │
                    └── User presses 'd'
                        ├── Creates DiffView from overlay.diff_lines
                        ├── State → DiffView
                        ├── Vim-style navigation (j/k/Ctrl+d/u/gg/G)
                        └── 'q' returns to AwaitingApproval
```

### Session Resume Flow

```
User types "/resume" and presses Enter
    │
    ▼
commands::dispatch("/resume", ...) → CommandResult::ShowSessionPicker
    │
    ▼
handle_command_result: Spawn task to load sessions
    │
    ▼
SessionStore::list_sessions() runs on tokio task
    │
    ├── Success → AppEvent::SessionListReady(sessions)
    │   │
    │   ▼
    │   App receives event
    │       │
    │       ├── Empty list → System message "No sessions found."
    │       │
    │       └── Non-empty → Create SessionPicker, State → SessionPicker
    │                │
    │                ├── User navigates (Up/Down/j/k)
    │                ├── User selects (Enter)
    │                │   ├── Load session events
    │                │   ├── Reconstruct history
    │                │   ├── System message "Resuming session: a1b2c3d4"
    │                │   └── State → Idle
    │                │
    │                └── User cancels (Esc)
    │                    ├── System message "Cancelled."
    │                    └── State → Idle
    │
    └── Error → AppEvent::SystemMessage("Error loading sessions: ...")
```

### Plan Acceptance Flow

```
User types "/accept" and presses Enter (while in Plan mode with a plan)
    │
    ▼
commands::dispatch("/accept", ...) → CommandResult::ShowModePicker
    │
    ▼
handle_command_result: Create ModePicker, State → ModePicker
    │
    ├── User selects Guided (Enter)
    │   ├── try_select() → Some(Mode::Guided)
    │   ├── Swap handler to TuiApprovalHandler
    │   ├── orchestrator.accept_plan(Mode::Guided)
    │   ├── System message "✓ Plan accepted. Switched to GUIDED mode."
    │   └── State → Thinking (executing plan)
    │
    ├── User selects Execute (Enter)
    │   ├── try_select() → Some(Mode::Execute)
    │   ├── Swap handler to DiffOnlyApprovalHandler
    │   ├── orchestrator.accept_plan(Mode::Execute)
    │   └── State → Thinking
    │
    ├── User selects Auto (Enter)
    │   ├── try_select() → None (dangerous → confirmation)
    │   ├── State → ModePicker { confirming_auto: true }
    │   ├── Render warning overlay
    │   │
    │   ├── User presses 'y'
    │   │   ├── confirm_auto() → Mode::Auto
    │   │   ├── Swap handler to DiffOnlyApprovalHandler
    │   │   ├── orchestrator.accept_plan(Mode::Auto)
    │   │   └── State → Thinking
    │   │
    │   └── User presses 'n'
    │       ├── cancel_auto()
    │       └── State → ModePicker { confirming_auto: false }
    │
    └── User cancels (Esc)
        ├── System message "Cancelled."
        └── State → Idle
```

### Handler Swapping on Mode Change

```
Mode switch requested (via /guided, /execute, /auto, or /accept)
    │
    ▼
handler_for_mode(mode, event_tx)
    │
    ├── Guided → TuiApprovalHandler::new(event_tx.clone())
    │     Uses the TUI overlay for file change approval.
    │
    ├── Execute → DiffOnlyApprovalHandler::new()
    │     Shows diff in chat (via tool events), auto-approves.
    │
    └── Auto → DiffOnlyApprovalHandler::new()
          Auto-approves without prompting.
    │
    ▼
orchestrator.set_mode_with_handler(mode, Some(handler))
    │
    ├── Swaps approval_handler (Arc<dyn ApprovalHandler>)
    ├── Calls set_mode(mode) which:
    │   ├── Emits SessionEvent::ModeChange
    │   ├── Rebuilds tool registry with new mode
    │   └── Rebuilds system prompt with mode-specific instructions
    └── All write tools now use the new handler
```

---

## 11. Implementation Order

Build in this order to keep the project compilable at each step:

| Step | File | Why this order |
|------|------|----------------|
| 1 | `src/tui/theme.rs` | Add OVERLAY_BG, OVERLAY_BORDER constants (no dependencies) |
| 2 | `src/tui/events.rs` | Add ApprovalRequest, SessionListReady variants (needs FileChange, SessionMeta imports) |
| 3 | `src/tui/approval_overlay.rs` | Data model + diff computation + rendering (depends on FileChange, theme, events) |
| 4 | `src/tui/diff_view.rs` | Full-screen viewer (depends on DiffLine from approval_overlay) |
| 5 | `src/tui/session_picker.rs` | Session list overlay (depends on SessionMeta, theme) |
| 6 | `src/tui/mode_picker.rs` | Mode selection overlay (depends on Mode, theme) |
| 7 | `src/tui/tui_approval_handler.rs` | ApprovalHandler impl (depends on events::AppEvent) |
| 8 | `src/tui/mod.rs` | Add 5 new module declarations |
| 9 | `src/tui/keybindings.rs` | Add new Action variants + state handlers |
| 10 | `src/tui/app.rs` | Add new AppState variants, overlay fields, event/action handling |
| 11 | `src/tui/layout.rs` | Wire overlay rendering based on AppState |
| 12 | `src/tui/commands.rs` | Wire ShowSessionPicker/ShowModePicker handling |
| 13 | — | `cargo test && cargo clippy` |

---

## 12. Tests

### Test Summary

| File | # Tests | Coverage |
|------|---------|----------|
| `approval_overlay.rs` | 5 | Diff computation (modify/new file), scroll clamping, approve/reject channels |
| `diff_view.rs` | 4 | Scroll to top/bottom, scroll up/down clamping |
| `session_picker.rs` | 4 | Empty sessions, move up/down clamping, ensure_visible scrolling |
| `mode_picker.rs` | 7 | Initial selection, move up/down, select Guided (immediate), select Auto (confirmation), cancel auto, confirm auto |
| `tui_approval_handler.rs` | 1 | Round-trip send/receive via channels |
| `keybindings.rs` | 8 | map_approval (y/n/d), map_diff_view (j/k/q), map_list_picker (up/down/enter/esc), map_mode_confirm (y/n) |
| **Total** | **29** | |

### `keybindings.rs` Additional Tests

```rust
#[cfg(test)]
mod tests {
    // ... existing tests ...

    #[test]
    fn approval_y_approves() {
        let state = AppState::AwaitingApproval;
        assert_eq!(map_key(key(KeyCode::Char('y')), &state), Action::ApprovalApprove);
    }

    #[test]
    fn approval_n_rejects() {
        let state = AppState::AwaitingApproval;
        assert_eq!(map_key(key(KeyCode::Char('n')), &state), Action::ApprovalReject);
    }

    #[test]
    fn approval_d_opens_diff() {
        let state = AppState::AwaitingApproval;
        assert_eq!(map_key(key(KeyCode::Char('d')), &state), Action::ApprovalViewDiff);
    }

    #[test]
    fn diff_view_j_scrolls_down() {
        let state = AppState::DiffView;
        assert_eq!(map_key(key(KeyCode::Char('j')), &state), Action::DiffScrollDown);
    }

    #[test]
    fn diff_view_q_quits() {
        let state = AppState::DiffView;
        assert_eq!(map_key(key(KeyCode::Char('q')), &state), Action::DiffQuit);
    }

    #[test]
    fn session_picker_enter_selects() {
        let state = AppState::SessionPicker;
        assert_eq!(map_key(key(KeyCode::Enter), &state), Action::ListSelect);
    }

    #[test]
    fn mode_picker_enter_selects() {
        let state = AppState::ModePicker { confirming_auto: false };
        assert_eq!(map_key(key(KeyCode::Enter), &state), Action::ListSelect);
    }

    #[test]
    fn mode_confirm_y_confirms() {
        let state = AppState::ModePicker { confirming_auto: true };
        assert_eq!(map_key(key(KeyCode::Char('y')), &state), Action::ModeConfirmYes);
    }
}
```

---

## 13. Verification Checklist

### Automated

```bash
cargo test          # All existing + ~29 new tests pass
cargo clippy        # No warnings
cargo fmt --check   # Formatted
```

### Manual

- [ ] In Guided mode, writing a file shows the approval overlay with colorized diff
- [ ] Approval overlay shows file path, "CREATE" or "MODIFY" badge
- [ ] Addition lines are green, deletion lines are red, context lines are dim
- [ ] Change summary shows correct addition/deletion counts
- [ ] Pressing `y` approves the change, system message confirms, orchestrator continues
- [ ] Pressing `n` rejects the change, system message confirms, orchestrator continues
- [ ] Pressing `Esc` rejects (same as `n`)
- [ ] Pressing `d` opens the full-screen diff viewer
- [ ] Diff viewer shows file path and line position (N/M)
- [ ] `j`/`k` scroll one line at a time
- [ ] `Ctrl+d`/`Ctrl+u` scroll half-page
- [ ] `g` scrolls to top, `G` scrolls to bottom
- [ ] `q` returns to the approval overlay (not to Idle)
- [ ] `/resume` opens session picker overlay with recent sessions
- [ ] Sessions sorted most recent first
- [ ] Each session shows ID prefix, relative time, mode, preview text
- [ ] Navigating with Up/Down highlights entries
- [ ] Enter resumes the selected session
- [ ] Esc cancels and returns to Idle
- [ ] Empty session list shows system message instead of overlay
- [ ] `/accept` in Plan mode with a plan opens mode picker
- [ ] Mode picker shows Guided, Execute, Auto with descriptions
- [ ] Auto option shows "DANGER" indicator
- [ ] Selecting Guided/Execute immediately accepts the plan
- [ ] Selecting Auto shows warning confirmation overlay
- [ ] Pressing `y` on Auto confirmation accepts plan in Auto mode
- [ ] Pressing `n` returns to mode selection
- [ ] `/accept` outside Plan mode shows error message
- [ ] `/accept` without a plan shows error message
- [ ] Switching to Guided mode enables TUI approval overlays
- [ ] Switching to Execute/Auto mode uses DiffOnlyApprovalHandler
- [ ] Status bar updates after mode switch
- [ ] Multiple rapid approvals don't cause channel deadlock

---

## Estimated Scope

| Metric | Value |
|--------|-------|
| New files | 5 |
| Modified files | 6 |
| New lines (est.) | ~1,500 |
| New tests | ~29 |
