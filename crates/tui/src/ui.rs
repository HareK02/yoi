//! Full-screen rendering for the TUI.
//!
//! The layout is stacked top-to-bottom:
//!
//! ```text
//!   history view (fills remaining space)
//!   ──────────── separator ──────────
//!   status line (1 row)
//!   > input area (1 row in Phase 1)
//!   actionbar (1 row)
//! ```
//!
//! Every frame we walk the entire `App::blocks` vector, produce styled
//! lines, and render the tail that fits the history area. No
//! `insert_before` use — the terminal scrollback stays untouched.

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block as UiBlock, BorderType, Borders, Clear, Padding, Paragraph, Widget, Wrap,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use protocol::{AlertLevel, CompletionEntry, Greeting, PodEvent, Segment};

use crate::app::{ActionbarNoticeLevel, App, CompletionState, alert_source_label, fmt_tokens};
use crate::block::{Block, CompactEvent, ThinkingBlock, ThinkingState};
use crate::command::CommandCandidate;
use crate::task::{TaskCounts, TaskEntry, TaskStatus, TaskStore};
use crate::text_selection::{HistoryViewport, SelectionRow};
use crate::view_mode::Mode;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    // Input content starts after the prompt (`> ` or `: `), so the width
    // available for wrapping is two columns narrower than the frame.
    let input_content_width = area.width.saturating_sub(2).max(1);
    let mut input_render = if app.is_command_mode() {
        app.command_input.render(input_content_width)
    } else {
        app.input.render(input_content_width)
    };
    let input_height = input_area_height(&input_render, area.height);
    if app.is_command_mode() {
        app.command_input
            .apply_cursor_viewport(&mut input_render, input_height);
    } else {
        app.input
            .apply_cursor_viewport(&mut input_render, input_height);
    }
    let mini_view_h = task_mini_view_height(&app.task_store);
    // One blank row separates the history tail from the mini-view so
    // the latest message doesn't visually crash into the task summary.
    // Folds away with the mini-view when there are no tasks.
    let mini_view_gap = if mini_view_h > 0 { 1 } else { 0 };

    let chunks = Layout::vertical([
        Constraint::Min(0),                // history view
        Constraint::Length(mini_view_gap), // gap above mini-view
        Constraint::Length(mini_view_h),   // task mini-view (0 when empty)
        Constraint::Length(1),             // separator
        Constraint::Length(1),             // status
        Constraint::Length(input_height),  // input area
        Constraint::Length(1),             // actionbar
    ])
    .split(area);

    draw_history(frame, app, chunks[0]);
    if mini_view_h > 0 {
        draw_task_mini_view(frame, &app.task_store, chunks[2]);
    }
    draw_separator(frame, chunks[3]);
    draw_status(frame, app, chunks[4]);
    draw_input(frame, app, &input_render, chunks[5]);
    draw_actionbar(frame, app, chunks[6]);
    if app.is_command_mode() {
        draw_command_popup(frame, app, chunks[5]);
    } else if let Some(state) = app.completion.as_ref().filter(|c| c.is_active()) {
        draw_completion_popup(frame, state, chunks[5]);
    }
}

/// Maximum number of active (pending / inprogress) tasks the mini-view
/// shows above the summary line. Exceeding tasks are still counted in
/// the summary.
const MINI_VIEW_MAX_ACTIVE: usize = 3;

/// Height the mini-view section occupies. Returns 0 when there are no
/// tasks at all, so the section collapses cleanly into surrounding
/// layout — there's no point reserving rows for an empty store.
fn task_mini_view_height(store: &TaskStore) -> u16 {
    if store.is_empty() {
        return 0;
    }
    let active_shown = store.counts().active().min(MINI_VIEW_MAX_ACTIVE);
    // active rows + 1 summary line
    (active_shown as u16).saturating_add(1)
}

fn draw_task_mini_view(frame: &mut Frame, store: &TaskStore, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let outer_block = UiBlock::default().padding(Padding::horizontal(HISTORY_PADDING));
    let inner = outer_block.inner(area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(area.height as usize);
    let mut shown = 0usize;
    for entry in store.tasks() {
        if shown >= MINI_VIEW_MAX_ACTIVE {
            break;
        }
        if !matches!(entry.status, TaskStatus::Pending | TaskStatus::Inprogress) {
            continue;
        }
        lines.push(mini_view_active_line(entry, inner.width));
        shown += 1;
    }
    lines.push(mini_view_summary_line(store.counts(), inner.width));

    Paragraph::new(lines)
        .block(outer_block)
        .render(area, frame.buffer_mut());
}

fn mini_view_active_line(entry: &TaskEntry, width: u16) -> Line<'static> {
    let mark = task_status_mark(entry.status);
    // Subject's first line only; embedded newlines would otherwise
    // wreck the one-row-per-task layout.
    let subject = entry.subject.lines().next().unwrap_or("");
    let mark_width = UnicodeWidthStr::width(mark.0);
    // Reserve mark + space.
    let budget = (width as usize).saturating_sub(mark_width + 1);
    let shown = truncate_with_ellipsis(subject, budget);
    Line::from(vec![
        Span::styled(mark.0.to_owned(), mark.1),
        Span::raw(" "),
        Span::raw(shown),
    ])
}

fn mini_view_summary_line(counts: TaskCounts, width: u16) -> Line<'static> {
    let text = format!(
        "{} task(s) — pending: {}, inprogress: {}, completed: {}, deleted: {}",
        counts.total(),
        counts.pending,
        counts.inprogress,
        counts.completed,
        counts.deleted,
    );
    let shown = truncate_with_ellipsis(&text, width as usize);
    Line::from(Span::styled(shown, Style::default().fg(Color::DarkGray)))
}

