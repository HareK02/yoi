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
//! back to their original captured content so the Worker sees the full
//! pasted text (without the placeholder label).

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

pub const MAX_PLAIN_TEXT_PASTE_CHARS: usize = 50;
pub const MAX_PLAIN_TEXT_PASTE_LOGICAL_LINES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasteMeasurement {
    pub chars: usize,
    pub logical_lines: usize,
}

impl PasteMeasurement {
    pub fn presentation(self) -> PastePresentation {
        if self.chars <= MAX_PLAIN_TEXT_PASTE_CHARS
            && self.logical_lines <= MAX_PLAIN_TEXT_PASTE_LOGICAL_LINES
        {
            PastePresentation::Text
        } else {
            PastePresentation::Chip
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PastePresentation {
    Text,
    Chip,
}

pub fn measure_paste(content: &str) -> PasteMeasurement {
    PasteMeasurement {
        chars: content.chars().count(),
        logical_lines: logical_line_count(content),
    }
}

/// Empty content has zero logical lines. Otherwise LF, lone CR, and CRLF each
/// advance one line; a CRLF pair is one break rather than two.
pub fn logical_line_count(content: &str) -> usize {
    if content.is_empty() {
        return 0;
    }

    let mut lines = 1;
    let mut chars = content.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                lines += 1;
            }
            '\n' => lines += 1,
            _ => {}
        }
    }
    lines
}

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
/// Directories remain valid chips because Worker resolves normal directory refs
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

#[derive(Debug, Clone)]
pub struct FlowRefAtom {
    pub selector: String,
}

impl FlowRefAtom {
    pub fn label(&self) -> String {
        format!("[Flow: {}]", self.selector)
    }
}

#[derive(Debug, Clone)]
pub enum Atom {
    Char(char),
    Paste(PasteRef),
    PasteArtifact(protocol::PasteArtifactRef),
    FileRef(FileRefAtom),
    FlowRef(FlowRefAtom),
}

