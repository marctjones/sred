//! Source-anchored editing engine (M1).
//!
//! The **raw markdown text is the buffer** (a `ropey::Rope`); the rich view is a
//! parsed *projection* of it (see [`crate::view`]). Every edit — typing, delete,
//! toolbar action — splices the raw text, so `text()` is byte-lossless and
//! unedited regions never change. Block/inline styling is *derived* from the
//! source on each render; markers (`#`, `**`, `- `) stay in the text and are
//! styled in place ("live-preview-lite").
//!
//! This replaces the old structured model (`Vec<EditBlock>` + reconstructive
//! save), which normalized markdown and could not round-trip real notes.

use ropey::Rope;

use crate::model::{Format, MarkSet};

pub use crate::view::{Decoration, Span};

/// A host-agnostic accessibility snapshot of the editor (a multi-line text
/// field). Character offsets index into `value`. Produced by [`EditorCore::a11y`]
/// / [`crate::Editor::a11y`]; hosts attach it to their a11y backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct A11ySnapshot {
    /// The full document text (the accessible value).
    pub value: String,
    /// Caret position, in characters.
    pub caret: usize,
    /// Selection start/end, in characters (`start == end` ⇒ no selection).
    pub selection_start: usize,
    pub selection_end: usize,
    /// Always true here — sred is a multi-line editor.
    pub multiline: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    /// Previous word boundary (Ctrl+Left).
    WordLeft,
    /// Next word boundary (Ctrl+Right).
    WordRight,
    /// Start of the document (Ctrl+Home).
    DocStart,
    /// End of the document (Ctrl+End).
    DocEnd,
}

/// Paragraph-level kind, used by `SetBlock`/`ToggleBlock` to choose which marker
/// to write into the source line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Heading(u8),
    Bullet,
    Ordered,
    Quote,
    Code,
    Divider,
}

#[derive(Debug, Clone)]
pub enum Command {
    Insert(String),
    DeleteBackward,
    DeleteForward,
    DeleteSelection,
    /// Delete from the caret to the previous word boundary (Ctrl+Backspace).
    DeleteWordBackward,
    /// Delete from the caret to the next word boundary (Ctrl+Delete).
    DeleteWordForward,
    Move(Motion),
    Select(Motion),
    SelectAll,
    ToggleMark(MarkSet),
    /// View-only text color is not representable in markdown; kept as a no-op in
    /// the source-anchored model (color is not persisted).
    SetColor(Option<u32>),
    Link(String),
    SetBlock(BlockKind),
    ToggleBlock(BlockKind),
    Undo,
    Redo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditKind {
    None,
    InsertWord,
    InsertBoundary,
    Delete,
    Structure,
}

#[derive(Clone)]
struct Snapshot {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
}

/// In-flight IME composition. The preedit text is **never** stored in the rope
/// (so `text()` round-trips byte-for-byte mid-composition); it is injected only
/// into the rendered [`display_text`](EditorCore::display_text). `pos` is the rope
/// char index where it sits; `caret` is the caret offset within the preedit.
#[derive(Clone)]
struct Preedit {
    pos: usize,
    text: String,
    caret: usize,
}

pub struct EditorCore {
    rope: Rope,
    cursor: usize,        // char index into the rope
    anchor: Option<usize>,
    format: Format,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_kind: EditKind,
    preedit: Option<Preedit>,
}

impl EditorCore {
    pub fn new(format: Format) -> Self {
        EditorCore {
            rope: Rope::new(),
            cursor: 0,
            anchor: None,
            format,
            undo: Vec::new(),
            redo: Vec::new(),
            last_kind: EditKind::None,
            preedit: None,
        }
    }

    /// Load source text verbatim (byte-lossless).
    pub fn from_source(src: &str, format: Format) -> Self {
        let mut ed = EditorCore::new(format);
        ed.rope = Rope::from_str(src);
        ed.cursor = ed.rope.len_chars();
        ed
    }

