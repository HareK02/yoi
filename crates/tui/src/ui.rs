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
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
    // Step 1: collect logical lines from each block (unwrapped).
    let mut logical: Vec<Line<'static>> = Vec::new();
    let mut logical_turn_starts: Vec<usize> = Vec::new();
    let mut first = true;
    let mut i = 0;
    while i < app.blocks.len() {
        if !first {
            logical.push(Line::from(""));
        }
        first = false;
        let block = &app.blocks[i];
        if matches!(block, Block::TurnHeader { .. }) {
            logical_turn_starts.push(logical.len());
        }
        if matches!(block, Block::ToolCall(_)) {
            let out = crate::tool::render_tool(&app.cache, &app.blocks, i, width, app.mode);
            logical.extend(out.lines);
            i += out.consumed.max(1);
            continue;
        }
        render_block_into(&mut logical, block, width, app.mode);
        i += 1;
    }

    // Step 2: pre-wrap every logical line to char-based terminal rows so
    // scroll math is exact. Track the logical → wrapped mapping so
    // turn-start indices get translated into wrapped-row coordinates.
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(logical.len());
    let mut logical_to_wrapped: Vec<usize> = Vec::with_capacity(logical.len() + 1);
    for line in logical {
        logical_to_wrapped.push(lines.len());
        wrap_line_into(line, width, &mut lines);
    }
    logical_to_wrapped.push(lines.len());

    let turn_starts = logical_turn_starts
        .into_iter()
        .map(|i| logical_to_wrapped.get(i).copied().unwrap_or(lines.len()))
        .collect();

    HistoryLayout { lines, turn_starts }
}

/// Horizontal gutter around the log area. Applied via a
/// [`Block`](ratatui::widgets::Block)'s padding so *every* row — including
/// continuation rows from wrapping — sits inside the same margin, no
/// leading-space hacks in the content itself.
const HISTORY_PADDING: u16 = 1;

fn draw_history(frame: &mut Frame, app: &mut App, area: Rect) {
    if area.height == 0 || area.width == 0 {
        app.scroll.area_height = area.height;
        app.scroll.total_lines = 0;
        app.scroll.tail_top_offset = 0;
        app.scroll.turn_starts.clear();
        return;
    }
    let outer_block = UiBlock::default().padding(Padding::horizontal(HISTORY_PADDING));
    let inner = outer_block.inner(area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let HistoryLayout { lines, turn_starts } = compute_history(app, inner.width);

    // `lines` is already pre-wrapped: 1 entry == 1 terminal row. Scroll
    // math degenerates to index arithmetic.
    let tail_top = lines.len().saturating_sub(inner.height as usize);
    app.scroll.area_height = inner.height;
    app.scroll.total_lines = lines.len();
    app.scroll.tail_top_offset = tail_top;
    app.scroll.turn_starts = turn_starts;

    if app.scroll.follow_tail {
        app.scroll.top_offset = tail_top;
    } else {
        app.scroll.top_offset = app.scroll.top_offset.min(tail_top);
    }

    let end = (app.scroll.top_offset + inner.height as usize).min(lines.len());
    let visible: Vec<Line<'static>> = lines[app.scroll.top_offset..end].to_vec();

    // Pre-wrapped input → render without ratatui's word-wrap (which
    // would otherwise re-wrap mid-row at word boundaries and desync the
    // height count). The outer Block handles left/right padding
    // uniformly for all rows.
    Paragraph::new(visible)
        .block(outer_block)
        .render(area, frame.buffer_mut());
}

/// Split one logical line into one-terminal-row `Line`s via char-aware
/// wrapping. Preserves per-span styles, the source line's alignment,
/// and the source line's own style. If the source line carries a
/// background color, each emitted row is padded to `width` with spaces
/// styled by that line style — so diff-style row highlights (red/green
/// backgrounds) extend cleanly to the right edge of the terminal.
fn wrap_line_into(line: Line<'static>, width: u16, out: &mut Vec<Line<'static>>) {
    if width == 0 {
        out.push(line);
        return;
    }
    let w = width as usize;
    let alignment = line.alignment;
    let line_style = line.style;
    let fill_to_width = line_style.bg.is_some();

    let mut current: Vec<Span<'static>> = Vec::new();
    let mut row_width: usize = 0;
    let mut pending = String::new();
    let mut pending_width: usize = 0;
    let mut pending_style = Style::default();

    let commit_pending = |pending: &mut String,
                          pending_width: &mut usize,
                          pending_style: Style,
                          current: &mut Vec<Span<'static>>,
                          row_width: &mut usize| {
        if pending.is_empty() {
            return;
        }
        current.push(Span::styled(std::mem::take(pending), pending_style));
        *row_width += *pending_width;
        *pending_width = 0;
    };

    let push_row = |current: &mut Vec<Span<'static>>,
                    row_width: &mut usize,
                    out: &mut Vec<Line<'static>>| {
        if fill_to_width && *row_width < w {
            let pad = w - *row_width;
            current.push(Span::styled(" ".repeat(pad), line_style));
            *row_width = w;
        }
        let mut l = Line::from(std::mem::take(current)).style(line_style);
        if let Some(a) = alignment {
            l = l.alignment(a);
        }
        out.push(l);
        *row_width = 0;
    };

    for span in line.spans {
        if !pending.is_empty() && span.style != pending_style {
            commit_pending(
                &mut pending,
                &mut pending_width,
                pending_style,
                &mut current,
                &mut row_width,
            );
        }
        pending_style = span.style;

        for c in span.content.chars() {
            let cw = UnicodeWidthChar::width(c).unwrap_or(0);
            if row_width + pending_width + cw > w && (row_width + pending_width) > 0 {
                commit_pending(
                    &mut pending,
                    &mut pending_width,
                    pending_style,
                    &mut current,
                    &mut row_width,
                );
                push_row(&mut current, &mut row_width, out);
            }
            pending.push(c);
            pending_width += cw;
        }

        commit_pending(
            &mut pending,
            &mut pending_width,
            pending_style,
            &mut current,
            &mut row_width,
        );
    }

    // Always emit the final row (empty line stays empty).
    push_row(&mut current, &mut row_width, out);
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
            Mode::Overview => push_overview_line(lines, text, width, MessageKind::User, "> "),
            // User input and assistant prose are the primary readable
            // content of a turn — never compressed in detail / normal.
            // Only `overview` folds them to a single line.
            _ => push_padded_lines(lines, text, MessageKind::User),
        },
        Block::AssistantText { text } => match mode {
            Mode::Overview => push_overview_line(lines, text, width, MessageKind::Assistant, ""),
            _ => push_padded_lines(lines, text, MessageKind::Assistant),
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
                Mode::Overview => push_overview_line(lines, &text, width, kind, ""),
                _ => push_padded_lines(lines, &text, kind),
            }
        }
        Block::Compact(evt) => render_compact(lines, evt, width, mode),
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
        lines.push(Line::from(Span::styled(raw.to_owned(), style)));
    }
    if text.is_empty() {
        lines.push(Line::from(""));
    }
}

