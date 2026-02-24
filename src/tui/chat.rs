use std::time::Duration;

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use super::message;
use super::theme::TuiTheme;

/// A single message in the chat history.
#[derive(Debug, Clone)]
pub enum ChatMessage {
    User {
        text: String,
    },
    Assistant {
        text: String,
        tool_calls: Vec<ToolCallDisplay>,
        is_streaming: bool,
    },
    System {
        text: String,
        severity: message::SystemSeverity,
        diff_lines: Option<Vec<super::approval_overlay::DiffLine>>,
    },
}

impl ChatMessage {
    /// Create a System message with no diff (convenience constructor).
    pub fn system(severity: message::SystemSeverity, text: impl Into<String>) -> Self {
        ChatMessage::System {
            text: text.into(),
            severity,
            diff_lines: None,
        }
    }
}

/// Display state for a tool call within an assistant message.
#[derive(Debug, Clone)]
pub enum ToolCallDisplay {
    Running {
        tool_call_id: u64,
        name: String,
        args_display: String,
    },
    Completed {
        tool_call_id: u64,
        name: String,
        duration: Duration,
    },
    Failed {
        tool_call_id: u64,
        name: String,
        error: String,
    },
    AgentRunning {
        agent_type: String,
        task: String,
        last_tool: Option<String>,
    },
    AgentCompleted {
        agent_type: String,
        duration: Duration,
    },
}

/// Scroll state for the chat viewport.
#[derive(Default)]
pub struct ChatViewport {
    /// None = auto-scroll (follow bottom). Some(n) = manual offset from top.
    scroll_offset: Option<usize>,
    /// Total number of rendered lines in the chat.
    pub total_height: usize,
    /// Number of visible rows in the chat area.
    pub visible_height: usize,
}

impl ChatViewport {
    pub fn new() -> Self {
        Self {
            scroll_offset: None,
            total_height: 0,
            visible_height: 0,
        }
    }

    /// Whether auto-scroll is active (following the bottom).
    pub fn is_auto_scroll(&self) -> bool {
        self.scroll_offset.is_none()
    }

    /// Get the effective scroll offset for rendering.
    pub fn effective_offset(&self) -> usize {
        match self.scroll_offset {
            Some(offset) => offset,
            None => self.total_height.saturating_sub(self.visible_height),
        }
    }

    /// Number of lines above the visible area.
    pub fn lines_above(&self) -> usize {
        self.effective_offset()
    }

    /// Scroll up by `n` lines. Switches from auto-scroll to manual.
    pub fn scroll_up(&mut self, n: usize) {
        let current = self.effective_offset();
        self.scroll_offset = Some(current.saturating_sub(n));
    }

    /// Scroll down by `n` lines. Re-enables auto-scroll if at bottom.
    pub fn scroll_down(&mut self, n: usize) {
        let current = self.effective_offset();
        let max = self.total_height.saturating_sub(self.visible_height);
        let new_offset = (current + n).min(max);
        if new_offset >= max {
            self.scroll_offset = None; // Re-enable auto-scroll
        } else {
            self.scroll_offset = Some(new_offset);
        }
    }

    pub fn page_up(&mut self) {
        let page = self.visible_height.max(1);
        self.scroll_up(page);
    }

    pub fn page_down(&mut self) {
        let page = self.visible_height.max(1);
        self.scroll_down(page);
    }

    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = Some(0);
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = None;
    }
}