    pub fn set_text(&mut self, src: &str) {
        self.rope = Rope::from_str(src);
        self.cursor = self.cursor.min(self.rope.len_chars());
        self.anchor = None;
        self.undo.clear();
        self.redo.clear();
        self.last_kind = EditKind::None;
    }

    pub fn format(&self) -> Format {
        self.format
    }
    pub fn set_format(&mut self, format: Format) {
        self.format = format;
    }
    pub fn cursor(&self) -> usize {
        self.cursor
    }
    pub fn len(&self) -> usize {
        self.rope.len_chars()
    }
    pub fn is_empty(&self) -> bool {
        self.rope.len_chars() == 0
    }

    /// The raw markdown — exactly what was loaded/typed, byte-for-byte.
    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// In the source-anchored model the source *is* the buffer.
    pub fn source(&self) -> String {
        self.text()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        self.selection_range()
    }

    pub fn selected_text(&self) -> String {
        match self.selection_range() {
            Some((s, e)) => self.rope.slice(s..e).to_string(),
            None => String::new(),
        }
    }

    pub fn set_cursor(&mut self, idx: usize) {
        self.anchor = None;
        self.cursor = idx.min(self.len());
        self.last_kind = EditKind::None;
    }

    pub fn extend_to(&mut self, idx: usize) {
        if self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }
        self.cursor = idx.min(self.len());
    }

    pub fn select_word_at(&mut self, idx: usize) {
        let (s, e) = self.word_range(idx);
        self.anchor = Some(s);
        self.cursor = e;
    }

    /// Select the whole line containing `idx` (including its trailing newline, so
    /// paste reinserts a clean line). Used by triple-click.
    pub fn select_line_at(&mut self, idx: usize) {
        let line = self.rope.char_to_line(idx.min(self.len()));
        let s = self.rope.line_to_char(line);
        let e = self.rope.line_to_char((line + 1).min(self.rope.len_lines())).min(self.len());
        self.anchor = Some(s);
        self.cursor = e;
        self.last_kind = EditKind::None;
    }

    // ---- clipboard contract (portable; host owns the system clipboard) -------

    /// The selected source text (empty if no selection) — what a host puts on the
    /// clipboard for Copy.
    pub fn copy(&self) -> String {
        self.selected_text()
    }

    /// Like [`copy`](Self::copy) but also deletes the selection — for Cut.
    pub fn cut(&mut self) -> String {
        let text = self.selected_text();
        if !text.is_empty() {
            self.checkpoint(EditKind::Delete);
            self.delete_selection();
        }
        text
    }

    /// Insert clipboard text at the caret (replacing any selection) — for Paste.
    pub fn paste(&mut self, text: &str) {
        self.checkpoint(EditKind::InsertBoundary);
        self.insert(text);
    }

    // ---- IME / preedit composition -----------------------------------------
    //
    // The preedit is kept out of the rope so `text()` stays byte-lossless during
    // composition; it is injected only into `display_text()` for rendering, with
    // its range available via `preedit_range()` for an underline decoration.

    /// Set/replace the in-flight IME composition. `caret` is the caret offset (in
    /// chars) within `text`. Starting a composition replaces any active selection.
    pub fn set_preedit(&mut self, text: &str, caret: usize) {
        if self.preedit.is_none() {
            // Begin: a composition replaces the selection (committed edit history).
            if self.selection_range().is_some() {
                self.checkpoint(EditKind::Delete);
                self.delete_selection();
            }
        }
        let pos = self.cursor;
        if text.is_empty() {
            self.preedit = None;
        } else {
            let caret = caret.min(text.chars().count());
            self.preedit = Some(Preedit { pos, text: text.to_string(), caret });
        }
    }

    /// Commit the composition (or `text` if given) as a real edit and end it.
    pub fn commit_preedit(&mut self, text: &str) {
        self.preedit = None;
        if !text.is_empty() {
            self.checkpoint(EditKind::InsertBoundary);
            self.insert(text);
        }
    }

