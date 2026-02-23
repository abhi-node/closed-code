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
        AppState::Thinking => map_thinking(key),
        AppState::Streaming => map_streaming(key),
        AppState::ToolExecuting { .. } => map_thinking(key),
        AppState::Exiting => Action::Noop,
    }
}

fn map_idle(key: KeyEvent) -> Action {
    match (key.code, key.modifiers) {
        // Submit
        (KeyCode::Enter, m) if !m.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => {
            Action::Submit
        }

        // Newline
        (KeyCode::Enter, m) if m.contains(KeyModifiers::SHIFT) => Action::InsertNewline,
        (KeyCode::Enter, m) if m.contains(KeyModifiers::ALT) => Action::InsertNewline,

        // Editor / Clear
        (KeyCode::Char('g'), m) if m.contains(KeyModifiers::CONTROL) => Action::OpenEditor,
        (KeyCode::Char('u'), m) if m.contains(KeyModifiers::CONTROL) => Action::ClearInput,

        // Cursor movement
        (KeyCode::Left, _) => Action::CursorLeft,
        (KeyCode::Right, _) => Action::CursorRight,
        (KeyCode::Home, _) => Action::CursorHome,
        (KeyCode::End, _) => Action::CursorEnd,

        // History / scroll
        (KeyCode::Up, _) => Action::HistoryPrev,
        (KeyCode::Down, _) => Action::HistoryNext,
        (KeyCode::PageUp, _) => Action::PageUp,
        (KeyCode::PageDown, _) => Action::PageDown,

        // Edit
        (KeyCode::Backspace, _) => Action::Backspace,
        (KeyCode::Delete, _) => Action::Delete,
        (KeyCode::Esc, _) => Action::ClearInput,

        // Printable characters
        (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => Action::InsertChar(c),

        _ => Action::Noop,
    }
}

/// While thinking or executing tools, only global keys (Cancel/Exit) work.
fn map_thinking(_key: KeyEvent) -> Action {
    Action::Noop
}

/// While streaming, allow scroll keys plus globals.
fn map_streaming(key: KeyEvent) -> Action {
    match (key.code, key.modifiers) {
        (KeyCode::PageUp, _) => Action::PageUp,
        (KeyCode::PageDown, _) => Action::PageDown,
        _ => Action::Noop,
    }
}

fn map_picker(key: KeyEvent) -> Action {
    match (key.code, key.modifiers) {
        (KeyCode::Up, _) => Action::PickerUp,
        (KeyCode::Down, _) => Action::PickerDown,
        (KeyCode::Enter, _) => Action::PickerSelect,
        (KeyCode::Esc, _) => Action::PickerDismiss,
        (KeyCode::Backspace, _) => Action::PickerBackspace,
        (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => Action::PickerFilter(c),
        _ => Action::Noop,
    }
}

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
        assert_eq!(
            map_key(key(KeyCode::Enter), &AppState::Idle),
            Action::Submit
        );
    }

    #[test]
    fn idle_shift_enter_newline() {
        assert_eq!(
            map_key(shift(KeyCode::Enter), &AppState::Idle),
            Action::InsertNewline
        );
    }

    #[test]
    fn idle_alt_enter_newline() {
        assert_eq!(
            map_key(alt(KeyCode::Enter), &AppState::Idle),
            Action::InsertNewline
        );
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
        assert_eq!(
            map_key(key(KeyCode::Char('a')), &AppState::Idle),
            Action::InsertChar('a')
        );
    }

    #[test]
    fn idle_escape_clears() {
        assert_eq!(
            map_key(key(KeyCode::Esc), &AppState::Idle),
            Action::ClearInput
        );
    }

    #[test]
    fn idle_arrow_up_history() {
        assert_eq!(
            map_key(key(KeyCode::Up), &AppState::Idle),
            Action::HistoryPrev
        );
    }

    #[test]
    fn picker_enter_selects() {
        let state = AppState::CommandPicker {
            filter: String::new(),
            selected: 0,
        };
        assert_eq!(map_key(key(KeyCode::Enter), &state), Action::PickerSelect);
    }

    #[test]
    fn picker_escape_dismisses() {
        let state = AppState::CommandPicker {
            filter: String::new(),
            selected: 0,
        };
        assert_eq!(map_key(key(KeyCode::Esc), &state), Action::PickerDismiss);
    }

    #[test]
    fn picker_char_filters() {
        let state = AppState::CommandPicker {
            filter: String::new(),
            selected: 0,
        };
        assert_eq!(
            map_key(key(KeyCode::Char('h')), &state),
            Action::PickerFilter('h')
        );
    }

    #[test]
    fn picker_arrows_navigate() {
        let state = AppState::CommandPicker {
            filter: String::new(),
            selected: 0,
        };
        assert_eq!(map_key(key(KeyCode::Up), &state), Action::PickerUp);
        assert_eq!(map_key(key(KeyCode::Down), &state), Action::PickerDown);
    }

    #[test]
    fn thinking_ignores_regular_keys() {
        assert_eq!(
            map_key(key(KeyCode::Char('a')), &AppState::Thinking),
            Action::Noop
        );
        assert_eq!(
            map_key(key(KeyCode::Enter), &AppState::Thinking),
            Action::Noop
        );
    }

    #[test]
    fn thinking_allows_cancel() {
        assert_eq!(map_key(ctrl('c'), &AppState::Thinking), Action::Cancel);
    }

    #[test]
    fn streaming_allows_page_scroll() {
        assert_eq!(
            map_key(key(KeyCode::PageUp), &AppState::Streaming),
            Action::PageUp
        );
        assert_eq!(
            map_key(key(KeyCode::PageDown), &AppState::Streaming),
            Action::PageDown
        );
    }

    #[test]
    fn streaming_ignores_regular_keys() {
        assert_eq!(
            map_key(key(KeyCode::Char('a')), &AppState::Streaming),
            Action::Noop
        );
    }

    #[test]
    fn tool_executing_ignores_keys() {
        let state = AppState::ToolExecuting {
            tool_name: "read_file".into(),
        };
        assert_eq!(map_key(key(KeyCode::Char('a')), &state), Action::Noop);
    }
}
