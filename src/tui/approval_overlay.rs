use ratatui::layout::{Constraint, Rect};
use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};

use crate::ui::approval::FileChange;

use super::theme::TuiTheme;

/// Tag for a diff line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLineTag {
    HunkHeader,
    Add,
    Delete,
    Context,
}

/// A single line in a unified diff.
#[derive(Debug, Clone)]
pub struct DiffLine {
    pub tag: DiffLineTag,
    pub content: String,
}

/// Overlay state for a file approval prompt.
pub struct ApprovalOverlay {
    pub file_path: String,
    pub is_new_file: bool,
    pub diff_lines: Vec<DiffLine>,
    pub additions: usize,
    pub deletions: usize,
    pub scroll_offset: usize,
}

impl ApprovalOverlay {
    /// Build an overlay from a FileChange using `similar` for diff computation.
    pub fn from_change(change: &FileChange) -> Self {
        let mut diff_lines = Vec::new();
        let mut additions = 0;
        let mut deletions = 0;

        if change.is_new_file {
            // New file: all lines are additions
            diff_lines.push(DiffLine {
                tag: DiffLineTag::HunkHeader,
                content: format!("@@ new file: {} @@", change.file_path),
            });
            for line in change.new_content.lines() {
                diff_lines.push(DiffLine {
                    tag: DiffLineTag::Add,
                    content: format!("+{}", line),
                });
                additions += 1;
            }
        } else {
            // Modified file: compute unified diff
            let text_diff = similar::TextDiff::from_lines(&change.old_content, &change.new_content);
            let unified = text_diff.unified_diff();

            for hunk in unified.iter_hunks() {
                diff_lines.push(DiffLine {
                    tag: DiffLineTag::HunkHeader,
                    content: format!("{}", hunk.header()),
                });
                for change in hunk.iter_changes() {
                    match change.tag() {
                        similar::ChangeTag::Equal => {
                            diff_lines.push(DiffLine {
                                tag: DiffLineTag::Context,
                                content: format!(" {}", change.value().trim_end_matches('\n')),
                            });
                        }
                        similar::ChangeTag::Insert => {
                            diff_lines.push(DiffLine {
                                tag: DiffLineTag::Add,
                                content: format!("+{}", change.value().trim_end_matches('\n')),
                            });
                            additions += 1;
                        }
                        similar::ChangeTag::Delete => {
                            diff_lines.push(DiffLine {
                                tag: DiffLineTag::Delete,
                                content: format!("-{}", change.value().trim_end_matches('\n')),
                            });
                            deletions += 1;
                        }
                    }
                }
            }
        }

        Self {
            file_path: change.file_path.clone(),
            is_new_file: change.is_new_file,
            diff_lines,
            additions,
            deletions,
            scroll_offset: 0,
        }
    }

    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    pub fn scroll_down(&mut self, n: usize, visible_height: usize) {
        let max = self.diff_lines.len().saturating_sub(visible_height);
        self.scroll_offset = (self.scroll_offset + n).min(max);
    }
}

pub fn render(frame: &mut Frame, overlay: &ApprovalOverlay, terminal_area: Rect) {
    let overlay_width = (terminal_area.width.saturating_sub(6)).min(100);
    let overlay_height =
        (terminal_area.height.saturating_sub(6)).min((overlay.diff_lines.len() as u16) + 8);
    let overlay_height = overlay_height.max(10);

    let x = (terminal_area.width.saturating_sub(overlay_width)) / 2;
    let y = (terminal_area.height.saturating_sub(overlay_height)) / 2;
    let area = Rect::new(x, y, overlay_width, overlay_height);

    frame.render_widget(Clear, area);

    let badge = if overlay.is_new_file {
        "CREATE"
    } else {
        "MODIFY"
    };
    let title = format!(" {} {} ", badge, overlay.file_path);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(TuiTheme::ACCENT))
        .padding(Padding::horizontal(1))
        .title(title)
        .title_bottom(
            " [y] approve  [n] reject  [d] full diff  [Esc] reject ".fg(TuiTheme::FG_DIM),
        );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 2 {
        return;
    }

    // Layout: diff body + summary line
    let [diff_area, summary_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);

    // Render diff lines
    let max_line_width = diff_area.width as usize;
    let visible_count = diff_area.height as usize;

    let visible_lines: Vec<Line> = overlay
        .diff_lines
        .iter()
        .skip(overlay.scroll_offset)
        .take(visible_count)
        .map(|dl| {
            let display = truncate_diff_line(&dl.content, max_line_width);
            let style = diff_line_style(&dl.tag);
            Line::from(Span::styled(display, style))
        })
        .collect();

    frame.render_widget(Paragraph::new(visible_lines), diff_area);

    // Summary
    let summary = Line::from(vec![
        Span::styled(
            format!("+{}", overlay.additions),
            Style::default().fg(TuiTheme::DIFF_ADD),
        ),
        Span::styled(" / ", Style::default().fg(TuiTheme::FG_DIM)),
        Span::styled(
            format!("-{}", overlay.deletions),
            Style::default().fg(TuiTheme::DIFF_DEL),
        ),
    ]);
    frame.render_widget(Paragraph::new(summary), summary_area);
}