    /// Cancel the composition with no insertion.
    pub fn clear_preedit(&mut self) {
        self.preedit = None;
    }

    pub fn has_preedit(&self) -> bool {
        self.preedit.is_some()
    }

    /// The preedit's char range in **display** space, for an underline decoration.
    pub fn preedit_range(&self) -> Option<(usize, usize)> {
        self.preedit
            .as_ref()
            .map(|p| (p.pos, p.pos + p.text.chars().count()))
    }

    /// The text to render: the buffer with any preedit injected at its position.
    /// Equals [`text`](Self::text) when not composing.
    pub fn display_text(&self) -> String {
        match &self.preedit {
            None => self.text(),
            Some(p) => {
                let mut s: String = self.rope.slice(..p.pos).to_string();
                s.push_str(&p.text);
                s.push_str(&self.rope.slice(p.pos..).to_string());
                s
            }
        }
    }

    /// Caret position in **display** space (inside the preedit while composing).
    pub fn display_cursor(&self) -> usize {
        match &self.preedit {
            None => self.cursor,
            Some(p) => p.pos + p.caret,
        }
    }

    /// Source line index of the display caret (for the styling projection).
    pub fn display_cursor_line(&self) -> usize {
        // char_to_line on the display text would need the injected rope; the
        // preedit is intra-line, so the buffer's caret line is correct.
        self.rope.char_to_line(self.display_cursor().min(self.len()))
    }

    // ---- accessibility ------------------------------------------------------

    /// A host-agnostic accessibility snapshot of the editor as a multi-line text
    /// field. Hosts map this onto their a11y backend (e.g. AccessKit). Offsets are
    /// **character** indices into `value`. See the `accesskit` feature for a
    /// ready-made node builder.
    pub fn a11y(&self) -> A11ySnapshot {
        let (sel_start, sel_end) = self.selection_range().unwrap_or((self.cursor, self.cursor));
        A11ySnapshot {
            value: self.text(),
            caret: self.cursor,
            selection_start: sel_start,
            selection_end: sel_end,
            multiline: true,
        }
    }

    /// Move the current selection's text to `target` (drag-and-drop). No-op
    /// without a selection or when the target is inside the selection.
    pub fn move_selection_to(&mut self, target: usize) {
        let Some((s, e)) = self.selection_range() else {
            return;
        };
        if target >= s && target <= e {
            return; // dropping inside the selection: nothing to do
        }
        let moved = self.rope.slice(s..e).to_string();
        self.checkpoint(EditKind::Structure);
        // Remove the source span first, then adjust the insert point if it sat
        // after the removed region.
        self.rope.remove(s..e);
        let dst = if target > e { target - (e - s) } else { target };
        self.rope.insert(dst, &moved);
        self.anchor = Some(dst);
        self.cursor = dst + moved.chars().count();
    }

    // ---- styling/decoration views (delegated to the view builder) ----------

    /// Source line index of the caret.
    pub fn cursor_line(&self) -> usize {
        self.rope.char_to_line(self.cursor)
    }

    pub fn styled_runs(&self, base: f32) -> (Vec<Span>, Vec<i32>) {
        crate::view::styled_runs(&self.text(), self.format, base, self.cursor_line(), &[], 0)
    }

    pub fn decorations(&self) -> Vec<(usize, usize, Decoration)> {
        crate::view::decorations(&self.text(), self.format)
    }

    // ---- links -------------------------------------------------------------

    pub fn link_at_cursor(&self) -> Option<String> {
        let text = self.text();
        crate::view::link_at(&text, self.cursor).map(|(_, url)| url)
    }

    pub fn update_link_at_cursor(&mut self, url: &str) -> bool {
        let text = self.text();
        if let Some((url_range, _)) = crate::view::link_at(&text, self.cursor) {
            self.checkpoint(EditKind::Structure);
            self.rope.remove(url_range.clone());
            self.rope.insert(url_range.start, url);
            true
        } else {
            false
        }
    }

