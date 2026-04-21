//! Full-screen rendering for the TUI.
//!
//! The layout is stacked top-to-bottom:
//!
//! ```text
//!   history view (fills remaining space)
//!   ──────────── separator ──────────
//!   status line (1 row)
//!   > input area (1 row in Phase 1)
//! ```
//!
//! Every frame we walk the entire `App::blocks` vector, produce styled
//! lines, and render the tail that fits the history area. No
//! `insert_before` use — the terminal scrollback stays untouched.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block as UiBlock, BorderType, Borders, Padding, Paragraph, Widget, Wrap};

use protocol::{Greeting, NotificationLevel};

use crate::app::{App, fmt_tokens, notification_source_label};
use crate::block::{Block, CompactEvent};

/// Display density for the history view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Every block fully expanded.
    Detail,
    /// Completed blocks compressed to roughly 5–6 lines; in-progress
    /// tool blocks stay in detail.
    Normal,
    /// Each block rendered as a single line.
    Overview,
}

impl Mode {
    pub fn cycle(self) -> Self {
        match self {
            Mode::Detail => Mode::Normal,
            Mode::Normal => Mode::Overview,
            Mode::Overview => Mode::Detail,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Detail => "detail",
            Mode::Normal => "normal",
            Mode::Overview => "overview",
        }
    }
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    // Input content starts after the "> " / "  " prompt, so the width
    // available for wrapping is two columns narrower than the frame.
    let input_content_width = area.width.saturating_sub(2).max(1);
    let input_render = app.input.render(input_content_width);
    let input_height = input_area_height(&input_render, area.height);

    let chunks = Layout::vertical([
        Constraint::Min(0),                  // history view
        Constraint::Length(1),               // separator
        Constraint::Length(1),               // status
        Constraint::Length(input_height),    // input area
    ])
    .split(area);

    draw_history(frame, app, chunks[0]);
    draw_separator(frame, chunks[1]);
    draw_status(frame, app, chunks[2]);
    draw_input(frame, &input_render, chunks[3]);
}

/// Cap the input area so it doesn't eat the history view: grows with the
/// buffer but never past `min(10, terminal_height / 3)`.
fn input_area_height(render: &crate::input::InputRender, terminal_height: u16) -> u16 {
    let needed = render.lines.len().max(1) as u16;
    let cap = (terminal_height / 3).max(1).min(10);
    needed.clamp(1, cap)
}

/// Pre-rendered history lines plus the line indices at which each turn
/// begins (used for Ctrl-[/] jumps).
pub struct HistoryLayout {
    pub lines: Vec<Line<'static>>,
    pub turn_starts: Vec<usize>,
}

pub fn compute_history(app: &App, width: u16) -> HistoryLayout {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut turn_starts: Vec<usize> = Vec::new();
    let mut first = true;
    let mut i = 0;
    while i < app.blocks.len() {
        if !first {
            lines.push(Line::from(""));
        }
        first = false;
        let block = &app.blocks[i];
        if matches!(block, Block::TurnHeader { .. }) {
            turn_starts.push(lines.len());
        }
        // Tool calls route through the per-tool renderer, which may
        // consume multiple adjacent blocks (Read aggregation).
        if matches!(block, Block::ToolCall(_)) {
            let out = crate::tool::render_tool(&app.cache, &app.blocks, i, app.mode);
            lines.extend(out.lines);
            i += out.consumed.max(1);
            continue;
        }
        render_block_into(&mut lines, block, width, app.mode);
        i += 1;
    }
    HistoryLayout { lines, turn_starts }
}

/// Maximum body lines a normal-mode block may emit before truncation.
const NORMAL_MAX_LINES: usize = 6;