fn diff_line_style(tag: &DiffLineTag) -> Style {
    match tag {
        DiffLineTag::HunkHeader => Style::default().fg(TuiTheme::DIFF_HUNK),
        DiffLineTag::Add => Style::default()
            .fg(TuiTheme::DIFF_ADD)
            .bg(TuiTheme::DIFF_ADD_BG),
        DiffLineTag::Delete => Style::default()
            .fg(TuiTheme::DIFF_DEL)
            .bg(TuiTheme::DIFF_DEL_BG),
        DiffLineTag::Context => Style::default().fg(TuiTheme::DIFF_CONTEXT),
    }
}

fn truncate_diff_line(s: &str, max_width: usize) -> String {
    if s.len() <= max_width {
        s.to_string()
    } else if max_width > 3 {
        format!("{}...", &s[..max_width - 3])
    } else {
        s[..max_width].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_change(old: &str, new: &str, is_new: bool) -> FileChange {
        FileChange {
            file_path: "test.rs".into(),
            resolved_path: "/tmp/test.rs".into(),
            old_content: old.into(),
            new_content: new.into(),
            is_new_file: is_new,
        }
    }

    #[test]
    fn new_file_all_additions() {
        let change = make_change("", "line1\nline2\nline3\n", true);
        let overlay = ApprovalOverlay::from_change(&change);
        assert!(overlay.is_new_file);
        assert_eq!(overlay.additions, 3);
        assert_eq!(overlay.deletions, 0);
        // Hunk header + 3 add lines
        assert_eq!(overlay.diff_lines.len(), 4);
    }

    #[test]
    fn modified_file_counts() {
        let old = "line1\nline2\nline3\n";
        let new = "line1\nmodified\nline3\nnew_line\n";
        let change = make_change(old, new, false);
        let overlay = ApprovalOverlay::from_change(&change);
        assert!(!overlay.is_new_file);
        assert!(overlay.additions > 0);
        assert!(overlay.deletions > 0);
    }

    #[test]
    fn identical_content_empty_diff() {
        let content = "same\n";
        let change = make_change(content, content, false);
        let overlay = ApprovalOverlay::from_change(&change);
        assert_eq!(overlay.additions, 0);
        assert_eq!(overlay.deletions, 0);
    }

    #[test]
    fn scroll_bounds() {
        let change = make_change("", "a\nb\nc\nd\ne\n", true);
        let mut overlay = ApprovalOverlay::from_change(&change);
        overlay.scroll_down(100, 3);
        assert!(overlay.scroll_offset <= overlay.diff_lines.len());
        overlay.scroll_up(100);
        assert_eq!(overlay.scroll_offset, 0);
    }

    #[test]
    fn truncate_diff_line_short() {
        assert_eq!(truncate_diff_line("+hello", 20), "+hello");
    }

    #[test]
    fn truncate_diff_line_long() {
        let long = "+".to_string() + &"x".repeat(100);
        let result = truncate_diff_line(&long, 20);
        assert_eq!(result.len(), 20);
        assert!(result.ends_with("..."));
    }
}
