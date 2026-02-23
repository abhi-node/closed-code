use ratatui::layout::Constraint;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use super::app::{App, AppState};
use super::theme::TuiTheme;
use super::{header, status_bar};

const MIN_WIDTH: u16 = 40;
const MIN_HEIGHT: u16 = 10;

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let msg = Paragraph::new("Terminal too small.\nResize to at least 40x10.")
            .style(Style::default().fg(TuiTheme::WARNING))
            .alignment(Alignment::Center);
        frame.render_widget(msg, area);
        return;
    }

    let input_height = app.input_pane.desired_height();

    let [header_area, chat_area, input_divider, input_area, status_divider, status_area] =
        Layout::vertical([
            Constraint::Length(1),            // Header bar
            Constraint::Fill(1),             // Chat area
            Constraint::Length(1),            // ══ divider
            Constraint::Length(input_height), // Input area (dynamic)
            Constraint::Length(1),            // ══ divider
            Constraint::Length(1),            // Status bar
        ])
        .areas(area);

    header::render(frame, header_area, app);
    render_chat_placeholder(frame, chat_area);
    render_divider(frame, input_divider);
    frame.render_widget(app.input_pane.textarea(), input_area);
    render_divider(frame, status_divider);
    status_bar::render(frame, status_area, app);

    // Overlay: command picker
    if let AppState::CommandPicker {
        ref filter,
        selected,
    } = app.state
    {
        let filter = filter.clone();
        app.command_picker
            .render(frame, &filter, selected, area, chat_area);
    }
}

/// Empty chat area with subtle side borders.
fn render_chat_placeholder(frame: &mut Frame, area: Rect) {
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::LEFT | ratatui::widgets::Borders::RIGHT)
        .border_style(Style::default().fg(TuiTheme::BORDER_DIM));
    frame.render_widget(block, area);
}

/// Double-line horizontal divider.
fn render_divider(frame: &mut Frame, area: Rect) {
    let line = "═".repeat(area.width as usize);
    let divider = Paragraph::new(line).style(Style::default().fg(TuiTheme::BORDER_DIM));
    frame.render_widget(divider, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_size_constants() {
        assert_eq!(MIN_WIDTH, 40);
        assert_eq!(MIN_HEIGHT, 10);
    }
}