    // ---- command application ----------------------------------------------

    pub fn apply(&mut self, cmd: Command) {
        // Any committed command cancels an in-flight composition.
        self.preedit = None;
        match cmd {
            Command::Insert(s) => {
                if s == "\n" && self.handle_enter() {
                    return;
                }
                let kind = if !s.is_empty() && s.chars().all(|c| c.is_alphanumeric()) {
                    EditKind::InsertWord
                } else {
                    EditKind::InsertBoundary
                };
                self.checkpoint(kind);
                self.insert(&s);
            }
            Command::DeleteBackward => {
                self.checkpoint(EditKind::Delete);
                self.delete_backward();
            }
            Command::DeleteForward => {
                self.checkpoint(EditKind::Delete);
                self.delete_forward();
            }
            Command::DeleteSelection => {
                self.checkpoint(EditKind::Delete);
                self.delete_selection();
            }
            Command::DeleteWordBackward => {
                self.checkpoint(EditKind::Delete);
                if !self.delete_selection() && self.cursor > 0 {
                    let to = self.word_left(self.cursor);
                    self.rope.remove(to..self.cursor);
                    self.cursor = to;
                }
            }
            Command::DeleteWordForward => {
                self.checkpoint(EditKind::Delete);
                if !self.delete_selection() && self.cursor < self.len() {
                    let to = self.word_right(self.cursor);
                    self.rope.remove(self.cursor..to);
                }
            }
            Command::Move(m) => {
                self.anchor = None;
                self.cursor = self.motion_target(m);
                self.last_kind = EditKind::None;
            }
            Command::Select(m) => {
                if self.anchor.is_none() {
                    self.anchor = Some(self.cursor);
                }
                self.cursor = self.motion_target(m);
                self.last_kind = EditKind::None;
            }
            Command::SelectAll => {
                self.anchor = Some(0);
                self.cursor = self.len();
                self.last_kind = EditKind::None;
            }
            Command::ToggleMark(m) => {
                self.checkpoint(EditKind::Structure);
                self.toggle_mark(m);
            }
            Command::SetColor(_) => { /* view-only; not representable in markdown */ }
            Command::Link(url) => {
                self.checkpoint(EditKind::Structure);
                self.make_link(&url);
            }
            Command::SetBlock(k) => {
                self.checkpoint(EditKind::Structure);
                self.set_block(k, false);
            }
            Command::ToggleBlock(k) => {
                self.checkpoint(EditKind::Structure);
                self.set_block(k, true);
            }
            Command::Undo => self.undo(),
            Command::Redo => self.redo(),
        }
    }

    // ---- primitive edits (splice the rope) --------------------------------

    fn insert(&mut self, s: &str) {
        let clean: String = s
            .chars()
            .filter(|c| *c == '\n' || (!c.is_control() && !is_private_use(*c)))
            .collect();
        if clean.is_empty() {
            return;
        }
        self.delete_selection();
        self.rope.insert(self.cursor, &clean);
        self.cursor += clean.chars().count();
    }

    fn delete_backward(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor > 0 {
            self.rope.remove(self.cursor - 1..self.cursor);
            self.cursor -= 1;
        }
    }

    fn delete_forward(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor < self.len() {
            self.rope.remove(self.cursor..self.cursor + 1);
        }
    }

    fn delete_selection(&mut self) -> bool {
        if let Some((s, e)) = self.selection_range() {
            self.rope.remove(s..e);
            self.cursor = s;
            self.anchor = None;
            true
        } else {
            false
        }
    }