/// Two-character status marker + the style to render it with. Mirrors
/// the four `TaskStatus` values; deleted ones never appear in the
/// mini-view but are listed in the side pane.
fn task_status_mark(status: TaskStatus) -> (&'static str, Style) {
    match status {
        TaskStatus::Pending => ("[ ]", Style::default().fg(Color::DarkGray)),
        TaskStatus::Inprogress => (
            "[~]",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        TaskStatus::Completed => ("[x]", Style::default().fg(Color::Green)),
        TaskStatus::Deleted => ("[-]", Style::default().fg(Color::Red)),
    }
}

/// Render the candidate list directly above the input area. The popup
/// overlays the status row (and history's bottom rows when it grows
/// taller than that single row); `Clear` blanks the cells first so
/// underlying text doesn't bleed through. The popup width matches the
/// widest visible label, capped at the input-area width.
fn draw_completion_popup(frame: &mut Frame, state: &CompletionState, input_area: Rect) {
    let entries = &state.entries;
    if entries.is_empty() || input_area.y == 0 {
        return;
    }
    let visible = entries.len().min(CompletionState::MAX_VISIBLE);
    // Scroll window keeps the selected item in view.
    let view_start = if state.selected + 1 <= visible {
        0
    } else {
        state.selected + 1 - visible
    };
    let view_end = (view_start + visible).min(entries.len());

    let label_for = |entry: &CompletionEntry| {
        let mut s = entry.value.clone();
        if entry.is_dir {
            s.push('/');
        }
        s
    };
    let max_label = entries[view_start..view_end]
        .iter()
        .map(|e| label_for(e).chars().count() as u16)
        .max()
        .unwrap_or(0);
    let popup_w = max_label.saturating_add(2).min(input_area.width).max(1);
    let popup_h = (visible as u16).min(input_area.y);
    let popup_area = Rect::new(
        input_area.x,
        input_area.y.saturating_sub(popup_h),
        popup_w,
        popup_h,
    );

    let highlight = Style::default()
        .bg(Color::DarkGray)
        .add_modifier(Modifier::BOLD);
    let dir_style = Style::default().fg(Color::Cyan);
    let plain = Style::default();

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(popup_h as usize);
    for (i, entry) in entries[view_start..view_end].iter().enumerate() {
        let abs = view_start + i;
        let text = label_for(entry);
        let base = if entry.is_dir { dir_style } else { plain };
        let style = if abs == state.selected {
            highlight.patch(base)
        } else {
            base
        };
        lines.push(Line::from(Span::styled(text, style)));
    }
    frame.render_widget(Clear, popup_area);
    frame.render_widget(Paragraph::new(lines), popup_area);
}

fn draw_command_popup(frame: &mut Frame, app: &App, input_area: Rect) {
    let suggestions = app.command_suggestions();
    if suggestions.is_empty() || input_area.y == 0 {
        return;
    }

    let visible = suggestions.len().min(CompletionState::MAX_VISIBLE);
    let visible_suggestions = &suggestions[..visible];
    let max_label = visible_suggestions
        .iter()
        .map(|candidate| command_suggestion_label(candidate).width() as u16)
        .max()
        .unwrap_or(0);
    let popup_w = max_label.saturating_add(2).min(input_area.width).max(1);
    let popup_h = (visible as u16).min(input_area.y);
    let popup_area = Rect::new(
        input_area.x,
        input_area.y.saturating_sub(popup_h),
        popup_w,
        popup_h,
    );

    let command_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let description_style = Style::default().fg(Color::DarkGray);
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(popup_h as usize);
    let selected = app.command_completion_selected();
    for (idx, candidate) in visible_suggestions
        .iter()
        .take(popup_h as usize)
        .enumerate()
    {
        let selected_style = if Some(idx) == selected {
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            Span::styled(
                candidate.name.to_owned(),
                command_style.patch(selected_style),
            ),
            Span::styled(" — ", description_style.patch(selected_style)),
            Span::styled(
                candidate.description.to_owned(),
                description_style.patch(selected_style),
            ),
        ]));
    }

    frame.render_widget(Clear, popup_area);
    frame.render_widget(Paragraph::new(lines), popup_area);
}

fn command_suggestion_label(candidate: &CommandCandidate) -> String {
    format!("{} — {}", candidate.name, candidate.description)
}

/// Cap the input area so it doesn't eat the history view: grows with the
/// buffer but never past `min(10, terminal_height / 3)`.
fn input_area_height(render: &crate::input::InputRender, terminal_height: u16) -> u16 {
    let needed = render.lines.len().max(1) as u16;
    let cap = (terminal_height / 3).max(1).min(10);
    needed.clamp(1, cap)
}

pub struct HistoryRow {
    pub line: Line<'static>,
    pub text: String,
    pub selectable: bool,
}

impl HistoryRow {
    fn new(line: Line<'static>, selectable: bool) -> Self {
        let text = line_text(&line);
        Self {
            line,
            text,
            selectable,
        }
    }

    fn selection_row(&self) -> SelectionRow {
        SelectionRow::new(self.text.clone(), self.selectable)
    }
}

/// Pre-rendered history lines plus the line indices at which each turn
/// begins (used for Ctrl-[/] jumps).
pub struct HistoryLayout {
    pub rows: Vec<HistoryRow>,
    pub turn_starts: Vec<usize>,
}

