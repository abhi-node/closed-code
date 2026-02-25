use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState},
    Frame,
};

use crate::tui::theme::TuiTheme;

/// File picker overlay widget for fuzzy searching and selecting files.
pub struct FilePicker {
    pub query: String,
    pub matches: Vec<String>,
    pub max_visible: usize,
    pub state: ListState,
}

impl FilePicker {
    pub fn new(max_visible: usize) -> Self {
        Self {
            query: String::new(),
            matches: Vec::new(),
            max_visible,
            state: ListState::default(),
        }
    }

    /// Update the file matches based on the current query using the provided indexer.
    pub fn update_matches(&mut self, indexer: &mut super::file_indexer::FileIndexer) {
        self.matches = indexer.search(&self.query, 10);
        self.state.select(if self.matches.is_empty() {
            None
        } else {
            Some(0)
        });
    }

    /// Select the next item in the list.
    pub fn next(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        let i = match self.state.selected() {
            Some(i) => {
                if i >= self.matches.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.state.select(Some(i));
    }

    /// Select the previous item in the list.
    pub fn previous(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        let i = match self.state.selected() {
            Some(i) => {
                if i == 0 {
                    self.matches.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.state.select(Some(i));
    }

    /// Get the currently selected file path, if any.
    pub fn get_selected(&self) -> Option<&String> {
        self.state.selected().and_then(|i| self.matches.get(i))
    }

    /// Render the file picker overlay.
    pub fn render(
        &mut self,
        frame: &mut Frame,
        terminal_area: Rect,
        chat_area: Rect,
    ) {
        let width = terminal_area.width.saturating_sub(4).min(80);
        let visible_items = self.matches.len().min(self.max_visible);
        let height = (visible_items as u16).saturating_add(4); // Borders + query + padding

        let x = (terminal_area.width.saturating_sub(width)) / 2;
        let y = chat_area.bottom().saturating_sub(height);
        let overlay_area = Rect::new(x, y, width, height);

        // Clear background
        frame.render_widget(Clear, overlay_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(TuiTheme::FILE_PICKER_BORDER))
            .title(format!(" {} ", self.matches.len())); // Matches count

        let query_line = Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Yellow)),
            Span::styled(&self.query, Style::default().fg(TuiTheme::TEXT_NORMAL)),
            Span::styled("█", Style::default().fg(Color::DarkGray)),
        ]);

        let items: Vec<ListItem> = self.matches.iter().map(|path| {
            let line = Line::from(vec![
                Span::raw("  "), // Padding
                Span::styled(path, Style::default().fg(TuiTheme::TEXT_NORMAL)),
            ]);
            ListItem::new(line)
        }).collect();

        // Create the list
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::default().bg(TuiTheme::FILE_PICKER_HIGHLIGHT_BG))
            .highlight_symbol("▸ ");

        frame.render_stateful_widget(list, overlay_area, &mut self.state);
        
        // Draw the query manually at the top of the block
        let query_area = Rect::new(
            overlay_area.x + 1,
            overlay_area.y + 1,
            overlay_area.width.saturating_sub(2),
            1,
        );
        frame.render_widget(query_line, query_area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_picker_navigation_bounds() {
        let mut picker = FilePicker::new(10);
        picker.matches = vec!["1.txt".into(), "2.txt".into(), "3.txt".into()];
        picker.state.select(Some(0));

        // previous from 0 should wrap to len - 1
        picker.previous();
        assert_eq!(picker.state.selected(), Some(2));

        // next from len - 1 should wrap to 0
        picker.next();
        assert_eq!(picker.state.selected(), Some(0));

        // next should go to 1
        picker.next();
        assert_eq!(picker.state.selected(), Some(1));

        // previous should go to 0
        picker.previous();
        assert_eq!(picker.state.selected(), Some(0));
    }

    #[test]
    fn test_file_picker_empty_matches() {
        let mut picker = FilePicker::new(10);
        picker.matches = vec![];
        
        picker.next();
        assert_eq!(picker.state.selected(), None);

        picker.previous();
        assert_eq!(picker.state.selected(), None);
    }
}