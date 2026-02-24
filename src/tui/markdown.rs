use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::prelude::*;

use super::theme::TuiTheme;

/// Convert markdown text to styled ratatui Lines for rendering in the TUI.
pub fn render_markdown(text: &str, width: usize) -> Vec<Line<'static>> {
    let options = Options::ENABLE_STRIKETHROUGH;
    let parser = Parser::new_ext(text, options);

    let mut renderer = MdRenderer::new(width);
    for event in parser {
        renderer.process(event);
    }
    renderer.finish()
}

/// Internal state for the markdown → Lines conversion.
struct MdRenderer {
    width: usize,
    lines: Vec<Line<'static>>,
    /// Spans accumulated for the current line being built.
    current_spans: Vec<Span<'static>>,
    /// Stack of style modifiers (bold, italic, etc.).
    style_stack: Vec<Style>,
    /// Whether we're inside a code block (collect raw text, don't style inline).
    in_code_block: bool,
    /// Accumulated text for code blocks.
    code_block_lines: Vec<String>,
    /// List nesting: each entry is Some(counter) for ordered, None for unordered.
    list_stack: Vec<Option<u64>>,
    /// Whether we're at the start of a list item (need to emit prefix).
    item_prefix_pending: bool,
    /// Blockquote nesting depth.
    blockquote_depth: usize,
    /// Whether we're inside a heading (to collect text for wrapping).
    in_heading: bool,
    heading_level: usize,
}

impl MdRenderer {
    fn new(width: usize) -> Self {
        Self {
            width,
            lines: Vec::new(),
            current_spans: Vec::new(),
            style_stack: Vec::new(),
            in_code_block: false,
            code_block_lines: Vec::new(),
            list_stack: Vec::new(),
            item_prefix_pending: false,
            blockquote_depth: 0,
            in_heading: false,
            heading_level: 0,
        }
    }

    fn current_style(&self) -> Style {
        let mut style = Style::default().fg(TuiTheme::FG);
        for s in &self.style_stack {
            style = style.patch(*s);
        }
        style
    }

    fn indent_width(&self) -> usize {
        let list_indent = self.list_stack.len() * 2;
        let bq_indent = self.blockquote_depth * 4;
        list_indent + bq_indent
    }

    fn content_width(&self) -> usize {
        self.width.saturating_sub(self.indent_width())
    }

