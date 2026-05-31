//! Per-tool renderers.
//!
//! Each tool name has a custom renderer that converts a
//! [`ToolCallBlock`] into styled lines. Dispatch is by name; unknown
//! tools fall back to [`render_default`]. Some renderers (notably
//! `Read`) consume multiple consecutive blocks to produce a single
//! aggregate display.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::block::{Block, ToolCallBlock, ToolCallState};
use crate::cache::FileCache;
use crate::view_mode::Mode;

/// Maximum body lines in normal mode for tool output previews.
const NORMAL_MAX_BODY: usize = 5;
/// Width of the context window used by the Edit diff renderer.
const EDIT_DIFF_CONTEXT: usize = 3;

pub struct ToolRenderOutput {
    pub lines: Vec<Line<'static>>,
    /// How many blocks were consumed from `blocks[start..]`. Always >= 1.
    pub consumed: usize,
}

pub fn render_tool(
    cache: &FileCache,
    blocks: &[Block],
    start: usize,
    width: u16,
    mode: Mode,
) -> ToolRenderOutput {
    let Some(Block::ToolCall(tc)) = blocks.get(start) else {
        return ToolRenderOutput {
            lines: Vec::new(),
            consumed: 1,
        };
    };

    match tc.name.as_str() {
        "Read" => render_read_aggregate(blocks, start, mode),
        "Write" => single(render_write(cache, tc, mode)),
        "Edit" => single(render_edit(cache, tc, width, mode)),
        "Glob" => single(render_search(tc, mode, "Glob")),
        "Grep" => single(render_search(tc, mode, "Grep")),
        _ => single(render_default(tc, mode)),
    }
}

fn single(lines: Vec<Line<'static>>) -> ToolRenderOutput {
    ToolRenderOutput { lines, consumed: 1 }
}

// ---------------------------------------------------------------------
// Read (aggregating)
// ---------------------------------------------------------------------

fn render_read_aggregate(blocks: &[Block], start: usize, mode: Mode) -> ToolRenderOutput {
    let mut end = start + 1;
    while end < blocks.len() {
        match &blocks[end] {
            Block::ToolCall(t) if t.name == "Read" => end += 1,
            _ => break,
        }
    }

    let group: Vec<&ToolCallBlock> = blocks[start..end]
        .iter()
        .filter_map(|b| match b {
            Block::ToolCall(tc) => Some(tc),
            _ => None,
        })
        .collect();

    let in_progress = group.iter().any(|tc| {
        !matches!(
            tc.state,
            ToolCallState::Done { .. } | ToolCallState::Error { .. } | ToolCallState::Incomplete
        )
    });

    let paths: Vec<String> = group.iter().map(|tc| read_path(tc)).collect();
    let count = paths.len();

    let tool_style = Style::default().fg(Color::Cyan);
    let mut lines: Vec<Line<'static>> = Vec::new();

    let header = if in_progress {
        format!("Read — reading ({count} file{}…)", plural(count))
    } else {
        format!("Read — {count} file{} read", plural(count))
    };
    lines.push(Line::from(vec![Span::styled(header, tool_style)]));

    if matches!(mode, Mode::Overview) {
        return ToolRenderOutput {
            lines,
            consumed: end - start,
        };
    }

    // Sliding window of 3 most-recent files while in progress;
    // full list when finished.
    let path_style = Style::default().fg(Color::White);
    let limit = if in_progress { 3 } else { paths.len() };
    let start_idx = paths.len().saturating_sub(limit);
    for p in &paths[start_idx..] {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(p.clone(), path_style),
        ]));
    }
    if in_progress && paths.len() > limit {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("… ({} earlier)", paths.len() - limit),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }

    ToolRenderOutput {
        lines,
        consumed: end - start,
    }
}

fn read_path(tc: &ToolCallBlock) -> String {
    parsed_args(tc)
        .and_then(|v| v["file_path"].as_str().map(|s| s.to_owned()))
        .unwrap_or_else(|| "?".to_owned())
}

// ---------------------------------------------------------------------
// Write
// ---------------------------------------------------------------------