pub fn compute_history(app: &App, width: u16) -> HistoryLayout {
    // Step 1: collect logical lines from each block (unwrapped).
    let mut logical: Vec<(Line<'static>, bool)> = Vec::new();
    let mut logical_turn_starts: Vec<usize> = Vec::new();
    let mut first = true;
    let mut previous_selectable = false;
    let mut i = 0;
    while i < app.blocks.len() {
        let block = &app.blocks[i];
        let current_selectable = block_is_selectable_text(block);
        if !first {
            // Preserve a deterministic blank-line separator when copying
            // across text-like items. Tool/non-text item rows remain
            // unselectable, but separators adjacent to text are copied as
            // blank lines so cross-item extraction is stable.
            logical.push((Line::from(""), previous_selectable || current_selectable));
        }
        first = false;
        if matches!(block, Block::TurnHeader { .. }) {
            logical_turn_starts.push(logical.len());
        }
        if matches!(block, Block::ToolCall(_)) {
            let out = crate::tool::render_tool(&app.cache, &app.blocks, i, width, app.mode);
            logical.extend(out.lines.into_iter().map(|line| (line, false)));
            i += out.consumed.max(1);
            previous_selectable = false;
            continue;
        }
        let mut block_lines = Vec::new();
        render_block_into(&mut block_lines, block, width, app.mode);
        logical.extend(
            block_lines
                .into_iter()
                .map(|line| (line, current_selectable)),
        );
        previous_selectable = current_selectable;
        i += 1;
    }

    // Step 2: pre-wrap every logical line to char-based terminal rows so
    // scroll math is exact. Track the logical → wrapped mapping so
    // turn-start indices get translated into wrapped-row coordinates.
    let mut rows: Vec<HistoryRow> = Vec::with_capacity(logical.len());
    let mut logical_to_wrapped: Vec<usize> = Vec::with_capacity(logical.len() + 1);
    for (line, selectable) in logical {
        logical_to_wrapped.push(rows.len());
        wrap_history_row_into(line, selectable, width, &mut rows);
    }
    logical_to_wrapped.push(rows.len());

    let turn_starts = logical_turn_starts
        .into_iter()
        .map(|i| logical_to_wrapped.get(i).copied().unwrap_or(rows.len()))
        .collect();

    HistoryLayout { rows, turn_starts }
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
        app.text_selection.clear_history_snapshot();
        return;
    }

    // When the task pane is open and the area is wide enough, carve a
    // vertical strip on the right for it. Side pane lives inside the
    // history rect only — separator / status / input stay full width to
    // keep the input experience and completion popup geometry intact.
    let pane_w = task_side_pane_width(area.width, app.task_pane_open);
    let history_area = if pane_w > 0 {
        let split =
            Layout::horizontal([Constraint::Min(1), Constraint::Length(pane_w)]).split(area);
        draw_task_side_pane(frame, app, split[1]);
        split[0]
    } else {
        area
    };

    let outer_block = UiBlock::default().padding(Padding::horizontal(HISTORY_PADDING));
    let inner = outer_block.inner(history_area);
    if inner.width == 0 || inner.height == 0 {
        app.text_selection.clear_history_snapshot();
        return;
    }

    if let Some(picker) = app.rewind_picker.as_mut() {
        app.text_selection.clear_history_snapshot();
        draw_rewind_picker(frame, history_area, inner, outer_block, picker);
        return;
    }

    let HistoryLayout { rows, turn_starts } = compute_history(app, inner.width);

    // `rows` is already pre-wrapped: 1 entry == 1 terminal row. Scroll
    // math degenerates to index arithmetic.
    let tail_top = rows.len().saturating_sub(inner.height as usize);
    app.scroll.area_height = inner.height;
    app.scroll.total_lines = rows.len();
    app.scroll.tail_top_offset = tail_top;
    app.scroll.turn_starts = turn_starts;

    if app.scroll.follow_tail {
        app.scroll.top_offset = tail_top;
    } else {
        app.scroll.top_offset = app.scroll.top_offset.min(tail_top);
    }

    app.text_selection.set_history_snapshot(
        HistoryViewport {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: inner.height,
            top_offset: app.scroll.top_offset,
            total_lines: rows.len(),
        },
        rows.iter().map(HistoryRow::selection_row).collect(),
    );

    let end = (app.scroll.top_offset + inner.height as usize).min(rows.len());
    let visible: Vec<Line<'static>> = rows[app.scroll.top_offset..end]
        .iter()
        .enumerate()
        .map(|(offset, row)| {
            let absolute_row = app.scroll.top_offset + offset;
            match app.text_selection.range_for_row(absolute_row) {
                Some((from, to)) => highlight_line_selection(&row.line, from, to),
                None => row.line.clone(),
            }
        })
        .collect();

    // Pre-wrapped input → render without ratatui's word-wrap (which
    // would otherwise re-wrap mid-row at word boundaries and desync the
    // height count). The outer Block handles left/right padding
    // uniformly for all rows.
    Paragraph::new(visible)
        .block(outer_block)
        .render(history_area, frame.buffer_mut());
}

