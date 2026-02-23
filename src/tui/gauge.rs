use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use super::theme::TuiTheme;

const BAR_WIDTH: usize = 10;

pub fn render(frame: &mut Frame, area: Rect, used: usize, total: usize) {
    let (ratio, percent) = if total > 0 {
        let r = used as f64 / total as f64;
        (r, (r * 100.0).round().min(100.0) as u16)
    } else {
        (0.0, 0)
    };

    let filled = ((ratio * BAR_WIDTH as f64).round() as usize).min(BAR_WIDTH);
    let empty = BAR_WIDTH - filled;
    let color = gauge_color(ratio);

    let line = Line::from(vec![
        Span::raw(" "),
        Span::styled("█".repeat(filled), Style::default().fg(color)),
        Span::styled("░".repeat(empty), Style::default().fg(TuiTheme::BORDER_DIM)),
        Span::styled(format!(" {}%", percent), Style::default().fg(color)),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}

/// Determine gauge color based on fill ratio.
pub fn gauge_color(ratio: f64) -> Color {
    if ratio >= 0.85 {
        TuiTheme::GAUGE_HIGH
    } else if ratio >= 0.60 {
        TuiTheme::GAUGE_MED
    } else {
        TuiTheme::GAUGE_LOW
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gauge_color_thresholds() {
        assert_eq!(gauge_color(0.0), TuiTheme::GAUGE_LOW);
        assert_eq!(gauge_color(0.59), TuiTheme::GAUGE_LOW);
        assert_eq!(gauge_color(0.60), TuiTheme::GAUGE_MED);
        assert_eq!(gauge_color(0.84), TuiTheme::GAUGE_MED);
        assert_eq!(gauge_color(0.85), TuiTheme::GAUGE_HIGH);
        assert_eq!(gauge_color(1.0), TuiTheme::GAUGE_HIGH);
    }
}