fn render_write(cache: &FileCache, tc: &ToolCallBlock, mode: Mode) -> Vec<Line<'static>> {
    let args = parsed_args(tc);
    let path = args
        .as_ref()
        .and_then(|v| v["file_path"].as_str().map(|s| s.to_owned()))
        .unwrap_or_else(|| "?".to_owned());
    let content_preview = args
        .as_ref()
        .and_then(|v| v["content"].as_str().map(|s| s.to_owned()))
        .unwrap_or_default();

    let action_is_overwrite = cache.get(&path).is_some();
    let label = if action_is_overwrite {
        "Overwrote"
    } else {
        "Created"
    };
    let label_style = if action_is_overwrite {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Green)
    };
    let tool_style = Style::default().fg(Color::Cyan);

    if matches!(mode, Mode::Overview) {
        return vec![Line::from(vec![
            Span::styled("Write — ".to_owned(), tool_style),
            Span::styled(format!("{label} "), label_style),
            Span::styled(path, Style::default().fg(Color::White)),
        ])];
    }

    let mut lines = vec![Line::from(vec![
        Span::styled("Write — ".to_owned(), tool_style),
        Span::styled(format!("{label} "), label_style),
        Span::styled(path.clone(), Style::default().fg(Color::White)),
        Span::styled(
            format!("  ({})", state_suffix(&tc.state)),
            Style::default().fg(Color::DarkGray),
        ),
    ])];

    // Body preview.
    let cap = match mode {
        Mode::Normal => NORMAL_MAX_BODY,
        Mode::Detail => usize::MAX,
        Mode::Overview => unreachable!(),
    };
    let body_lines: Vec<&str> = content_preview.lines().collect();
    let shown = body_lines.len().min(cap);
    let body_style = Style::default().fg(Color::Gray);
    for l in &body_lines[..shown] {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled((*l).to_owned(), body_style),
        ]));
    }
    if body_lines.len() > shown {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("… +{} more lines", body_lines.len() - shown),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }

    maybe_error_line(&mut lines, &tc.state);
    lines
}

// ---------------------------------------------------------------------
// Edit
// ---------------------------------------------------------------------

fn render_edit(
    cache: &FileCache,
    tc: &ToolCallBlock,
    width: u16,
    mode: Mode,
) -> Vec<Line<'static>> {
    let args = parsed_args(tc);
    let path = args
        .as_ref()
        .and_then(|v| v["file_path"].as_str().map(|s| s.to_owned()))
        .unwrap_or_else(|| "?".to_owned());
    let old = args
        .as_ref()
        .and_then(|v| v["old_string"].as_str().map(|s| s.to_owned()))
        .unwrap_or_default();
    let new = args
        .as_ref()
        .and_then(|v| v["new_string"].as_str().map(|s| s.to_owned()))
        .unwrap_or_default();

    let tool_style = Style::default().fg(Color::Cyan);
    let header = Line::from(vec![
        Span::styled(format!("Edit — {}", path), tool_style),
        Span::styled(
            format!("  ({})", state_suffix(&tc.state)),
            Style::default().fg(Color::DarkGray),
        ),
    ]);

    if matches!(mode, Mode::Overview) {
        return vec![header];
    }

    let mut lines = vec![header];

    // Prefer the snapshot captured right before the cache was rolled
    // forward (so we always diff against the true "before" even after
    // subsequent mutations). Fall back to the current cache for
    // replayed history where no snapshot was recorded.
    let before = tc.edit_snapshot.as_deref().or_else(|| cache.get(&path));
    if let Some(content) = before {
        for l in build_edit_diff(content, &old, &new, width) {
            lines.push(l);
        }
    } else {
        lines.push(Line::from(Span::styled(
            "  (no cached content — run Read first for a diff view)".to_owned(),
            Style::default().fg(Color::DarkGray),
        )));
    }

    maybe_error_line(&mut lines, &tc.state);
    lines
}