fn draw_history(frame: &mut Frame, app: &mut App, area: Rect) {
    if area.height == 0 || area.width == 0 {
        app.scroll.area_height = area.height;
        app.scroll.total_lines = 0;
        app.scroll.tail_top_offset = 0;
        app.scroll.turn_starts.clear();
        return;
    }
    let width = area.width;
    let HistoryLayout { lines, turn_starts } = compute_history(app, width);

    // Cache for key handlers. Computing `tail_top_offset` wrap-aware
    // — i.e. in post-wrap terminal rows — is what keeps long CJK
    // responses visible at the tail; otherwise the naive
    // `total_lines - area_height` formula under-counts rows and the
    // viewport anchors too far up.
    let tail_top = compute_tail_top_offset(&lines, area.height, width);
    app.scroll.area_height = area.height;
    app.scroll.total_lines = lines.len();
    app.scroll.tail_top_offset = tail_top;
    app.scroll.turn_starts = turn_starts;

    if app.scroll.follow_tail {
        app.scroll.top_offset = tail_top;
    } else {
        app.scroll.top_offset = app.scroll.top_offset.min(tail_top);
    }

    let visible = visible_slice(&lines, app.scroll.top_offset, area.height, width);
    Paragraph::new(visible)
        .wrap(Wrap { trim: false })
        .render(area, frame.buffer_mut());
}

/// Smallest top offset that still keeps the last logical line on screen
/// once wrapping is applied. Walks the lines from the tail and counts
/// wrapped rows; returns the first line index that no longer fits.
fn compute_tail_top_offset(lines: &[Line<'_>], area_height: u16, width: u16) -> usize {
    if lines.is_empty() || area_height == 0 {
        return 0;
    }
    let mut used: u32 = 0;
    let cap = area_height as u32;
    for (i, line) in lines.iter().enumerate().rev() {
        let h = wrapped_line_height(line, width) as u32;
        if used + h > cap {
            return i + 1;
        }
        used += h;
    }
    0
}

fn visible_slice(
    lines: &[Line<'static>],
    top_offset: usize,
    area_height: u16,
    width: u16,
) -> Vec<Line<'static>> {
    if lines.is_empty() || area_height == 0 {
        return Vec::new();
    }
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut used: u32 = 0;
    for line in lines.iter().skip(top_offset) {
        let h = wrapped_line_height(line, width) as u32;
        if used + h > area_height as u32 {
            break;
        }
        out.push(line.clone());
        used += h;
        if used >= area_height as u32 {
            break;
        }
    }
    out
}

fn wrapped_line_height(line: &Line, width: u16) -> u16 {
    if width == 0 {
        return 1;
    }
    let w = line.width() as u16;
    if w == 0 { 1 } else { w.div_ceil(width) }
}

fn render_block_into(
    lines: &mut Vec<Line<'static>>,
    block: &Block,
    width: u16,
    mode: Mode,
) {
    match block {
        Block::Greeting(g) => match mode {
            Mode::Overview => {
                let text = format!("{}  {} ({})", g.pod_name, g.model, g.provider);
                lines.push(Line::from(Span::styled(
                    text,
                    Style::default().fg(Color::Cyan),
                )));
            }
            _ => render_greeting(lines, g, width),
        },
        Block::TurnHeader { turn } => {
            lines.push(Line::from(Span::styled(
                format!("#{turn}"),
                kind_style(MessageKind::TurnHeader),
            )));
        }
        Block::UserMessage { text } => match mode {
            Mode::Overview => push_overview_line(lines, text, MessageKind::User, "> "),
            _ => push_padded_truncated(lines, text, MessageKind::User, mode),
        },
        Block::AssistantText { text } => match mode {
            Mode::Overview => push_overview_line(lines, text, MessageKind::Assistant, ""),
            _ => push_padded_truncated(lines, text, MessageKind::Assistant, mode),
        },
        // ToolCall is dispatched in `compute_history` via `tool::render_tool`
        // so it can consume multiple adjacent blocks (Read aggregation).
        Block::ToolCall(_) => unreachable!("ToolCall handled by compute_history"),
        Block::Notification {
            level,
            source,
            message,
        } => {
            let kind = match level {
                NotificationLevel::Warn => MessageKind::NoticeWarn,
                NotificationLevel::Error => MessageKind::NoticeError,
            };
            let prefix = match level {
                NotificationLevel::Warn => "[notice]",
                NotificationLevel::Error => "[notice error]",
            };
            let label = notification_source_label(*source);
            let text = format!("{prefix} {label}: {message}");
            match mode {
                Mode::Overview => push_overview_line(lines, &text, kind, ""),
                _ => push_padded_truncated(lines, &text, kind, mode),
            }
        }
        Block::Compact(evt) => render_compact(lines, evt, mode),
        Block::TurnStats {
            requests,
            input_tokens,
            output_tokens,
        } => {
            let text = format!(
                "{} reqs ↑{}/↓{}",
                requests,
                fmt_tokens(*input_tokens),
                fmt_tokens(*output_tokens),
            );
            lines.push(
                Line::from(Span::styled(text, kind_style(MessageKind::TurnStats)))
                    .alignment(ratatui::layout::Alignment::Right),
            );
        }
    }
}

fn push_padded_lines(lines: &mut Vec<Line<'static>>, text: &str, kind: MessageKind) {
    let style = kind_style(kind);
    for raw in text.lines() {
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(raw.to_owned(), style),
        ]));
    }
    if text.is_empty() {
        lines.push(Line::from(""));
    }
}