impl Atom {
    /// Style + visible label for atoms that render as a single
    /// indivisible chip. Returns `None` for `Atom::Char`.
    fn chip(&self) -> Option<(Style, String)> {
        match self {
            Atom::Char(_) => None,
            Atom::Paste(p) => Some((Style::default().fg(Color::Magenta), p.label())),
            Atom::PasteArtifact(artifact) => Some((
                Style::default().fg(Color::Magenta),
                format!(
                    "[Paste artifact {} | {} chars, {} lines, {}, {}, created {} ms]",
                    artifact.artifact_id,
                    artifact.char_count,
                    artifact.line_count,
                    artifact.media_type.as_str(),
                    artifact.availability.as_str(),
                    artifact.created_at_ms
                ),
            )),
            Atom::FileRef(r) => Some((Style::default().fg(Color::Cyan), r.label())),
            Atom::FlowRef(r) => Some((Style::default().fg(Color::Yellow), r.label())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AtomClass {
    Word(WordKind),
    Sep,
    /// Indivisible chip — paste / file ref. Word motion treats one chip as one unit; deletion
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
        Atom::Paste(_) | Atom::PasteArtifact(_) | Atom::FileRef(_) | Atom::FlowRef(_) => {
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
    /// Top wrapped row of the visible composer viewport.
    scroll_offset: usize,
    /// Monotonic counter reused across the TUI process lifetime.
    next_paste_id: u32,
}

impl Default for InputBuffer {
    fn default() -> Self {
        Self {
            atoms: Vec::new(),
            cursor: 0,
            scroll_offset: 0,
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
        self.scroll_offset = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.atoms.is_empty()
    }

    pub fn cursor_at_start(&self) -> bool {
        self.cursor == 0
    }

    pub fn cursor_at_end(&self) -> bool {
        self.cursor == self.atoms.len()
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
                protocol::Segment::PasteArtifact { artifact } => {
                    self.atoms.push(Atom::PasteArtifact(artifact.clone()));
                }
                protocol::Segment::FileRef { path } => {
                    self.atoms
                        .push(Atom::FileRef(FileRefAtom { path: path.clone() }));
                }
                protocol::Segment::Flow { selector } => {
                    self.atoms.push(Atom::FlowRef(FlowRefAtom {
                        selector: selector.clone(),
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
                Atom::PasteArtifact(artifact) => {
                    text.push_str(&protocol::Segment::flatten_to_text(&[
                        protocol::Segment::PasteArtifact {
                            artifact: artifact.clone(),
                        },
                    ]))
                }
                Atom::FileRef(file) => text.push_str(&file.path),
                Atom::FlowRef(flow) => text.push_str(&flow.selector),
            }
        }
        text
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    pub fn insert_paste(&mut self, content: String) {
        let measurement = measure_paste(&content);
        if measurement.presentation() == PastePresentation::Text {
            let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
            self.insert_str(&normalized);
            return;
        }

        let id = self.next_paste_id;
        self.next_paste_id = self.next_paste_id.wrapping_add(1);
        self.atoms.insert(
            self.cursor,
            Atom::Paste(PasteRef {
                id,
                chars: measurement.chars,
                lines: measurement.logical_lines,
                content,
            }),
        );
        self.cursor += 1;
    }

    /// Replace `atoms[start..self.cursor]` (the in-flight `@<typed>` /
    /// active `@<typed>` file token) with the corresponding chip atom
    /// and place the cursor right after the chip. Used by the completion
    /// confirm path.
    pub fn replace_with_file_ref(&mut self, start: usize, path: String) {
        self.atoms.drain(start..self.cursor);
        self.atoms
            .insert(start, Atom::FileRef(FileRefAtom { path }));
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

    /// If the cursor is currently inside a `@<typed>` /
    /// `/<typed>` token that satisfies the trigger rules, return the
    /// kind, the index of the leading sigil atom, and the typed text
    /// after the sigil (sigil itself excluded).
    ///
    /// Trigger rules:
    /// - The sigil (`@` / `#`) must be preceded by start-of-input,
    ///   whitespace, or another chip atom — otherwise this is normal
    ///   text (e.g. the `/` in `src/main.rs` is not a completion trigger).
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
    /// chip atom (`Paste` / `FileRef`)
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
                Atom::PasteArtifact(artifact) => {
                    flush_text(&mut buf, &mut out);
                    out.push(protocol::Segment::PasteArtifact {
                        artifact: artifact.clone(),
                    });
                }
                Atom::FileRef(r) => {
                    flush_text(&mut buf, &mut out);
                    out.push(protocol::Segment::FileRef {
                        path: r.path.clone(),
                    });
                }
                Atom::FlowRef(r) => {
                    flush_text(&mut buf, &mut out);
                    out.push(protocol::Segment::Flow {
                        selector: r.selector.clone(),
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
            viewport_start_row: 0,
        }
    }

    /// Clip a full render to `visible_height` rows, updating the stored
    /// vertical scroll offset just enough to keep the cursor row visible.
    pub fn apply_cursor_viewport(&mut self, render: &mut InputRender, visible_height: u16) {
        let height = visible_height.max(1) as usize;
        let total_rows = render.lines.len().max(1);
        let max_offset = total_rows.saturating_sub(height);
        self.scroll_offset = self.scroll_offset.min(max_offset);

        let cursor_row = render.cursor_row as usize;
        if cursor_row < self.scroll_offset {
            self.scroll_offset = cursor_row;
        } else if cursor_row >= self.scroll_offset.saturating_add(height) {
            self.scroll_offset = cursor_row.saturating_add(1).saturating_sub(height);
        }
        self.scroll_offset = self.scroll_offset.min(max_offset);
        render.apply_viewport(self.scroll_offset, height);
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
    /// First wrapped row included in `lines` after viewport clipping.
    pub viewport_start_row: u16,
}

impl InputRender {
    fn apply_viewport(&mut self, offset: usize, height: usize) {
        let offset = offset.min(self.lines.len().saturating_sub(1));
        self.viewport_start_row = offset as u16;
        self.cursor_row = self.cursor_row.saturating_sub(self.viewport_start_row);
        let lines = std::mem::take(&mut self.lines);
        self.lines = lines.into_iter().skip(offset).take(height).collect();
        if self.lines.is_empty() {
            self.lines.push(Line::raw(""));
        }
    }
}

#[cfg(test)]
mod render_viewport_tests {
    use super::*;

    fn buf_from(text: &str) -> InputBuffer {
        let mut buf = InputBuffer::new();
        for c in text.chars() {
            buf.insert_char(c);
        }
        buf
    }

    fn render_lines(buf: &mut InputBuffer, width: u16, height: u16) -> Vec<String> {
        let mut render = buf.render(width);
        buf.apply_cursor_viewport(&mut render, height);
        render
            .lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn short_input_rendering_stays_unscrolled() {
        let mut buf = buf_from("one\ntwo");
        let mut render = buf.render(20);
        buf.apply_cursor_viewport(&mut render, 5);

        assert_eq!(buf.scroll_offset, 0);
        assert_eq!(render.viewport_start_row, 0);
        assert_eq!(render.cursor_row, 1);
        assert_eq!(render.cursor_col, 3);
        assert_eq!(render_lines(&mut buf, 20, 5), ["one", "two"]);
    }

    #[test]
    fn input_viewport_follows_cursor_at_bottom() {
        let mut buf = buf_from("0\n1\n2\n3\n4");
        let mut render = buf.render(20);
        buf.apply_cursor_viewport(&mut render, 3);

        assert_eq!(buf.scroll_offset, 2);
        assert_eq!(render.viewport_start_row, 2);
        assert_eq!(render.cursor_row, 2);
        assert_eq!(render.cursor_col, 1);
        assert_eq!(render_lines(&mut buf, 20, 3), ["2", "3", "4"]);
    }

    #[test]
    fn input_viewport_scrolls_when_cursor_moves_above_or_below() {
        let mut buf = buf_from("0\n1\n2\n3\n4");
        assert_eq!(render_lines(&mut buf, 20, 3), ["2", "3", "4"]);
        assert_eq!(buf.scroll_offset, 2);

        buf.move_up();
        assert_eq!(render_lines(&mut buf, 20, 3), ["2", "3", "4"]);
        assert_eq!(buf.scroll_offset, 2);

        buf.move_up();
        assert_eq!(render_lines(&mut buf, 20, 3), ["2", "3", "4"]);
        assert_eq!(buf.scroll_offset, 2);

        buf.move_up();
        assert_eq!(render_lines(&mut buf, 20, 3), ["1", "2", "3"]);
        assert_eq!(buf.scroll_offset, 1);

        buf.move_down();
        assert_eq!(render_lines(&mut buf, 20, 3), ["1", "2", "3"]);
        assert_eq!(buf.scroll_offset, 1);

        buf.move_down();
        assert_eq!(render_lines(&mut buf, 20, 3), ["1", "2", "3"]);
        assert_eq!(buf.scroll_offset, 1);

        buf.move_down();
        assert_eq!(render_lines(&mut buf, 20, 3), ["2", "3", "4"]);
        assert_eq!(buf.scroll_offset, 2);
    }

    #[test]
    fn input_viewport_clamps_after_line_deletion() {
        let mut buf = buf_from("0\n1\n2\n3\n4\n5");
        assert_eq!(render_lines(&mut buf, 20, 3), ["3", "4", "5"]);
        assert_eq!(buf.scroll_offset, 3);

        for _ in 0..6 {
            buf.delete_before();
        }
        assert_eq!(render_lines(&mut buf, 20, 3), ["0", "1", "2"]);
        assert_eq!(buf.scroll_offset, 0);
    }
}

#[cfg(test)]
mod paste_policy_tests {
    use super::*;
    use protocol::Segment;
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct Fixture {
        max_plain_text_chars: usize,
        max_plain_text_logical_lines: usize,
        cases: Vec<FixtureCase>,
    }

    #[derive(Debug, Deserialize)]
    struct FixtureCase {
        name: String,
        parts: Vec<FixturePart>,
        char_count: usize,
        logical_line_count: usize,
        presentation: FixturePresentation,
    }

    #[derive(Debug, Deserialize)]
    struct FixturePart {
        value: String,
        repeat: usize,
    }

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "lowercase")]
    enum FixturePresentation {
        Text,
        Chip,
    }

    fn fixture() -> Fixture {
        serde_json::from_str(include_str!(
            "../../../tests/fixtures/composer-paste-policy.json"
        ))
        .expect("shared composer paste policy fixture must be valid")
    }

    fn fixture_content(case: &FixtureCase) -> String {
        case.parts
            .iter()
            .map(|part| part.value.repeat(part.repeat))
            .collect()
    }

    #[test]
    fn tui_follows_shared_paste_presentation_contract() {
        let fixture = fixture();
        assert_eq!(fixture.max_plain_text_chars, MAX_PLAIN_TEXT_PASTE_CHARS);
        assert_eq!(
            fixture.max_plain_text_logical_lines,
            MAX_PLAIN_TEXT_PASTE_LOGICAL_LINES
        );

        for case in fixture.cases {
            let content = fixture_content(&case);
            let measurement = measure_paste(&content);
            let expected_presentation = match case.presentation {
                FixturePresentation::Text => PastePresentation::Text,
                FixturePresentation::Chip => PastePresentation::Chip,
            };
            assert_eq!(measurement.chars, case.char_count, "{} chars", case.name);
            assert_eq!(
                measurement.logical_lines, case.logical_line_count,
                "{} logical lines",
                case.name
            );
            assert_eq!(
                measurement.presentation(),
                expected_presentation,
                "{} presentation",
                case.name
            );
        }
    }

    #[test]
    fn short_paste_is_editable_text_at_the_cursor() {
        let mut buffer = InputBuffer::new();
        buffer.insert_str("ac");
        buffer.move_left();
        buffer.insert_paste("b".to_owned());

        assert_eq!(buffer.plain_text(), "abc");
        assert!(
            buffer
                .atoms
                .iter()
                .all(|atom| matches!(atom, Atom::Char(_)))
        );
        assert_eq!(
            buffer.submit_segments(),
            vec![Segment::text("abc".to_owned())]
        );
    }

    #[test]
    fn short_multiline_paste_normalizes_line_endings_as_text() {
        let mut buffer = InputBuffer::new();
        buffer.insert_paste("a\r\nb\rc".to_owned());

        assert_eq!(buffer.plain_text(), "a\nb\nc");
        assert!(
            buffer
                .atoms
                .iter()
                .all(|atom| matches!(atom, Atom::Char(_)))
        );
        assert_eq!(
            buffer.submit_segments(),
            vec![Segment::text("a\nb\nc".to_owned())]
        );
    }

    #[test]
    fn empty_paste_is_a_noop() {
        let mut buffer = InputBuffer::new();
        buffer.insert_str("unchanged");
        let paste_id = buffer.next_paste_id;
        buffer.insert_paste(String::new());

        assert_eq!(buffer.plain_text(), "unchanged");
        assert_eq!(buffer.next_paste_id, paste_id);
    }
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
        let pasted = "line1\nline2\nline3\nline4";
        buf.insert_paste(pasted.into());
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
                assert_eq!(content, pasted);
                assert_eq!(*chars, pasted.chars().count() as u32);
                assert_eq!(*lines, 4);
            }
            other => panic!("expected Paste, got {other:?}"),
        }
        match &segs[2] {
            Segment::Text { content } => assert_eq!(content, " end"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn restored_direct_paste_remains_a_typed_segment_without_reclassification() {
        let original = Segment::Paste {
            id: 7,
            chars: 1,
            lines: 1,
            content: "x".to_owned(),
        };
        let mut buf = InputBuffer::new();
        buf.replace_with_segments(std::slice::from_ref(&original));

        assert_eq!(buf.submit_segments(), vec![original]);
    }

    #[test]
    fn restored_paste_artifact_remains_a_typed_segment() {
        let artifact = protocol::PasteArtifactRef {
            artifact_id: "019ca7c8-57b6-7f05-8edf-524147aba7b2".to_string(),
            created_at_ms: 1_700_000_000_000,
            media_type: protocol::PasteArtifactMediaType::TextPlainUtf8,
            availability: protocol::PasteArtifactAvailability::Available,
            byte_len: 65_536,
            char_count: 65_530,
            line_count: 200,
            sha256: "a".repeat(64),
            source_entry_id: "entry-1".to_string(),
        };
        let original = Segment::PasteArtifact {
            artifact: artifact.clone(),
        };
        let mut buf = InputBuffer::new();
        buf.replace_with_segments(std::slice::from_ref(&original));

        assert_eq!(
            buf.submit_segments(),
            vec![Segment::PasteArtifact { artifact }]
        );
    }

    #[test]
    fn empty_buffer_yields_empty_segments() {
        let buf = InputBuffer::new();
        assert!(buf.submit_segments().is_empty());
    }

    #[test]
    fn leading_paste_does_not_emit_empty_text() {
        let mut buf = InputBuffer::new();
        buf.insert_paste("X".repeat(MAX_PLAIN_TEXT_PASTE_CHARS + 1));
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
    fn slash_inside_path_is_not_a_completion_trigger() {
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
        buf.insert_paste("X".repeat(MAX_PLAIN_TEXT_PASTE_CHARS + 1));
        for c in "@sr".chars() {
            buf.insert_char(c);
        }
        let (kind, start, prefix) = buf.pending_completion_prefix().unwrap();
        assert_eq!(kind, CompletionKind::File);
        assert_eq!(start, 1); // chip at 0, sigil at 1
        assert_eq!(prefix, "sr");
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
        buf.insert_paste("anything".repeat(MAX_PLAIN_TEXT_PASTE_CHARS + 1));
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
                Atom::Paste(_) | Atom::PasteArtifact(_) | Atom::FileRef(_) | Atom::FlowRef(_) => {
                    out.push_str("<P>")
                }
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
        buf.insert_paste("anything".repeat(MAX_PLAIN_TEXT_PASTE_CHARS + 1));
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
