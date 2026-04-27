//! Multi-line input buffer with paste placeholders.
//!
//! The buffer stores a sequence of [`Atom`]s — each either a single
//! character (including `\n`) or an atomic paste reference. The cursor
//! is an index in `0..=atoms.len()` marking the insertion point between
//! atoms. Paste atoms are indivisible: Backspace deletes the whole
//! placeholder, the cursor can't land "inside" one.
//!
//! Display form: paste atoms render as
//! `[Clipboard #N | X chars, Y lines]`. Submit form: paste atoms expand
//! back to their original captured content so the Pod sees the full
//! pasted text (without the placeholder label).

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone)]
pub struct PasteRef {
    pub id: u32,
    pub chars: usize,
    pub lines: usize,
    pub content: String,
}

impl PasteRef {
    pub fn label(&self) -> String {
        format!(
            "[Clipboard #{} | {} chars, {} lines]",
            self.id, self.chars, self.lines
        )
    }
}

#[derive(Debug, Clone)]
pub enum Atom {
    Char(char),
    Paste(PasteRef),
}

pub struct InputBuffer {
    atoms: Vec<Atom>,
    /// Insertion point in `0..=atoms.len()`.
    cursor: usize,
    /// Monotonic counter reused across the TUI process lifetime.
    next_paste_id: u32,
}

impl Default for InputBuffer {
    fn default() -> Self {
        Self {
            atoms: Vec::new(),
            cursor: 0,
            next_paste_id: 1,
        }
    }
}