/// Normal / detail padded text: detail prints every line; normal caps at
/// `NORMAL_MAX_LINES` and appends a "+N more" footer.
fn push_padded_truncated(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    kind: MessageKind,
    mode: Mode,
) {
    if matches!(mode, Mode::Detail) {
        push_padded_lines(lines, text, kind);
        return;
    }
    let style = kind_style(kind);
    let all: Vec<&str> = text.lines().collect();
    let shown = all.len().min(NORMAL_MAX_LINES);
    for raw in &all[..shown] {
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled((*raw).to_owned(), style),
        ]));
    }
    if all.len() > shown {
        let hidden = all.len() - shown;
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                format!("… +{hidden} more lines"),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
    if text.is_empty() {
        lines.push(Line::from(""));
    }
}

/// Single-line summary for overview mode. First non-empty line of the
/// source text, with an optional prefix (e.g. "> " for user messages).
fn push_overview_line(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    kind: MessageKind,
    prefix: &str,
) {
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let more = text.lines().count().saturating_sub(1);
    let style = kind_style(kind);
    let mut spans = vec![Span::raw(" ")];
    if !prefix.is_empty() {
        spans.push(Span::styled(prefix.to_owned(), style));
    }
    spans.push(Span::styled(first.to_owned(), style));
    if more > 0 {
        spans.push(Span::styled(
            format!(" (+{more} lines)"),
            Style::default().fg(Color::DarkGray),
        ));
    }
    lines.push(Line::from(spans));
}

fn render_compact(lines: &mut Vec<Line<'static>>, evt: &CompactEvent, mode: Mode) {
    let (text, kind) = match evt {
        CompactEvent::Start => ("[compact] starting".to_owned(), MessageKind::NoticeWarn),
        CompactEvent::Done { new_session_id } => {
            let short = new_session_id.to_string().chars().take(8).collect::<String>();
            (
                format!("[compact] done (new session {short})"),
                MessageKind::NoticeWarn,
            )
        }
        CompactEvent::Failed { error } => (
            format!("[compact error] {error}"),
            MessageKind::NoticeError,
        ),
    };
    match mode {
        Mode::Overview => push_overview_line(lines, &text, kind, ""),
        _ => push_padded_lines(lines, &text, kind),
    }
}

