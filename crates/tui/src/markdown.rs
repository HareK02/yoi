//! Markdown renderer for assistant text.
//!
//! Streams `pulldown-cmark` events into ratatui `Line`s that drop straight
//! into the rest of the TUI's wrap/scroll pipeline. Scope (which Markdown
//! features get styled) and exclusions are documented in
//! `tickets/tui-assistant-markdown.md`.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

const LIST_INDENT: &str = "  ";
const RULE_WIDTH: usize = 40;

pub fn render(text: &str, base: Style) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut r = Renderer::new(base);
    let parser = Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH);
    for event in parser {
        r.handle(event, &mut out);
    }
    r.finish(&mut out);
    out
}

struct Renderer {
    base: Style,
    line_prefix: Vec<Span<'static>>,
    pending_marker: Option<Span<'static>>,
    current: Vec<Span<'static>>,

    bold: u32,
    italic: u32,
    strike: u32,
    in_link: u32,
    in_inline_code: u32,
    image_depth: u32,
    heading: Option<HeadingLevel>,
    in_code_block: bool,

    /// One entry per open list. `Some(n)` carries the next ordinal to
    /// emit for an ordered list; `None` means a bullet list.
    list_stack: Vec<Option<u64>>,

    has_emitted: bool,
    just_blanked: bool,
}

impl Renderer {
    fn new(base: Style) -> Self {
        Self {
            base,
            line_prefix: Vec::new(),
            pending_marker: None,
            current: Vec::new(),
            bold: 0,
            italic: 0,
            strike: 0,
            in_link: 0,
            in_inline_code: 0,
            image_depth: 0,
            heading: None,
            in_code_block: false,
            list_stack: Vec::new(),
            has_emitted: false,
            just_blanked: false,
        }
    }

    fn span_style(&self) -> Style {
        if self.in_inline_code > 0 {
            return Style::default().fg(Color::Yellow).bg(Color::Rgb(40, 40, 40));
        }
        if self.in_code_block {
            return Style::default().fg(Color::Cyan);
        }
        if let Some(level) = self.heading {
            return heading_style(level);
        }
        let mut s = self.base;
        if self.bold > 0 {
            s = s.add_modifier(Modifier::BOLD);
        }
        if self.italic > 0 {
            s = s.add_modifier(Modifier::ITALIC);
        }
        if self.strike > 0 {
            s = s.add_modifier(Modifier::CROSSED_OUT);
        }
        if self.in_link > 0 {
            s = s.fg(Color::Cyan).add_modifier(Modifier::UNDERLINED);
        }
        s
    }

    fn push_text(&mut self, content: String) {
        if self.image_depth > 0 || content.is_empty() {
            return;
        }
        let style = self.span_style();
        self.current.push(Span::styled(content, style));
    }

    fn flush_line(&mut self, out: &mut Vec<Line<'static>>) {
        if self.current.is_empty() && self.pending_marker.is_none() {
            return;
        }
        let mut spans: Vec<Span<'static>> = self.line_prefix.clone();
        if let Some(m) = self.pending_marker.take() {
            spans.push(m);
        }
        spans.extend(self.current.drain(..));
        out.push(Line::from(spans));
        self.has_emitted = true;
        self.just_blanked = false;
    }

    fn emit_blank(&mut self, out: &mut Vec<Line<'static>>) {
        if !self.has_emitted || self.just_blanked {
            return;
        }
        out.push(Line::from(""));
        self.just_blanked = true;
    }

    fn handle(&mut self, ev: Event<'_>, out: &mut Vec<Line<'static>>) {
        match ev {
            Event::Start(tag) => self.start(tag, out),
            Event::End(tag) => self.end(tag, out),
            Event::Text(s) => {
                if self.in_code_block {
                    let mut iter = s.split('\n').peekable();
                    while let Some(piece) = iter.next() {
                        if !piece.is_empty() {
                            self.push_text(piece.to_owned());
                        }
                        if iter.peek().is_some() {
                            self.flush_line(out);
                        }
                    }
                } else {
                    self.push_text(s.into_string());
                }
            }
            Event::Code(s) => {
                self.in_inline_code += 1;
                self.push_text(s.into_string());
                self.in_inline_code -= 1;
            }
            Event::SoftBreak => self.push_text(" ".to_owned()),
            Event::HardBreak => self.flush_line(out),
            Event::Rule => {
                self.emit_blank(out);
                out.push(Line::from(Span::styled(
                    "─".repeat(RULE_WIDTH),
                    Style::default().fg(Color::DarkGray),
                )));
                self.has_emitted = true;
                self.just_blanked = false;
                self.emit_blank(out);
            }
            // HTML / inline HTML / footnote refs / task list markers etc.
            // are intentionally dropped or fall through as raw text in
            // Text events that surround them — the ticket scopes those
            // out explicitly.
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>, out: &mut Vec<Line<'static>>) {
        match tag {
            Tag::Paragraph => {
                self.emit_blank(out);
            }
            Tag::Heading { level, .. } => {
                self.emit_blank(out);
                self.heading = Some(level);
            }
            Tag::CodeBlock(_) => {
                self.emit_blank(out);
                self.in_code_block = true;
            }
            Tag::List(start) => {
                // Close any in-flight line (in tight nested lists the
                // parent item's text arrives without a Paragraph wrapper,
                // so it's still sitting in `current` when the child list
                // opens).
                self.flush_line(out);
                if self.list_stack.is_empty() {
                    self.emit_blank(out);
                }
                if !self.list_stack.is_empty() {
                    self.line_prefix.push(Span::raw(LIST_INDENT));
                }
                self.list_stack.push(start);
            }
            Tag::Item => {
                self.flush_line(out);
                let marker_text = match self.list_stack.last_mut() {
                    Some(Some(n)) => {
                        let s = format!("{}. ", *n);
                        *n += 1;
                        s
                    }
                    _ => "• ".to_owned(),
                };
                self.pending_marker = Some(Span::styled(
                    marker_text,
                    Style::default().fg(Color::DarkGray),
                ));
            }
            Tag::BlockQuote(_) => {
                self.emit_blank(out);
                self.line_prefix.push(Span::styled(
                    "│ ",
                    Style::default().fg(Color::DarkGray),
                ));
            }
            Tag::Strong => self.bold += 1,
            Tag::Emphasis => self.italic += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { .. } => self.in_link += 1,
            Tag::Image { .. } => self.image_depth += 1,
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd, out: &mut Vec<Line<'static>>) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_line(out);
            }
            TagEnd::Heading(_) => {
                self.flush_line(out);
                self.heading = None;
            }
            TagEnd::CodeBlock => {
                self.flush_line(out);
                self.in_code_block = false;
            }
            TagEnd::List(_) => {
                self.list_stack.pop();
                if !self.list_stack.is_empty() {
                    self.line_prefix.pop();
                }
                // Don't emit a blank between a closing inner list and
                // its parent item's continuation — the parent will close
                // its own paragraph if it had one.
            }
            TagEnd::Item => {
                self.flush_line(out);
                // Empty list item: marker was never consumed, drop it
                // so it doesn't bleed onto the next item.
                self.pending_marker = None;
            }
            TagEnd::BlockQuote(_) => {
                self.flush_line(out);
                self.line_prefix.pop();
            }
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => self.in_link = self.in_link.saturating_sub(1),
            TagEnd::Image => self.image_depth = self.image_depth.saturating_sub(1),
            _ => {}
        }
    }

