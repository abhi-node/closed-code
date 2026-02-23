# Phase 9: Full-Screen TUI with Ratatui

> Transform closed-code from a line-based REPL into a beautiful, modern full-screen terminal UI using ratatui, with polished visuals, interactive command picker, context gauge, and seamless streaming.

---

## Table of Contents

1. [Goal & Vision](#1-goal--vision)
2. [Architecture Overview](#2-architecture-overview)
3. [Visual Design & Layout](#3-visual-design--layout)
4. [Color Theme System](#4-color-theme-system)
5. [Header Bar](#5-header-bar)
6. [Chat Area](#6-chat-area)
7. [Input Pane](#7-input-pane)
8. [Status Bar](#8-status-bar)
9. [Command Picker Overlay](#9-command-picker-overlay)
10. [Approval Overlay](#10-approval-overlay)
11. [Full-Screen Diff Viewer](#11-full-screen-diff-viewer)
12. [Session Picker Overlay](#12-session-picker-overlay)
13. [Streaming & Animations](#13-streaming--animations)
14. [Keyboard Handling](#14-keyboard-handling)
15. [App State Machine](#15-app-state-machine)
16. [Event System](#16-event-system)
17. [Integration with Existing Systems](#17-integration-with-existing-systems)
18. [File Structure](#18-file-structure)
19. [Dependencies](#19-dependencies)
20. [Migration Strategy](#20-migration-strategy)
21. [Verification](#21-verification)

---

## 1. Goal & Vision

Replace the current line-based REPL (`rustyline` + `indicatif` + `dialoguer` + raw `crossterm` printing) with a full-screen ratatui terminal UI that:

- Runs in **alternate screen mode** (clean entry/exit, no pollution of scrollback)
- Renders a **three-zone layout**: header bar, scrollable chat area, input pane + status bar
- Shows a **command picker overlay** when `/` is typed (filterable, arrow-key navigable)
- Displays a **context usage gauge** (tokens used / max, as a visual bar)
- Streams LLM responses **character-by-character** with a blinking cursor indicator
- Presents **diffs and approvals** as modal overlays within the TUI
- Maintains **all existing functionality** (22+ slash commands, modes, sessions, git, sandbox)
- Looks **beautiful** — Tailwind-inspired color palette, rounded borders, clean typography

### What We're Replacing

| Current (REPL) | Phase 9 (TUI) |
|----------------|----------------|
| `rustyline` line editor | `tui-textarea` multi-line input widget |
| `indicatif` spinner | Custom ratatui spinner widget (braille animation) |
| `dialoguer::Select` pickers | Ratatui overlay widgets (command picker, session picker) |
| `dialoguer::Confirm` approval | Ratatui approval overlay with inline diff |
| Raw `println!` + crossterm colors | Ratatui styled `Paragraph` / `Line` / `Span` widgets |
| Scrollback-based history | Scrollable viewport with explicit scroll state |

### What We're Keeping

- **All business logic** — orchestrator, agents, tools, sessions, git, sandbox
- **Gemini streaming pipeline** — `consume_stream()` + `StreamEvent` enum
- **Session persistence** — JSONL events, SessionStore
- **Approval trait** — new `TuiApprovalHandler` implements existing `ApprovalHandler` trait
- **Config system** — all settings, TOML files, CLI flags

---

## 2. Architecture Overview

```
┌─────────────────────────────────────────────────────┐
│                    App (tui/app.rs)                  │
│  ┌────────────┐    ┌──────────────┐                 │
│  │  Terminal   │    │  Event Loop  │                 │
│  │  (ratatui)  │◄──│  (tokio +    │                 │
│  │             │    │   crossterm) │                 │
│  └────────────┘    └──────┬───────┘                 │
│                           │                         │
│         ┌─────────────────┼─────────────────┐       │
│         ▼                 ▼                 ▼       │
│  ┌────────────┐   ┌────────────┐   ┌────────────┐  │
│  │  Terminal   │   │    App     │   │  Overlay   │  │
│  │  Events     │   │  Events    │   │  State     │  │
│  │ (keys,mouse │   │ (text delta│   │ (command   │  │
│  │  resize)    │   │  tool call │   │  picker,   │  │
│  │             │   │  done)     │   │  approval, │  │
│  └────────────┘   └────────────┘   │  diff view)│  │
│                                     └────────────┘  │
│                                                     │
│  Render Pipeline:                                   │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐         │
│  │ Header   │  │ Chat     │  │ Input +  │         │
│  │ Bar      │  │ Area     │  │ Status   │         │
│  └──────────┘  └──────────┘  └──────────┘         │
│       + Overlay Layer (command picker / approval)   │
└─────────────────────────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────────────────────────┐
│            Existing Business Logic (unchanged)       │
│  Orchestrator │ Agents │ Tools │ Sessions │ Git     │
└─────────────────────────────────────────────────────┘
```

**Key architectural decisions:**
- **Immediate-mode rendering** — rebuild entire UI every frame (ratatui standard)
- **Dual event sources** — terminal events (crossterm) + app events (mpsc channel)
- **Overlay system** — command picker, approval, diff, session picker rendered on top of base layout
- **100ms tick rate** — smooth spinner animations without excessive CPU

---

## 3. Visual Design & Layout

### Main Screen Layout

```
╭─ closed-code ─────────────────────────────── session: a1b2c3d4 ─╮
│                                                                    │
│  ╭─ You ──────────────────────────────────────────────────────╮   │
│  │ What files are in this project?                             │   │
│  ╰─────────────────────────────────────────────────────────────╯   │
│                                                                    │
│  ╭─ Assistant ────────────────────────────────────────────────╮   │
│  │ ⠋ Using list_directory...                                   │   │
│  │                                                             │   │
│  │ Here are the files in your project:                         │   │
│  │                                                             │   │
│  │ ┌──────────────────────────────────────────────────────┐   │   │
│  │ │ Cargo.toml                                            │   │   │
│  │ │ src/                                                  │   │   │
│  │ │   main.rs                                             │   │   │
│  │ │   lib.rs                                              │   │   │
│  │ └──────────────────────────────────────────────────────┘   │   │
│  │                                                             │   │
│  │ The project has 2 source files and a manifest.              │   │
│  ╰─────────────────────────────────────────────────────────────╯   │
│                                                                    │
│                                                     ↑ 12 more     │
╞════════════════════════════════════════════════════════════════════╡
│ Type a message, / for commands                                     │
│ █                                                                  │
╞════════════════════════════════════════════════════════════════════╡
│ EXPLORE │ gemini-3.1-pro │ 42/50 turns │ ████████░░ 78% │ main ▲3│
╰────────────────────────────────────────────────────────────────────╯
```

### Layout Constraints (ratatui)

```rust
// Root: vertical stack
let [header, chat, input_divider, input, status_divider, status] = Layout::vertical([
    Constraint::Length(1),      // Header bar (single line)
    Constraint::Fill(1),        // Chat area (takes all remaining space)
    Constraint::Length(1),      // Divider
    Constraint::Length(3),      // Input area (min 3 lines, grows)
    Constraint::Length(1),      // Divider
    Constraint::Length(1),      // Status bar (single line)
]).areas(frame.area());
```

---

## 4. Color Theme System

Migrate from `crossterm::style::Color` to ratatui's built-in Tailwind palette for a modern, cohesive look.

### Theme Constants (`tui/theme.rs`)

```rust
use ratatui::style::{Color, Style, Stylize};
use ratatui::style::palette::tailwind;

pub struct TuiTheme;

impl TuiTheme {
    // ── Base ──
    pub const BG: Color = tailwind::SLATE.c950;           // Deep dark background
    pub const FG: Color = tailwind::SLATE.c200;           // Primary text
    pub const FG_DIM: Color = tailwind::SLATE.c500;       // Secondary/muted text
    pub const FG_MUTED: Color = tailwind::SLATE.c600;     // Very dim text (hints)

    // ── Borders ──
    pub const BORDER: Color = tailwind::SLATE.c700;       // Default borders
    pub const BORDER_FOCUS: Color = tailwind::BLUE.c400;  // Focused/active borders
    pub const BORDER_DIM: Color = tailwind::SLATE.c800;   // Subtle borders

    // ── Accents ──
    pub const ACCENT: Color = tailwind::BLUE.c400;        // Primary accent (mode badges, links)
    pub const ACCENT_DIM: Color = tailwind::BLUE.c800;    // Subtle accent background

    // ── Semantic ──
    pub const SUCCESS: Color = tailwind::EMERALD.c400;    // Success indicators
    pub const WARNING: Color = tailwind::AMBER.c400;      // Warnings
    pub const ERROR: Color = tailwind::RED.c400;          // Errors
    pub const INFO: Color = tailwind::SKY.c400;           // Informational

    // ── Message Roles ──
    pub const USER: Color = tailwind::CYAN.c400;          // User message headers
    pub const USER_BG: Color = tailwind::CYAN.c950;       // Subtle user message background
    pub const ASSISTANT: Color = tailwind::VIOLET.c400;   // Assistant message headers
    pub const TOOL: Color = tailwind::AMBER.c400;         // Tool call indicators
    pub const AGENT: Color = tailwind::TEAL.c400;         // Sub-agent indicators

    // ── Diff ──
    pub const DIFF_ADD: Color = tailwind::EMERALD.c400;   // Added lines
    pub const DIFF_ADD_BG: Color = tailwind::EMERALD.c950;// Added line background
    pub const DIFF_DEL: Color = tailwind::RED.c400;       // Deleted lines
    pub const DIFF_DEL_BG: Color = tailwind::RED.c950;    // Deleted line background
    pub const DIFF_HUNK: Color = tailwind::BLUE.c400;     // @@ hunk headers
    pub const DIFF_CONTEXT: Color = tailwind::SLATE.c500; // Context lines

    // ── Mode Colors ──
    pub const MODE_EXPLORE: Color = tailwind::BLUE.c400;
    pub const MODE_PLAN: Color = tailwind::VIOLET.c400;
    pub const MODE_GUIDED: Color = tailwind::AMBER.c400;
    pub const MODE_EXECUTE: Color = tailwind::EMERALD.c400;
    pub const MODE_AUTO: Color = tailwind::RED.c400;

    // ── Gauge ──
    pub const GAUGE_LOW: Color = tailwind::EMERALD.c400;  // 0-60%
    pub const GAUGE_MED: Color = tailwind::AMBER.c400;    // 60-85%
    pub const GAUGE_HIGH: Color = tailwind::RED.c400;     // 85-100%

    // ── Command Picker ──
    pub const PICKER_HIGHLIGHT_BG: Color = tailwind::BLUE.c800;
    pub const PICKER_HIGHLIGHT_FG: Color = tailwind::SLATE.c100;
    pub const PICKER_MATCH: Color = tailwind::AMBER.c400; // Fuzzy match highlight

    // ── Spinner Frames ──
    pub const SPINNER_FRAMES: &'static [&'static str] = &[
        "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"
    ];
}
```

### Mode Badge Styling

Each mode gets a distinct colored badge in the status bar:

| Mode | Color | Badge |
|------|-------|-------|
| Explore | Blue | ` EXPLORE ` |
| Plan | Violet | ` PLAN ` |
| Guided | Amber | ` GUIDED ` |
| Execute | Emerald | ` EXECUTE ` |
| Auto | Red | ` AUTO ` |

---

## 5. Header Bar

Single-line header with app title, session info, and optional git context.

### Layout

```
╭─ closed-code ──────────────────────────────── session: a1b2c3d4 ─╮
```

### Structure

```rust
// Left: app name
// Right: session ID (dimmed)
let [left, right] = Layout::horizontal([
    Constraint::Fill(1),
    Constraint::Length(22),
]).areas(header);
```

### Content

| Element | Style | Content |
|---------|-------|---------|
| App name | `ACCENT` + bold | `closed-code` |
| Session ID | `FG_DIM` | `session: {id[..8]}` (or blank if no session) |

Border: Rounded top border only (`Borders::TOP | Borders::LEFT | Borders::RIGHT`), `BORDER_DIM` color.

---

## 6. Chat Area

The main conversation viewport. Scrollable, auto-scrolls to bottom on new content, supports manual scroll-back.

### Message Types & Rendering

#### User Messages

```
╭─ You ──────────────────────────────────────────────────────────╮
│ What files are in this project?                                 │
╰─────────────────────────────────────────────────────────────────╯
```

- Border: `Rounded`, color `USER` (Cyan)
- Title: ` You ` in `USER` color, bold
- Content: `FG` color, left-aligned
- Padding: `Padding::horizontal(1)`
- Background: very subtle `USER_BG` tint (optional, can be default)

#### Assistant Messages

```
╭─ Assistant ────────────────────────────────────────────────────╮
│ Here are the files in your project:                             │
│                                                                 │
│ ┌──────────────────────────────────────────────────────────┐   │
│ │ Cargo.toml                                                │   │
│ │ src/main.rs                                               │   │
│ └──────────────────────────────────────────────────────────┘   │
│                                                                 │
│ The project has 2 source files and a manifest.                  │
╰─────────────────────────────────────────────────────────────────╯
```

- Border: `Rounded`, color `ASSISTANT` (Violet)
- Title: ` Assistant ` in `ASSISTANT` color, bold
- Content: Full markdown rendering (see Markdown Widget below)
- Padding: `Padding::horizontal(1)`

#### Tool Call Indicators (Inline in Assistant Messages)

```
 ✓ read_file(path: "src/main.rs")                         0.3s
```

- Checkmark: `SUCCESS` color
- Tool name: `TOOL` color, bold
- Args: `FG_DIM` color, truncated at 50 chars
- Duration: `FG_MUTED`, right-aligned

While executing:
```
 ⠋ Using read_file(path: "src/main.rs")...
```

- Spinner: `TOOL` color, animated
- Text: `FG_DIM`

#### Sub-Agent Activity

```
 ┌─ explorer ──────────────────────────────────────────────────┐
 │  ⠋ Researching... (3 tool calls)                            │
 └─────────────────────────────────────────────────────────────┘
```

- Border: `Rounded`, `AGENT` color
- Agent name in title: `AGENT` color
- Progress inside: spinner + tool call count

When complete:
```
 ┌─ explorer ───────────────────────────────────────── 4.2s ──┐
 │  ✓ Complete (5 tool calls)                                  │
 └─────────────────────────────────────────────────────────────┘
```

#### System Messages (Mode Changes, Compact, etc.)

```
 ── Switched to execute mode (9 tools) ──────────────────────────
```

- Centered text, `FG_DIM` color
- Horizontal rule style using `─` characters
- No border block

### Scrolling Behavior

- **Auto-scroll**: When at bottom, new content scrolls into view automatically
- **Manual scroll**: Arrow Up/Down, Page Up/Down, mouse wheel
- **Scroll lock**: When user scrolls up, auto-scroll pauses. Resume with `End` key or scroll to bottom
- **Scroll indicator**: Bottom-right corner shows `↑ N more` when scrolled up, or `↓ N more` when scrolled down from middle
- **Smooth**: Line-by-line scrolling (not page-jump)

### Scroll Position Indicator

```
                                                       ↑ 42 more
```

Rendered in the chat area's bottom-right margin, `FG_MUTED` color.

### Viewport Implementation

Use a `scroll_offset: usize` state variable. On each render:
1. Calculate total content height (all messages rendered)
2. Visible window = chat area height
3. Render only messages/lines within `[scroll_offset .. scroll_offset + visible_height]`
4. Use ratatui `Paragraph::scroll((offset, 0))` for efficient rendering

---

## 7. Input Pane

Multi-line text input area with placeholder hint, powered by `tui-textarea`.

### Layout

```
╞════════════════════════════════════════════════════════════════════╡
│ Type a message, / for commands                                     │
│ █                                                                  │
╞════════════════════════════════════════════════════════════════════╡
```

### Features

| Feature | Implementation |
|---------|----------------|
| Multi-line input | `tui-textarea::TextArea` widget |
| Placeholder text | `"Type a message, / for commands"` in `FG_MUTED` when empty |
| Cursor | Block cursor (`█`), blinks via tick animation |
| Submit | `Enter` sends message (when not in multi-line mode) |
| Newline | `Shift+Enter` or `Alt+Enter` for multi-line input |
| History | `Arrow Up/Down` when input is empty cycles through history |
| Clear | `Ctrl+U` clears input line |
| Editor | `Ctrl+G` opens `$EDITOR` for long prompts |

### Dividers

Top divider: Double line (`═`) in `BORDER_DIM` color — visually separates chat from input.
Bottom divider: Double line (`═`) in `BORDER_DIM` color — separates input from status.

### Input Height

- Default: 3 lines (1 line content + padding)
- Grows to max 8 lines as user types multi-line content
- Shrinks back after submission

---

## 8. Status Bar

Single-line information-dense status bar at the very bottom.

### Layout

```
│ EXPLORE │ gemini-3.1-pro │ 42/50 turns │ ████████░░ 78% │ main ▲3│
```

### Segments (left to right)

```rust
let [mode, model, turns, gauge, git] = Layout::horizontal([
    Constraint::Length(10),     // Mode badge
    Constraint::Length(18),     // Model name
    Constraint::Length(14),     // Turn counter
    Constraint::Length(20),     // Context gauge
    Constraint::Fill(1),        // Git info (fills remaining)
]).areas(status_bar);
```

#### Mode Badge

```
 EXPLORE
```

- Background: mode color (e.g., `MODE_EXPLORE` = Blue)
- Foreground: White, bold
- Padding: 1 space each side

#### Model Name

```
 gemini-3.1-pro
```

- Color: `FG_DIM`
- Truncated if too long (max 16 chars)
- Separated by `│` divider in `BORDER_DIM`

#### Turn Counter

```
 42/50 turns
```

- Numbers: `FG` color
- "turns" label: `FG_DIM`
- Changes color when approaching limit:
  - Normal (< 80%): `FG`
  - Warning (80-95%): `WARNING`
  - Critical (> 95%): `ERROR`

#### Context Gauge

Visual progress bar showing context window usage.

```
 ████████░░ 78%
```

- Filled blocks: `█` in gauge color (GREEN < 60%, AMBER < 85%, RED >= 85%)
- Empty blocks: `░` in `BORDER_DIM`
- Percentage: color matches gauge
- Width: 10 block characters + 4 chars for percentage

**Calculation**: `used_turns / max_turns * 100`

Alternative when token data is available: `total_tokens / estimated_max_tokens * 100`

#### Git Info

```
 main ▲3
```

- Branch name: `FG_DIM`
- Change indicator: `▲` + count in `WARNING` color (or `✓` in `SUCCESS` if clean)
- If not a git repo: empty or `─` fill

### Separator Characters

Segments separated by `│` in `BORDER_DIM` color with 1-space padding.

---

## 9. Command Picker Overlay

When the user types `/` in the input field, a floating overlay appears above the input area showing all available commands. The overlay filters as the user continues typing.

### Visual Design

```
╭─ Commands ─────────────────────────────────────────────╮
│ > /                                                     │
│                                                         │
│  /help              Show all available commands          │
│  /mode [name]       Show or switch mode                 │
│  /explore           Switch to Explore mode              │
│  /plan              Switch to Plan mode                 │
│  /execute           Switch to Execute mode              │
│  /status            Show session status                 │
│  /diff [opts]       Show git diff                       │
│  /review [target]   Review changes with AI              │
│  /commit [msg]      Generate commit message & commit    │
│                                      ── 9 of 22 ──     │
╰────────────────────────────────────────────────────────╯
```

With filter applied (`/com`):

```
╭─ Commands ─────────────────────────────────────────────╮
│ > /com                                                  │
│                                                         │
│  /commit [msg]      Generate commit message & commit    │
│  /compact [prompt]  Compact conversation history        │
│                                       ── 2 of 22 ──    │
╰────────────────────────────────────────────────────────╯
```

### Behavior

1. **Trigger**: Typing `/` as the first character in empty input activates the picker
2. **Filter**: Each subsequent character narrows the list (fuzzy substring match on command name)
3. **Navigation**: `Arrow Up/Down` moves highlight, `Enter` selects, `Escape` dismisses
4. **Selection**: Selected command replaces input with the command text (e.g., `/commit `)
5. **Dismiss**: `Escape` or `Backspace` past `/` closes the overlay
6. **Pagination**: Shows max 10 items, scrollable. Footer shows `── N of M ──`

### Styling

- Border: `Rounded`, `ACCENT` color
- Title: ` Commands ` in `ACCENT`, bold
- Search input: `> /` prefix in `ACCENT`, user text in `FG`
- Command name: `ACCENT` color, bold
- Command description: `FG_DIM`
- Highlight row: `PICKER_HIGHLIGHT_BG` background, `PICKER_HIGHLIGHT_FG` text
- Match characters: `PICKER_MATCH` color (highlighted matching chars in fuzzy search)
- Count footer: `FG_MUTED`, right-aligned

### Overlay Positioning

- Width: min(terminal_width - 4, 60)
- Height: min(matched_commands + 3, 15)  (3 = search + padding + footer)
- Position: Anchored to bottom of chat area, centered horizontally
- Renders on top of chat content (overlay layer)

### Command Registry for Picker

```rust
struct CommandEntry {
    name: &'static str,           // "/help"
    args: &'static str,           // "" or "[name]" or "[opts]"
    description: &'static str,    // "Show all available commands"
    category: CommandCategory,    // Navigation, Mode, Git, Session, etc.
}

enum CommandCategory {
    Navigation,   // /help, /quit, /clear
    Mode,         // /mode, /explore, /plan, /guided, /execute, /auto, /accept
    Git,          // /diff, /review, /commit
    Session,      // /new, /fork, /compact, /history, /export, /resume
    Config,       // /model, /personality, /sandbox, /status
}
```

All 22+ existing slash commands are registered with their descriptions.

---

## 10. Approval Overlay

Modal overlay for file change approvals. Replaces current `TerminalApprovalHandler` + `DiffOnlyApprovalHandler` with a TUI-native experience.

### Visual Design

```
╭─ Proposed change ── src/main.rs ───────────────────────────────╮
│                                                                  │
│  --- a/src/main.rs                                               │
│  +++ b/src/main.rs                                               │
│  @@ -10,3 +10,7 @@                                              │
│   fn main() {                                                    │
│       println!("Hello, world!");                                 │
│  +    goodbye();                                                 │
│  +}                                                              │
│  +                                                               │
│  +fn goodbye() {                                                 │
│  +    println!("Goodbye!");                                      │
│   }                                                              │
│                                                                  │
│  4 additions, 0 deletions                                        │
│                                                                  │
╞══════════════════════════════════════════════════════════════════╡
│  y  Apply     n  Reject     d  Full diff view     Esc  Cancel   │
╰──────────────────────────────────────────────────────────────────╯
```

### Features

| Feature | Details |
|---------|---------|
| File path | Shown in title, `ACCENT` color |
| Diff display | Colorized unified diff (green adds, red deletes, cyan hunks) |
| Scrollable | Arrow keys scroll through large diffs |
| Line numbers | Gutter with line numbers in `FG_MUTED` |
| Summary | `"N additions, M deletions"` below diff |
| Actions | Key hints at bottom in styled bar |
| New files | Title shows `(new file)` indicator |

### Keybindings

| Key | Action |
|-----|--------|
| `y` | Apply change (write file) |
| `n` | Reject change |
| `d` | Open full-screen diff viewer |
| `Escape` | Same as reject |
| `Arrow Up/Down` | Scroll diff |
| `Page Up/Down` | Fast scroll |

### Overlay Sizing

- Width: min(terminal_width - 6, 100)
- Height: min(terminal_height - 6, diff_lines + 8)
- Centered on screen
- Background: `BG` with border in `ACCENT`

### TuiApprovalHandler

New implementation of `ApprovalHandler` trait:

```rust
pub struct TuiApprovalHandler {
    tx: mpsc::Sender<AppEvent>,
    rx: mpsc::Receiver<ApprovalDecision>,
}

#[async_trait]
impl ApprovalHandler for TuiApprovalHandler {
    async fn request_approval(&self, change: &FileChange) -> Result<ApprovalDecision> {
        // Send event to TUI to show overlay
        self.tx.send(AppEvent::ApprovalRequest(change.clone())).await?;
        // Wait for user decision from TUI
        let decision = self.rx.recv().await?;
        Ok(decision)
    }
}
```

---

## 11. Full-Screen Diff Viewer

Activated from the approval overlay with `d`. Takes over the entire screen.

### Visual Design

```
╭─ Diff ── src/main.rs ──────────── unified ── 4+, 0- ── q to close ─╮
│                                                                        │
│   8 │  use std::io;                                                    │
│   9 │                                                                  │
│  10 │  fn main() {                                                     │
│  11 │      println!("Hello, world!");                                  │
│  12 │+     goodbye();                                                  │
│  13 │+ }                                                               │
│  14 │+                                                                 │
│  15 │+ fn goodbye() {                                                  │
│  16 │+     println!("Goodbye!");                                       │
│  17 │  }                                                               │
│                                                                        │
╞════════════════════════════════════════════════════════════════════════╡
│  j/k  scroll     Ctrl+d/u  half-page     gg/G  top/bottom     q  back│
╰────────────────────────────────────────────────────────────────────────╯
```

### Features

- Full terminal width and height
- Line number gutter
- Syntax-highlighted context lines (via `syntect`)
- Added lines: green text on subtle green background
- Deleted lines: red text on subtle red background
- Vim-style navigation (`j`/`k`, `Ctrl+d`/`Ctrl+u`, `gg`/`G`)
- `q` returns to approval overlay
- Title shows: file path, view mode, change summary, exit hint

---

## 12. Session Picker Overlay

Replaces the current `dialoguer::Select` for `/resume`. Shows as a modal overlay.

### Visual Design

```
╭─ Resume Session ───────────────────────────────────────────────╮
│                                                                  │
│  ▸ a1b2c3d4  (2 minutes ago) — explore — What files exist?      │
│    e5f6g7h8  (1 hour ago)    — plan    — Add caching to API     │
│    i9j0k1l2  (2 days ago)    — execute — Fix auth bug (current) │
│                                                                  │
│                                              ── 3 sessions ──   │
╰──────────────────────────────────────────────────────────────────╯
```

### Features

- Arrow key navigation with highlight
- Current session marked with `(current)` tag
- Pre-selected on current session
- `Enter` to select, `Escape` to cancel
- Shows: short ID, relative time, mode, preview text
- Scrollable for large session lists

---

## 13. Streaming & Animations

### Text Streaming

When the LLM generates text, tokens appear incrementally:

1. **Thinking state**: Show spinner in chat area: `⠋ Thinking...`
2. **First token**: Clear spinner, begin rendering text
3. **Subsequent tokens**: Append to current assistant message, re-render
4. **Cursor indicator**: Blinking `▌` at end of streaming text (toggles every 500ms)
5. **Done**: Remove cursor indicator, finalize message block

### Spinner Widget

Custom ratatui widget for animated spinners:

```rust
pub struct SpinnerWidget {
    message: String,
    frame: usize,      // Current animation frame (0-9)
    style: Style,       // Spinner character style
    msg_style: Style,   // Message text style
}

impl Widget for &SpinnerWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let frame_char = TuiTheme::SPINNER_FRAMES[self.frame % 10];
        let line = Line::from(vec![
            Span::styled(frame_char, self.style),
            Span::raw(" "),
            Span::styled(&self.message, self.msg_style),
        ]);
        line.render(area, buf);
    }
}
```

Spinner frame advances every tick (100ms) while in `Thinking` or `ToolExecuting` state.

### Tool Execution Animation

When a tool is executing:
```
⠹ Using grep(pattern: "TODO")...
```

When complete:
```
✓ grep(pattern: "TODO")                                      0.3s
```

Transition: spinner replaced with checkmark, duration appears right-aligned.

### Status Bar Pulse

When mode changes, the mode badge briefly flashes (inverted colors for 300ms = 3 ticks) then settles.

---

## 14. Keyboard Handling

### Global Keys (Always Active)

| Key | Action |
|-----|--------|
| `Ctrl+C` | Cancel current operation / clear input |
| `Ctrl+D` | Exit application |
| `Ctrl+L` | Force redraw |

### Idle State (Input Focused)

| Key | Action |
|-----|--------|
| `Enter` | Submit input |
| `Shift+Enter` | Insert newline (multi-line) |
| `Ctrl+G` | Open $EDITOR for long prompt |
| `Arrow Up/Down` | Scroll chat (when input empty) / input history |
| `Page Up/Down` | Scroll chat |
| `Home`/`End` | Scroll to top/bottom of chat |
| `Tab` | Accept top command picker suggestion |
| `/` (first char) | Open command picker |
| `Escape` | Clear input / dismiss overlay |

### Command Picker Active

| Key | Action |
|-----|--------|
| `Arrow Up/Down` | Navigate commands |
| `Enter` | Select command |
| `Escape` | Close picker |
| `Backspace` (past /) | Close picker |
| Any char | Filter commands |

### Approval Overlay Active

| Key | Action |
|-----|--------|
| `y` | Apply change |
| `n` | Reject change |
| `d` | Open full diff viewer |
| `Escape` | Reject (same as `n`) |
| `Arrow Up/Down` | Scroll diff |
| `Page Up/Down` | Fast scroll diff |

### Diff Viewer Active

| Key | Action |
|-----|--------|
| `j`/`k` | Scroll line by line |
| `Ctrl+d`/`Ctrl+u` | Half-page scroll |
| `gg` | Jump to top |
| `G` | Jump to bottom |
| `q` | Return to approval overlay |

### Mouse Support

| Action | Effect |
|--------|--------|
| Scroll wheel | Scroll chat area |
| Click on input | Focus input area |

---

## 15. App State Machine

```rust
pub enum AppState {
    /// Idle — waiting for user input. Input area focused.
    Idle,

    /// Thinking — spinner shown, waiting for first LLM token.
    Thinking,

    /// Streaming — LLM tokens arriving, rendering incrementally.
    Streaming,

    /// ToolExecuting — a tool call is in progress (spinner in chat).
    ToolExecuting { tool_name: String },

    /// AwaitingApproval — approval overlay is displayed.
    AwaitingApproval { change: FileChange },

    /// DiffView — full-screen diff viewer active.
    DiffView { change: FileChange },

    /// CommandPicker — command picker overlay is shown.
    CommandPicker { filter: String, selected: usize },

    /// SessionPicker — session resume overlay is shown.
    SessionPicker { sessions: Vec<SessionMeta>, selected: usize },

    /// Exiting — cleanup in progress.
    Exiting,
}
```

### State Transitions

```
Idle
  ├─ User types "/" → CommandPicker
  ├─ User presses Enter → Thinking
  ├─ User types "/resume" → SessionPicker
  └─ Ctrl+D → Exiting

Thinking
  ├─ TextDelta received → Streaming
  ├─ FunctionCall received → ToolExecuting
  └─ Ctrl+C → Idle (cancel)

Streaming
  ├─ More TextDelta → Streaming (append)
  ├─ Done → Idle
  ├─ FunctionCall → ToolExecuting
  └─ Ctrl+C → Idle (cancel)

ToolExecuting
  ├─ Tool complete → Thinking (next API call)
  ├─ ApprovalRequest → AwaitingApproval
  └─ Ctrl+C → Idle (cancel)

AwaitingApproval
  ├─ y → ToolExecuting (approved, continue)
  ├─ n → ToolExecuting (rejected, continue)
  ├─ d → DiffView
  └─ Escape → ToolExecuting (rejected)

DiffView
  └─ q → AwaitingApproval

CommandPicker
  ├─ Enter → Idle (command inserted into input)
  ├─ Escape → Idle
  └─ Backspace past "/" → Idle

SessionPicker
  ├─ Enter → Idle (session loaded)
  └─ Escape → Idle

Exiting
  └─ (cleanup, restore terminal, exit)
```

---

## 16. Event System

### AppEvent Enum

```rust
pub enum AppEvent {
    // ── Terminal Events ──
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize(u16, u16),

    // ── LLM Stream Events ──
    TextDelta(String),
    StreamDone { usage: Option<UsageData> },
    FunctionCallDetected,

    // ── Tool Events ──
    ToolStart { name: String, args_display: String },
    ToolComplete { name: String, duration: Duration },
    ToolError { name: String, error: String },

    // ── Sub-Agent Events ──
    AgentStart { agent_type: String, task: String },
    AgentToolCall { agent_type: String, tool_name: String },
    AgentComplete { agent_type: String, duration: Duration, tool_calls: usize },

    // ── Approval Events ──
    ApprovalRequest(FileChange),
    ApprovalDecision(ApprovalDecision),

    // ── System Events ──
    ModeChanged(Mode),
    SessionChanged(SessionId),
    ContextPruned { removed: usize, remaining: usize },
    RateLimited { retry_after: Duration },
    Error(String),

    // ── Animation Tick ──
    Tick,
}
```

### Event Flow

```
crossterm::event::EventStream ─────┐
                                    ├──► mpsc::Receiver<AppEvent> ──► App::update()
tokio::spawn(orchestrator work) ───┘

100ms Tick Timer ──────────────────►  (Tick events for animations)
```

Two producers:
1. **Terminal events**: crossterm EventStream → map to AppEvent::Key/Mouse/Resize
2. **Orchestrator events**: Sent via `mpsc::Sender<AppEvent>` from the orchestrator task

One consumer:
- **App::update()**: Processes events, updates state, triggers re-render

---

## 17. Integration with Existing Systems

### Orchestrator Integration

The orchestrator currently uses callbacks for streaming. For the TUI, instead of printing directly, it sends events through the channel:

```rust
// Current pattern (REPL):
orchestrator.handle_user_input_streaming(input, |event| {
    match event {
        StreamEvent::TextDelta(text) => print!("{}", text),
        StreamEvent::Done { .. } => {},
    }
}).await?;

// TUI pattern:
let tx = app_event_sender.clone();
orchestrator.handle_user_input_streaming(input, move |event| {
    match event {
        StreamEvent::TextDelta(text) => {
            let _ = tx.blocking_send(AppEvent::TextDelta(text));
        }
        StreamEvent::Done { usage, .. } => {
            let _ = tx.blocking_send(AppEvent::StreamDone { usage });
        }
    }
}).await?;
```

### Approval Handler Integration

New `TuiApprovalHandler` sends `AppEvent::ApprovalRequest` and waits for `AppEvent::ApprovalDecision` via a oneshot channel:

```rust
// In orchestrator setup:
let (approval_tx, approval_rx) = tokio::sync::oneshot::channel();
let handler = TuiApprovalHandler::new(event_tx.clone(), approval_rx);

// In TUI on 'y'/'n' press:
approval_tx.send(ApprovalDecision::Approved).unwrap();
```

### Slash Command Execution

Slash commands are still processed by the existing handler logic in `repl.rs` (or factored out). Their output is sent as `AppEvent` messages to render in the chat area as system messages.

Commands that need overlays (`/resume`, `/accept`) trigger state transitions instead of using `dialoguer`.

### Session Store

Unchanged. The orchestrator still calls `emit_event()` to persist to JSONL. The TUI reads session data through the same `SessionStore` API.

### Config

Add new TUI-specific settings:

```toml
[tui]
# Enable mouse support (default: true)
mouse = true
# Tick rate in milliseconds (default: 100)
tick_rate_ms = 100
```

---

## 18. File Structure

```
src/
  tui/
    mod.rs                 # Module exports, run_tui() entry point
    app.rs                 # App struct, event loop, terminal setup/restore
    theme.rs               # TuiTheme constants (Tailwind palette)
    layout.rs              # Root layout (header, chat, input, status)
    header.rs              # Header bar widget
    chat.rs                # Chat area: message list, scrolling, viewport
    message.rs             # Individual message rendering (user, assistant, tool, system)
    input.rs               # Input pane: tui-textarea wrapper, placeholder
    status_bar.rs          # Status bar: mode badge, model, turns, gauge, git
    command_picker.rs      # Command picker overlay
    approval_overlay.rs    # Approval overlay with inline diff
    diff_view.rs           # Full-screen diff viewer
    session_picker.rs      # Session resume picker overlay
    spinner.rs             # Spinner widget (ratatui native)
    markdown.rs            # Markdown → ratatui Lines/Spans renderer
    gauge.rs               # Context usage gauge widget
    events.rs              # AppEvent enum, event channel setup
    keybindings.rs         # Key event → action mapping per state
```

**17 files** in `src/tui/`, approximately **3,500-4,000 lines** total.

### Files Modified (Not New)

| File | Change |
|------|--------|
| `src/main.rs` | Route to `run_tui()` instead of `run_repl()` |
| `src/repl.rs` | Extract slash command logic into shared module; keep for `--no-tui` fallback |
| `src/ui/approval.rs` | Add `TuiApprovalHandler` implementation |
| `src/agent/orchestrator.rs` | Accept event sender for TUI integration |
| `Cargo.toml` | Add `ratatui`, `tui-textarea` dependencies |
| `src/lib.rs` | Add `pub mod tui;` |
| `src/config.rs` | Add `[tui]` config section |

---

## 19. Dependencies

### Add to Cargo.toml

```toml
ratatui = "0.29"
tui-textarea = "0.7"
```

### Already Present (Reused)

- `crossterm = "0.28"` — ratatui backend + event handling
- `similar = "2"` — diff generation
- `tokio = "1"` — async runtime, channels
- `chrono = "0.4"` — timestamps
- `serde_json = "1"` — data handling

### Potentially Remove (Post-Migration)

- `indicatif = "0.17"` — replaced by ratatui spinner widget
- `dialoguer = "0.11"` — replaced by TUI overlays
- `rustyline = "15"` — replaced by tui-textarea

(Keep until `--no-tui` fallback is confirmed unnecessary.)

---

## 20. Migration Strategy

### Phase 9a: Foundation (App Shell + Layout + Status Bar)

**Goal**: Bare TUI skeleton that launches, renders layout, handles quit.

1. Create `src/tui/mod.rs` with `run_tui()` entry point
2. Create `src/tui/app.rs` — terminal setup, event loop, state machine
3. Create `src/tui/theme.rs` — Tailwind color palette
4. Create `src/tui/layout.rs` — root layout with all zones
5. Create `src/tui/header.rs` — header bar
6. Create `src/tui/status_bar.rs` — mode badge, model, turns, context gauge
7. Create `src/tui/gauge.rs` — context usage gauge widget
8. Create `src/tui/events.rs` — AppEvent enum, dual event sources
9. Wire `src/main.rs` to call `run_tui()` instead of `run_repl()`

**Checkpoint**: Binary launches full-screen TUI with header and status bar. Ctrl+D exits cleanly.

### Phase 9b: Input + Command Picker

**Goal**: User can type messages and use the command picker.

1. Create `src/tui/input.rs` — tui-textarea wrapper with placeholder
2. Create `src/tui/keybindings.rs` — key → action mapping
3. Create `src/tui/command_picker.rs` — overlay with filtering
4. Wire Enter to submit, Escape to clear, `/` to open picker

**Checkpoint**: User can type, command picker opens/filters, Enter submits.

### Phase 9c: Chat Area + Streaming

**Goal**: Messages render in chat, LLM responses stream live.

1. Create `src/tui/chat.rs` — scrollable message list
2. Create `src/tui/message.rs` — user/assistant/tool/system message widgets
3. Create `src/tui/markdown.rs` — markdown → ratatui rendering
4. Create `src/tui/spinner.rs` — animated spinner widget
5. Integrate orchestrator: send AppEvents for text deltas, tool calls
6. Wire scrolling (arrow keys, page up/down, mouse wheel)

**Checkpoint**: Full conversation works with streaming. Tool calls show spinners.

### Phase 9d: Overlays (Approval + Diff + Session Picker)

**Goal**: All interactive overlays work.

1. Create `src/tui/approval_overlay.rs`
2. Create `src/tui/diff_view.rs`
3. Create `src/tui/session_picker.rs`
4. Create `TuiApprovalHandler` in `src/ui/approval.rs`
5. Wire approval flow through event system
6. Wire `/resume` to use session picker overlay

**Checkpoint**: File changes show approval overlay. `/resume` shows picker.

### Phase 9e: Polish + Testing

**Goal**: Everything works smoothly, edge cases handled.

1. Terminal resize handling
2. Error display (rate limits, API errors)
3. Sub-agent activity indicators
4. Scroll position indicator
5. Input history (arrow up/down)
6. `Ctrl+G` editor integration
7. `!` shell prefix execution
8. All slash commands working through TUI
9. Manual QA testing

**Checkpoint**: Full parity with REPL. All 22+ commands work. All modes work.

---

## 21. Verification

### Automated Tests

```bash
# All existing tests pass (no regressions)
cargo test

# TUI-specific unit tests
cargo test --lib tui::
```

Tests for:
- Theme constants are valid
- Command picker filtering logic
- Markdown → Lines/Spans conversion
- Gauge percentage calculation
- State machine transitions
- Message rendering (snapshot tests)

### Manual Testing Checklist

- [ ] Launch: TUI opens in alternate screen, header/status/input visible
- [ ] Exit: Ctrl+D exits cleanly, terminal fully restored
- [ ] Panic: Terminal restores even on crash (panic hook)
- [ ] Resize: Layout reflows correctly on terminal resize
- [ ] Input: Type message, Enter submits, Shift+Enter adds newline
- [ ] Streaming: Response appears token-by-token with cursor indicator
- [ ] Scrolling: Arrow Up/Down, Page Up/Down, mouse wheel all work
- [ ] Auto-scroll: Chat stays at bottom during streaming, pauses on scroll-up
- [ ] Command picker: `/` opens picker, typing filters, Enter selects, Escape closes
- [ ] All slash commands: Test each of the 22+ commands
- [ ] Mode switch: `/explore`, `/plan`, `/execute` — badge updates
- [ ] Status bar: Token count updates, turn counter increments
- [ ] Context gauge: Fills as conversation grows, changes color at thresholds
- [ ] Approval overlay: File write in guided mode triggers overlay
- [ ] Approval keys: `y` applies, `n` rejects, `d` opens diff viewer
- [ ] Diff viewer: Vim navigation works, `q` returns to overlay
- [ ] Session picker: `/resume` shows overlay with sessions
- [ ] Tool execution: Spinner during tool call, checkmark on completion
- [ ] Sub-agents: Explorer/Planner show progress in chat
- [ ] Shell prefix: `!ls` executes and shows output
- [ ] Editor: Ctrl+G opens $EDITOR, returns content to input
- [ ] Git info: Branch and change count shown in status bar
- [ ] Error handling: API errors display in chat without crash

### Performance

- Idle CPU: < 1% (only tick events at 100ms)
- Streaming: Smooth rendering at 60fps text throughput
- Large conversations: Viewport rendering keeps frame time < 16ms
- Memory: Conversation content stored once, not duplicated for rendering

---

## Appendix: ASCII Mockups

### Thinking State

```
╭─ closed-code ──────────────────────────────── session: a1b2c3d4 ─╮
│                                                                    │
│  ╭─ You ──────────────────────────────────────────────────────╮   │
│  │ Analyze the authentication module                           │   │
│  ╰─────────────────────────────────────────────────────────────╯   │
│                                                                    │
│  ⠹ Thinking...                                                     │
│                                                                    │
╞════════════════════════════════════════════════════════════════════╡
│ Type a message, / for commands                                     │
│ █                                                                  │
╞════════════════════════════════════════════════════════════════════╡
│ EXPLORE │ gemini-3.1-pro │  2/50 turns │ ██░░░░░░░░  4% │ main ✓│
╰────────────────────────────────────────────────────────────────────╯
```

### Streaming with Tool Calls

```
╭─ closed-code ──────────────────────────────── session: a1b2c3d4 ─╮
│                                                                    │
│  ╭─ You ──────────────────────────────────────────────────────╮   │
│  │ What does the main function do?                             │   │
│  ╰─────────────────────────────────────────────────────────────╯   │
│                                                                    │
│  ╭─ Assistant ────────────────────────────────────────────────╮   │
│  │  ✓ read_file(path: "src/main.rs")                    0.2s  │   │
│  │                                                             │   │
│  │ The `main` function in `src/main.rs` does the following:    │   │
│  │                                                             │   │
│  │ 1. Installs a panic hook to restore the terminal▌           │   │
│  ╰─────────────────────────────────────────────────────────────╯   │
│                                                                    │
╞════════════════════════════════════════════════════════════════════╡
│ Type a message, / for commands                                     │
│ █                                                                  │
╞════════════════════════════════════════════════════════════════════╡
│ EXPLORE │ gemini-3.1-pro │  4/50 turns │ ███░░░░░░░  8% │ main ✓│
╰────────────────────────────────────────────────────────────────────╯
```

### Command Picker Active

```
╭─ closed-code ──────────────────────────────── session: a1b2c3d4 ─╮
│                                                                    │
│  ╭─ You ──────────────────────────────────────────────────────╮   │
│  │ Analyze the auth module                                     │   │
│  ╰─────────────────────────────────────────────────────────────╯   │
│                                                                    │
│  ╭─ Commands ───────────────────────────────────────────────╮     │
│  │ > /co                                                     │     │
│  │                                                           │     │
│  │  ▸ /commit [msg]      Generate commit message & commit    │     │
│  │    /compact [prompt]  Compact conversation history         │     │
│  │                                        ── 2 of 22 ──     │     │
│  ╰───────────────────────────────────────────────────────────╯     │
│                                                                    │
╞════════════════════════════════════════════════════════════════════╡
│ /co█                                                               │
╞════════════════════════════════════════════════════════════════════╡
│ EXPLORE │ gemini-3.1-pro │  4/50 turns │ ███░░░░░░░  8% │ main ✓│
╰────────────────────────────────────────────────────────────────────╯
```

### High Context Usage Warning

```
│ EXECUTE │ gemini-3.1-pro │ 46/50 turns │ █████████░ 92% │ main ▲5│
```

Mode badge green (Execute), gauge red (92%), turns in amber.

### Guided Mode Approval

```
╭─ closed-code ──────────────────────────────── session: a1b2c3d4 ─╮
│                                                                    │
│  ╭─ Proposed change ── src/auth.rs ─────────────────────────╮     │
│  │                                                           │     │
│  │   12 │  fn validate_token(token: &str) -> bool {          │     │
│  │   13 │-     token.len() > 0                               │     │
│  │   13 │+     !token.is_empty()                             │     │
│  │   14 │  }                                                 │     │
│  │                                                           │     │
│  │  1 addition, 1 deletion                                   │     │
│  │                                                           │     │
│  ╞═══════════════════════════════════════════════════════════╡     │
│  │ y Apply   n Reject   d Full diff   Esc Cancel             │     │
│  ╰───────────────────────────────────────────────────────────╯     │
│                                                                    │
╞════════════════════════════════════════════════════════════════════╡
│ Waiting for approval...                                            │
╞════════════════════════════════════════════════════════════════════╡
│ GUIDED │ gemini-3.1-pro │ 12/50 turns │ █████░░░░░ 24% │ main ▲1│
╰────────────────────────────────────────────────────────────────────╯
```

---

## Estimated Scope

| Sub-phase | Files | Est. Lines | Effort |
|-----------|-------|------------|--------|
| 9a: Foundation | 9 | ~800 | Medium |
| 9b: Input + Picker | 3 | ~500 | Medium |
| 9c: Chat + Streaming | 5 | ~1,200 | High |
| 9d: Overlays | 4 | ~800 | High |
| 9e: Polish + Testing | — | ~500 | Medium |
| **Total** | **17 new + 7 modified** | **~3,800** | **Very High** |
