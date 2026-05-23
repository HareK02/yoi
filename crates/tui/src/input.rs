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

/// `@<path>` chip — confirmed completion of a file-system reference.
/// Directories remain valid chips because Pod resolves normal directory refs
/// to shallow `[Dir: <path>]` listings at submit time.
#[derive(Debug, Clone)]
pub struct FileRefAtom {
    pub path: String,
}

impl FileRefAtom {
    pub fn label(&self) -> String {
        format!("@{}", self.path)
    }
}

/// `#<slug>` chip — confirmed completion of a Knowledge reference.
#[derive(Debug, Clone)]
pub struct KnowledgeRefAtom {
    pub slug: String,
}

impl KnowledgeRefAtom {
    pub fn label(&self) -> String {
        format!("#{}", self.slug)
    }
}

/// `/<slug>` chip — confirmed completion of a Workflow invocation.
#[derive(Debug, Clone)]
pub struct WorkflowInvokeAtom {
    pub slug: String,
}

impl WorkflowInvokeAtom {
    pub fn label(&self) -> String {
        format!("/{}", self.slug)
    }
}

#[derive(Debug, Clone)]
pub enum Atom {
    Char(char),
    Paste(PasteRef),
    FileRef(FileRefAtom),
    KnowledgeRef(KnowledgeRefAtom),
    WorkflowInvoke(WorkflowInvokeAtom),
}