fn build_edit_diff(content: &str, old: &str, new: &str, width: u16) -> Vec<Line<'static>> {
    // Locate the first (and typically only) match.
    let Some(idx) = content.find(old) else {
        return vec![Line::from(Span::styled(
            "  (old_string not found in cached content)".to_owned(),
            Style::default().fg(Color::DarkGray),
        ))];
    };
    let end = idx + old.len();

    // Convert byte ranges to line ranges.
    //
    // Zero-based line index of `idx` = number of newlines preceding
    // it. `before.lines().count()` does NOT give this: for content
    // without a trailing newline it over-counts on the last line
    // (e.g. "h".lines() == ["h"].count() == 1 but "h" is still on
    // line 0).
    let all_lines: Vec<&str> = content.lines().collect();
    let line_of_idx = content[..idx].matches('\n').count().min(all_lines.len());
    let replaced_line_count = content[idx..end].lines().count().max(1);
    // Keep subsequent slice math in-bounds no matter what the
    // positions say — Edit never wants to panic the whole TUI just
    // because cached content and live args disagree.
    let replaced_end = (line_of_idx + replaced_line_count).min(all_lines.len());
    let ctx_start = line_of_idx.saturating_sub(EDIT_DIFF_CONTEXT);
    let ctx_end = (replaced_end + EDIT_DIFF_CONTEXT).min(all_lines.len());

    let old_line_count = old.lines().count().max(1);
    let new_line_count = new.lines().count().max(1);

    // Width for the line-number gutter: fit the largest number we'll
    // print across either file's version of this hunk.
    let max_line = ctx_end.max(line_of_idx + new_line_count).max(1);
    let num_w = max_line.to_string().len();

    // BG-highlighted rows for -/+ so the change stripe extends full
    // width; context lines stay plain. FG is left at terminal default
    // on colored rows so contrast survives user terminal themes.
    let ctx_text = Style::default().fg(Color::Gray);
    let num_style_ctx = Style::default().fg(Color::DarkGray);
    let num_style_dim = Style::default().fg(Color::White);
    // kanagawa-nvim palette:
    //   winterRed #43242B / winterGreen #2B3328 — diff row backgrounds
    //   autumnRed #C34043 / autumnGreen #76946A — git add/delete markers
    let minus_line_style = Style::default().bg(Color::Rgb(0x43, 0x24, 0x2B));
    let plus_line_style = Style::default().bg(Color::Rgb(0x2B, 0x33, 0x28));
    let minus_marker_style = Style::default().fg(Color::Rgb(0xC3, 0x40, 0x43));
    let plus_marker_style = Style::default().fg(Color::Rgb(0x76, 0x94, 0x6A));
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Layout per row (independent of outer history padding):
    //   1-space gutter + line number (right-aligned to num_w) + 1 space
    //   + 1-char marker (" " / "-" / "+") + content
    let num_gutter_w = num_w + 2; // leading sp + num + trailing sp
    let fmt_num = |n: usize| format!(" {n:>num_w$} ", n = n, num_w = num_w);
    let blank_num = " ".repeat(num_gutter_w);

    // Pre-context: 1-based line numbers in the original file.
    for (offset, l) in all_lines[ctx_start..line_of_idx].iter().enumerate() {
        let n = ctx_start + offset + 1;
        emit_diff_row(
            &mut lines,
            fmt_num(n),
            &blank_num,
            num_style_ctx,
            ' ',
            Style::default(),
            l,
            Style::default(), // no BG
            ctx_text,
            width,
        );
    }
    // Removed: old line numbers (line_of_idx + offset + 1). Row BG is
    // red; continuation rows keep the same BG via the wrap helper.
    for (offset, l) in old.lines().enumerate() {
        let n = line_of_idx + offset + 1;
        emit_diff_row(
            &mut lines,
            fmt_num(n),
            &blank_num,
            num_style_dim,
            '-',
            minus_marker_style,
            l,
            minus_line_style,
            Style::default(),
            width,
        );
    }
    // Added: new line numbers — same base as removed since everything
    // above the hunk is unchanged.
    for (offset, l) in new.lines().enumerate() {
        let n = line_of_idx + offset + 1;
        emit_diff_row(
            &mut lines,
            fmt_num(n),
            &blank_num,
            num_style_dim,
            '+',
            plus_marker_style,
            l,
            plus_line_style,
            Style::default(),
            width,
        );
    }
    // Post-context: positions relative to the ORIGINAL file so numbers
    // stay consistent with the snapshot we're diffing against. After a
    // successful edit the real file's numbering will shift by
    // `new_line_count - old_line_count`, but the purpose of this view
    // is to read the change itself, not to navigate the updated file.
    for (offset, l) in all_lines[replaced_end..ctx_end].iter().enumerate() {
        let n = replaced_end + offset + 1;
        emit_diff_row(
            &mut lines,
            fmt_num(n),
            &blank_num,
            num_style_ctx,
            ' ',
            Style::default(),
            l,
            Style::default(),
            ctx_text,
            width,
        );
    }

    // Suppress unused-variable lint for the value used only in
    // `max_line` computation (kept for readability there).
    let _ = old_line_count;

    lines
}