    fn finish(&mut self, out: &mut Vec<Line<'static>>) {
        self.flush_line(out);
        while matches!(out.last(), Some(l) if l.spans.iter().all(|s| s.content.is_empty())) {
            out.pop();
        }
    }
}

fn heading_style(level: HeadingLevel) -> Style {
    let base = Style::default().add_modifier(Modifier::BOLD);
    match level {
        HeadingLevel::H1 | HeadingLevel::H2 => base.fg(Color::Cyan),
        HeadingLevel::H3 => base.fg(Color::Magenta),
        HeadingLevel::H4 | HeadingLevel::H5 | HeadingLevel::H6 => base.fg(Color::White),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn render_plain(text: &str) -> Vec<String> {
        render(text, Style::default())
            .iter()
            .map(line_text)
            .collect()
    }

    #[test]
    fn plain_paragraph() {
        assert_eq!(render_plain("hello world"), vec!["hello world"]);
    }

    #[test]
    fn paragraphs_separated_by_blank_line() {
        let lines = render_plain("first\n\nsecond");
        assert_eq!(lines, vec!["first", "", "second"]);
    }

    #[test]
    fn soft_break_collapses_to_space() {
        // CommonMark: a single newline inside a paragraph is a soft break.
        let lines = render_plain("a\nb");
        assert_eq!(lines, vec!["a b"]);
    }

    #[test]
    fn heading_emits_dedicated_line() {
        let lines = render_plain("# Title\n\nbody");
        assert_eq!(lines, vec!["Title", "", "body"]);
    }

    #[test]
    fn unordered_list_uses_bullet_marker() {
        let lines = render_plain("- a\n- b");
        assert_eq!(lines, vec!["• a", "• b"]);
    }

    #[test]
    fn ordered_list_numbers_continue() {
        let lines = render_plain("1. a\n2. b");
        assert_eq!(lines, vec!["1. a", "2. b"]);
    }

    #[test]
    fn nested_list_indents() {
        let lines = render_plain("- a\n  - b\n- c");
        assert_eq!(lines, vec!["• a", "  • b", "• c"]);
    }

    #[test]
    fn block_quote_prefixes_pipe() {
        let lines = render_plain("> quoted");
        assert_eq!(lines, vec!["│ quoted"]);
    }

    #[test]
    fn fenced_code_block_preserves_lines() {
        let lines = render_plain("```rust\nlet x = 1;\nlet y = 2;\n```");
        assert!(lines.contains(&"let x = 1;".to_owned()));
        assert!(lines.contains(&"let y = 2;".to_owned()));
    }

    #[test]
    fn rule_renders_horizontal_line() {
        let lines = render_plain("a\n\n---\n\nb");
        assert!(lines.iter().any(|l| l.contains('─')));
    }

    #[test]
    fn image_alt_is_dropped() {
        let lines = render_plain("![alt text](http://x)");
        // Empty image paragraph collapses to nothing visible.
        assert!(lines.iter().all(|l| !l.contains("alt text")));
    }

    #[test]
    fn link_text_is_kept() {
        let lines = render_plain("see [here](http://x)");
        assert_eq!(lines, vec!["see here"]);
    }

    #[test]
    fn empty_input_yields_no_lines() {
        assert!(render_plain("").is_empty());
    }

    #[test]
    fn unfinished_emphasis_is_treated_as_text() {
        // Streaming partial: opener arrived, closer hasn't.
        let lines = render_plain("hello **world");
        assert_eq!(lines, vec!["hello **world"]);
    }
}