    /// Enter with list/quote continuation + exit-on-empty. Returns true if it
    /// fully handled the key.
    fn handle_enter(&mut self) -> bool {
        if self.selection_range().is_some() {
            return false; // normal path replaces the selection
        }
        let line = self.rope.char_to_line(self.cursor);
        let ls = self.rope.line_to_char(line);
        let line_str = self.rope.line(line).to_string();
        let line_str = line_str.trim_end_matches('\n');
        let Some(marker) = leading_marker(line_str) else {
            return false;
        };
        // Content after the marker on this line.
        let content = &line_str[marker.byte_len.min(line_str.len())..];
        self.checkpoint(EditKind::Structure);
        if content.trim().is_empty() {
            // Exit the list: remove the marker, leaving an empty line.
            let marker_chars = line_str[..marker.byte_len].chars().count();
            self.rope.remove(ls..ls + marker_chars);
            self.cursor = ls;
        } else {
            // Continue the list on a new line with the same prefix.
            let cont = marker.continuation();
            self.rope.insert(self.cursor, "\n");
            self.cursor += 1;
            self.rope.insert(self.cursor, &cont);
            self.cursor += cont.chars().count();
        }
        true
    }

    fn toggle_mark(&mut self, m: MarkSet) {
        let pair = mark_delim(m);
        let plen = pair.chars().count();
        if let Some((s, e)) = self.selection_range() {
            let sel = self.rope.slice(s..e).to_string();
            let wrapped = sel.chars().count() >= 2 * plen
                && sel.starts_with(pair)
                && sel.ends_with(pair);
            if wrapped {
                let inner: String = {
                    let chars: Vec<char> = sel.chars().collect();
                    chars[plen..chars.len() - plen].iter().collect()
                };
                self.rope.remove(s..e);
                self.rope.insert(s, &inner);
                self.anchor = Some(s);
                self.cursor = s + inner.chars().count();
            } else {
                self.rope.insert(e, pair);
                self.rope.insert(s, pair);
                self.anchor = Some(s);
                self.cursor = e + 2 * plen;
            }
        } else {
            // Insert an empty pair and place the caret inside.
            self.rope.insert(self.cursor, pair);
            self.rope.insert(self.cursor + plen, pair);
            self.cursor += plen;
        }
    }

    fn make_link(&mut self, url: &str) {
        if let Some((s, e)) = self.selection_range() {
            let text = self.rope.slice(s..e).to_string();
            let replacement = format!("[{text}]({url})");
            self.rope.remove(s..e);
            self.rope.insert(s, &replacement);
            self.anchor = None;
            self.cursor = s + replacement.chars().count();
        } else {
            let snippet = "[text](https://)";
            self.rope.insert(self.cursor, snippet);
            self.cursor += snippet.chars().count();
        }
    }

    /// Rewrite the leading marker of every line touched by the selection (or the
    /// caret line) to express `kind`. `toggle` flips a matching kind back to a
    /// plain paragraph.
    fn set_block(&mut self, kind: BlockKind, toggle: bool) {
        let (s, e) = self.selection_range().unwrap_or((self.cursor, self.cursor));
        let first = self.rope.char_to_line(s);
        let last = self.rope.char_to_line(e);
        // Decide add-vs-remove once (based on the first line) so a multi-line
        // selection toggles consistently.
        let add = !(toggle && self.line_has_kind(first, kind));
        // Process bottom-up so earlier line offsets stay valid.
        for line in (first..=last).rev() {
            self.retarget_line(line, kind, add);
        }
        // Keep the caret sane.
        self.cursor = self.cursor.min(self.len());
        if let Some(a) = self.anchor {
            self.anchor = Some(a.min(self.len()));
        }
    }

    fn line_has_kind(&self, line: usize, kind: BlockKind) -> bool {
        let s = self.rope.line(line).to_string();
        line_kind(s.trim_end_matches('\n')) == kind
    }