/// Emit one diff row, wrapping long content into continuation rows that
/// keep the vertical diff structure intact:
///
/// ```text
///   3 +very long line that overflows into a second terminal row
///     +of the same logical diff line
/// ```
///
/// `num_text` is the gutter text for the first row (e.g. " 3 "),
/// `blank_num` is the same width but all spaces for continuation rows.
/// `line_style` applies to every emitted row so BG-highlighted diffs
/// retain their highlight across the wrap.
#[allow(clippy::too_many_arguments)]
fn emit_diff_row(
    out: &mut Vec<Line<'static>>,
    num_text: String,
    blank_num: &str,
    num_style: Style,
    marker: char,
    marker_style: Style,
    content: &str,
    line_style: Style,
    content_text_style: Style,
    width: u16,
) {
    let total = width as usize;
    let gutter_w = UnicodeWidthStr::width(num_text.as_str());
    // Reserve one more col for the marker char; anything beyond goes
    // into the content.
    let content_budget = total.saturating_sub(gutter_w).saturating_sub(1);

    let mut is_first = true;
    let mut remaining = content;
    loop {
        let (chunk, rest) = if content_budget == 0 {
            (remaining, "")
        } else {
            split_at_width(remaining, content_budget)
        };

        let gutter = if is_first {
            num_text.clone()
        } else {
            blank_num.to_owned()
        };
        let content_span = if content_text_style == Style::default() {
            Span::raw(chunk.to_owned())
        } else {
            Span::styled(chunk.to_owned(), content_text_style)
        };
        let line = Line::from(vec![
            Span::styled(gutter, num_style),
            Span::styled(marker.to_string(), marker_style),
            content_span,
        ])
        .style(line_style);
        out.push(line);

        is_first = false;
        remaining = rest;
        if remaining.is_empty() {
            break;
        }
    }
}

/// Split `s` at the first char boundary whose accumulated display width
/// exceeds `max_cols`. Returns `(head, tail)` — head's display width is
/// guaranteed ≤ `max_cols`.
fn split_at_width(s: &str, max_cols: usize) -> (&str, &str) {
    if max_cols == 0 {
        return (s, "");
    }
    let mut col = 0usize;
    let mut end_byte = 0usize;
    for (i, c) in s.char_indices() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if col + cw > max_cols {
            return s.split_at(i);
        }
        col += cw;
        end_byte = i + c.len_utf8();
    }
    s.split_at(end_byte)
}

// ---------------------------------------------------------------------
// Glob / Grep
// ---------------------------------------------------------------------

fn render_search(tc: &ToolCallBlock, mode: Mode, label: &str) -> Vec<Line<'static>> {
    let tool_style = Style::default().fg(Color::Cyan);
    let summary_source: String = match &tc.state {
        ToolCallState::Done { summary, .. } | ToolCallState::Error { summary, .. } => {
            summary.clone()
        }
        _ => String::new(),
    };

    if matches!(mode, Mode::Overview) {
        let first = summary_source
            .lines()
            .next()
            .unwrap_or(state_suffix(&tc.state))
            .to_owned();
        return vec![Line::from(vec![
            Span::styled(format!("{label} — "), tool_style),
            Span::styled(first, Style::default().fg(Color::White)),
        ])];
    }

    let mut lines = vec![Line::from(vec![Span::styled(
        format!("{label} — {}", state_suffix(&tc.state)),
        tool_style,
    )])];

    let cap = match mode {
        Mode::Normal => NORMAL_MAX_BODY,
        Mode::Detail => usize::MAX,
        Mode::Overview => unreachable!(),
    };
    let body_lines: Vec<&str> = summary_source.lines().collect();
    let shown = body_lines.len().min(cap);
    let body_style = Style::default().fg(Color::Gray);
    for l in &body_lines[..shown] {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled((*l).to_owned(), body_style),
        ]));
    }
    if body_lines.len() > shown {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("… +{} more lines", body_lines.len() - shown),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }

    lines
}