fn draw_separator(frame: &mut Frame, area: Rect) {
    let line = "─".repeat(area.width as usize);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            line,
            Style::default().fg(Color::DarkGray),
        ))),
        area,
    );
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let conn = if app.connected {
        Span::styled("●", Style::default().fg(Color::Green))
    } else {
        Span::styled("○", Style::default().fg(Color::Red))
    };

    let mut spans = vec![
        conn,
        Span::raw(" "),
        Span::styled(
            app.pod_name.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ];

    if app.running {
        let status = if let Some(tool) = &app.current_tool {
            format!(
                "request: {} | ↑{}/↓{} | tool: {tool}",
                app.run_requests,
                fmt_tokens(app.run_input_tokens),
                fmt_tokens(app.run_output_tokens),
            )
        } else {
            format!(
                "request: {} | ↑{}/↓{}",
                app.run_requests,
                fmt_tokens(app.run_input_tokens),
                fmt_tokens(app.run_output_tokens),
            )
        };
        spans.push(Span::raw(" | "));
        spans.push(Span::styled(status, Style::default().fg(Color::Yellow)));
    } else if app.paused {
        spans.push(Span::raw(" | "));
        spans.push(Span::styled(
            "paused",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            " — Enter to resume, type to start new turn",
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        spans.push(Span::styled(" idle", Style::default().fg(Color::DarkGray)));
    }

    // Right-aligned mode / scroll indicator.
    let mut right: Vec<Span<'static>> = Vec::new();
    if !app.scroll.follow_tail {
        right.push(Span::styled(
            "↑ scrolled  ",
            Style::default().fg(Color::Yellow),
        ));
    }
    right.push(Span::styled(
        format!("[{}]", app.mode.label()),
        Style::default().fg(Color::DarkGray),
    ));
    let right_line = Line::from(right).alignment(ratatui::layout::Alignment::Right);

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
    frame.render_widget(Paragraph::new(right_line), area);
}

fn draw_input(frame: &mut Frame, render: &crate::input::InputRender, area: Rect) {
    // Prefix "> " on the first row, two-space gutter for continuation
    // rows so multi-line input aligns visually.
    let prompt_style = Style::default().fg(Color::DarkGray);
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(render.lines.len());
    for (i, src) in render.lines.iter().enumerate() {
        let prefix = if i == 0 { "> " } else { "  " };
        let mut spans = vec![Span::styled(prefix.to_owned(), prompt_style)];
        spans.extend(src.spans.iter().cloned());
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), area);

    let cursor_x = area.x + 2 + render.cursor_col;
    let cursor_y = area.y + render.cursor_row;
    if cursor_y < area.y + area.height {
        frame.set_cursor_position(Position::new(cursor_x, cursor_y));
    }
}

fn render_greeting(lines: &mut Vec<Line<'static>>, g: &Greeting, width: u16) {
    let inner = greeting_lines(g);
    let border_style = Style::default().fg(Color::DarkGray);

    // Render greeting into its own buffer so we can turn it into lines
    // for the outer history stream. Use a fixed width = area width.
    let box_width = width.min(80);
    let mut body_height: u16 = 0;
    let inner_width = box_width.saturating_sub(4);
    for l in &inner {
        let w = l.width() as u16;
        body_height += if inner_width == 0 || w == 0 {
            1
        } else {
            w.div_ceil(inner_width)
        };
    }
    let total_height = body_height + 2;
    let area = Rect::new(0, 0, box_width, total_height);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    Paragraph::new(inner)
        .block(
            UiBlock::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(border_style)
                .padding(Padding::horizontal(1)),
        )
        .wrap(Wrap { trim: false })
        .render(area, &mut buf);

    for y in 0..total_height {
        let mut spans: Vec<Span<'static>> = Vec::new();
        for x in 0..box_width {
            let cell = &buf[(x, y)];
            spans.push(Span::styled(cell.symbol().to_string(), cell.style()));
        }
        lines.push(Line::from(spans));
    }
}

fn greeting_lines(g: &Greeting) -> Vec<Line<'static>> {
    let label = Style::default().fg(Color::DarkGray);
    let value = Style::default().fg(Color::White);
    let mut lines: Vec<Line<'static>> = Vec::new();

    lines.push(Line::from(Span::styled(
        g.pod_name.clone(),
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        format!("{} ({})", g.model, g.provider),
        Style::default().fg(Color::Cyan),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("cwd:   ", label),
        Span::styled(g.cwd.clone(), value),
    ]));
    lines.push(Line::from(vec![
        Span::styled("tools: ", label),
        Span::styled(g.tools.join(", "), value),
    ]));

    if !g.scope_summary.is_empty() {
        lines.push(Line::from(""));
        for line in g.scope_summary.lines() {
            lines.push(Line::from(Span::styled(line.to_owned(), value)));
        }
    }

    lines
}

#[derive(Clone, Copy)]
pub enum MessageKind {
    TurnHeader,
    User,
    Assistant,
    TurnStats,
    NoticeWarn,
    NoticeError,
}

pub fn kind_style(kind: MessageKind) -> Style {
    match kind {
        MessageKind::TurnHeader => Style::default().fg(Color::DarkGray),
        MessageKind::User => Style::default().fg(Color::Green),
        MessageKind::Assistant => Style::default().fg(Color::White),
        MessageKind::TurnStats => Style::default().fg(Color::DarkGray),
        MessageKind::NoticeWarn => Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        MessageKind::NoticeError => Style::default()
            .fg(Color::White)
            .bg(Color::Red)
            .add_modifier(Modifier::BOLD),
    }
}

