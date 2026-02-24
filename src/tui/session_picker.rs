use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};

use crate::session::SessionMeta;

use super::theme::TuiTheme;

const MAX_VISIBLE: usize = 10;

/// Overlay for selecting a session to resume.
pub struct SessionPicker {
    pub sessions: Vec<SessionMeta>,
    pub selected: usize,
    pub scroll_offset: usize,
}

impl SessionPicker {
    pub fn new(sessions: Vec<SessionMeta>) -> Self {
        Self {
            sessions,
            selected: 0,
            scroll_offset: 0,
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
        }
        if self.selected >= self.scroll_offset + MAX_VISIBLE {
            self.scroll_offset = self.selected + 1 - MAX_VISIBLE;
        }
    }
}

pub fn render(frame: &mut Frame, picker: &SessionPicker, terminal_area: Rect) {
    let overlay_width = (terminal_area.width.saturating_sub(6)).min(90);
    let visible_count = picker.sessions.len().min(MAX_VISIBLE);
    let overlay_height = (visible_count as u16) + 4; // border + title + bottom

    let x = (terminal_area.width.saturating_sub(overlay_width)) / 2;
    let y = (terminal_area.height.saturating_sub(overlay_height)) / 2;
    let area = Rect::new(x, y, overlay_width, overlay_height);

    frame.render_widget(Clear, area);

    let title = " Resume Session ".to_string();
    let count = format!(" {} sessions ", picker.sessions.len());

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(TuiTheme::ACCENT))
        .padding(Padding::horizontal(1))
        .title_top(title)
        .title_top(Line::from(count.fg(TuiTheme::FG_DIM)).right_aligned())
        .title_bottom(" [Enter] resume  [Esc] cancel ".fg(TuiTheme::FG_DIM));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let max_width = inner.width as usize;

    let rows: Vec<Line> = picker
        .sessions
        .iter()
        .enumerate()
        .skip(picker.scroll_offset)
        .take(visible_count)
        .map(|(i, meta)| {
            let id_prefix = &meta.session_id.as_str()[..8];
            let time = meta.relative_time();
            let preview = meta.truncated_preview();

            let text = format!(
                " {}  {:>16}  {:>7}  \"{}\"",
                id_prefix, time, meta.mode, preview
            );
            let display = if text.len() > max_width {
                format!("{}...", &text[..max_width.saturating_sub(3)])
            } else {
                text
            };

            if i == picker.selected {
                Line::from(Span::styled(
                    display,
                    Style::default()
                        .fg(TuiTheme::PICKER_HIGHLIGHT_FG)
                        .bg(TuiTheme::PICKER_HIGHLIGHT_BG),
                ))
            } else {
                Line::from(Span::styled(display, Style::default().fg(TuiTheme::FG)))
            }
        })
        .collect();

    frame.render_widget(Paragraph::new(rows), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionId;
    use chrono::Utc;

    fn make_meta(preview: &str) -> SessionMeta {
        SessionMeta {
            session_id: SessionId::new(),
            model: "test".into(),
            mode: "auto".into(),
            working_directory: "/tmp".into(),
            started_at: Utc::now(),
            last_active: Utc::now(),
            preview: preview.into(),
        }
    }

    #[test]
    fn empty_sessions() {
        let picker = SessionPicker::new(vec![]);
        assert!(picker.selected_session().is_none());
    }

    #[test]
    fn move_up_at_top() {
        let mut picker = SessionPicker::new(vec![make_meta("a"), make_meta("b")]);
        picker.move_up();
        assert_eq!(picker.selected, 0);
    }

    #[test]
    fn move_down_at_bottom() {
        let mut picker = SessionPicker::new(vec![make_meta("a"), make_meta("b")]);
        picker.move_down();
        assert_eq!(picker.selected, 1);
        picker.move_down();
        assert_eq!(picker.selected, 1);
    }

    #[test]
    fn ensure_visible_scrolls() {
        let sessions: Vec<_> = (0..15)
            .map(|i| make_meta(&format!("session {}", i)))
            .collect();
        let mut picker = SessionPicker::new(sessions);
        for _ in 0..14 {
            picker.move_down();
        }
        assert_eq!(picker.selected, 14);
        assert!(picker.scroll_offset > 0);
        assert!(picker.selected < picker.scroll_offset + MAX_VISIBLE);
    }
}