/// Single-line summary for overview mode. The output is clipped to
/// exactly one rendered terminal row at `width` columns — the first
/// non-empty logical line is truncated (with `…`) to fit alongside an
/// optional prefix and a `(+N lines)` tail that counts visual rows
/// hidden by the fold (auto-wrap inclusive, so a single long paragraph
/// that wraps to many rows correctly reports the hidden rows).
fn push_overview_line(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    width: u16,
    kind: MessageKind,
    prefix: &str,
) {
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let total_visual = count_visual_rows(text, width);
    let first_visual = count_visual_rows(first, width);

    // Budget for the first line's truncated content. Reserve columns
    // for the prefix up front and for the "(+N lines)" tail if we'll
    // emit one. `more` counts visual rows hidden by the fold; the one
    // row we're about to render is subtracted.
    let total_cols = width.max(1) as usize;
    let prefix_width = UnicodeWidthStr::width(prefix);
    let more = total_visual.saturating_sub(1);
    let tentative_tail = if more > 0 {
        format!(" (+{more} lines)")
    } else {
        String::new()
    };
    let avail = total_cols.saturating_sub(prefix_width);
    // On very narrow widths where the tail alone would eat the row,
    // drop it — keeping at least some of the actual content visible
    // matters more than the hidden-rows label.
    let (tail, budget) = {
        let tw = UnicodeWidthStr::width(tentative_tail.as_str());
        if !tentative_tail.is_empty() && tw >= avail {
            (String::new(), avail)
        } else {
            (tentative_tail, avail.saturating_sub(tw))
        }
    };
    let first_width = UnicodeWidthStr::width(first);
    let needs_truncation = first_width > budget || first_visual > 1;
    let shown = if needs_truncation {
        truncate_with_ellipsis(first, budget)
    } else {
        first.to_owned()
    };

    let style = kind_style(kind);
    let mut spans: Vec<Span<'static>> = Vec::new();
    if !prefix.is_empty() {
        spans.push(Span::styled(prefix.to_owned(), style));
    }
    spans.push(Span::styled(shown, style));
    if !tail.is_empty() {
        spans.push(Span::styled(tail, Style::default().fg(Color::DarkGray)));
    }
    lines.push(Line::from(spans));
}

/// Truncate `s` so its display width fits within `max_width`, appending
/// `…` when a cut is actually applied. A budget of 0 yields an empty
/// string; a budget of 1 drops the ellipsis and returns at most one
/// column of content so we never blow past the cap.
fn truncate_with_ellipsis(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let s_width = UnicodeWidthStr::width(s);
    if s_width <= max_width {
        return s.to_owned();
    }
    // Reserve 1 col for the ellipsis when we have room; on a 1-col
    // budget there's no room for both content and the marker, so just
    // clip to whatever single column fits.
    let (inner_budget, append_ellipsis) = if max_width >= 2 {
        (max_width - 1, true)
    } else {
        (max_width, false)
    };
    let mut out = String::new();
    let mut w = 0usize;
    for c in s.chars() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw > inner_budget {
            break;
        }
        out.push(c);
        w += cw;
    }
    if append_ellipsis {
        out.push('…');
    }
    out
}

/// Visual row count of `text` when rendered at `width` columns with
/// simple char-based wrap. Each empty logical line counts as 1 row.
fn count_visual_rows(text: &str, width: u16) -> usize {
    if text.is_empty() {
        return 0;
    }
    let w = width.max(1) as usize;
    let mut total = 0usize;
    for line in text.lines() {
        let lw = UnicodeWidthStr::width(line);
        total += if lw == 0 { 1 } else { lw.div_ceil(w) };
    }
    total.max(1)
}

fn render_compact(lines: &mut Vec<Line<'static>>, evt: &CompactEvent, width: u16, mode: Mode) {
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
        Mode::Overview => push_overview_line(lines, &text, width, kind, ""),
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