    fn retarget_line(&mut self, line: usize, kind: BlockKind, add: bool) {
        let ls = self.rope.line_to_char(line);
        let raw = self.rope.line(line).to_string();
        let body_str = raw.trim_end_matches('\n');
        let strip = existing_marker_len(body_str); // chars of any current block marker
        let content: String = body_str.chars().skip(strip).collect();
        let new_prefix = if add { block_marker(kind) } else { String::new() };
        let divider = matches!(kind, BlockKind::Divider) && add;
        let new_line = if divider {
            "---".to_string()
        } else {
            format!("{new_prefix}{content}")
        };
        let old_chars = body_str.chars().count();
        // Replace [ls, ls+old_chars) with new_line, preserving the trailing '\n'.
        let delta_cursor = new_line.chars().count() as isize - old_chars as isize;
        self.rope.remove(ls..ls + old_chars);
        self.rope.insert(ls, &new_line);
        // Shift caret/anchor if they were at/after this line's content.
        let line_end = ls + old_chars;
        self.cursor = shift_offset(self.cursor, ls, line_end, delta_cursor);
        if let Some(a) = self.anchor {
            self.anchor = Some(shift_offset(a, ls, line_end, delta_cursor));
        }
    }

    fn selection_range(&self) -> Option<(usize, usize)> {
        self.anchor
            .filter(|a| *a != self.cursor)
            .map(|a| (a.min(self.cursor), a.max(self.cursor)))
    }

    fn word_range(&self, idx: usize) -> (usize, usize) {
        let n = self.len();
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let mut s = idx.min(n);
        let mut e = s;
        while s > 0 && is_word(self.rope.char(s - 1)) {
            s -= 1;
        }
        while e < n && is_word(self.rope.char(e)) {
            e += 1;
        }
        (s, e)
    }

    fn motion_target(&self, m: Motion) -> usize {
        match m {
            Motion::Left => self.cursor.saturating_sub(1),
            Motion::Right => (self.cursor + 1).min(self.len()),
            Motion::LineStart => {
                let line = self.rope.char_to_line(self.cursor);
                self.rope.line_to_char(line)
            }
            Motion::LineEnd => {
                let line = self.rope.char_to_line(self.cursor);
                let next = self.rope.line_to_char((line + 1).min(self.rope.len_lines()));
                // Back up over a trailing newline if present.
                let mut end = next;
                if end > self.cursor && end <= self.len() && self.rope.char(end - 1) == '\n' {
                    end -= 1;
                }
                end.max(self.rope.line_to_char(line))
            }
            Motion::WordLeft => self.word_left(self.cursor),
            Motion::WordRight => self.word_right(self.cursor),
            Motion::DocStart => 0,
            Motion::DocEnd => self.len(),
            Motion::Up | Motion::Down => self.cursor, // resolved by layout in the controller
        }
    }

    /// Previous word boundary: skip separators left, then the word run left.
    fn word_left(&self, idx: usize) -> usize {
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let mut j = idx.min(self.len());
        while j > 0 && !is_word(self.rope.char(j - 1)) {
            j -= 1;
        }
        while j > 0 && is_word(self.rope.char(j - 1)) {
            j -= 1;
        }
        j
    }

    /// Next word boundary: skip separators right, then the word run right.
    fn word_right(&self, idx: usize) -> usize {
        let n = self.len();
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let mut j = idx.min(n);
        while j < n && !is_word(self.rope.char(j)) {
            j += 1;
        }
        while j < n && is_word(self.rope.char(j)) {
            j += 1;
        }
        j
    }

    // ---- undo / redo -------------------------------------------------------

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.rope.to_string(),
            cursor: self.cursor,
            anchor: self.anchor,
        }
    }

    fn restore(&mut self, s: Snapshot) {
        self.rope = Rope::from_str(&s.text);
        self.cursor = s.cursor.min(self.len());
        self.anchor = s.anchor;
    }

    fn checkpoint(&mut self, kind: EditKind) {
        let coalesce =
            kind == self.last_kind && matches!(kind, EditKind::InsertWord | EditKind::Delete);
        if !coalesce {
            self.undo.push(self.snapshot());
            self.redo.clear();
        }
        self.last_kind = kind;
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(self.snapshot());
            self.restore(prev);
        }
        self.last_kind = EditKind::None;
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(self.snapshot());
            self.restore(next);
        }
        self.last_kind = EditKind::None;
    }
}

