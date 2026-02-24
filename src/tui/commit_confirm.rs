use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap};

use super::theme::TuiTheme;

pub fn render(frame: &mut Frame, message: &str, terminal_area: Rect) {
    let overlay_width = (terminal_area.width.saturating_sub(6)).min(70);
    let overlay_height = 7u16;

    let x = (terminal_area.width.saturating_sub(overlay_width)) / 2;
    let y = (terminal_area.height.saturating_sub(overlay_height)) / 2;
    let area = Rect::new(x, y, overlay_width, overlay_height);

    frame.render_widget(Clear, area);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(TuiTheme::ACCENT))
        .padding(Padding::horizontal(1))
        .title(" Confirm Commit ")
        .title_bottom(" [y] commit  [n] cancel ".fg(TuiTheme::FG_DIM));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let content = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("\"{}\"", message),
            Style::default().fg(TuiTheme::FG).bold(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Commit with this message?",
            Style::default().fg(TuiTheme::FG_DIM),
        )),
    ];

    frame.render_widget(Paragraph::new(content).wrap(Wrap { trim: false }), inner);
}
