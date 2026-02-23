use ratatui::layout::Constraint;
use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};

use super::theme::TuiTheme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandCategory {
    Navigation,
    Mode,
    Git,
    Session,
    Config,
}

#[derive(Debug, Clone)]
pub struct CommandEntry {
    pub name: &'static str,
    pub args: &'static str,
    pub description: &'static str,
    pub category: CommandCategory,
}

impl CommandEntry {
    pub fn display_name(&self) -> String {
        if self.args.is_empty() {
            self.name.to_string()
        } else {
            format!("{} {}", self.name, self.args)
        }
    }
}

pub fn all_commands() -> Vec<CommandEntry> {
    use CommandCategory::*;
    vec![
        // Navigation
        CommandEntry { name: "/help",        args: "",         description: "Show this help",                                  category: Navigation },
        CommandEntry { name: "/quit",        args: "",         description: "Exit closed-code",                                category: Navigation },
        CommandEntry { name: "/clear",       args: "",         description: "Clear conversation history",                      category: Navigation },
        // Mode
        CommandEntry { name: "/mode",        args: "[name]",   description: "Show or switch mode",                             category: Mode },
        CommandEntry { name: "/explore",     args: "",         description: "Switch to Explore mode",                          category: Mode },
        CommandEntry { name: "/plan",        args: "",         description: "Switch to Plan mode",                             category: Mode },
        CommandEntry { name: "/guided",      args: "",         description: "Switch to Guided mode (writes require approval)", category: Mode },
        CommandEntry { name: "/execute",     args: "",         description: "Switch to Execute mode",                          category: Mode },
        CommandEntry { name: "/auto",        args: "",         description: "Switch to Auto mode (unrestricted shell)",        category: Mode },
        CommandEntry { name: "/accept",      args: "",         description: "Accept plan and choose execution mode",           category: Mode },
        // Git
        CommandEntry { name: "/diff",        args: "[opts]",   description: "Show git diff (staged, branch, HEAD~N)",          category: Git },
        CommandEntry { name: "/review",      args: "[HEAD~N]", description: "Review changes with sub-agent",                   category: Git },
        CommandEntry { name: "/commit",      args: "[message]",description: "Generate commit message and commit",              category: Git },
        // Session
        CommandEntry { name: "/new",         args: "",         description: "Start a new session (clears history)",            category: Session },
        CommandEntry { name: "/fork",        args: "",         description: "Fork current session into a new one",             category: Session },
        CommandEntry { name: "/compact",     args: "[prompt]", description: "Compact conversation history via LLM",            category: Session },
        CommandEntry { name: "/history",     args: "[N]",      description: "Show last N conversation turns",                  category: Session },
        CommandEntry { name: "/export",      args: "[file]",   description: "Export session transcript to markdown",           category: Session },
        CommandEntry { name: "/resume",      args: "",         description: "List recent sessions",                            category: Session },
        // Config
        CommandEntry { name: "/model",       args: "[name]",   description: "Show or switch model",                            category: Config },
        CommandEntry { name: "/personality", args: "[style]",  description: "Show or change personality",                      category: Config },
        CommandEntry { name: "/status",      args: "",         description: "Show session status and token usage",             category: Config },
        CommandEntry { name: "/sandbox",     args: "",         description: "Show sandbox mode and protected paths",           category: Config },
    ]
}

pub struct CommandPicker {
    commands: Vec<CommandEntry>,
    pub max_visible: usize,
    pub scroll_offset: usize,
}

impl Default for CommandPicker {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandPicker {
    pub fn new() -> Self {
        Self {
            commands: all_commands(),
            max_visible: 10,
            scroll_offset: 0,
        }
    }

    /// Filter commands by case-insensitive substring match on name.
    /// `filter` should NOT include the leading `/`.
    pub fn filtered(&self, filter: &str) -> Vec<&CommandEntry> {
        if filter.is_empty() {
            return self.commands.iter().collect();
        }
        let filter_lower = filter.to_lowercase();
        self.commands
            .iter()
            .filter(|cmd| {
                let name = cmd.name.strip_prefix('/').unwrap_or(cmd.name);
                name.to_lowercase().contains(&filter_lower)
            })
            .collect()
    }

    pub fn filtered_count(&self, filter: &str) -> usize {
        self.filtered(filter).len()
    }

    pub fn get_selected(&self, filter: &str, index: usize) -> Option<&CommandEntry> {
        self.filtered(filter).get(index).copied()
    }

    pub fn ensure_visible(&mut self, selected: usize) {
        if selected < self.scroll_offset {
            self.scroll_offset = selected;
        } else if selected >= self.scroll_offset + self.max_visible {
            self.scroll_offset = selected - self.max_visible + 1;
        }
    }

