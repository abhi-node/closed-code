use ratatui::layout::Constraint;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

use super::app::{App, AppState};
use super::file_completion::FileCompletion;
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
        &mut app.message_line_cache,
    );
    render_divider(frame, input_divider);
    app.input_pane.set_viewport_width(input_area.width);
    frame.render_widget(app.input_pane.textarea(), input_area);
    render_divider(frame, status_divider);
    status_bar::render(frame, status_area, app);

    // ── File completion popup ──
    if let Some(ref fc) = app.file_completion {
        render_completion_popup(frame, fc, input_area);
    }

    // ── Overlays — rendered last, on top ──

    // Command picker
    if let AppState::CommandPicker {
        ref filter,
        selected,
    } = app.state
    {
        app.command_picker
            .render(frame, filter, selected, area, chat_area);
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

    // Commit confirmation
    if app.state == AppState::CommitConfirm {
        if let Some(ref msg) = app.commit_message {
            super::commit_confirm::render(frame, msg, area);
        }
    }
}

/// Double-line horizontal divider.
fn render_divider(frame: &mut Frame, area: Rect) {
    let line = "═".repeat(area.width as usize);
    let divider = Paragraph::new(line).style(Style::default().fg(TuiTheme::BORDER_DIM));
    frame.render_widget(divider, area);
}

/// Render a file completion popup above the input area.
fn render_completion_popup(frame: &mut Frame, fc: &FileCompletion, input_area: Rect) {
    const MAX_VISIBLE: usize = 8;

    let visible_count = fc.candidates.len().min(MAX_VISIBLE);
    let popup_height = visible_count as u16 + 2; // +2 for borders

    // Position above the input area
    let popup_y = input_area.y.saturating_sub(popup_height);
    let popup_width = input_area.width.min(50);
    let popup_area = Rect::new(input_area.x + 1, popup_y, popup_width, popup_height);

    // Determine visible window around the selected item
    let start = if fc.selected >= MAX_VISIBLE {
        fc.selected - MAX_VISIBLE + 1
    } else {
        0
    };

    let items: Vec<ListItem> = fc
        .candidates
        .iter()
        .enumerate()
        .skip(start)
        .take(MAX_VISIBLE)
        .map(|(i, candidate)| {
            let style = if i == fc.selected {
                Style::default()
                    .fg(TuiTheme::PICKER_HIGHLIGHT_FG)
                    .bg(TuiTheme::PICKER_HIGHLIGHT_BG)
            } else {
                Style::default().fg(TuiTheme::FG)
            };
            ListItem::new(Line::from(Span::styled(candidate.clone(), style)))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(TuiTheme::BORDER))
            .title(" Completions ")
            .title_style(Style::default().fg(TuiTheme::FG_DIM)),
    );

    // Clear the area first
    frame.render_widget(ratatui::widgets::Clear, popup_area);
    frame.render_widget(list, popup_area);
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