impl InputBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.atoms.clear();
        self.cursor = 0;
    }

    pub fn insert_char(&mut self, c: char) {
        self.atoms.insert(self.cursor, Atom::Char(c));
        self.cursor += 1;
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    pub fn insert_paste(&mut self, content: String) {
        let id = self.next_paste_id;
        self.next_paste_id = self.next_paste_id.wrapping_add(1);
        let chars = content.chars().count();
        let lines = content.lines().count().max(1);
        self.atoms.insert(
            self.cursor,
            Atom::Paste(PasteRef {
                id,
                chars,
                lines,
                content,
            }),
        );
        self.cursor += 1;
    }

    pub fn delete_before(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        self.atoms.remove(self.cursor);
    }

    pub fn delete_after(&mut self) {
        if self.cursor < self.atoms.len() {
            self.atoms.remove(self.cursor);
        }
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.atoms.len());
    }

    pub fn move_home(&mut self) {
        while self.cursor > 0 {
            if matches!(self.atoms[self.cursor - 1], Atom::Char('\n')) {
                break;
            }
            self.cursor -= 1;
        }
    }

    pub fn move_end(&mut self) {
        while self.cursor < self.atoms.len() {
            if matches!(self.atoms[self.cursor], Atom::Char('\n')) {
                break;
            }
            self.cursor += 1;
        }
    }

    /// Move one logical line up, preserving column (atom count from
    /// current line start). No-op if already on the first line.
    pub fn move_up(&mut self) {
        let (line_start, col) = self.line_start_and_col();
        if line_start == 0 {
            return;
        }
        // `atoms[line_start - 1]` is the '\n' that opens the current
        // line; find the previous line's start.
        let prev_end = line_start - 1;
        let mut prev_start = 0;
        for i in (0..prev_end).rev() {
            if matches!(self.atoms[i], Atom::Char('\n')) {
                prev_start = i + 1;
                break;
            }
        }
        let prev_len = prev_end - prev_start;
        self.cursor = prev_start + col.min(prev_len);
    }

    /// Move one logical line down, preserving column.
    pub fn move_down(&mut self) {
        let (line_start, col) = self.line_start_and_col();
        // End of current line.
        let mut cur_end = self.atoms.len();
        for i in line_start..self.atoms.len() {
            if matches!(self.atoms[i], Atom::Char('\n')) {
                cur_end = i;
                break;
            }
        }
        if cur_end == self.atoms.len() {
            return; // no next line
        }
        let next_start = cur_end + 1;
        let mut next_end = self.atoms.len();
        for i in next_start..self.atoms.len() {
            if matches!(self.atoms[i], Atom::Char('\n')) {
                next_end = i;
                break;
            }
        }
        let next_len = next_end - next_start;
        self.cursor = next_start + col.min(next_len);
    }

    fn line_start_and_col(&self) -> (usize, usize) {
        let mut start = 0;
        for i in (0..self.cursor).rev() {
            if matches!(self.atoms[i], Atom::Char('\n')) {
                start = i + 1;
                break;
            }
        }
        (start, self.cursor - start)
    }

    /// Build the typed `Vec<Segment>` sent over the protocol. Adjacent
    /// `Atom::Char`s are concatenated into a single `Segment::Text`;
    /// each `Atom::Paste` becomes a standalone `Segment::Paste` so the
    /// `[Clipboard #N | X chars, Y lines]` chip can be reconstructed by
    /// any client subscribed to the resulting `Event::UserMessage`.
    pub fn submit_segments(&self) -> Vec<protocol::Segment> {
        let mut out = Vec::new();
        let mut buf = String::new();
        for a in &self.atoms {
            match a {
                Atom::Char(c) => buf.push(*c),
                Atom::Paste(p) => {
                    if !buf.is_empty() {
                        out.push(protocol::Segment::text(std::mem::take(&mut buf)));
                    }
                    out.push(protocol::Segment::Paste {
                        id: p.id,
                        chars: p.chars as u32,
                        lines: p.lines as u32,
                        content: p.content.clone(),
                    });
                }
            }
        }
        if !buf.is_empty() {
            out.push(protocol::Segment::text(buf));
        }
        out
    }

    /// Visible rendering wrapped to `content_width` display columns, plus
    /// `(row, col)` of the cursor where `col` is a Unicode display column
    /// within the wrapped layout.
    pub fn render(&self, content_width: u16) -> InputRender {
        let w = content_width.max(1) as usize;
        let paste_style = Style::default().fg(Color::Magenta);
        let text_style = Style::default();

        // Row-builder state. `pending` + `pending_width` batch consecutive
        // same-style chars into one Span per flush.
        let mut rows: Vec<Vec<Span<'static>>> = vec![Vec::new()];
        let mut row_width: usize = 0;
        let mut pending = String::new();
        let mut pending_width: usize = 0;
        let mut pending_style = text_style;

        let mut cursor_row: u16 = 0;
        let mut cursor_col: u16 = 0;
        let mut cursor_set = false;

        // Record cursor once, at the point right before `atom` would be
        // placed — accounting for a wrap that the atom itself will cause.
        fn cursor_before(
            leading_width: usize,
            row_width: usize,
            pending_width: usize,
            content_w: usize,
            cur_rows: usize,
        ) -> (u16, u16) {
            let here = row_width + pending_width;
            // If the atom's first-char width would overflow and the row
            // isn't empty, the cursor sits at the start of the wrap row.
            if leading_width > 0 && here + leading_width > content_w && here > 0 {
                (cur_rows as u16, 0)
            } else {
                ((cur_rows - 1) as u16, here as u16)
            }
        }

        for (i, atom) in self.atoms.iter().enumerate() {
            if !cursor_set && i == self.cursor {
                let leading = match atom {
                    Atom::Char('\n') => 0,
                    Atom::Char(c) => UnicodeWidthChar::width(*c).unwrap_or(0),
                    Atom::Paste(p) => p
                        .label()
                        .chars()
                        .next()
                        .and_then(UnicodeWidthChar::width)
                        .unwrap_or(0),
                };
                let (r, c) = cursor_before(leading, row_width, pending_width, w, rows.len());
                cursor_row = r;
                cursor_col = c;
                cursor_set = true;
            }

            match atom {
                Atom::Char('\n') => {
                    flush_pending(
                        &mut pending,
                        &mut pending_width,
                        pending_style,
                        &mut rows,
                        &mut row_width,
                    );
                    rows.push(Vec::new());
                    row_width = 0;
                }
                Atom::Char(c) => {
                    let cw = UnicodeWidthChar::width(*c).unwrap_or(0);
                    if pending_style != text_style && !pending.is_empty() {
                        flush_pending(
                            &mut pending,
                            &mut pending_width,
                            pending_style,
                            &mut rows,
                            &mut row_width,
                        );
                    }
                    pending_style = text_style;
                    place_char(
                        *c,
                        cw,
                        &mut pending,
                        &mut pending_width,
                        pending_style,
                        &mut rows,
                        &mut row_width,
                        w,
                    );
                }
                Atom::Paste(p) => {
                    if pending_style != paste_style && !pending.is_empty() {
                        flush_pending(
                            &mut pending,
                            &mut pending_width,
                            pending_style,
                            &mut rows,
                            &mut row_width,
                        );
                    }
                    pending_style = paste_style;
                    for c in p.label().chars() {
                        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
                        place_char(
                            c,
                            cw,
                            &mut pending,
                            &mut pending_width,
                            pending_style,
                            &mut rows,
                            &mut row_width,
                            w,
                        );
                    }
                }
            }
        }

        // Flush trailing pending chars.
        flush_pending(
            &mut pending,
            &mut pending_width,
            pending_style,
            &mut rows,
            &mut row_width,
        );

        // Cursor at end-of-buffer.
        if !cursor_set && self.cursor == self.atoms.len() {
            if row_width >= w && w > 0 {
                // Last row is full — land the cursor on a fresh line so
                // it stays visible instead of hanging off the right edge.
                rows.push(Vec::new());
                cursor_row = (rows.len() - 1) as u16;
                cursor_col = 0;
            } else {
                cursor_row = (rows.len() - 1) as u16;
                cursor_col = row_width as u16;
            }
        }

        let lines: Vec<Line<'static>> = rows.into_iter().map(Line::from).collect();

        InputRender {
            lines,
            cursor_row,
            cursor_col,
        }
    }
}