    pub fn render(
        &mut self,
        frame: &mut Frame,
        filter: &str,
        selected: usize,
        terminal_area: Rect,
        chat_area: Rect,
    ) {
        let matched = self.filtered_count(filter);
        let total = self.commands.len();

        if matched == 0 {
            return;
        }

        self.ensure_visible(selected);

        let filtered = self.filtered(filter);

        // Overlay dimensions
        let width = terminal_area.width.saturating_sub(4).min(60);
        let visible_items = matched.min(self.max_visible);
        let height = (visible_items as u16) + 4; // border + search + gap + footer

        // Position: anchored to bottom of chat area, centered horizontally
        let x = (terminal_area.width.saturating_sub(width)) / 2;
        let y = chat_area.bottom().saturating_sub(height);
        let overlay = Rect::new(x, y, width, height);

        // Clear background
        frame.render_widget(Clear, overlay);

        // Border
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(TuiTheme::ACCENT))
            .title(
                Line::from(" Commands ")
                    .style(Style::new().fg(TuiTheme::ACCENT).bold()),
            )
            .title_bottom(
                Line::from(format!(" {} of {} ", matched, total))
                    .right_aligned()
                    .style(Style::new().fg(TuiTheme::FG_MUTED)),
            )
            .padding(Padding::horizontal(1));

        let inner = block.inner(overlay);
        frame.render_widget(block, overlay);

        // Layout: search line + gap + command list
        let [search_area, _gap, list_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(inner);

        // Search line
        let search = Line::from(vec![
            Span::styled("> ", Style::new().fg(TuiTheme::ACCENT)),
            Span::styled(format!("/{}", filter), Style::new().fg(TuiTheme::FG)),
        ]);
        frame.render_widget(Paragraph::new(search), search_area);

        // Command list
        let visible: Vec<&CommandEntry> = filtered
            .iter()
            .skip(self.scroll_offset)
            .take(self.max_visible)
            .copied()
            .collect();

        let name_width = 20.min(list_area.width as usize / 2);

        for (i, cmd) in visible.iter().enumerate() {
            let row_y = list_area.y + i as u16;
            if row_y >= list_area.bottom() {
                break;
            }
            let row = Rect::new(list_area.x, row_y, list_area.width, 1);

            let is_selected = (self.scroll_offset + i) == selected;
            let display = cmd.display_name();
            let padded = format!("{:<width$}", display, width = name_width);

            let (indicator, name_style, desc_style) = if is_selected {
                (
                    " \u{25b8} ",
                    Style::new()
                        .fg(TuiTheme::PICKER_HIGHLIGHT_FG)
                        .bg(TuiTheme::PICKER_HIGHLIGHT_BG)
                        .bold(),
                    Style::new()
                        .fg(TuiTheme::PICKER_HIGHLIGHT_FG)
                        .bg(TuiTheme::PICKER_HIGHLIGHT_BG),
                )
            } else {
                (
                    "   ",
                    Style::new().fg(TuiTheme::ACCENT).bold(),
                    Style::new().fg(TuiTheme::FG_DIM),
                )
            };

            let line = Line::from(vec![
                Span::styled(
                    indicator,
                    if is_selected {
                        Style::new().bg(TuiTheme::PICKER_HIGHLIGHT_BG)
                    } else {
                        Style::default()
                    },
                ),
                Span::styled(padded, name_style),
                Span::styled(cmd.description, desc_style),
            ]);
            frame.render_widget(Paragraph::new(line), row);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_commands_start_with_slash() {
        for cmd in all_commands() {
            assert!(
                cmd.name.starts_with('/'),
                "{} should start with /",
                cmd.name
            );
        }
    }

    #[test]
    fn all_commands_have_descriptions() {
        for cmd in all_commands() {
            assert!(
                !cmd.description.is_empty(),
                "{} has empty description",
                cmd.name
            );
        }
    }

    #[test]
    fn filter_empty_returns_all() {
        let picker = CommandPicker::new();
        assert_eq!(picker.filtered("").len(), all_commands().len());
    }

    #[test]
    fn filter_narrows_results() {
        let picker = CommandPicker::new();
        let matches = picker.filtered("com");
        assert!(matches.len() >= 2); // /commit and /compact at minimum
        assert!(matches.iter().any(|c| c.name == "/commit"));
        assert!(matches.iter().any(|c| c.name == "/compact"));
    }

    #[test]
    fn filter_exact_match() {
        let picker = CommandPicker::new();
        let matches = picker.filtered("help");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "/help");
    }

    #[test]
    fn filter_case_insensitive() {
        let picker = CommandPicker::new();
        assert_eq!(
            picker.filtered("quit").len(),
            picker.filtered("QUIT").len(),
        );
    }

    #[test]
    fn filter_no_match() {
        let picker = CommandPicker::new();
        assert_eq!(picker.filtered("zzzzz").len(), 0);
    }

    #[test]
    fn get_selected_valid() {
        let picker = CommandPicker::new();
        assert!(picker.get_selected("", 0).is_some());
    }

    #[test]
    fn get_selected_out_of_bounds() {
        let picker = CommandPicker::new();
        assert!(picker.get_selected("", 999).is_none());
    }

    #[test]
    fn display_name_with_args() {
        let cmd = CommandEntry {
            name: "/mode",
            args: "[name]",
            description: "test",
            category: CommandCategory::Mode,
        };
        assert_eq!(cmd.display_name(), "/mode [name]");
    }

    #[test]
    fn display_name_without_args() {
        let cmd = CommandEntry {
            name: "/help",
            args: "",
            description: "test",
            category: CommandCategory::Navigation,
        };
        assert_eq!(cmd.display_name(), "/help");
    }

    #[test]
    fn ensure_visible_scrolls() {
        let mut picker = CommandPicker::new();
        picker.max_visible = 3;
        picker.scroll_offset = 0;
        picker.ensure_visible(5);
        assert_eq!(picker.scroll_offset, 3); // 5 - 3 + 1
    }
}