impl Atom {
    /// Style + visible label for atoms that render as a single
    /// indivisible chip. Returns `None` for `Atom::Char`.
    fn chip(&self) -> Option<(Style, String)> {
        match self {
            Atom::Char(_) => None,
            Atom::Paste(p) => Some((Style::default().fg(Color::Magenta), p.label())),
            Atom::FileRef(r) => Some((Style::default().fg(Color::Cyan), r.label())),
            Atom::KnowledgeRef(r) => Some((Style::default().fg(Color::Green), r.label())),
            Atom::WorkflowInvoke(r) => Some((Style::default().fg(Color::Yellow), r.label())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AtomClass {
    Word(WordKind),
    Sep,
    /// Indivisible chip — paste / file ref / knowledge ref / workflow
    /// invocation. Word motion treats one chip as one unit; deletion
    /// removes the whole atom.
    Chip,
}

/// Sub-classification of word atoms. A run of equal `WordKind` is one word;
/// a kind switch is a word boundary. Lets `Ctrl+Left/Right` step over
/// runs of hiragana/katakana/han/ASCII independently when they sit adjacent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WordKind {
    Ascii,
    Hiragana,
    Katakana,
    Han,
    Other,
}

fn atom_class(atom: &Atom) -> AtomClass {
    match atom {
        Atom::Char(c) => char_class(*c),
        Atom::Paste(_) | Atom::FileRef(_) | Atom::KnowledgeRef(_) | Atom::WorkflowInvoke(_) => {
            AtomClass::Chip
        }
    }
}

fn char_class(c: char) -> AtomClass {
    if c.is_ascii_alphanumeric() || c == '_' {
        return AtomClass::Word(WordKind::Ascii);
    }
    let cp = c as u32;
    match cp {
        0x3040..=0x309F => AtomClass::Word(WordKind::Hiragana),
        0x30A0..=0x30FF | 0x31F0..=0x31FF | 0xFF65..=0xFF9F => AtomClass::Word(WordKind::Katakana),
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FFFF => {
            AtomClass::Word(WordKind::Han)
        }
        _ if c.is_alphanumeric() => AtomClass::Word(WordKind::Other),
        _ => AtomClass::Sep,
    }
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

    pub fn is_empty(&self) -> bool {
        self.atoms.is_empty()
    }

    /// Replace the whole composer with protocol segments previously emitted
    /// by [`submit_segments`](Self::submit_segments), preserving typed chips
    /// and placing the cursor at the end of the restored input.
    pub fn replace_with_segments(&mut self, segments: &[protocol::Segment]) {
        self.atoms.clear();
        for segment in segments {
            match segment {
                protocol::Segment::Text { content } => {
                    self.atoms.extend(content.chars().map(Atom::Char));
                }
                protocol::Segment::Paste {
                    id,
                    chars,
                    lines,
                    content,
                } => {
                    self.next_paste_id = self.next_paste_id.max(id.saturating_add(1).max(1));
                    self.atoms.push(Atom::Paste(PasteRef {
                        id: *id,
                        chars: *chars as usize,
                        lines: *lines as usize,
                        content: content.clone(),
                    }));
                }
                protocol::Segment::FileRef { path } => {
                    self.atoms
                        .push(Atom::FileRef(FileRefAtom { path: path.clone() }));
                }
                protocol::Segment::KnowledgeRef { slug } => {
                    self.atoms
                        .push(Atom::KnowledgeRef(KnowledgeRefAtom { slug: slug.clone() }));
                }
                protocol::Segment::WorkflowInvoke { slug } => {
                    self.atoms.push(Atom::WorkflowInvoke(WorkflowInvokeAtom {
                        slug: slug.clone(),
                    }));
                }
                protocol::Segment::Unknown => {
                    self.atoms
                        .extend("[unknown input segment]".chars().map(Atom::Char));
                }
            }
        }
        self.cursor = self.atoms.len();
    }

    pub fn insert_char(&mut self, c: char) {
        self.atoms.insert(self.cursor, Atom::Char(c));
        self.cursor += 1;
    }

    pub fn insert_str(&mut self, text: &str) {
        for c in text.chars() {
            self.insert_char(c);
        }
    }

    pub fn plain_text(&self) -> String {
        let mut text = String::new();
        for atom in &self.atoms {
            match atom {
                Atom::Char(c) => text.push(*c),
                Atom::Paste(paste) => text.push_str(&paste.content),
                Atom::FileRef(file) => text.push_str(&file.path),
                Atom::KnowledgeRef(knowledge) => text.push_str(&knowledge.slug),
                Atom::WorkflowInvoke(workflow) => text.push_str(&workflow.slug),
            }
        }
        text
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

    /// Replace `atoms[start..self.cursor]` (the in-flight `@<typed>` /
    /// `#<typed>` / `/<typed>` token) with the corresponding chip atom
    /// and place the cursor right after the chip. Used by the completion
    /// confirm path.
    pub fn replace_with_file_ref(&mut self, start: usize, path: String) {
        self.atoms.drain(start..self.cursor);
        self.atoms
            .insert(start, Atom::FileRef(FileRefAtom { path }));
        self.cursor = start + 1;
    }

    pub fn replace_with_knowledge_ref(&mut self, start: usize, slug: String) {
        self.atoms.drain(start..self.cursor);
        self.atoms
            .insert(start, Atom::KnowledgeRef(KnowledgeRefAtom { slug }));
        self.cursor = start + 1;
    }

    pub fn replace_with_workflow_invoke(&mut self, start: usize, slug: String) {
        self.atoms.drain(start..self.cursor);
        self.atoms
            .insert(start, Atom::WorkflowInvoke(WorkflowInvokeAtom { slug }));
        self.cursor = start + 1;
    }

    /// Replace `atoms[start..self.cursor]` with the chars of `text`,
    /// leaving cursor at the end of the inserted run. Used by the Tab
    /// completion path: the popup-selected entry is inserted as raw
    /// text (not a chip) so the user can keep typing — e.g. drill into
    /// a directory whose value ends with `/`.
    pub fn replace_with_text_at(&mut self, start: usize, text: &str) {
        self.atoms.drain(start..self.cursor);
        let mut idx = start;
        for c in text.chars() {
            self.atoms.insert(idx, Atom::Char(c));
            idx += 1;
        }
        self.cursor = idx;
    }

    /// If the cursor is currently inside a `@<typed>` / `#<typed>` /
    /// `/<typed>` token that satisfies the trigger rules, return the
    /// kind, the index of the leading sigil atom, and the typed text
    /// after the sigil (sigil itself excluded).
    ///
    /// Trigger rules:
    /// - The sigil (`@` / `#` / `/`) must be preceded by start-of-input,
    ///   whitespace, or another chip atom — otherwise this is normal
    ///   text (e.g. the `/` in `src/main.rs` is not a workflow trigger).
    /// - Whitespace, newlines and chip atoms invalidate an in-flight
    ///   token — `@foo /` closes the `@foo` candidate as soon as the
    ///   space lands.
    pub fn pending_completion_prefix(&self) -> Option<(protocol::CompletionKind, usize, String)> {
        if self.cursor == 0 {
            return None;
        }
        let mut typed = String::new();
        for i in (0..self.cursor).rev() {
            match &self.atoms[i] {
                Atom::Char(c) => {
                    if c.is_whitespace() {
                        return None;
                    }
                    let kind = match c {
                        '@' => Some(protocol::CompletionKind::File),
                        '#' => Some(protocol::CompletionKind::Knowledge),
                        '/' => Some(protocol::CompletionKind::Workflow),
                        _ => None,
                    };
                    if let Some(k) = kind {
                        let leading_ok = match self.atoms.get(i.wrapping_sub(1)).filter(|_| i > 0) {
                            None => true, // start of input
                            Some(Atom::Char(prev)) => prev.is_whitespace(),
                            Some(_) => true, // chip
                        };
                        if leading_ok {
                            return Some((k, i, typed));
                        }
                    }
                    typed.insert(0, *c);
                }
                _ => {
                    // Chip atoms cannot appear inside a candidate token.
                    return None;
                }
            }
        }
        None
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

    /// Delete one word backward — the same span [`move_word_left`] would
    /// jump over.
    pub fn delete_word_before(&mut self) {
        let end = self.cursor;
        self.move_word_left();
        let start = self.cursor;
        self.atoms.drain(start..end);
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.atoms.len());
    }

    /// Move backward by one word. Skips a run of separators, then a run of
    /// atoms sharing the same [`AtomClass`] — so `Word(Hiragana)` next to
    /// `Word(Han)` are separate blocks, and a `Paste` atom is its own block.
    pub fn move_word_left(&mut self) {
        while self.cursor > 0 && atom_class(&self.atoms[self.cursor - 1]) == AtomClass::Sep {
            self.cursor -= 1;
        }
        if self.cursor == 0 {
            return;
        }
        let kind = atom_class(&self.atoms[self.cursor - 1]);
        while self.cursor > 0 && atom_class(&self.atoms[self.cursor - 1]) == kind {
            self.cursor -= 1;
        }
    }

    /// Move forward by one word. Mirror of [`move_word_left`].
    pub fn move_word_right(&mut self) {
        while self.cursor < self.atoms.len()
            && atom_class(&self.atoms[self.cursor]) == AtomClass::Sep
        {
            self.cursor += 1;
        }
        if self.cursor == self.atoms.len() {
            return;
        }
        let kind = atom_class(&self.atoms[self.cursor]);
        while self.cursor < self.atoms.len() && atom_class(&self.atoms[self.cursor]) == kind {
            self.cursor += 1;
        }
    }

    pub fn move_start(&mut self) {
        self.cursor = 0;
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
    /// `Atom::Char`s are concatenated into a single `Segment::Text`; each
    /// chip atom (`Paste` / `FileRef` / `KnowledgeRef` / `WorkflowInvoke`)
    /// becomes a standalone `Segment` so that clients re-rendering an
    /// `Event::UserMessage` see the same indivisible chip rather than a
    /// flattened string.
    pub fn submit_segments(&self) -> Vec<protocol::Segment> {
        let mut out = Vec::new();
        let mut buf = String::new();
        let flush_text = |buf: &mut String, out: &mut Vec<protocol::Segment>| {
            if !buf.is_empty() {
                out.push(protocol::Segment::text(std::mem::take(buf)));
            }
        };
        for a in &self.atoms {
            match a {
                Atom::Char(c) => buf.push(*c),
                Atom::Paste(p) => {
                    flush_text(&mut buf, &mut out);
                    out.push(protocol::Segment::Paste {
                        id: p.id,
                        chars: p.chars as u32,
                        lines: p.lines as u32,
                        content: p.content.clone(),
                    });
                }
                Atom::FileRef(r) => {
                    flush_text(&mut buf, &mut out);
                    out.push(protocol::Segment::FileRef {
                        path: r.path.clone(),
                    });
                }
                Atom::KnowledgeRef(r) => {
                    flush_text(&mut buf, &mut out);
                    out.push(protocol::Segment::KnowledgeRef {
                        slug: r.slug.clone(),
                    });
                }
                Atom::WorkflowInvoke(r) => {
                    flush_text(&mut buf, &mut out);
                    out.push(protocol::Segment::WorkflowInvoke {
                        slug: r.slug.clone(),
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
                    other => other
                        .chip()
                        .and_then(|(_, label)| label.chars().next())
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
                other => {
                    let (chip_style, label) = other.chip().expect("non-char atom has a chip");
                    if pending_style != chip_style && !pending.is_empty() {
                        flush_pending(
                            &mut pending,
                            &mut pending_width,
                            pending_style,
                            &mut rows,
                            &mut row_width,
                        );
                    }
                    pending_style = chip_style;
                    for c in label.chars() {
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

    #[test]
    fn file_ref_chip_emits_file_ref_segment() {
        let mut buf = InputBuffer::new();
        for c in "see @sr".chars() {
            buf.insert_char(c);
        }
        buf.replace_with_file_ref(4, "src/main.rs".into());
        let segs = buf.submit_segments();
        assert_eq!(segs.len(), 2);
        assert!(matches!(&segs[0], Segment::Text { content } if content == "see "));
        match &segs[1] {
            Segment::FileRef { path } => assert_eq!(path, "src/main.rs"),
            other => panic!("expected FileRef, got {other:?}"),
        }
    }

    #[test]
    fn replace_with_file_ref_swallows_in_flight_token() {
        let mut buf = InputBuffer::new();
        for c in "see @sr".chars() {
            buf.insert_char(c);
        }
        // pending_completion_prefix returns the sigil index (4 = '@').
        let (_, start, prefix) = buf.pending_completion_prefix().unwrap();
        assert_eq!(start, 4);
        assert_eq!(prefix, "sr");
        buf.replace_with_file_ref(start, "src/main.rs".into());
        let segs = buf.submit_segments();
        assert_eq!(segs.len(), 2);
        assert!(matches!(&segs[0], Segment::Text { content } if content == "see "));
        assert!(matches!(&segs[1], Segment::FileRef { path } if path == "src/main.rs"));
    }

    #[test]
    fn knowledge_and_workflow_chips_emit_typed_segments() {
        let mut buf = InputBuffer::new();
        for c in "#r".chars() {
            buf.insert_char(c);
        }
        buf.replace_with_knowledge_ref(0, "rust-style".into());
        buf.insert_char(' ');
        for c in "/p".chars() {
            buf.insert_char(c);
        }
        buf.replace_with_workflow_invoke(2, "plan".into());
        let segs = buf.submit_segments();
        assert_eq!(segs.len(), 3);
        match &segs[0] {
            Segment::KnowledgeRef { slug } => assert_eq!(slug, "rust-style"),
            other => panic!("expected KnowledgeRef, got {other:?}"),
        }
        match &segs[1] {
            Segment::Text { content } => assert_eq!(content, " "),
            other => panic!("expected Text, got {other:?}"),
        }
        match &segs[2] {
            Segment::WorkflowInvoke { slug } => assert_eq!(slug, "plan"),
            other => panic!("expected WorkflowInvoke, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod completion_prefix_tests {
    use super::*;
    use protocol::CompletionKind;

    fn buf_from(text: &str) -> InputBuffer {
        let mut buf = InputBuffer::new();
        for c in text.chars() {
            buf.insert_char(c);
        }
        buf
    }

    #[test]
    fn at_sigil_at_start_triggers_file_completion() {
        let buf = buf_from("@sr");
        let (kind, start, prefix) = buf.pending_completion_prefix().unwrap();
        assert_eq!(kind, CompletionKind::File);
        assert_eq!(start, 0);
        assert_eq!(prefix, "sr");
    }

    #[test]
    fn sigil_after_space_triggers() {
        let buf = buf_from("see @x");
        let (kind, start, prefix) = buf.pending_completion_prefix().unwrap();
        assert_eq!(kind, CompletionKind::File);
        assert_eq!(start, 4);
        assert_eq!(prefix, "x");
    }

    #[test]
    fn slash_inside_path_is_not_a_workflow_trigger() {
        // After `@src/m`, the only valid trigger is `@`, not the `/`.
        let buf = buf_from("@src/m");
        let (kind, start, prefix) = buf.pending_completion_prefix().unwrap();
        assert_eq!(kind, CompletionKind::File);
        assert_eq!(start, 0);
        assert_eq!(prefix, "src/m");
    }

    #[test]
    fn space_after_sigil_invalidates_token() {
        // `@x ` — once a space lands after the typed text, the candidate
        // is gone (until the user types another sigil).
        let buf = buf_from("@x ");
        assert!(buf.pending_completion_prefix().is_none());
    }

    #[test]
    fn sigil_glued_to_word_is_not_a_trigger() {
        // `foo@bar` — `@` is preceded by a word char, so it stays plain
        // text (covers the case of email addresses and similar).
        let buf = buf_from("foo@bar");
        assert!(buf.pending_completion_prefix().is_none());
    }

    #[test]
    fn trigger_after_chip_atom() {
        let mut buf = InputBuffer::new();
        buf.insert_paste("X".into());
        for c in "@sr".chars() {
            buf.insert_char(c);
        }
        let (kind, start, prefix) = buf.pending_completion_prefix().unwrap();
        assert_eq!(kind, CompletionKind::File);
        assert_eq!(start, 1); // chip at 0, sigil at 1
        assert_eq!(prefix, "sr");
    }

    #[test]
    fn hash_sigil_triggers_knowledge_completion() {
        let buf = buf_from("#abc");
        let (kind, _, prefix) = buf.pending_completion_prefix().unwrap();
        assert_eq!(kind, CompletionKind::Knowledge);
        assert_eq!(prefix, "abc");
    }

    #[test]
    fn slash_at_start_triggers_workflow_completion() {
        let buf = buf_from("/cl");
        let (kind, _, prefix) = buf.pending_completion_prefix().unwrap();
        assert_eq!(kind, CompletionKind::Workflow);
        assert_eq!(prefix, "cl");
    }

    #[test]
    fn newline_before_cursor_invalidates_trigger() {
        let buf = buf_from("@a\nbc");
        assert!(buf.pending_completion_prefix().is_none());
    }
}

#[cfg(test)]
mod word_motion_tests {
    use super::*;

    fn buf_from(text: &str) -> InputBuffer {
        let mut buf = InputBuffer::new();
        for c in text.chars() {
            buf.insert_char(c);
        }
        buf
    }

    fn cursor(buf: &InputBuffer) -> usize {
        buf.cursor
    }

    #[test]
    fn empty_buffer_is_noop() {
        let mut buf = InputBuffer::new();
        buf.move_word_left();
        assert_eq!(cursor(&buf), 0);
        buf.move_word_right();
        assert_eq!(cursor(&buf), 0);
    }

    #[test]
    fn move_start_lands_at_beginning_of_buffer() {
        let mut buf = buf_from("foo\nbar");
        assert_eq!(cursor(&buf), 7);
        buf.move_start();
        assert_eq!(cursor(&buf), 0);
    }

    #[test]
    fn forward_from_start_lands_after_first_word() {
        let mut buf = buf_from("foo bar baz");
        buf.cursor = 0;
        buf.move_word_right();
        assert_eq!(cursor(&buf), 3); // after "foo"
        buf.move_word_right();
        assert_eq!(cursor(&buf), 7); // after "foo bar"
        buf.move_word_right();
        assert_eq!(cursor(&buf), 11); // after "foo bar baz"
        buf.move_word_right();
        assert_eq!(cursor(&buf), 11); // end stays put
    }

    #[test]
    fn backward_from_end_lands_at_last_word_start() {
        let mut buf = buf_from("foo bar baz");
        buf.move_word_left();
        assert_eq!(cursor(&buf), 8); // start of "baz"
        buf.move_word_left();
        assert_eq!(cursor(&buf), 4); // start of "bar"
        buf.move_word_left();
        assert_eq!(cursor(&buf), 0); // start of "foo"
        buf.move_word_left();
        assert_eq!(cursor(&buf), 0);
    }

    #[test]
    fn skips_runs_of_separators() {
        let mut buf = buf_from("a  ,  b");
        buf.cursor = 1; // just after "a"
        buf.move_word_right();
        assert_eq!(cursor(&buf), 7); // after "b"
        buf.move_word_left();
        assert_eq!(cursor(&buf), 6); // start of "b"
        buf.move_word_left();
        assert_eq!(cursor(&buf), 0); // start of "a"
    }

    #[test]
    fn newline_is_a_separator() {
        let mut buf = buf_from("foo\nbar");
        buf.cursor = 0;
        buf.move_word_right();
        assert_eq!(cursor(&buf), 3);
        buf.move_word_right();
        assert_eq!(cursor(&buf), 7);
        buf.move_word_left();
        assert_eq!(cursor(&buf), 4);
        buf.move_word_left();
        assert_eq!(cursor(&buf), 0);
    }

    #[test]
    fn paste_counts_as_one_word() {
        let mut buf = InputBuffer::new();
        for c in "foo ".chars() {
            buf.insert_char(c);
        }
        buf.insert_paste("anything".into());
        for c in " bar".chars() {
            buf.insert_char(c);
        }
        // atoms: f o o ' ' [P] ' ' b a r  → 9 atoms, paste at index 4
        let end = 9;
        buf.cursor = end;
        buf.move_word_left();
        assert_eq!(cursor(&buf), 6); // start of "bar"
        buf.move_word_left();
        assert_eq!(cursor(&buf), 4); // before paste
        buf.move_word_left();
        assert_eq!(cursor(&buf), 0); // start of "foo"

        buf.cursor = 0;
        buf.move_word_right();
        assert_eq!(cursor(&buf), 3); // after "foo"
        buf.move_word_right();
        assert_eq!(cursor(&buf), 5); // after paste
        buf.move_word_right();
        assert_eq!(cursor(&buf), 9); // after "bar"
    }

    #[test]
    fn underscore_is_a_word_char() {
        let mut buf = buf_from("foo_bar baz");
        buf.cursor = 0;
        buf.move_word_right();
        assert_eq!(cursor(&buf), 7); // "foo_bar" is one word
    }

    #[test]
    fn hiragana_run_is_one_word() {
        // "こんにちは" — 5 hiragana atoms, no separators.
        let mut buf = buf_from("こんにちは");
        buf.cursor = 0;
        buf.move_word_right();
        assert_eq!(cursor(&buf), 5);
        buf.move_word_left();
        assert_eq!(cursor(&buf), 0);
    }

    #[test]
    fn script_switch_is_a_word_boundary() {
        // 漢字 | ひらがな | ASCII
        let mut buf = buf_from("日本語のtest");
        buf.cursor = 0;
        buf.move_word_right();
        assert_eq!(cursor(&buf), 3); // after "日本語"
        buf.move_word_right();
        assert_eq!(cursor(&buf), 4); // after "の"
        buf.move_word_right();
        assert_eq!(cursor(&buf), 8); // after "test"

        buf.move_word_left();
        assert_eq!(cursor(&buf), 4); // start of "test"
        buf.move_word_left();
        assert_eq!(cursor(&buf), 3); // start of "の"
        buf.move_word_left();
        assert_eq!(cursor(&buf), 0); // start of "日本語"
    }

    #[test]
    fn halfwidth_katakana_is_treated_as_katakana() {
        // 半角カナ「ｱｲｳｴｵ」は5 atom、すべて Katakana 種別。
        let mut buf = buf_from("ｱｲｳｴｵfoo");
        buf.cursor = 0;
        buf.move_word_right();
        assert_eq!(cursor(&buf), 5); // after "ｱｲｳｴｵ"
        buf.move_word_right();
        assert_eq!(cursor(&buf), 8); // after "foo"

        // 全角と半角のカタカナは同じ Katakana 種別なので1単語につながる。
        let mut buf2 = buf_from("カタｶﾅ");
        buf2.cursor = 0;
        buf2.move_word_right();
        assert_eq!(cursor(&buf2), 4);
    }

    #[test]
    fn katakana_separates_from_ascii() {
        let mut buf = buf_from("カタカナsecret");
        buf.cursor = 0;
        buf.move_word_right();
        assert_eq!(cursor(&buf), 4); // after "カタカナ"
        buf.move_word_right();
        assert_eq!(cursor(&buf), 10); // after "secret"
        buf.move_word_left();
        assert_eq!(cursor(&buf), 4);
        buf.move_word_left();
        assert_eq!(cursor(&buf), 0);
    }

    /// Render atoms as a string for assertions; chip atoms become `<P>`.
    fn as_text(buf: &InputBuffer) -> String {
        let mut out = String::new();
        for a in &buf.atoms {
            match a {
                Atom::Char(c) => out.push(*c),
                Atom::Paste(_)
                | Atom::FileRef(_)
                | Atom::KnowledgeRef(_)
                | Atom::WorkflowInvoke(_) => out.push_str("<P>"),
            }
        }
        out
    }

    #[test]
    fn delete_word_removes_trailing_word_at_end() {
        let mut buf = buf_from("foo bar");
        buf.delete_word_before();
        assert_eq!(as_text(&buf), "foo ");
        assert_eq!(cursor(&buf), 4);
    }

    #[test]
    fn delete_word_removes_word_at_cursor() {
        let mut buf = buf_from("foo bar");
        buf.cursor = 3; // right after "foo"
        buf.delete_word_before();
        assert_eq!(as_text(&buf), " bar");
        assert_eq!(cursor(&buf), 0);
    }

    #[test]
    fn delete_word_swallows_trailing_separators() {
        let mut buf = buf_from("foo   ");
        buf.delete_word_before();
        assert_eq!(as_text(&buf), "");
        assert_eq!(cursor(&buf), 0);
    }

    #[test]
    fn delete_word_at_start_is_noop() {
        let mut buf = buf_from("foo");
        buf.cursor = 0;
        buf.delete_word_before();
        assert_eq!(as_text(&buf), "foo");
        assert_eq!(cursor(&buf), 0);
    }

    #[test]
    fn delete_word_respects_script_boundary() {
        // 「日本語の」末尾から1回削除すると、ひらがな部分「の」だけ消える
        let mut buf = buf_from("日本語の");
        buf.delete_word_before();
        assert_eq!(as_text(&buf), "日本語");
        assert_eq!(cursor(&buf), 3);
        buf.delete_word_before();
        assert_eq!(as_text(&buf), "");
        assert_eq!(cursor(&buf), 0);
    }

    #[test]
    fn delete_word_treats_paste_as_one_unit() {
        let mut buf = InputBuffer::new();
        for c in "foo ".chars() {
            buf.insert_char(c);
        }
        buf.insert_paste("anything".into());
        for c in " bar".chars() {
            buf.insert_char(c);
        }
        // atoms: f o o ' ' [P] ' ' b a r  (cursor at end = 9)
        buf.delete_word_before();
        assert_eq!(as_text(&buf), "foo <P> ");
        assert_eq!(cursor(&buf), 6);
        // Next deletion: trailing space then the paste atom (kind=Paste)
        buf.delete_word_before();
        assert_eq!(as_text(&buf), "foo ");
        assert_eq!(cursor(&buf), 4);
    }

    #[test]
    fn japanese_punctuation_is_a_separator() {
        // 「、」 (U+3001) and 「。」 (U+3002) are not word chars.
        let mut buf = buf_from("読んだ、走った。");
        buf.cursor = 0;
        buf.move_word_right();
        assert_eq!(cursor(&buf), 1); // after "読" (han run of 1)
        buf.move_word_right();
        assert_eq!(cursor(&buf), 3); // after "んだ" (hiragana run)
        // "、" is sep — skipped, then han "走"
        buf.move_word_right();
        assert_eq!(cursor(&buf), 5); // after "走"
        buf.move_word_right();
        assert_eq!(cursor(&buf), 7); // after "った"
    }
}
