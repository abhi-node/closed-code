use ratatui::layout::Constraint;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use super::app::{App, AppState};
use super::gauge;
use super::theme::{self, TuiTheme};

pub fn render(frame: &mut Frame, area: Rect, app: &App<'_>) {
    let status = &app.status;
    let is_working = matches!(
        app.state,
        AppState::Thinking | AppState::Streaming | AppState::ToolExecuting { .. }
    );
    let working_width: u16 = if is_working { 14 } else { 0 };

    let [mode_area, working_area, s1, model_area, s2, turns_area, s3, gauge_area, s4, git_area] =
        Layout::horizontal([
            Constraint::Length(10),
            Constraint::Length(working_width),
            Constraint::Length(1),
            Constraint::Length(18),
            Constraint::Length(1),
            Constraint::Length(14),
            Constraint::Length(1),
            Constraint::Length(18),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(area);

    // Mode badge
    render_mode_badge(frame, mode_area, &status.mode);

    // Thinking/Working indicator
    if is_working {
        render_working_indicator(frame, working_area, &app.state, app.tick_count);
    }

    // Separators
    let sep = Paragraph::new("│").style(Style::default().fg(TuiTheme::BORDER_DIM));
    for area in [s1, s2, s3, s4] {
        frame.render_widget(sep.clone(), area);
    }

    // Model name (truncated to 16 chars)
    let model_display = truncate(&status.model, 16);
    frame.render_widget(
        Paragraph::new(format!(" {}", model_display)).style(Style::default().fg(TuiTheme::FG_DIM)),
        model_area,
    );

    // Token / turn counter
    render_context_counter(frame, turns_area, status);

    // Context gauge (token-based)
    let (used, total) = (
        status.last_prompt_tokens as usize,
        status.context_limit_tokens as usize,
    );
    gauge::render(frame, gauge_area, used, total);

    // Git info
    render_git_info(frame, git_area, status);
}

fn render_working_indicator(frame: &mut Frame, area: Rect, state: &AppState, tick: usize) {
    let frames = TuiTheme::SPINNER_FRAMES;
    let spinner = frames[tick % frames.len()];
    let label = match state {
        AppState::Thinking => "Thinking...",
        AppState::Streaming => "Streaming..",
        AppState::ToolExecuting { count } if *count > 1 => {
            // Leak a formatted string for the static-lifetime label.
            // Acceptable since this only happens during active tool execution.
            return render_working_custom(frame, area, &format!("Working ({count})..."), tick);
        }
        AppState::ToolExecuting { .. } => "Working...",
        _ => "",
    };
    frame.render_widget(
        Paragraph::new(format!(" {} {}", spinner, label))
            .style(Style::default().fg(TuiTheme::ACCENT)),
        area,
    );
}

fn render_working_custom(frame: &mut Frame, area: Rect, label: &str, tick: usize) {
    let frames = TuiTheme::SPINNER_FRAMES;
    let spinner = frames[tick % frames.len()];
    frame.render_widget(
        Paragraph::new(format!(" {} {}", spinner, label))
            .style(Style::default().fg(TuiTheme::ACCENT)),
        area,
    );
}

fn render_mode_badge(frame: &mut Frame, area: Rect, mode: &crate::mode::Mode) {
    let color = theme::mode_color(mode);
    let label = theme::mode_label(mode);
    let padded = format!(" {:^8}", label);
    frame.render_widget(
        Paragraph::new(padded).style(
            Style::default()
                .fg(TuiTheme::BG)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ),
        area,
    );
}

fn render_context_counter(frame: &mut Frame, area: Rect, status: &super::app::StatusSnapshot) {
    if status.last_prompt_tokens > 0 {
        let ratio = if status.context_limit_tokens > 0 {
            status.last_prompt_tokens as f64 / status.context_limit_tokens as f64
        } else {
            0.0
        };
        let color = if ratio >= 0.85 {
            TuiTheme::ERROR
        } else if ratio >= 0.60 {
            TuiTheme::WARNING
        } else {
            TuiTheme::FG
        };
        let display = format!(
            " {}/{}",
            format_token_count(status.last_prompt_tokens),
            format_token_count(status.context_limit_tokens),
        );
        frame.render_widget(
            Paragraph::new(display).style(Style::default().fg(color)),
            area,
        );
    } else {
        frame.render_widget(
            Paragraph::new(" --/--").style(Style::default().fg(TuiTheme::FG_DIM)),
            area,
        );
    }
}

/// Format a token count with K/M suffixes: 1234 → "1.2K", 1234567 → "1.2M".
fn format_token_count(count: u32) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.0}K", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

fn render_git_info(frame: &mut Frame, area: Rect, status: &super::app::StatusSnapshot) {
    let line = match &status.git_branch {
        Some(branch) if status.git_is_clean => Line::from(vec![
            Span::styled(
                format!(" {}", branch),
                Style::default().fg(TuiTheme::FG_DIM),
            ),
            Span::styled(" ✓", Style::default().fg(TuiTheme::SUCCESS)),
        ]),
        Some(branch) => Line::from(vec![
            Span::styled(
                format!(" {}", branch),
                Style::default().fg(TuiTheme::FG_DIM),
            ),
            Span::styled(
                format!(" ▲{}", status.git_change_count),
                Style::default().fg(TuiTheme::WARNING),
            ),
        ]),
        None => Line::from(""),
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else if max > 3 {
        format!("{}...", &s[..max - 3])
    } else {
        s[..max].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_exact() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn truncate_long() {
        assert_eq!(truncate("gemini-3.1-pro-preview", 16), "gemini-3.1-pr...");
    }
}