/// Append a single char, wrapping to a new row first when it would
/// overflow `content_w`. The row is allowed to hold a single oversized
/// char (e.g. a wide CJK glyph on a 1-column layout) so we never loop.
fn place_char(
    c: char,
    cw: usize,
    pending: &mut String,
    pending_width: &mut usize,
    style: Style,
    rows: &mut Vec<Vec<Span<'static>>>,
    row_width: &mut usize,
    content_w: usize,
) {
    let here = *row_width + *pending_width;
    if here + cw > content_w && here > 0 {
        flush_pending(pending, pending_width, style, rows, row_width);
        rows.push(Vec::new());
        *row_width = 0;
    }
    pending.push(c);
    *pending_width += cw;
}

fn flush_pending(
    pending: &mut String,
    pending_width: &mut usize,
    style: Style,
    rows: &mut [Vec<Span<'static>>],
    row_width: &mut usize,
) {
    if pending.is_empty() {
        return;
    }
    let taken = std::mem::take(pending);
    *row_width += *pending_width;
    *pending_width = 0;
    if let Some(last) = rows.last_mut() {
        last.push(Span::styled(taken, style));
    }
}

pub struct InputRender {
    pub lines: Vec<Line<'static>>,
    pub cursor_row: u16,
    pub cursor_col: u16,
}

#[cfg(test)]
mod submit_segments_tests {
    use super::*;
    use protocol::Segment;

    #[test]
    fn pure_text_collapses_to_one_text_segment() {
        let mut buf = InputBuffer::new();
        for c in "hello".chars() {
            buf.insert_char(c);
        }
        let segs = buf.submit_segments();
        assert_eq!(segs.len(), 1);
        match &segs[0] {
            Segment::Text { content } => assert_eq!(content, "hello"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn paste_emits_separate_segment_with_metadata() {
        let mut buf = InputBuffer::new();
        for c in "see ".chars() {
            buf.insert_char(c);
        }
        buf.insert_paste("line1\nline2".into());
        for c in " end".chars() {
            buf.insert_char(c);
        }
        let segs = buf.submit_segments();
        assert_eq!(segs.len(), 3);
        match &segs[0] {
            Segment::Text { content } => assert_eq!(content, "see "),
            other => panic!("expected Text, got {other:?}"),
        }
        match &segs[1] {
            Segment::Paste {
                chars,
                lines,
                content,
                ..
            } => {
                assert_eq!(content, "line1\nline2");
                assert_eq!(*chars, "line1\nline2".chars().count() as u32);
                assert_eq!(*lines, 2);
            }
            other => panic!("expected Paste, got {other:?}"),
        }
        match &segs[2] {
            Segment::Text { content } => assert_eq!(content, " end"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn empty_buffer_yields_empty_segments() {
        let buf = InputBuffer::new();
        assert!(buf.submit_segments().is_empty());
    }

    #[test]
    fn leading_paste_does_not_emit_empty_text() {
        let mut buf = InputBuffer::new();
        buf.insert_paste("X".into());
        let segs = buf.submit_segments();
        assert_eq!(segs.len(), 1);
        assert!(matches!(segs[0], Segment::Paste { .. }));
    }
}
