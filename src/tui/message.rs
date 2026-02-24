use ratatui::prelude::*;

use super::chat::{ChatMessage, ToolCallDisplay};
use super::theme::TuiTheme;

/// Render a single ChatMessage into a vector of Lines.
pub fn render_message<'a>(msg: &ChatMessage, width: usize, tick: usize) -> Vec<Line<'a>> {
    match msg {
        ChatMessage::User { text } => render_user(text, width),
        ChatMessage::Assistant {
            text, tool_calls, ..
        } => render_assistant(text, tool_calls, width, tick),
        ChatMessage::System { text } => render_system(text, width),
    }
}

fn render_user<'a>(text: &str, width: usize) -> Vec<Line<'a>> {
    let mut lines = Vec::new();
    // "┌─ You " = 7 display chars, fill rest with ─
    let top_fill = width.saturating_sub(7);
    let bottom_fill = width.saturating_sub(1); // "└" + fill

    // Top border with "You" title
    lines.push(Line::from(vec![
        Span::styled("┌─ ", Style::new().fg(TuiTheme::USER)),
        Span::styled("You", Style::new().fg(TuiTheme::USER).bold()),
        Span::styled(
            format!(" {}", "─".repeat(top_fill)),
            Style::new().fg(TuiTheme::USER),
        ),
    ]));

    // Content lines (wrapped)
    for line in text.lines() {
        for wline in wrap_text(line, width.saturating_sub(4)) {
            lines.push(Line::from(vec![
                Span::styled("│ ", Style::new().fg(TuiTheme::USER)),
                Span::raw(wline),
            ]));
        }
    }

    // Bottom border
    lines.push(Line::from(Span::styled(
        format!("└{}", "─".repeat(bottom_fill)),
        Style::new().fg(TuiTheme::USER),
    )));

    lines
}

fn render_assistant<'a>(
    text: &str,
    tool_calls: &[ToolCallDisplay],
    width: usize,
    tick: usize,
) -> Vec<Line<'a>> {
    let mut lines = Vec::new();

    // Tool calls first
    for tc in tool_calls {
        lines.extend(render_tool_call(tc, tick));
    }

    // Text content (markdown rendered)
    if !text.is_empty() {
        lines.extend(super::markdown::render_markdown(text, width));
    }

    lines
}

fn render_tool_call<'a>(tc: &ToolCallDisplay, tick: usize) -> Vec<Line<'a>> {
    match tc {
        ToolCallDisplay::Running { name, args_display } => {
            let frames = TuiTheme::SPINNER_FRAMES;
            let frame = frames[tick % frames.len()];
            vec![Line::from(vec![
                Span::styled(format!("{} ", frame), Style::new().fg(TuiTheme::TOOL)),
                Span::styled(
                    truncate_display(&format!("{}({})", name, args_display), 60),
                    Style::new().fg(TuiTheme::TOOL),
                ),
            ])]
        }
        ToolCallDisplay::Completed { name, duration } => vec![Line::from(vec![
            Span::styled("\u{2713} ", Style::new().fg(TuiTheme::SUCCESS)),
            Span::styled(name.to_string(), Style::new().fg(TuiTheme::FG_DIM)),
            Span::styled(
                format!(" ({:.1}s)", duration.as_secs_f64()),
                Style::new().fg(TuiTheme::FG_MUTED),
            ),
        ])],
        ToolCallDisplay::Failed { name, error } => vec![Line::from(vec![
            Span::styled("\u{2717} ", Style::new().fg(TuiTheme::ERROR)),
            Span::styled(name.to_string(), Style::new().fg(TuiTheme::ERROR)),
            Span::styled(
                format!(": {}", truncate_display(error, 50)),
                Style::new().fg(TuiTheme::FG_MUTED),
            ),
        ])],
        ToolCallDisplay::AgentRunning {
            agent_type,
            task,
            last_tool,
        } => {
            let frames = TuiTheme::SPINNER_FRAMES;
            let frame = frames[tick % frames.len()];
            let mut result = vec![Line::from(vec![
                Span::styled(format!("{} ", frame), Style::new().fg(TuiTheme::AGENT)),
                Span::styled(
                    format!("[agent:{}] ", agent_type),
                    Style::new().fg(TuiTheme::AGENT).bold(),
                ),
                Span::styled(
                    truncate_display(task, 50),
                    Style::new().fg(TuiTheme::FG_DIM),
                ),
            ])];
            if let Some(tool) = last_tool {
                result.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled("\u{2713} ", Style::new().fg(TuiTheme::FG_MUTED)),
                    Span::styled(
                        truncate_display(tool, 56),
                        Style::new().fg(TuiTheme::FG_MUTED),
                    ),
                ]));
            }
            result
        }
        ToolCallDisplay::AgentCompleted {
            agent_type,
            duration,
        } => vec![Line::from(vec![
            Span::styled("\u{2713} ", Style::new().fg(TuiTheme::SUCCESS)),
            Span::styled(
                format!("[agent:{}]", agent_type),
                Style::new().fg(TuiTheme::AGENT),
            ),
            Span::styled(
                format!(" done ({:.1}s)", duration.as_secs_f64()),
                Style::new().fg(TuiTheme::FG_MUTED),
            ),
        ])],
    }
}

