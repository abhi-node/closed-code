use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Padding, Paragraph};

use super::approval_overlay::{DiffLine, DiffLineTag};
use super::theme::TuiTheme;

/// Full-screen scrollable diff viewer.
pub struct DiffView {
    pub file_path: String,
    pub diff_lines: Vec<DiffLine>,
    pub additions: usize,
    pub deletions: usize,
    pub scroll_offset: usize,
    pub visible_height: usize,
}

impl DiffView {
    pub fn new(
        file_path: String,
        diff_lines: Vec<DiffLine>,
        additions: usize,
        deletions: usize,
    ) -> Self {
        Self {
            file_path,
            diff_lines,
            additions,
            deletions,
            scroll_offset: 0,
            visible_height: 0,
        }
    }

    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    pub fn scroll_down(&mut self, n: usize) {
        let max = self.diff_lines.len().saturating_sub(self.visible_height);
        self.scroll_offset = (self.scroll_offset + n).min(max);
    }

    pub fn page_up(&mut self) {
        self.scroll_up(self.visible_height / 2);
    }

    pub fn page_down(&mut self) {
        self.scroll_down(self.visible_height / 2);
    }

    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.diff_lines.len().saturating_sub(self.visible_height);
    }
}

pub fn render(frame: &mut Frame, view: &mut DiffView, area: Rect) {
    let position = format!(" {}/{} ", view.scroll_offset + 1, view.diff_lines.len());
    let title = format!(" Diff: {} ", view.file_path);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(TuiTheme::ACCENT))
        .padding(Padding::horizontal(1))
        .title_top(title)
        .title_top(Line::from(position.fg(TuiTheme::FG_DIM)).right_aligned())
        .title_bottom(" j/k scroll  Ctrl+d/u page  g top  G bottom  q back ".fg(TuiTheme::FG_DIM));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    view.visible_height = inner.height as usize;
    let max_line_width = inner.width as usize;

    let visible_lines: Vec<Line> = view
        .diff_lines
        .iter()
        .skip(view.scroll_offset)
        .take(view.visible_height)
        .map(|dl| {
            let display = truncate_line(&dl.content, max_line_width);
            let style = diff_line_style(&dl.tag);
            Line::from(Span::styled(display, style))
        })
        .collect();

    frame.render_widget(Paragraph::new(visible_lines), inner);
}

fn diff_line_style(tag: &DiffLineTag) -> Style {
    match tag {
        DiffLineTag::HunkHeader => Style::default().fg(TuiTheme::DIFF_HUNK),
        DiffLineTag::Add => Style::default()
            .fg(TuiTheme::DIFF_ADD)
            .bg(TuiTheme::DIFF_ADD_BG),
        DiffLineTag::Delete => Style::default()
            .fg(TuiTheme::DIFF_DEL)
            .bg(TuiTheme::DIFF_DEL_BG),
        DiffLineTag::Context => Style::default().fg(TuiTheme::DIFF_CONTEXT),
    }
}

fn truncate_line(s: &str, max_width: usize) -> String {
    if s.len() <= max_width {
        s.to_string()
    } else if max_width > 3 {
        format!("{}...", &s[..max_width - 3])
    } else {
        s[..max_width].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_view(n_lines: usize) -> DiffView {
        let lines: Vec<DiffLine> = (0..n_lines)
            .map(|i| DiffLine {
                tag: DiffLineTag::Context,
                content: format!(" line {}", i),
            })
            .collect();
        let mut view = DiffView::new("test.rs".into(), lines, 0, 0);
        view.visible_height = 10;
        view
    }

    #[test]
    fn scroll_to_top_resets() {
        let mut view = make_view(50);
        view.scroll_offset = 30;
        view.scroll_to_top();
        assert_eq!(view.scroll_offset, 0);
    }

    #[test]
    fn scroll_to_bottom_clamps() {
        let mut view = make_view(50);
        view.scroll_to_bottom();
        assert_eq!(view.scroll_offset, 40); // 50 - 10
    }

    #[test]
    fn scroll_down_clamps() {
        let mut view = make_view(20);
        view.scroll_down(100);
        assert_eq!(view.scroll_offset, 10); // 20 - 10
    }

    #[test]
    fn scroll_up_clamps() {
        let mut view = make_view(20);
        view.scroll_offset = 5;
        view.scroll_up(100);
        assert_eq!(view.scroll_offset, 0);
    }

    #[test]
    fn page_up_down() {
        let mut view = make_view(50);
        view.page_down(); // 5 (half of 10)
        assert_eq!(view.scroll_offset, 5);
        view.page_up();
        assert_eq!(view.scroll_offset, 0);
    }
}