    fn process(&mut self, event: Event<'_>) {
        match event {
            // ── Block-level Start ──
            Event::Start(Tag::Heading { level, .. }) => {
                self.flush_line();
                self.in_heading = true;
                self.heading_level = level as usize;
                self.style_stack
                    .push(Style::default().fg(TuiTheme::MD_HEADING).bold());
                let prefix = "#".repeat(self.heading_level);
                self.current_spans
                    .push(Span::styled(format!("{} ", prefix), self.current_style()));
            }
            Event::End(TagEnd::Heading(_)) => {
                self.style_stack.pop();
                self.in_heading = false;
                self.flush_line();
                // Blank line after heading
                self.lines.push(Line::from(""));
            }

            Event::Start(Tag::Paragraph) => {
                self.flush_line();
            }
            Event::End(TagEnd::Paragraph) => {
                self.flush_line();
                // Blank line after paragraph (unless in a list item)
                if self.list_stack.is_empty() {
                    self.lines.push(Line::from(""));
                }
            }

            Event::Start(Tag::CodeBlock(_kind)) => {
                self.flush_line();
                self.in_code_block = true;
                self.code_block_lines.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                self.in_code_block = false;
                let code_lines = std::mem::take(&mut self.code_block_lines);
                let style = Style::default()
                    .fg(TuiTheme::MD_CODE_BLOCK_FG)
                    .bg(TuiTheme::MD_CODE_BLOCK_BG);
                let max_w = self.width.saturating_sub(2);
                for code_line in &code_lines {
                    let display = if code_line.len() > max_w && max_w > 3 {
                        format!("  {}...", &code_line[..max_w - 3])
                    } else {
                        format!("  {}", code_line)
                    };
                    self.lines.push(Line::from(Span::styled(display, style)));
                }
                self.lines.push(Line::from(""));
            }

            Event::Start(Tag::BlockQuote(_)) => {
                self.flush_line();
                self.blockquote_depth += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                self.flush_line();
                self.blockquote_depth = self.blockquote_depth.saturating_sub(1);
            }

            Event::Start(Tag::List(start)) => {
                self.flush_line();
                self.list_stack.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                self.flush_line();
                self.list_stack.pop();
                // Blank line after top-level list
                if self.list_stack.is_empty() {
                    self.lines.push(Line::from(""));
                }
            }

            Event::Start(Tag::Item) => {
                self.flush_line();
                self.item_prefix_pending = true;
            }
            Event::End(TagEnd::Item) => {
                self.flush_line();
            }

            // ── Inline Start ──
            Event::Start(Tag::Emphasis) => {
                self.style_stack.push(Style::default().italic());
            }
            Event::End(TagEnd::Emphasis) => {
                self.style_stack.pop();
            }

            Event::Start(Tag::Strong) => {
                self.style_stack.push(Style::default().bold());
            }
            Event::End(TagEnd::Strong) => {
                self.style_stack.pop();
            }

            Event::Start(Tag::Strikethrough) => {
                self.style_stack
                    .push(Style::default().add_modifier(Modifier::CROSSED_OUT));
            }
            Event::End(TagEnd::Strikethrough) => {
                self.style_stack.pop();
            }

            Event::Start(Tag::Link { dest_url, .. }) => {
                self.style_stack
                    .push(Style::default().fg(TuiTheme::MD_LINK));
                // Store URL for potential display after link text
                // For now, just style the text — URLs are rarely useful in a TUI
                let _ = dest_url;
            }
            Event::End(TagEnd::Link) => {
                self.style_stack.pop();
            }

            // ── Leaf content ──
            Event::Text(text) => {
                if self.in_code_block {
                    // Collect code block lines raw
                    for line in text.split('\n') {
                        self.code_block_lines.push(line.to_string());
                    }
                } else {
                    self.emit_text(&text);
                }
            }

            Event::Code(code) => {
                // Inline code
                let style = Style::default().fg(TuiTheme::MD_CODE_FG);
                self.maybe_emit_item_prefix();
                self.maybe_emit_blockquote_prefix();
                self.current_spans
                    .push(Span::styled(format!("`{}`", code), style));
            }

            Event::SoftBreak => {
                // Treat as space
                self.current_spans
                    .push(Span::styled(" ", self.current_style()));
            }

            Event::HardBreak => {
                self.flush_line();
            }

            Event::Rule => {
                self.flush_line();
                let rule = "─".repeat(self.width);
                self.lines.push(Line::from(Span::styled(
                    rule,
                    Style::default().fg(TuiTheme::FG_MUTED),
                )));
                self.lines.push(Line::from(""));
            }

            // Ignore unsupported events
            _ => {}
        }
    }

    fn emit_text(&mut self, text: &str) {
        self.maybe_emit_item_prefix();
        self.maybe_emit_blockquote_prefix();

        let style = self.current_style();
        self.current_spans
            .push(Span::styled(text.to_string(), style));
    }

    fn maybe_emit_item_prefix(&mut self) {
        if !self.item_prefix_pending {
            return;
        }
        self.item_prefix_pending = false;

        let depth = self.list_stack.len().saturating_sub(1);
        let indent = "  ".repeat(depth);
        let marker = if let Some(Some(n)) = self.list_stack.last() {
            let s = format!("{}{}. ", indent, n);
            // Increment counter for next item
            if let Some(Some(ref mut counter)) = self.list_stack.last_mut() {
                *counter += 1;
            }
            s
        } else {
            format!("{}- ", indent)
        };

        self.current_spans.push(Span::styled(
            marker,
            Style::default().fg(TuiTheme::MD_LIST_MARKER),
        ));
    }

    fn maybe_emit_blockquote_prefix(&mut self) {
        if self.blockquote_depth > 0 && self.current_spans.is_empty() {
            let prefix = "  > ".repeat(self.blockquote_depth);
            self.current_spans.push(Span::styled(
                prefix,
                Style::default().fg(TuiTheme::MD_BLOCKQUOTE),
            ));
        }
    }

    fn flush_line(&mut self) {
        if self.current_spans.is_empty() {
            return;
        }

        let spans = std::mem::take(&mut self.current_spans);
        let wrapped = wrap_spans(spans, self.content_width());
        self.lines.extend(wrapped);
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush_line();
        // Remove trailing blank lines
        while self.lines.last().is_some_and(|l| line_is_blank(l)) {
            self.lines.pop();
        }
        self.lines
    }
}

/// Check if a Line is effectively blank.
fn line_is_blank(line: &Line<'_>) -> bool {
    line.spans.is_empty() || line.spans.iter().all(|s| s.content.trim().is_empty())
}