fn draw_rewind_picker(
    frame: &mut Frame,
    history_area: Rect,
    inner: Rect,
    outer_block: UiBlock<'_>,
    picker: &mut crate::app::RewindPickerState,
) {
    let mut logical: Vec<Line<'static>> = Vec::new();
    let action_spans = if picker.applying {
        vec![
            Span::styled(
                "Applying rewind…",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" waiting for Pod response"),
        ]
    } else {
        vec![
            Span::styled("Enter", Style::default().fg(Color::Green)),
            Span::raw(" apply  "),
            Span::styled("Esc", Style::default().fg(Color::Green)),
            Span::raw(" cancel"),
        ]
    };
    let mut header = vec![
        Span::styled(
            "Rewind targets",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("  head={} ", picker.head_entries)),
    ];
    header.extend(action_spans);
    logical.push(Line::from(header));
    logical.push(Line::from(Span::styled(
        "Selecting a target discards the later history suffix; tool side effects are not undone.",
        Style::default().fg(Color::DarkGray),
    )));
    logical.push(Line::from(""));

    if picker.targets.is_empty() {
        logical.push(Line::from(Span::styled(
            "No previous user messages are available to rewind.",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for (idx, target) in picker.targets.iter().enumerate() {
            let selected = idx == picker.selected;
            let marker = if selected { "▶" } else { " " };
            let base_style = if selected {
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else if target.eligible {
                Style::default()
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let ts = target
                .timestamp_ms
                .map(|ts| format!("{}", ts))
                .unwrap_or_else(|| "-".into());
            logical.push(Line::from(vec![
                Span::styled(marker.to_owned(), base_style),
                Span::styled(
                    format!(
                        " turn {}  idx {}  ts {}  ",
                        target.turn_index, target.id.user_input_entry_index, ts
                    ),
                    base_style,
                ),
                Span::styled(target.preview.clone(), base_style),
            ]));
            if let Some(warning) = target.warning.as_ref() {
                logical.push(Line::from(Span::styled(
                    format!("  warning: {warning}"),
                    Style::default().fg(Color::Yellow),
                )));
            }
            if let Some(reason) = target.disabled_reason.as_ref() {
                logical.push(Line::from(Span::styled(
                    format!("  disabled: {reason}"),
                    Style::default().fg(Color::Red),
                )));
            }
        }
    }

    let mut lines = Vec::new();
    for line in logical {
        wrap_line_into(line, inner.width, &mut lines);
    }

    let tail_top = lines.len().saturating_sub(inner.height as usize);
    picker.scroll.area_height = inner.height;
    picker.scroll.total_lines = lines.len();
    picker.scroll.tail_top_offset = tail_top;
    picker.scroll.top_offset = picker.scroll.top_offset.min(tail_top);

    let end = (picker.scroll.top_offset + inner.height as usize).min(lines.len());
    let visible = lines[picker.scroll.top_offset..end].to_vec();
    Paragraph::new(visible)
        .block(outer_block)
        .render(history_area, frame.buffer_mut());
}

/// Width to reserve for the task side pane within the history rect.
/// Returns 0 when the pane is closed or the rect is too narrow to host
/// it without crushing the history view.
fn task_side_pane_width(area_width: u16, open: bool) -> u16 {
    if !open {
        return 0;
    }
    // Need a reasonable history column on the left, and enough room on
    // the right for taskid + status mark + a few words of subject. Skip
    // entirely on narrow terminals.
    if area_width < 60 {
        return 0;
    }
    (area_width / 3).clamp(28, 44)
}

fn draw_task_side_pane(frame: &mut Frame, app: &mut App, area: Rect) {
    if area.width < 4 || area.height < 1 {
        return;
    }
    let pane_block = UiBlock::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(Color::DarkGray))
        .padding(Padding::horizontal(1));
    let inner = pane_block.inner(area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let store = &app.task_store;
    let counts = store.counts();
    let title_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let body_style = Style::default().fg(Color::DarkGray);
    let muted_style = Style::default().fg(Color::DarkGray);

    let mut logical: Vec<Line<'static>> = Vec::new();
    logical.push(Line::from(Span::styled(
        format!("Tasks ({})", counts.total()),
        title_style,
    )));
    logical.push(Line::from(""));

    if store.is_empty() {
        logical.push(Line::from(Span::styled("(no tasks)", muted_style)));
    } else {
        for entry in store.tasks() {
            let mark = task_status_mark(entry.status);
            let subject_first = entry.subject.lines().next().unwrap_or("");
            logical.push(Line::from(vec![
                Span::styled(format!("#{} ", entry.taskid), muted_style),
                Span::styled(mark.0.to_owned(), mark.1),
                Span::raw(" "),
                Span::raw(subject_first.to_owned()),
            ]));
            // Subject continuations (multiline subjects).
            for cont in entry.subject.lines().skip(1) {
                logical.push(Line::from(vec![
                    Span::raw("    "),
                    Span::raw(cont.to_owned()),
                ]));
            }
            for raw in entry.description.lines() {
                logical.push(Line::from(vec![
                    Span::styled("    ", body_style),
                    Span::styled(raw.to_owned(), body_style),
                ]));
            }
            logical.push(Line::from(""));
        }
    }

    // Pre-wrap to inner width so scroll math degenerates to row indices.
    let mut wrapped: Vec<Line<'static>> = Vec::with_capacity(logical.len());
    for line in logical {
        wrap_line_into(line, inner.width, &mut wrapped);
    }

    let max_scroll = wrapped.len().saturating_sub(inner.height as usize);
    if app.task_pane_scroll > max_scroll {
        app.task_pane_scroll = max_scroll;
    }
    let start = app.task_pane_scroll;
    let end = (start + inner.height as usize).min(wrapped.len());
    let visible: Vec<Line<'static>> = wrapped[start..end].to_vec();

    Paragraph::new(visible)
        .block(pane_block)
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

    let push_row =
        |current: &mut Vec<Span<'static>>, row_width: &mut usize, out: &mut Vec<Line<'static>>| {
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

fn wrap_history_row_into(
    line: Line<'static>,
    selectable: bool,
    width: u16,
    out: &mut Vec<HistoryRow>,
) {
    let mut wrapped = Vec::new();
    wrap_line_into(line, width, &mut wrapped);
    out.extend(
        wrapped
            .into_iter()
            .map(|line| HistoryRow::new(line, selectable)),
    );
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

fn highlight_line_selection(line: &Line<'static>, start: usize, end: usize) -> Line<'static> {
    if start >= end {
        return line.clone();
    }

    let highlight = Style::default().fg(Color::Black).bg(Color::Cyan);
    let mut col = 0usize;
    let mut spans = Vec::new();
    for span in &line.spans {
        let mut plain = String::new();
        let mut selected = String::new();
        let push_pending =
            |plain: &mut String, selected: &mut String, spans: &mut Vec<Span<'static>>| {
                if !plain.is_empty() {
                    spans.push(Span::styled(std::mem::take(plain), span.style));
                }
                if !selected.is_empty() {
                    spans.push(Span::styled(
                        std::mem::take(selected),
                        span.style.patch(highlight),
                    ));
                }
            };

        let mut in_selected = false;
        for ch in span.content.chars() {
            let width = UnicodeWidthChar::width(ch).unwrap_or(0);
            let next = col.saturating_add(width);
            let selected_char = next > start && col < end;
            if selected_char != in_selected {
                push_pending(&mut plain, &mut selected, &mut spans);
                in_selected = selected_char;
            }
            if selected_char {
                selected.push(ch);
            } else {
                plain.push(ch);
            }
            col = next;
        }
        push_pending(&mut plain, &mut selected, &mut spans);
    }

    Line {
        spans,
        style: line.style,
        alignment: line.alignment,
    }
}

/// Only text-like transcript Items are selectable/copyable. Tool calls and
/// other non-text Items remain visible but unselectable, so their rendered
/// diagnostics/output are never copied through this path.
fn block_is_selectable_text(block: &Block) -> bool {
    matches!(
        block,
        Block::UserMessage { .. } | Block::SystemMessage { .. } | Block::AssistantText { .. }
    )
}

fn render_block_into(lines: &mut Vec<Line<'static>>, block: &Block, width: u16, mode: Mode) {
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
        Block::UserMessage { segments } => render_user_message(lines, segments, width, mode),
        Block::SystemMessage { text } => render_system_message(lines, text, width, mode),
        Block::Notify { message } => {
            let text = format!("[notify] {message}");
            match mode {
                Mode::Overview => push_overview_line(lines, &text, width, MessageKind::Notify, ""),
                _ => push_padded_lines(lines, &text, MessageKind::Notify),
            }
        }
        Block::PodEvent { event } => {
            let text = format_pod_event(event);
            match mode {
                Mode::Overview => push_overview_line(lines, &text, width, MessageKind::Notify, ""),
                _ => push_padded_lines(lines, &text, MessageKind::Notify),
            }
        }
        Block::AssistantText { text } => match mode {
            Mode::Overview => push_overview_line(lines, text, width, MessageKind::Assistant, ""),
            _ => lines.extend(crate::markdown::render(
                text,
                kind_style(MessageKind::Assistant),
            )),
        },
        Block::Thinking(t) => render_thinking(lines, t, width, mode),
        // ToolCall is dispatched in `compute_history` via `tool::render_tool`
        // so it can consume multiple adjacent blocks (Read aggregation).
        Block::ToolCall(_) => unreachable!("ToolCall handled by compute_history"),
        Block::Alert {
            level,
            source,
            message,
        } => {
            let kind = match level {
                AlertLevel::Warn => MessageKind::NoticeWarn,
                AlertLevel::Error => MessageKind::NoticeError,
            };
            let prefix = match level {
                AlertLevel::Warn => "[notice]",
                AlertLevel::Error => "[notice error]",
            };
            let label = alert_source_label(*source);
            let text = format!("{prefix} {label}: {message}");
            match mode {
                Mode::Overview => push_overview_line(lines, &text, width, kind, ""),
                _ => push_padded_lines(lines, &text, kind),
            }
        }
        Block::Compact(evt) => render_compact(lines, evt, width, mode),
        Block::TurnStats {
            requests,
            upload_tokens,
            output_tokens,
        } => {
            let text = format!(
                "{} reqs ↑{}/↓{}",
                requests,
                fmt_tokens(*upload_tokens),
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

/// Render `Block::UserMessage` from typed segments. Each non-text
/// segment renders as a one-piece chip whose colour matches the input
/// area's chip presentation (paste = magenta, `@` file = cyan,
/// `#` knowledge = green, `/` workflow = yellow), so the user
/// recognises their own typed atoms in the scrollback.
fn render_user_message(
    lines: &mut Vec<Line<'static>>,
    segments: &[Segment],
    width: u16,
    mode: Mode,
) {
    if matches!(mode, Mode::Overview) {
        let text = segments
            .iter()
            .map(segment_display_text)
            .collect::<Vec<_>>()
            .join("");
        push_overview_line(lines, &text, width, MessageKind::User, "> ");
        return;
    }

    let user_style = kind_style(MessageKind::User);
    let mut current: Vec<Span<'static>> = Vec::new();

    for seg in segments {
        match seg {
            Segment::Text { content } => {
                let mut iter = content.split('\n').peekable();
                while let Some(line) = iter.next() {
                    if !line.is_empty() {
                        current.push(Span::styled(line.to_owned(), user_style));
                    }
                    if iter.peek().is_some() {
                        lines.push(Line::from(std::mem::take(&mut current)));
                    }
                }
            }
            other => {
                let (style, text) = chip_span_for(other, user_style);
                current.push(Span::styled(text, style));
            }
        }
    }
    if !current.is_empty() {
        lines.push(Line::from(current));
    }
}

fn render_system_message(lines: &mut Vec<Line<'static>>, text: &str, width: u16, mode: Mode) {
    let header_style = kind_style(MessageKind::System);
    let body_style = Style::default().fg(Color::DarkGray);
    let (header, body) = split_system_message(text);
    let overview_text = if body.is_empty() {
        header.to_owned()
    } else {
        format!("{header} {body}")
    };

    match mode {
        Mode::Overview => push_overview_line(lines, &overview_text, width, MessageKind::System, ""),
        Mode::Detail => {
            lines.push(Line::from(Span::styled(header.to_owned(), header_style)));
            for raw in body.lines() {
                lines.push(Line::from(vec![
                    Span::styled("  ", body_style),
                    Span::styled(raw.to_owned(), body_style),
                ]));
            }
            if body.is_empty() && header.is_empty() {
                lines.push(Line::from(""));
            }
        }
        Mode::Normal => {
            lines.push(Line::from(Span::styled(header.to_owned(), header_style)));
            let preview = system_message_preview(body, 4);
            for line in preview.lines {
                lines.push(Line::from(vec![
                    Span::styled("  ", body_style),
                    Span::styled(line, body_style),
                ]));
            }
            if preview.omitted_lines > 0 {
                lines.push(Line::from(vec![
                    Span::styled("  ", body_style),
                    Span::styled(
                        format!("… ({} more lines)", preview.omitted_lines),
                        body_style.add_modifier(Modifier::ITALIC),
                    ),
                ]));
            }
        }
    }
}

fn split_system_message(text: &str) -> (&str, &str) {
    match text.split_once('\n') {
        Some((header, body)) => (header, body.trim_start_matches('\n')),
        None => (text, ""),
    }
}

struct SystemMessagePreview {
    lines: Vec<String>,
    omitted_lines: usize,
}

fn system_message_preview(body: &str, max_lines: usize) -> SystemMessagePreview {
    let all: Vec<&str> = body.lines().collect();
    let lines = all
        .iter()
        .take(max_lines)
        .map(|line| (*line).to_owned())
        .collect();
    SystemMessagePreview {
        lines,
        omitted_lines: all.len().saturating_sub(max_lines),
    }
}

/// Style + display text for a single chip-style `Segment`. `fallback`
/// is used for `Segment::Text` (which the caller handles inline) and
/// for `Segment::Unknown` so future variants degrade gracefully.
fn chip_span_for(seg: &Segment, fallback: Style) -> (Style, String) {
    match seg {
        Segment::Text { content } => (fallback, content.clone()),
        Segment::Paste {
            id,
            chars,
            lines: line_count,
            ..
        } => (
            Style::default().fg(Color::Magenta),
            format!("[Clipboard #{id} | {chars} chars, {line_count} lines]"),
        ),
        Segment::FileRef { path } => (Style::default().fg(Color::Cyan), format!("@{path}")),
        Segment::KnowledgeRef { slug } => (Style::default().fg(Color::Green), format!("#{slug}")),
        Segment::WorkflowInvoke { slug } => {
            (Style::default().fg(Color::Yellow), format!("/{slug}"))
        }
        Segment::Unknown => (fallback, "[unknown segment]".to_owned()),
    }
}

/// One-line textual rendering of a segment, used by `Mode::Overview`
/// (which collapses everything to a single string) and as the fallback
/// inline rendering for non-paste, non-text segments.
fn segment_display_text(seg: &Segment) -> String {
    match seg {
        Segment::Text { content } => content.replace('\n', " "),
        Segment::Paste {
            id, chars, lines, ..
        } => format!("[Clipboard #{id} | {chars} chars, {lines} lines]"),
        Segment::FileRef { path } => format!("@{path}"),
        Segment::KnowledgeRef { slug } => format!("#{slug}"),
        Segment::WorkflowInvoke { slug } => format!("/{slug}"),
        Segment::Unknown => "[unknown segment]".to_owned(),
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

fn render_thinking(lines: &mut Vec<Line<'static>>, t: &ThinkingBlock, width: u16, mode: Mode) {
    let header_style = kind_style(MessageKind::Thinking);
    let body_style = Style::default().fg(Color::DarkGray);

    let header = match &t.state {
        ThinkingState::Streaming { started_at } => {
            let secs = started_at.elapsed().as_secs();
            format!("Thinking... ({})", fmt_elapsed(secs))
        }
        ThinkingState::Finished { elapsed_secs } => match elapsed_secs {
            Some(s) => format!("Thought for {}", fmt_elapsed(*s)),
            None => "Thought".to_owned(),
        },
        ThinkingState::Incomplete { elapsed_secs } => match elapsed_secs {
            Some(s) => format!("Thinking interrupted ({})", fmt_elapsed(*s)),
            None => "Thinking interrupted".to_owned(),
        },
    };

    if matches!(mode, Mode::Overview) {
        push_overview_line(lines, &header, width, MessageKind::Thinking, "");
        return;
    }

    lines.push(Line::from(Span::styled(header, header_style)));

    if t.text.is_empty() {
        return;
    }

    match mode {
        Mode::Detail => {
            for raw in t.text.lines() {
                lines.push(Line::from(vec![
                    Span::styled("  ", body_style),
                    Span::styled(raw.to_owned(), body_style),
                ]));
            }
        }
        Mode::Normal => {
            // Streaming: show the *latest* tail to keep the cursor of
            // attention near where new tokens are appearing. Finished:
            // show the first line as a static preview — collapsing it
            // entirely would lose the only context most users want
            // ("what was it thinking about").
            let preview = match &t.state {
                ThinkingState::Streaming { .. } => trailing_line_preview(&t.text),
                _ => first_line_preview(&t.text),
            };
            if !preview.is_empty() {
                let budget = width.saturating_sub(2) as usize;
                let truncated = truncate_with_ellipsis(&preview, budget);
                lines.push(Line::from(vec![
                    Span::styled("  ", body_style),
                    Span::styled(truncated, body_style),
                ]));
            }
        }
        Mode::Overview => unreachable!("handled above"),
    }
}

/// Last segment of `text` after the final newline (or the whole string
/// if it has no newline). Used as the live "what is it thinking now"
/// 1-liner.
fn trailing_line_preview(text: &str) -> String {
    text.rsplit_once('\n')
        .map(|(_, tail)| tail)
        .unwrap_or(text)
        .trim_end()
        .to_owned()
}

/// First non-empty line of `text`. Used as the static preview after a
/// thinking block finishes, mirroring the "first line + (+N lines)"
/// idiom of the overview mode.
fn first_line_preview(text: &str) -> String {
    text.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .to_owned()
}

fn fmt_elapsed(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m{:02}s", secs / 60, secs % 60)
    }
}

fn render_compact(lines: &mut Vec<Line<'static>>, evt: &CompactEvent, width: u16, mode: Mode) {
    let (text, kind) = match evt {
        CompactEvent::Streaming { started_at } => {
            let secs = started_at.elapsed().as_secs();
            (
                format!("Compacting... ({})", fmt_elapsed(secs)),
                MessageKind::NoticeWarn,
            )
        }
        CompactEvent::Done {
            new_segment_id,
            elapsed_secs,
        } => {
            let short = new_segment_id
                .to_string()
                .chars()
                .take(8)
                .collect::<String>();
            let elapsed = elapsed_suffix(*elapsed_secs);
            (
                format!("[compact] done (new session {short}){elapsed}"),
                MessageKind::NoticeWarn,
            )
        }
        CompactEvent::Failed {
            error,
            elapsed_secs,
        } => {
            let elapsed = elapsed_suffix(*elapsed_secs);
            (
                format!("[compact error] {error}{elapsed}"),
                MessageKind::NoticeError,
            )
        }
        CompactEvent::Incomplete { elapsed_secs } => match elapsed_secs {
            Some(s) => (
                format!("[compact] interrupted ({})", fmt_elapsed(*s)),
                MessageKind::NoticeError,
            ),
            None => ("[compact] interrupted".to_owned(), MessageKind::NoticeError),
        },
    };
    match mode {
        Mode::Overview => push_overview_line(lines, &text, width, kind, ""),
        _ => push_padded_lines(lines, &text, kind),
    }
}

fn elapsed_suffix(elapsed_secs: Option<u64>) -> String {
    elapsed_secs
        .map(|s| format!(" ({})", fmt_elapsed(s)))
        .unwrap_or_default()
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

fn context_usage_text(app: &App) -> String {
    let pct = if app.context_window == 0 {
        0
    } else {
        ((app.session_context_tokens as f64 / app.context_window as f64) * 100.0).round() as u64
    };
    format!(
        "{} / {} ({}%)",
        fmt_tokens(app.session_context_tokens),
        fmt_tokens(app.context_window),
        pct
    )
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
        let status = if let Some(wait_event) = &app.latest_llm_wait_event {
            format!(
                "request: {} | ↑{}/↓{} | {wait_event}",
                app.run_requests,
                fmt_tokens(app.run_upload_tokens),
                fmt_tokens(app.run_output_tokens),
            )
        } else if let Some(tool) = &app.current_tool {
            format!(
                "request: {} | ↑{}/↓{} | tool: {tool}",
                app.run_requests,
                fmt_tokens(app.run_upload_tokens),
                fmt_tokens(app.run_output_tokens),
            )
        } else {
            format!(
                "request: {} | ↑{}/↓{}",
                app.run_requests,
                fmt_tokens(app.run_upload_tokens),
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
            " — Enter to resume, Ctrl-X to cancel, type to start new turn",
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        spans.push(Span::styled(" idle", Style::default().fg(Color::DarkGray)));
    }

    if let Some(queue) = queue_status_text(app) {
        spans.push(Span::raw(" | "));
        spans.push(Span::styled(queue, Style::default().fg(Color::Magenta)));
    }

    let right_text = context_usage_text(app);
    let right_line = Line::from(Span::styled(right_text, Style::default().fg(Color::Gray)))
        .alignment(ratatui::layout::Alignment::Right);

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
    frame.render_widget(Paragraph::new(right_line), area);
}

fn actionbar_left_item(app: &App, now: Instant) -> Option<(String, Style)> {
    // Priority is deliberately actionable UI state first, then transient notices,
    // then lower-priority lifecycle status. Right-side scroll/view labels are
    // rendered independently below.
    if app.is_command_mode() {
        return Some((
            "COMMAND".to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if app.queued_input_count() > 0 {
        return Some((
            "Alt-q edit queued  Alt-c clear queued".to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    }
    if let Some(notice) = app.current_actionbar_notice(now) {
        return Some((
            truncate_with_ellipsis(&notice.text, 96),
            actionbar_notice_style(notice.level),
        ));
    }
    if let Some(llm_event) = app.latest_llm_wait_event.as_deref() {
        return Some((
            truncate_with_ellipsis(llm_event, 96),
            Style::default().fg(Color::Yellow),
        ));
    }
    if let Some(memory_event) = app.latest_memory_worker_event.as_deref() {
        return Some((
            truncate_with_ellipsis(memory_event, 72),
            Style::default().fg(Color::Blue),
        ));
    }
    None
}

fn actionbar_notice_style(level: ActionbarNoticeLevel) -> Style {
    match level {
        ActionbarNoticeLevel::Info => Style::default().fg(Color::Cyan),
        ActionbarNoticeLevel::Warn => Style::default().fg(Color::Yellow),
        ActionbarNoticeLevel::Error => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    }
}

fn draw_actionbar(frame: &mut Frame, app: &mut App, area: Rect) {
    let now = Instant::now();
    app.clear_expired_actionbar_notice(now);

    let mut left: Vec<Span<'static>> = Vec::new();
    if let Some((text, style)) = actionbar_left_item(app, now) {
        left.push(Span::styled(text, style));
    }

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
    let left_line = Line::from(left);
    let right_line = Line::from(right).alignment(ratatui::layout::Alignment::Right);
    frame.render_widget(Paragraph::new(left_line), area);
    frame.render_widget(Paragraph::new(right_line), area);
}

fn queue_status_text(app: &App) -> Option<String> {
    let count = app.queued_input_count();
    if count == 0 {
        return None;
    }
    let mut text = format!("queued: {count}");
    if let Some(preview) = app.next_queued_input_preview() {
        let preview = truncate_with_ellipsis(preview.trim(), 40);
        if !preview.is_empty() {
            text.push_str(" — ");
            text.push_str(&preview);
        }
    }
    Some(text)
}

fn draw_input(frame: &mut Frame, app: &App, render: &crate::input::InputRender, area: Rect) {
    // Prefix prompt on the first row, matching-width gutter for continuation
    // rows so multi-line input aligns visually.
    let prompt = if app.is_command_mode() { ": " } else { "> " };
    let continuation = if app.is_command_mode() { ": " } else { "  " };
    let prompt_style = if app.is_command_mode() {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(render.lines.len());
    for (i, src) in render.lines.iter().enumerate() {
        let absolute_row = render.viewport_start_row as usize + i;
        let prefix = if absolute_row == 0 {
            prompt
        } else {
            continuation
        };
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
    /// External-input echoes (`Method::Notify` / `Method::PodEvent`).
    /// Visually distinct from User / Assistant / Notice so it's clear
    /// the line came from another Pod or operator, not the local user.
    Notify,
    /// Persisted role:system history item preview.
    System,
    Assistant,
    Thinking,
    TurnStats,
    NoticeWarn,
    NoticeError,
}

pub fn kind_style(kind: MessageKind) -> Style {
    match kind {
        MessageKind::TurnHeader => Style::default().fg(Color::DarkGray),
        MessageKind::User => Style::default().fg(Color::Green),
        MessageKind::Notify => Style::default().fg(Color::Yellow),
        MessageKind::System => Style::default().fg(Color::Cyan),
        MessageKind::Assistant => Style::default().fg(Color::White),
        MessageKind::Thinking => Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::ITALIC),
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

/// One-line summary of a `PodEvent` for display in the activity log.
/// Independent from the LLM-injection wrapper (`crate::ipc::event::render_event`
/// in the pod crate) — that path applies prompt-pack wrapping, while
/// this is the human-facing rendering of the raw structured event.
fn format_pod_event(event: &PodEvent) -> String {
    match event {
        PodEvent::TurnEnded { pod_name } => {
            format!("[pod_event] {pod_name} → turn_ended")
        }
        PodEvent::Errored { pod_name, message } => {
            format!("[pod_event] {pod_name} → errored: {message}")
        }
        PodEvent::ShutDown { pod_name } => {
            format!("[pod_event] {pod_name} → shut_down")
        }
        PodEvent::ScopeSubDelegated {
            parent_pod,
            sub_pod,
            ..
        } => {
            format!("[pod_event] {parent_pod} → scope_sub_delegated: {sub_pod}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{ActionbarNoticeLevel, ActionbarNoticeSource, App};
    use protocol::PodStatus;
    use std::time::{Duration, Instant};

    #[test]
    fn queue_status_text_includes_count_and_preview() {
        let mut app = App::new("test".into());
        app.set_pod_status(PodStatus::Running);
        for c in "queued preview".chars() {
            app.insert_char(c);
        }
        assert!(app.submit_input().is_none());

        assert_eq!(
            queue_status_text(&app),
            Some("queued: 1 — queued preview".to_string())
        );
    }

    #[test]
    fn queue_status_text_is_absent_without_queue() {
        let app = App::new("test".into());

        assert_eq!(queue_status_text(&app), None);
    }

    #[test]
    fn actionbar_notice_priority_sits_below_actionable_hints_and_above_lifecycle_status() {
        let mut app = App::new("test".into());
        let now = Instant::now();
        app.latest_llm_wait_event = Some("retrying LLM request".into());
        app.latest_memory_worker_event = Some("memory extract running".into());
        app.flash_actionbar_notice_at(
            "Pod keeps running. Press Ctrl-C again to exit TUI.",
            ActionbarNoticeLevel::Warn,
            ActionbarNoticeSource::Tui,
            now,
            Duration::from_secs(3),
        );

        assert_eq!(
            actionbar_left_item(&app, now).map(|(text, _)| text),
            Some("Pod keeps running. Press Ctrl-C again to exit TUI.".into())
        );

        app.set_pod_status(PodStatus::Running);
        for c in "queued turn".chars() {
            app.insert_char(c);
        }
        assert!(app.submit_input().is_none());
        assert_eq!(
            actionbar_left_item(&app, now).map(|(text, _)| text),
            Some("Alt-q edit queued  Alt-c clear queued".into())
        );

        app.enter_command_mode();
        assert_eq!(
            actionbar_left_item(&app, now).map(|(text, _)| text),
            Some("COMMAND".into())
        );
    }

    #[test]
    fn expired_actionbar_notice_is_skipped_for_lifecycle_status() {
        let mut app = App::new("test".into());
        let now = Instant::now();
        app.latest_llm_wait_event = Some("retrying LLM request".into());
        app.flash_actionbar_notice_at(
            "expired",
            ActionbarNoticeLevel::Info,
            ActionbarNoticeSource::Tui,
            now,
            Duration::from_secs(1),
        );

        assert_eq!(
            actionbar_left_item(&app, now + Duration::from_secs(1)).map(|(text, _)| text),
            Some("retrying LLM request".into())
        );
    }

    #[test]
    fn history_rows_mark_text_items_selectable_and_non_text_unselectable() {
        let mut app = App::new("pod".to_string());
        app.blocks = vec![
            Block::UserMessage {
                segments: vec![Segment::Text {
                    content: "hello".to_string(),
                }],
            },
            Block::AssistantText {
                text: "world".to_string(),
            },
            Block::Thinking(ThinkingBlock {
                text: "private reasoning".to_string(),
                state: ThinkingState::Finished { elapsed_secs: None },
            }),
        ];

        let layout = compute_history(&app, 80);
        let rows: Vec<_> = layout
            .rows
            .iter()
            .map(|row| (row.text.as_str(), row.selectable))
            .collect();

        assert!(rows.contains(&("hello", true)));
        assert!(rows.contains(&("world", true)));
        assert!(
            rows.iter()
                .any(|(text, selectable)| text.contains("private reasoning") && !*selectable)
        );
        assert!(
            rows.iter()
                .any(|(text, selectable)| text.is_empty() && *selectable)
        );
    }
}
