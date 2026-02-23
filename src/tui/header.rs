use ratatui::layout::Constraint;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use super::app::App;
use super::theme::TuiTheme;

pub fn render(frame: &mut Frame, area: Rect, app: &App<'_>) {
    let [left, right] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(22)]).areas(area);

    // App name
    let title = Paragraph::new(Span::styled(
        " closed-code",
        Style::default()
            .fg(TuiTheme::ACCENT)
            .add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(title, left);

    // Session ID (first 8 chars)
    if let Some(ref sid) = app.status.session_id {
        let id_str = sid.as_str();
        let short = if id_str.len() >= 8 {
            &id_str[..8]
        } else {
            &id_str
        };
        let session = Paragraph::new(Span::styled(
            format!("session: {} ", short),
            Style::default().fg(TuiTheme::FG_DIM),
        ))
        .alignment(Alignment::Right);
        frame.render_widget(session, right);
    }
}