// ---------------------------------------------------------------------
// Default (unknown tool)
// ---------------------------------------------------------------------

fn render_default(tc: &ToolCallBlock, mode: Mode) -> Vec<Line<'static>> {
    let tool_style = Style::default().fg(Color::Cyan);

    if matches!(mode, Mode::Overview) {
        let suffix = match &tc.state {
            ToolCallState::Done { summary, .. } | ToolCallState::Error { summary, .. } => {
                summary.lines().next().unwrap_or("").to_owned()
            }
            _ => state_suffix(&tc.state).to_owned(),
        };
        let label = if suffix.is_empty() {
            format!("{} — {}", tc.name, state_suffix(&tc.state))
        } else {
            format!("{} — {suffix}", tc.name)
        };
        return vec![Line::from(vec![Span::styled(label, tool_style)])];
    }

    let mut lines = vec![Line::from(vec![Span::styled(
        format!("{} — {}", tc.name, state_suffix(&tc.state)),
        tool_style,
    )])];

    let args_pretty = parsed_args(tc)
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| tc.args_stream.clone());
    let arg_cap = match mode {
        Mode::Normal => 3,
        Mode::Detail => usize::MAX,
        Mode::Overview => unreachable!(),
    };
    emit_capped_lines(
        &mut lines,
        &args_pretty,
        arg_cap,
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    );

    // Body source: prefer the full output (e.g. Bash's stdout/stderr) so
    // Detail mode can expose it. Fall back to the summary when the tool
    // didn't emit any content.
    let body_source: String = match &tc.state {
        ToolCallState::Done {
            output: Some(out), ..
        }
        | ToolCallState::Error {
            output: Some(out), ..
        } => out.clone(),
        ToolCallState::Done { summary, .. } | ToolCallState::Error { summary, .. } => {
            summary.clone()
        }
        _ => String::new(),
    };
    let body_cap = match mode {
        Mode::Normal => 3,
        Mode::Detail => usize::MAX,
        Mode::Overview => unreachable!(),
    };
    if !body_source.is_empty() {
        emit_capped_lines(
            &mut lines,
            &body_source,
            body_cap,
            Style::default().fg(Color::Gray),
        );
    }

    lines
}

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

fn parsed_args(tc: &ToolCallBlock) -> Option<serde_json::Value> {
    tc.arguments
        .as_ref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
}

fn state_suffix(state: &ToolCallState) -> &'static str {
    match state {
        ToolCallState::Pending => "pending",
        ToolCallState::Streaming => "streaming args",
        ToolCallState::Executing => "running",
        ToolCallState::Done { .. } => "done",
        ToolCallState::Error { .. } => "error",
        ToolCallState::Incomplete => "incomplete",
    }
}

fn maybe_error_line(lines: &mut Vec<Line<'static>>, state: &ToolCallState) {
    match state {
        ToolCallState::Error { summary, .. } => {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("error: {}", first_line(summary)),
                    Style::default().fg(Color::Red),
                ),
            ]));
        }
        ToolCallState::Incomplete => {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    "(no result before turn ended)".to_owned(),
                    Style::default().fg(Color::Red),
                ),
            ]));
        }
        _ => {}
    }
}

fn emit_capped_lines(out: &mut Vec<Line<'static>>, text: &str, cap: usize, style: Style) {
    let all: Vec<&str> = text.lines().collect();
    let shown = all.len().min(cap);
    for l in &all[..shown] {
        out.push(Line::from(vec![
            Span::raw("  "),
            Span::styled((*l).to_owned(), style),
        ]));
    }
    if all.len() > shown {
        out.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("… +{} more lines", all.len() - shown),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}