fn render_system<'a>(text: &str, width: usize) -> Vec<Line<'a>> {
    let mut lines = Vec::new();

    // Single-line system messages get a centered rule
    if !text.contains('\n') && text.len() < width.saturating_sub(4) {
        let pad = width.saturating_sub(text.len() + 4);
        let left = pad / 2;
        let right = pad - left;
        lines.push(Line::from(vec![
            Span::styled("─".repeat(left), Style::new().fg(TuiTheme::FG_MUTED)),
            Span::styled(format!(" {} ", text), Style::new().fg(TuiTheme::FG_DIM)),
            Span::styled("─".repeat(right), Style::new().fg(TuiTheme::FG_MUTED)),
        ]));
    } else {
        for line in text.lines() {
            for wline in wrap_text(line, width) {
                lines.push(Line::from(Span::styled(
                    wline,
                    Style::new().fg(TuiTheme::FG_DIM).italic(),
                )));
            }
        }
    }

    lines
}

/// Truncate a string to fit within `max_width` characters.
/// Used for UI labels (tool names, args, errors) where single-line display is needed.
pub fn truncate_display(s: &str, max_width: usize) -> String {
    if s.len() <= max_width {
        s.to_string()
    } else if max_width <= 3 {
        s[..max_width].to_string()
    } else {
        format!("{}...", &s[..max_width - 3])
    }
}

/// Wrap a string into lines of at most `max_width` characters.
/// Prefers breaking at word boundaries (spaces). Falls back to hard break.
pub(crate) fn wrap_text(s: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 || s.len() <= max_width {
        return vec![s.to_string()];
    }
    let mut lines = Vec::new();
    let mut remaining = s;
    while remaining.len() > max_width {
        // If the character right after max_width is a space, break at max_width
        let break_at = if remaining.as_bytes()[max_width] == b' ' {
            max_width
        } else {
            // Find last space within max_width for a word-boundary break
            remaining[..max_width]
                .rfind(' ')
                .map(|i| i + 1)
                .unwrap_or(max_width)
        };
        lines.push(remaining[..break_at].trim_end().to_string());
        remaining = remaining[break_at..].trim_start();
    }
    if !remaining.is_empty() {
        lines.push(remaining.to_string());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_message_has_borders() {
        let lines = render_user("Hello", 40);
        let first = lines[0].to_string();
        assert!(first.contains("You"));
        assert!(first.contains("┌"));
        let last = lines.last().unwrap().to_string();
        assert!(last.contains("└"));
    }

    #[test]
    fn system_single_line_centered() {
        let lines = render_system("Mode changed", 40);
        assert_eq!(lines.len(), 1);
        let text = lines[0].to_string();
        assert!(text.contains("Mode changed"));
        assert!(text.contains("─"));
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate_display("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let long = "a".repeat(50);
        let result = truncate_display(&long, 20);
        assert_eq!(result.len(), 20);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn wrap_short_string() {
        let result = wrap_text("hello world", 40);
        assert_eq!(result, vec!["hello world"]);
    }

    #[test]
    fn wrap_at_word_boundary() {
        let result = wrap_text("hello world foo bar", 11);
        assert_eq!(result, vec!["hello world", "foo bar"]);
    }

    #[test]
    fn wrap_hard_break() {
        let long = "a".repeat(30);
        let result = wrap_text(&long, 10);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].len(), 10);
        assert_eq!(result[1].len(), 10);
        assert_eq!(result[2].len(), 10);
    }

    #[test]
    fn wrap_zero_width() {
        let result = wrap_text("hello", 0);
        assert_eq!(result, vec!["hello"]);
    }

    #[test]
    fn user_message_wraps_long_text() {
        let long = "word ".repeat(20).trim().to_string(); // 99 chars
        let lines = render_user(&long, 30);
        // Should have top border + multiple content lines + bottom border
        assert!(lines.len() > 3);
    }
}
