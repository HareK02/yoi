//! Local, non-persistent text selection state for the single-Pod transcript view.
//!
//! This module deliberately stores only the most recent rendered history rows and
//! the active drag endpoints. Selected/copied text never leaves TUI-local state
//! unless the user explicitly presses the copy key, and even then the caller is
//! responsible for using a non-history clipboard path.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionPoint {
    pub row: usize,
    pub col: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryViewport {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub top_offset: usize,
    pub total_lines: usize,
}

impl HistoryViewport {
    pub fn contains(self, col: u16, row: u16) -> bool {
        col >= self.x
            && col < self.x.saturating_add(self.width)
            && row >= self.y
            && row < self.y.saturating_add(self.height)
    }

    pub fn point_at(self, col: u16, row: u16) -> Option<SelectionPoint> {
        if !self.contains(col, row) {
            return None;
        }
        Some(SelectionPoint {
            row: self.top_offset + row.saturating_sub(self.y) as usize,
            col: col.saturating_sub(self.x) as usize,
        })
    }

    pub fn clamped_point_at(self, col: u16, row: u16) -> Option<SelectionPoint> {
        if self.width == 0 || self.height == 0 || self.total_lines == 0 {
            return None;
        }
        let max_col = self.x.saturating_add(self.width.saturating_sub(1));
        let max_row = self.y.saturating_add(self.height.saturating_sub(1));
        let col = col.clamp(self.x, max_col);
        let row = row.clamp(self.y, max_row);
        let absolute_row = self.top_offset + row.saturating_sub(self.y) as usize;
        if absolute_row >= self.total_lines {
            return None;
        }
        Some(SelectionPoint {
            row: absolute_row,
            col: col.saturating_sub(self.x) as usize,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionRow {
    pub text: String,
    /// Whether this rendered row belongs to selectable transcript text. Tool
    /// calls, thinking blocks, greetings, notices, stats, and other non-text
    /// items are intentionally false.
    pub selectable: bool,
}

impl SelectionRow {
    pub fn new(text: String, selectable: bool) -> Self {
        Self { text, selectable }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveSelection {
    pub anchor: SelectionPoint,
    pub focus: SelectionPoint,
    pub dragging: bool,
}

#[derive(Debug, Clone, Default)]
pub struct TextSelectionState {
    active: Option<ActiveSelection>,
    viewport: Option<HistoryViewport>,
    rows: Vec<SelectionRow>,
}

impl TextSelectionState {
    pub fn set_history_snapshot(&mut self, viewport: HistoryViewport, rows: Vec<SelectionRow>) {
        let rows_changed = self.rows != rows;
        self.viewport = Some(viewport);
        self.rows = rows;
        if rows_changed {
            self.active = None;
        } else {
            self.drop_selection_if_stale();
        }
    }

    pub fn clear_history_snapshot(&mut self) {
        self.viewport = None;
        self.rows.clear();
        self.active = None;
    }

    #[cfg(test)]
    pub fn active(&self) -> Option<ActiveSelection> {
        self.active
    }

    pub fn has_selection(&self) -> bool {
        self.active.is_some()
    }

    pub fn clear(&mut self) -> bool {
        self.active.take().is_some()
    }

    pub fn begin_drag(&mut self, col: u16, row: u16) -> bool {
        let Some(point) = self
            .viewport
            .and_then(|viewport| viewport.point_at(col, row))
        else {
            return false;
        };
        if !self.row_is_selectable(point.row) {
            self.active = None;
            return false;
        }
        self.active = Some(ActiveSelection {
            anchor: point,
            focus: point,
            dragging: true,
        });
        true
    }

    pub fn update_drag(&mut self, col: u16, row: u16) -> bool {
        let Some(mut active) = self.active else {
            return false;
        };
        if !active.dragging {
            return false;
        }
        let Some(point) = self
            .viewport
            .and_then(|viewport| viewport.clamped_point_at(col, row))
        else {
            return false;
        };
        active.focus = point;
        self.active = Some(active);
        true
    }

    pub fn finish_drag(&mut self, col: u16, row: u16) -> bool {
        let updated = self.update_drag(col, row);
        if let Some(active) = self.active.as_mut() {
            active.dragging = false;
            true
        } else {
            updated
        }
    }

    pub fn copy_text(&self) -> Option<String> {
        let active = self.active?;
        selected_text_from_rows(&self.rows, active.anchor, active.focus)
    }

    pub fn range_for_row(&self, row: usize) -> Option<(usize, usize)> {
        let active = self.active?;
        let (start, end) = ordered_points(active.anchor, active.focus);
        if row < start.row || row > end.row || !self.row_is_selectable(row) {
            return None;
        }
        let row_width = display_width(&self.rows.get(row)?.text);
        let (from, to) = if start.row == end.row {
            (
                start.col.min(row_width),
                end.col.saturating_add(1).min(row_width),
            )
        } else if row == start.row {
            (start.col.min(row_width), row_width)
        } else if row == end.row {
            (0, end.col.saturating_add(1).min(row_width))
        } else {
            (0, row_width)
        };
        if from >= to { None } else { Some((from, to)) }
    }

    fn row_is_selectable(&self, row: usize) -> bool {
        self.rows.get(row).is_some_and(|row| row.selectable)
    }

    fn drop_selection_if_stale(&mut self) {
        let Some(active) = self.active else {
            return;
        };
        if active.anchor.row >= self.rows.len() || active.focus.row >= self.rows.len() {
            self.active = None;
        }
    }
}

pub fn selected_text_from_rows(
    rows: &[SelectionRow],
    anchor: SelectionPoint,
    focus: SelectionPoint,
) -> Option<String> {
    let (start, end) = ordered_points(anchor, focus);
    if start.row >= rows.len() || end.row >= rows.len() {
        return None;
    }

    let mut copied = Vec::new();
    for row_idx in start.row..=end.row {
        let row = &rows[row_idx];
        if !row.selectable {
            continue;
        }
        let row_width = display_width(&row.text);
        let (from, to) = if start.row == end.row {
            (
                start.col.min(row_width),
                end.col.saturating_add(1).min(row_width),
            )
        } else if row_idx == start.row {
            (start.col.min(row_width), row_width)
        } else if row_idx == end.row {
            (0, end.col.saturating_add(1).min(row_width))
        } else {
            (0, row_width)
        };
        copied.push(slice_display_cols(&row.text, from, to));
    }

    if copied.iter().any(|line| !line.is_empty()) {
        Some(copied.join("\n"))
    } else {
        None
    }
}

pub fn ordered_points(a: SelectionPoint, b: SelectionPoint) -> (SelectionPoint, SelectionPoint) {
    if (a.row, a.col) <= (b.row, b.col) {
        (a, b)
    } else {
        (b, a)
    }
}

pub fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

pub fn slice_display_cols(text: &str, start: usize, end: usize) -> String {
    if start >= end {
        return String::new();
    }

    let mut out = String::new();
    let mut col = 0usize;
    for c in text.chars() {
        let width = UnicodeWidthChar::width(c).unwrap_or(0);
        let next = col.saturating_add(width);
        if next > start && col < end {
            out.push(c);
        }
        col = next;
        if col >= end {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinate_mapping_uses_history_inner_origin_and_scroll_offset() {
        let viewport = HistoryViewport {
            x: 2,
            y: 3,
            width: 10,
            height: 5,
            top_offset: 7,
            total_lines: 20,
        };

        assert_eq!(
            viewport.point_at(4, 5),
            Some(SelectionPoint { row: 9, col: 2 })
        );
        assert_eq!(viewport.point_at(1, 5), None);
        assert_eq!(viewport.point_at(4, 8), None);
    }

    #[test]
    fn selection_state_drag_update_and_clear() {
        let mut state = TextSelectionState::default();
        state.set_history_snapshot(
            HistoryViewport {
                x: 0,
                y: 0,
                width: 20,
                height: 3,
                top_offset: 0,
                total_lines: 3,
            },
            vec![
                SelectionRow::new("alpha".into(), true),
                SelectionRow::new("tool".into(), false),
                SelectionRow::new("omega".into(), true),
            ],
        );

        assert!(state.begin_drag(1, 0));
        assert!(state.update_drag(2, 2));
        assert_eq!(state.copy_text().as_deref(), Some("lpha\nome"));
        assert!(state.finish_drag(2, 2));
        assert!(!state.active().unwrap().dragging);
        assert!(state.clear());
        assert!(!state.has_selection());
    }

    #[test]
    fn selected_text_preserves_blank_separator_between_text_items() {
        let rows = vec![
            SelectionRow::new("first".into(), true),
            SelectionRow::new("".into(), true),
            SelectionRow::new("second".into(), true),
        ];

        let copied = selected_text_from_rows(
            &rows,
            SelectionPoint { row: 0, col: 0 },
            SelectionPoint { row: 2, col: 5 },
        )
        .unwrap();
        assert_eq!(copied, "first\n\nsecond");
    }

    #[test]
    fn non_text_rows_are_skipped_during_extraction() {
        let rows = vec![
            SelectionRow::new("user".into(), true),
            SelectionRow::new("tool output".into(), false),
            SelectionRow::new("assistant".into(), true),
        ];

        let copied = selected_text_from_rows(
            &rows,
            SelectionPoint { row: 0, col: 0 },
            SelectionPoint { row: 2, col: 8 },
        )
        .unwrap();
        assert_eq!(copied, "user\nassistant");
    }
}
