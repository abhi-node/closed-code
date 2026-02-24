use ratatui::layout::Constraint;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use super::app::{App, AppState};
use super::theme::TuiTheme;
use super::{approval_overlay, diff_view, header, mode_picker, session_picker, status_bar};

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

    // Full-screen DiffView takes over entire terminal
    if app.state == AppState::DiffView {
        if let Some(ref mut view) = app.diff_view_state {
            diff_view::render(frame, view, area);
        }
        return;
    }

    let input_height = app.input_pane.desired_height();

    let [header_area, chat_area, input_divider, input_area, status_divider, status_area] =
        Layout::vertical([
            Constraint::Length(1),            // Header bar
            Constraint::Fill(1),              // Chat area
            Constraint::Length(1),            // ══ divider
            Constraint::Length(input_height), // Input area (dynamic)
            Constraint::Length(1),            // ══ divider
            Constraint::Length(1),            // Status bar
        ])
        .areas(area);

    header::render(frame, header_area, app);
    super::chat::render(
        frame,
        chat_area,
        &app.messages,
        &mut app.chat_viewport,
        app.tick_count,
    );
    render_divider(frame, input_divider);
    app.input_pane.set_viewport_width(input_area.width);
    frame.render_widget(app.input_pane.textarea(), input_area);
    render_divider(frame, status_divider);
    status_bar::render(frame, status_area, app);

    // ── Overlays — rendered last, on top ──

    // Command picker
    if let AppState::CommandPicker {
        ref filter,
        selected,
    } = app.state
    {
        let filter = filter.clone();
        app.command_picker
            .render(frame, &filter, selected, area, chat_area);
    }

    // Approval overlay
    if app.state == AppState::AwaitingApproval {
        if let Some(ref overlay) = app.approval_overlay {
            approval_overlay::render(frame, overlay, area);
        }
    }

    // Session picker
    if app.state == AppState::SessionPicker {
        if let Some(ref picker) = app.session_picker {
            session_picker::render(frame, picker, area);
        }
    }

    // Mode picker
    if matches!(app.state, AppState::ModePicker { .. }) {
        if let Some(ref picker) = app.mode_picker {
            let confirming = matches!(
                app.state,
                AppState::ModePicker {
                    confirming_auto: true
                }
            );
            mode_picker::render(frame, picker, area, confirming);
        }
    }
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