/// Wrap a list of styled spans to fit within `max_width`, preserving styles.
fn wrap_spans(spans: Vec<Span<'static>>, max_width: usize) -> Vec<Line<'static>> {
    if max_width == 0 {
        return vec![Line::from(spans)];
    }

    // Calculate total width
    let total: usize = spans.iter().map(|s| s.content.len()).sum();
    if total <= max_width {
        return vec![Line::from(spans)];
    }

    let mut result: Vec<Line<'static>> = Vec::new();
    let mut current_line: Vec<Span<'static>> = Vec::new();
    let mut line_width: usize = 0;

    for span in spans {
        let span_text = span.content.to_string();
        let style = span.style;

        if line_width + span_text.len() <= max_width {
            line_width += span_text.len();
            current_line.push(Span::styled(span_text, style));
            continue;
        }

        // Need to split this span
        let mut remaining = span_text.as_str();
        while !remaining.is_empty() {
            let available = max_width.saturating_sub(line_width);
            if available == 0 {
                // Current line is full, push it
                if !current_line.is_empty() {
                    result.push(Line::from(std::mem::take(&mut current_line)));
                }
                line_width = 0;
                continue;
            }

            if remaining.len() <= available {
                line_width += remaining.len();
                current_line.push(Span::styled(remaining.to_string(), style));
                break;
            }

            // Find a good break point
            let break_at = find_break_point(remaining, available);
            let (chunk, rest) = remaining.split_at(break_at);
            let chunk = chunk.trim_end();
            let rest = rest.trim_start();

            if !chunk.is_empty() {
                current_line.push(Span::styled(chunk.to_string(), style));
            }
            if !current_line.is_empty() {
                result.push(Line::from(std::mem::take(&mut current_line)));
            }
            line_width = 0;
            remaining = rest;
        }
    }

    if !current_line.is_empty() {
        result.push(Line::from(current_line));
    }

    if result.is_empty() {
        vec![Line::from("")]
    } else {
        result
    }
}

/// Find the best break point within `max_width` characters.
fn find_break_point(text: &str, max_width: usize) -> usize {
    if max_width >= text.len() {
        return text.len();
    }
    // Check if character after max_width is a space
    if text.as_bytes().get(max_width) == Some(&b' ') {
        return max_width;
    }
    // Find last space within max_width
    text[..max_width]
        .rfind(' ')
        .map(|i| i + 1)
        .unwrap_or(max_width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_passthrough() {
        let lines = render_markdown("Hello world", 80);
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("Hello world"));
    }

    #[test]
    fn heading_styled() {
        let lines = render_markdown("# Title", 80);
        assert!(!lines.is_empty());
        let first_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(first_text.contains("# "));
        assert!(first_text.contains("Title"));
        // Check that heading has bold modifier
        let has_bold = lines[0]
            .spans
            .iter()
            .any(|s| s.style.add_modifier.contains(Modifier::BOLD));
        assert!(has_bold);
    }

    #[test]
    fn bold_text() {
        let lines = render_markdown("Hello **bold** world", 80);
        assert!(!lines.is_empty());
        let has_bold = lines[0]
            .spans
            .iter()
            .any(|s| s.style.add_modifier.contains(Modifier::BOLD));
        assert!(has_bold);
    }

    #[test]
    fn italic_text() {
        let lines = render_markdown("Hello *italic* world", 80);
        assert!(!lines.is_empty());
        let has_italic = lines[0]
            .spans
            .iter()
            .any(|s| s.style.add_modifier.contains(Modifier::ITALIC));
        assert!(has_italic);
    }

    #[test]
    fn inline_code() {
        let lines = render_markdown("Use `foo()` here", 80);
        assert!(!lines.is_empty());
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("`foo()`"));
    }

    #[test]
    fn code_block() {
        let md = "```rust\nfn main() {}\n```";
        let lines = render_markdown(md, 80);
        // Should have at least the code line
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all_text.contains("fn main()"));
    }

    #[test]
    fn unordered_list() {
        let md = "- item one\n- item two";
        let lines = render_markdown(md, 80);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("|");
        assert!(all_text.contains("- "));
        assert!(all_text.contains("item one"));
        assert!(all_text.contains("item two"));
    }

    #[test]
    fn horizontal_rule() {
        let lines = render_markdown("---", 40);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("");
        assert!(all_text.contains("─"));
    }

    #[test]
    fn long_text_wraps() {
        let long = "word ".repeat(30);
        let lines = render_markdown(&long, 40);
        // Should produce multiple lines
        assert!(lines.len() > 1);
    }

    #[test]
    fn empty_input() {
        let lines = render_markdown("", 80);
        assert!(lines.is_empty());
    }

    #[test]
    fn blockquote() {
        let lines = render_markdown("> quoted text", 80);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("");
        assert!(all_text.contains("> "));
        assert!(all_text.contains("quoted text"));
    }
}