// ---- helpers ---------------------------------------------------------------

fn shift_offset(off: usize, region_start: usize, region_end: usize, delta: isize) -> usize {
    if off <= region_start {
        off
    } else if off >= region_end {
        (off as isize + delta).max(region_start as isize) as usize
    } else {
        // Inside the rewritten content: clamp to the new content end.
        (off as isize + delta).max(region_start as isize) as usize
    }
}

fn mark_delim(m: MarkSet) -> &'static str {
    if m.contains(MarkSet::BOLD) {
        "**"
    } else if m.contains(MarkSet::STRIKE) {
        "~~"
    } else if m.contains(MarkSet::CODE) {
        "`"
    } else {
        "*" // italic / default
    }
}

fn block_marker(kind: BlockKind) -> String {
    match kind {
        BlockKind::Heading(n) => format!("{} ", "#".repeat((n.clamp(1, 6)) as usize)),
        BlockKind::Bullet => "- ".to_string(),
        BlockKind::Ordered => "1. ".to_string(),
        BlockKind::Quote => "> ".to_string(),
        BlockKind::Code => String::new(), // handled elsewhere / no inline marker
        BlockKind::Divider => String::new(),
        BlockKind::Paragraph => String::new(),
    }
}

/// Char length of any leading block marker on a line (heading/list/quote).
fn existing_marker_len(line: &str) -> usize {
    let t = line;
    // heading
    let hashes = t.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) {
        let rest = &t[hashes..];
        if rest.starts_with(' ') {
            return hashes + 1;
        }
    }
    // blockquote
    if let Some(r) = t.strip_prefix("> ") {
        let _ = r;
        return 2;
    }
    if t == ">" {
        return 1;
    }
    // bullet
    for m in ["- ", "* ", "+ "] {
        if t.starts_with(m) {
            return 2;
        }
    }
    // ordered: digits then ". "
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && t[digits..].starts_with(". ") {
        return digits + 2;
    }
    0
}

/// Classify a line's current block kind from its leading marker.
fn line_kind(line: &str) -> BlockKind {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
        return BlockKind::Heading(hashes as u8);
    }
    if line.starts_with("> ") || line == ">" {
        return BlockKind::Quote;
    }
    if line.starts_with("- ") || line.starts_with("* ") || line.starts_with("+ ") {
        return BlockKind::Bullet;
    }
    let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && line[digits..].starts_with(". ") {
        return BlockKind::Ordered;
    }
    if line.trim() == "---" || line.trim() == "***" || line.trim() == "___" {
        return BlockKind::Divider;
    }
    BlockKind::Paragraph
}

struct Marker {
    byte_len: usize,
    kind: BlockKind,
}

impl Marker {
    fn continuation(&self) -> String {
        match self.kind {
            BlockKind::Bullet => "- ".to_string(),
            BlockKind::Ordered => "1. ".to_string(),
            BlockKind::Quote => "> ".to_string(),
            _ => String::new(),
        }
    }
}

/// A list/quote marker at the very start of a line (for Enter continuation).
fn leading_marker(line: &str) -> Option<Marker> {
    for (m, k) in [
        ("- ", BlockKind::Bullet),
        ("* ", BlockKind::Bullet),
        ("+ ", BlockKind::Bullet),
        ("> ", BlockKind::Quote),
    ] {
        if line.starts_with(m) {
            return Some(Marker {
                byte_len: m.len(),
                kind: k,
            });
        }
    }
    let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && line[digits..].starts_with(". ") {
        return Some(Marker {
            byte_len: digits + 2,
            kind: BlockKind::Ordered,
        });
    }
    None
}

fn is_private_use(c: char) -> bool {
    let u = c as u32;
    (0xE000..=0xF8FF).contains(&u)
        || (0xF_0000..=0xF_FFFD).contains(&u)
        || (0x10_0000..=0x10_FFFD).contains(&u)
}
