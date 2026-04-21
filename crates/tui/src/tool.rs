//! Per-tool renderers.
//!
//! Each tool name has a custom renderer that converts a
//! [`ToolCallBlock`] into styled lines. Dispatch is by name; unknown
//! tools fall back to [`render_default`]. Some renderers (notably
//! `Read`) consume multiple consecutive blocks to produce a single
//! aggregate display.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::block::{Block, ToolCallBlock, ToolCallState};
use crate::cache::FileCache;
use crate::ui::Mode;

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
        "Edit" => single(render_edit(cache, tc, mode)),
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

    let in_progress = group
        .iter()
        .any(|tc| !matches!(tc.state, ToolCallState::Done { .. } | ToolCallState::Error { .. } | ToolCallState::Incomplete));

    let paths: Vec<String> = group.iter().map(|tc| read_path(tc)).collect();
    let count = paths.len();

    let tool_style = Style::default().fg(Color::Cyan);
    let mut lines: Vec<Line<'static>> = Vec::new();

    let header = if in_progress {
        format!("[tool] Read — reading ({count} file{}…)", plural(count))
    } else {
        format!("[tool] Read — {count} file{} read", plural(count))
    };
    lines.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(header, tool_style),
    ]));

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
            Span::raw("   "),
            Span::styled(p.clone(), path_style),
        ]));
    }
    if in_progress && paths.len() > limit {
        lines.push(Line::from(vec![
            Span::raw("   "),
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
            Span::raw(" "),
            Span::styled("[tool] Write — ".to_owned(), tool_style),
            Span::styled(format!("{label} "), label_style),
            Span::styled(path, Style::default().fg(Color::White)),
        ])];
    }

    let mut lines = vec![
        Line::from(vec![
            Span::raw(" "),
            Span::styled("[tool] Write — ".to_owned(), tool_style),
            Span::styled(format!("{label} "), label_style),
            Span::styled(path.clone(), Style::default().fg(Color::White)),
            Span::styled(
                format!("  ({})", state_suffix(&tc.state)),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
    ];

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
            Span::raw("   "),
            Span::styled((*l).to_owned(), body_style),
        ]));
    }
    if body_lines.len() > shown {
        lines.push(Line::from(vec![
            Span::raw("   "),
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

fn render_edit(cache: &FileCache, tc: &ToolCallBlock, mode: Mode) -> Vec<Line<'static>> {
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
        Span::raw(" "),
        Span::styled(format!("[tool] Edit — {}", path), tool_style),
        Span::styled(
            format!("  ({})", state_suffix(&tc.state)),
            Style::default().fg(Color::DarkGray),
        ),
    ]);

    if matches!(mode, Mode::Overview) {
        return vec![header];
    }

    let mut lines = vec![header];

    // Best-effort diff. Uses the cached content as the "before" snapshot
    // so what we show is consistent with the TUI's own state even if
    // the on-disk file has since diverged.
    let diff_lines = cache
        .get(&path)
        .map(|content| build_edit_diff(content, &old, &new));
    if let Some(diff) = diff_lines {
        for l in diff {
            lines.push(l);
        }
    } else {
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled(
                "(no cached content — run Read first for a diff view)".to_owned(),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }

    maybe_error_line(&mut lines, &tc.state);
    lines
}

fn build_edit_diff(content: &str, old: &str, new: &str) -> Vec<Line<'static>> {
    // Locate the first (and typically only) match.
    let Some(idx) = content.find(old) else {
        return vec![Line::from(vec![
            Span::raw("   "),
            Span::styled(
                "(old_string not found in cached content)".to_owned(),
                Style::default().fg(Color::DarkGray),
            ),
        ])];
    };
    let end = idx + old.len();

    // Convert byte ranges to line ranges.
    let before = &content[..idx];
    let line_of_idx = before.lines().count();
    // `lines()` omits a trailing empty line when the text ends in \n —
    // fine for our purposes (we only need approximate line ranges).
    let replaced_line_count = content[idx..end].lines().count().max(1);

    let all_lines: Vec<&str> = content.lines().collect();
    let ctx_start = line_of_idx.saturating_sub(EDIT_DIFF_CONTEXT);
    let ctx_end = (line_of_idx + replaced_line_count + EDIT_DIFF_CONTEXT).min(all_lines.len());

    let ctx_style = Style::default().fg(Color::Gray);
    let minus_style = Style::default().fg(Color::Red);
    let plus_style = Style::default().fg(Color::Green);
    let mut lines: Vec<Line<'static>> = Vec::new();

    for l in &all_lines[ctx_start..line_of_idx] {
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled(format!(" {l}"), ctx_style),
        ]));
    }
    for l in old.lines() {
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled(format!("-{l}"), minus_style),
        ]));
    }
    for l in new.lines() {
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled(format!("+{l}"), plus_style),
        ]));
    }
    for l in &all_lines[line_of_idx + replaced_line_count..ctx_end] {
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled(format!(" {l}"), ctx_style),
        ]));
    }

    lines
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
            Span::raw(" "),
            Span::styled(format!("[tool] {label} — "), tool_style),
            Span::styled(first, Style::default().fg(Color::White)),
        ])];
    }

    let mut lines = vec![Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!("[tool] {label} — {}", state_suffix(&tc.state)),
            tool_style,
        ),
    ])];

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
            Span::raw("   "),
            Span::styled((*l).to_owned(), body_style),
        ]));
    }
    if body_lines.len() > shown {
        lines.push(Line::from(vec![
            Span::raw("   "),
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
            format!("[tool] {} — {}", tc.name, state_suffix(&tc.state))
        } else {
            format!("[tool] {} — {suffix}", tc.name)
        };
        return vec![Line::from(vec![
            Span::raw(" "),
            Span::styled(label, tool_style),
        ])];
    }

    let mut lines = vec![Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!("[tool] {} — {}", tc.name, state_suffix(&tc.state)),
            tool_style,
        ),
    ])];

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
        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
    );

    let summary_source: String = match &tc.state {
        ToolCallState::Done { summary, .. } | ToolCallState::Error { summary, .. } => {
            summary.clone()
        }
        _ => String::new(),
    };
    let summary_cap = match mode {
        Mode::Normal => 3,
        Mode::Detail => usize::MAX,
        Mode::Overview => unreachable!(),
    };
    if !summary_source.is_empty() {
        emit_capped_lines(
            &mut lines,
            &summary_source,
            summary_cap,
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
                Span::raw("   "),
                Span::styled(
                    format!("error: {}", first_line(summary)),
                    Style::default().fg(Color::Red),
                ),
            ]));
        }
        ToolCallState::Incomplete => {
            lines.push(Line::from(vec![
                Span::raw("   "),
                Span::styled(
                    "(no result before turn ended)".to_owned(),
                    Style::default().fg(Color::Red),
                ),
            ]));
        }
        _ => {}
    }
}

fn emit_capped_lines(
    out: &mut Vec<Line<'static>>,
    text: &str,
    cap: usize,
    style: Style,
) {
    let all: Vec<&str> = text.lines().collect();
    let shown = all.len().min(cap);
    for l in &all[..shown] {
        out.push(Line::from(vec![
            Span::raw("   "),
            Span::styled((*l).to_owned(), style),
        ]));
    }
    if all.len() > shown {
        out.push(Line::from(vec![
            Span::raw("   "),
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
