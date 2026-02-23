use std::path::PathBuf;

use ratatui::style::{Style, Stylize};
use tui_textarea::TextArea;

use super::theme::TuiTheme;

const INPUT_MIN_HEIGHT: u16 = 3;
const INPUT_MAX_HEIGHT: u16 = 8;
const HISTORY_MAX: usize = 200;

pub struct InputPane<'a> {
    textarea: TextArea<'a>,
    pub(crate) history: Vec<String>,
    history_index: Option<usize>,
    saved_input: Option<String>,
    #[allow(dead_code)] // Used in Phase 9c for file path completion
    working_directory: PathBuf,
}

impl<'a> InputPane<'a> {
    pub fn new(working_directory: PathBuf) -> Self {
        let mut textarea = TextArea::default();
        apply_textarea_config(&mut textarea);

        Self {
            textarea,
            history: Vec::new(),
            history_index: None,
            saved_input: None,
            working_directory,
        }
    }

    // ── Text Operations ──

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

    // ── Clear & Submit ──

    pub fn clear(&mut self) {
        self.textarea = TextArea::default();
        apply_textarea_config(&mut self.textarea);
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
        if self.history.last() != Some(&text) {
            self.history.push(text.clone());
            if self.history.len() > HISTORY_MAX {
                self.history.remove(0);
            }
        }
        self.clear();
        Some(text)
    }

    // ── Dynamic Height ──

    /// Calculate the desired height in terminal rows.
    pub fn desired_height(&self) -> u16 {
        let lines = self.textarea.lines().len().max(1) as u16;
        lines.clamp(INPUT_MIN_HEIGHT, INPUT_MAX_HEIGHT)
    }

    // ── Input History ──

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
        let entry = self.history[idx].clone();
        self.set_text(&entry);
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
            let entry = self.history[idx].clone();
            self.set_text(&entry);
        }
    }

    fn reset_history_cycling(&mut self) {
        self.history_index = None;
        self.saved_input = None;
    }

    pub fn set_text(&mut self, text: &str) {
        let lines: Vec<String> = text.lines().map(String::from).collect();
        let lines = if lines.is_empty() {
            vec![String::new()]
        } else {
            lines
        };
        self.textarea = TextArea::new(lines);
        apply_textarea_config(&mut self.textarea);
        // Move cursor to end of content
        self.textarea.move_cursor(tui_textarea::CursorMove::Bottom);
        self.textarea.move_cursor(tui_textarea::CursorMove::End);
    }

    // ── External Editor (Ctrl+G) ──

    /// Open $EDITOR with current input. Returns Ok(true) if content was updated.
    pub fn open_editor(&mut self) -> anyhow::Result<bool> {
        let editor = std::env::var("EDITOR")
            .or_else(|_| std::env::var("VISUAL"))
            .unwrap_or_else(|_| "vi".to_string());

        let temp_path =
            std::env::temp_dir().join(format!("closed-code-input-{}.txt", std::process::id()));
        std::fs::write(&temp_path, self.text())?;

        // Leave alternate screen for the editor
        crossterm::terminal::disable_raw_mode()?;
        crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen)?;

        let status = std::process::Command::new(&editor).arg(&temp_path).status();

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

    // ── Rendering ──

    pub fn textarea(&self) -> &TextArea<'a> {
        &self.textarea
    }
}

fn apply_textarea_config(textarea: &mut TextArea<'_>) {
    textarea.set_placeholder_text("Type a message, / for commands");
    textarea.set_placeholder_style(Style::new().fg(TuiTheme::FG_MUTED));
    textarea.set_cursor_style(Style::new().reversed());
    textarea.set_style(Style::new().fg(TuiTheme::FG));
}

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