/// Render the chat area with all messages, using a line-count cache for performance.
///
/// The `line_cache` stores the number of rendered lines per message (including the
/// trailing blank spacer line). Only new or changed messages are re-measured, and
/// only the visible range of messages is fully rendered.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    messages: &[ChatMessage],
    viewport: &mut ChatViewport,
    tick: usize,
    line_cache: &mut Vec<usize>,
) {
    let width = area.width.saturating_sub(2) as usize; // account for side borders

    // ── Incrementally update line cache ──
    // Truncate if messages were removed (e.g. /clear)
    if line_cache.len() > messages.len() {
        line_cache.truncate(messages.len());
    }
    // Compute line counts for any new messages
    for msg in messages.iter().skip(line_cache.len()) {
        let count = message::render_message(msg, width, tick).len() + 1; // +1 spacer
        line_cache.push(count);
    }
    // Always recompute the last message (it may be streaming / growing)
    if let Some(last_idx) = messages.len().checked_sub(1) {
        line_cache[last_idx] =
            message::render_message(&messages[last_idx], width, tick).len() + 1;
    }

    // Compute total height from cache
    let total_height: usize = line_cache.iter().sum();

    // Update viewport dimensions
    viewport.total_height = total_height;
    viewport.visible_height = area.height as usize;

    let offset = viewport.effective_offset();

    // ── Find the first visible message via prefix-sum scan ──
    let mut cumulative = 0usize;
    let mut first_msg_idx = 0;
    let mut skip_lines_in_first = 0; // lines to skip within the first visible message

    for (i, &count) in line_cache.iter().enumerate() {
        if cumulative + count > offset {
            first_msg_idx = i;
            skip_lines_in_first = offset - cumulative;
            break;
        }
        cumulative += count;
        first_msg_idx = i + 1; // past all messages
    }

    // ── Render only the visible range ──
    let mut visible_lines: Vec<Line<'_>> = Vec::with_capacity(viewport.visible_height);

    for (i, msg) in messages.iter().enumerate().skip(first_msg_idx) {
        let mut msg_lines = message::render_message(msg, width, tick);
        msg_lines.push(Line::default()); // spacer

        let lines_to_skip = if i == first_msg_idx { skip_lines_in_first } else { 0 };
        for line in msg_lines.into_iter().skip(lines_to_skip) {
            visible_lines.push(line);
            if visible_lines.len() >= viewport.visible_height {
                break;
            }
        }
        if visible_lines.len() >= viewport.visible_height {
            break;
        }
    }

    // Render with side borders
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::LEFT | ratatui::widgets::Borders::RIGHT)
        .border_style(Style::default().fg(TuiTheme::BORDER_DIM));

    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(visible_lines), inner);

    // ── Scroll indicators ──
    let lines_above = viewport.lines_above();
    if lines_above > 0 {
        let indicator = format!(" {} lines above ", lines_above);
        let indicator_line =
            Line::from(Span::styled(indicator, Style::new().fg(TuiTheme::FG_MUTED)))
                .alignment(Alignment::Center);
        let indicator_area = Rect::new(area.x, area.y, area.width, 1);
        frame.render_widget(Paragraph::new(indicator_line), indicator_area);
    }

    let lines_below = total_height.saturating_sub(offset + viewport.visible_height);
    if lines_below > 0 {
        let indicator = format!(" {} lines below ", lines_below);
        let indicator_line =
            Line::from(Span::styled(indicator, Style::new().fg(TuiTheme::FG_MUTED)))
                .alignment(Alignment::Center);
        let indicator_area = Rect::new(area.x, area.y + area.height.saturating_sub(1), area.width, 1);
        frame.render_widget(Paragraph::new(indicator_line), indicator_area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_auto_scroll_default() {
        let vp = ChatViewport::new();
        assert!(vp.is_auto_scroll());
        assert_eq!(vp.effective_offset(), 0);
    }

    #[test]
    fn viewport_scroll_up_switches_to_manual() {
        let mut vp = ChatViewport::new();
        vp.total_height = 100;
        vp.visible_height = 20;
        vp.scroll_up(5);
        assert!(!vp.is_auto_scroll());
        assert_eq!(vp.effective_offset(), 75); // 80 - 5
    }

    #[test]
    fn viewport_scroll_down_to_bottom_re_enables_auto() {
        let mut vp = ChatViewport::new();
        vp.total_height = 100;
        vp.visible_height = 20;
        vp.scroll_offset = Some(70);
        vp.scroll_down(100); // Past the bottom
        assert!(vp.is_auto_scroll());
    }

    #[test]
    fn viewport_page_up_down() {
        let mut vp = ChatViewport::new();
        vp.total_height = 100;
        vp.visible_height = 20;
        vp.page_up();
        assert_eq!(vp.effective_offset(), 60); // 80 - 20
        vp.page_down();
        assert!(vp.is_auto_scroll()); // 60 + 20 = 80 = max
    }

    #[test]
    fn viewport_scroll_to_top() {
        let mut vp = ChatViewport::new();
        vp.total_height = 100;
        vp.visible_height = 20;
        vp.scroll_to_top();
        assert_eq!(vp.effective_offset(), 0);
        assert!(!vp.is_auto_scroll());
    }

    #[test]
    fn viewport_scroll_to_bottom() {
        let mut vp = ChatViewport::new();
        vp.total_height = 100;
        vp.visible_height = 20;
        vp.scroll_offset = Some(30);
        vp.scroll_to_bottom();
        assert!(vp.is_auto_scroll());
        assert_eq!(vp.effective_offset(), 80);
    }
}
