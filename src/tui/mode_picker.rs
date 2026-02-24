use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};

use crate::mode::Mode;

use super::theme::{self, TuiTheme};

struct ModeOption {
    mode: Mode,
    label: &'static str,
    description: &'static str,
    is_dangerous: bool,
}

/// Overlay for selecting the execution mode when accepting a plan.
pub struct ModePicker {
    pub selected: usize,
    pub confirming_auto: bool,
    entries: Vec<ModeOption>,
}

impl Default for ModePicker {
    fn default() -> Self {
        Self::new()
    }
}

impl ModePicker {
    pub fn new() -> Self {
        Self {
            selected: 0,
            confirming_auto: false,
            entries: vec![
                ModeOption {
                    mode: Mode::Guided,
                    label: "Guided",
                    description: "File changes require your approval before writing",
                    is_dangerous: false,
                },
                ModeOption {
                    mode: Mode::Execute,
                    label: "Execute",
                    description: "All file changes auto-approved, shell restricted",
                    is_dangerous: false,
                },
                ModeOption {
                    mode: Mode::Auto,
                    label: "Auto",
                    description: "Full autonomy — all tools unrestricted",
                    is_dangerous: true,
                },
            ],
        }
    }

    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.entries.len() {
            self.selected += 1;
        }
    }

    /// Try to select the current entry.
    /// Returns `Some(mode)` for safe modes, `None` for Auto (triggers confirmation).
    pub fn try_select(&mut self) -> Option<Mode> {
        let entry = &self.entries[self.selected];
        if entry.is_dangerous {
            self.confirming_auto = true;
            None
        } else {
            Some(entry.mode)
        }
    }

    pub fn confirm_auto(&self) -> Mode {
        Mode::Auto
    }

    pub fn cancel_auto(&mut self) {
        self.confirming_auto = false;
    }

    pub fn selected_mode(&self) -> Mode {
        self.entries[self.selected].mode
    }
}

pub fn render(frame: &mut Frame, picker: &ModePicker, terminal_area: Rect, confirming: bool) {
    if confirming {
        render_auto_confirm(frame, terminal_area);
    } else {
        render_mode_list(frame, picker, terminal_area);
    }
}

fn render_mode_list(frame: &mut Frame, picker: &ModePicker, terminal_area: Rect) {
    let overlay_width = (terminal_area.width.saturating_sub(6)).min(60);
    let overlay_height = (picker.entries.len() as u16) + 4;

    let x = (terminal_area.width.saturating_sub(overlay_width)) / 2;
    let y = (terminal_area.height.saturating_sub(overlay_height)) / 2;
    let area = Rect::new(x, y, overlay_width, overlay_height);

    frame.render_widget(Clear, area);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(TuiTheme::ACCENT))
        .padding(Padding::horizontal(1))
        .title(" Accept Plan — Choose Mode ")
        .title_bottom(" [Enter] select  [Esc] cancel ".fg(TuiTheme::FG_DIM));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows: Vec<Line> = picker
        .entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let mode_color = theme::mode_color(&entry.mode);
            let mut spans = vec![
                Span::styled(
                    format!(" {:>8} ", entry.label),
                    if i == picker.selected {
                        Style::default()
                            .fg(TuiTheme::PICKER_HIGHLIGHT_FG)
                            .bg(TuiTheme::PICKER_HIGHLIGHT_BG)
                            .bold()
                    } else {
                        Style::default().fg(mode_color).bold()
                    },
                ),
                Span::styled(
                    format!(" {}", entry.description),
                    if i == picker.selected {
                        Style::default()
                            .fg(TuiTheme::PICKER_HIGHLIGHT_FG)
                            .bg(TuiTheme::PICKER_HIGHLIGHT_BG)
                    } else {
                        Style::default().fg(TuiTheme::FG_DIM)
                    },
                ),
            ];
            if entry.is_dangerous {
                spans.push(Span::styled(
                    " DANGER",
                    Style::default().fg(TuiTheme::ERROR).bold(),
                ));
            }
            Line::from(spans)
        })
        .collect();

    frame.render_widget(Paragraph::new(rows), inner);
}

fn render_auto_confirm(frame: &mut Frame, terminal_area: Rect) {
    let overlay_width = (terminal_area.width.saturating_sub(6)).min(50);
    let overlay_height = 6u16;

    let x = (terminal_area.width.saturating_sub(overlay_width)) / 2;
    let y = (terminal_area.height.saturating_sub(overlay_height)) / 2;
    let area = Rect::new(x, y, overlay_width, overlay_height);

    frame.render_widget(Clear, area);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(TuiTheme::ERROR))
        .padding(Padding::horizontal(1))
        .title(" Confirm Auto Mode ".fg(TuiTheme::ERROR).bold())
        .title_bottom(" [y] confirm  [n] go back ".fg(TuiTheme::FG_DIM));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let warning = vec![
        Line::from(Span::styled(
            "Auto mode gives full autonomy to the AI.",
            Style::default().fg(TuiTheme::WARNING),
        )),
        Line::from(Span::styled(
            "All tools run without restriction.",
            Style::default().fg(TuiTheme::FG_DIM),
        )),
    ];

    frame.render_widget(Paragraph::new(warning), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_selection() {
        let picker = ModePicker::new();
        assert_eq!(picker.selected, 0);
        assert!(!picker.confirming_auto);
    }

    #[test]
    fn move_bounds() {
        let mut picker = ModePicker::new();
        picker.move_up();
        assert_eq!(picker.selected, 0);
        picker.move_down();
        picker.move_down();
        assert_eq!(picker.selected, 2);
        picker.move_down();
        assert_eq!(picker.selected, 2); // clamped
    }

    #[test]
    fn guided_immediate_select() {
        let mut picker = ModePicker::new();
        // First entry is Guided
        let result = picker.try_select();
        assert_eq!(result, Some(Mode::Guided));
        assert!(!picker.confirming_auto);
    }

    #[test]
    fn auto_requires_confirmation() {
        let mut picker = ModePicker::new();
        picker.move_down(); // Execute
        picker.move_down(); // Auto
        let result = picker.try_select();
        assert_eq!(result, None);
        assert!(picker.confirming_auto);
    }

    #[test]
    fn cancel_auto_confirmation() {
        let mut picker = ModePicker::new();
        picker.move_down();
        picker.move_down();
        picker.try_select();
        assert!(picker.confirming_auto);
        picker.cancel_auto();
        assert!(!picker.confirming_auto);
    }

    #[test]
    fn confirm_auto_returns_auto_mode() {
        let picker = ModePicker::new();
        assert_eq!(picker.confirm_auto(), Mode::Auto);
    }
}
