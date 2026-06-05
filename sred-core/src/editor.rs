//! The editing engine — block-aware, with per-character attributes.
//!
//! The editable buffer is a flat char sequence (`text`) with a parallel
//! per-character attribute map (`attrs`: marks + optional view color + link id)
//! **plus** a per-block kind vector (`kinds`). Blocks are the `'\n'`-separated
//! segments of `text`, so block `i` is the i-th line and
//! `kinds.len() == newlines(text) + 1` always holds.
//!
//! - Inline *marks* (bold/italic/code/strike) round-trip through the backends.
//! - Text *color* is a view-only visual indicator (not serialized).
//! - *Links* carry a URL (interned in `links`) and serialize to `[text](url)`.

use crate::format::backend_for;
use crate::model::{Block, Document, Format, Inline, List, MarkSet};

#[derive(Debug, Clone, Copy)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
}

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

/// Per-character style. `color` is packed `0xRRGGBBAA` (view-only). `link` is
/// `0` for none, else `1 + index` into [`EditorCore`]'s link table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharAttr {
    pub marks: MarkSet,
    pub color: Option<u32>,
    pub link: u32,
}

impl CharAttr {
    pub const PLAIN: CharAttr = CharAttr {
        marks: MarkSet::empty(),
        color: None,
        link: 0,
    };
}

#[derive(Debug, Clone)]
pub enum Command {
    Insert(String),
    DeleteBackward,
    DeleteForward,
    DeleteSelection,
    Move(Motion),
    Select(Motion),
    SelectAll,
    ToggleMark(MarkSet),
    SetColor(Option<u32>),
    /// Turn the current selection into a link to `url`.
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
    Mark,
    Structure,
}

#[derive(Clone)]
struct Snapshot {
    text: Vec<char>,
    attrs: Vec<CharAttr>,
    kinds: Vec<BlockKind>,
    links: Vec<String>,
    cursor: usize,
    anchor: Option<usize>,
}

pub struct EditorCore {
    text: Vec<char>,
    attrs: Vec<CharAttr>,
    kinds: Vec<BlockKind>,
    links: Vec<String>,
    cursor: usize,
    anchor: Option<usize>,
    active: CharAttr,
    format: Format,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_kind: EditKind,
}

/// A contiguous run of identically-styled text for layout.
pub struct Span {
    pub text: String,
    pub marks: MarkSet,
    pub color: Option<[u8; 4]>,
    pub size: f32,
}

/// A line decoration drawn by the rasterizer (cosmic-text renders neither).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoration {
    Strike,
    Underline,
}

struct Segment {
    kind: BlockKind,
    text: String,
    attrs: Vec<CharAttr>,
}

impl EditorCore {
    pub fn new(format: Format) -> Self {
        EditorCore {
            text: Vec::new(),
            attrs: Vec::new(),
            kinds: vec![BlockKind::Paragraph],
            links: Vec::new(),
            cursor: 0,
            anchor: None,
            active: CharAttr::PLAIN,
            format,
            undo: Vec::new(),
            redo: Vec::new(),
            last_kind: EditKind::None,
        }
    }

    pub fn from_source(src: &str, format: Format) -> Self {
        let backend = backend_for(format);
        let doc = backend.parse(src).unwrap_or_else(|_| Document::empty());
        let mut ed = EditorCore::new(format);
        ed.load_document(&doc);
        ed
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
        self.text.len()
    }
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
    pub fn text(&self) -> String {
        self.text.iter().collect()
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
            Some((s, e)) => self.text[s..e].iter().collect(),
            None => String::new(),
        }
    }

    fn link_id_at_cursor(&self) -> u32 {
        // Prefer the char at the caret, else the char just before it.
        let here = self.attrs.get(self.cursor).map(|a| a.link).unwrap_or(0);
        if here != 0 {
            return here;
        }
        self.cursor
            .checked_sub(1)
            .and_then(|i| self.attrs.get(i))
            .map(|a| a.link)
            .unwrap_or(0)
    }

    /// URL of the link under the caret (if any).
    pub fn link_at_cursor(&self) -> Option<String> {
        let id = self.link_id_at_cursor();
        if id == 0 {
            None
        } else {
            self.links.get((id - 1) as usize).cloned()
        }
    }

    /// Update the target of the link under the caret. Returns false if the caret
    /// is not inside a link.
    pub fn update_link_at_cursor(&mut self, url: &str) -> bool {
        let id = self.link_id_at_cursor();
        if id == 0 {
            return false;
        }
        self.checkpoint(EditKind::Structure);
        self.links[(id - 1) as usize] = url.to_string();
        true
    }

    pub fn set_cursor(&mut self, idx: usize) {
        self.anchor = None;
        self.cursor = idx.min(self.text.len());
        self.last_kind = EditKind::None;
    }

    pub fn extend_to(&mut self, idx: usize) {
        if self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }
        self.cursor = idx.min(self.text.len());
    }

    pub fn select_word_at(&mut self, idx: usize) {
        let (s, e) = self.word_range(idx);
        self.anchor = Some(s);
        self.cursor = e;
    }

    fn word_range(&self, idx: usize) -> (usize, usize) {
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let mut s = idx.min(self.text.len());
        let mut e = s;
        while s > 0 && self.text.get(s - 1).is_some_and(|c| is_word(*c)) {
            s -= 1;
        }
        while e < self.text.len() && self.text.get(e).is_some_and(|c| is_word(*c)) {
            e += 1;
        }
        (s, e)
    }

    // ---- loading -----------------------------------------------------------

    fn load_document(&mut self, doc: &Document) {
        let mut lines: Vec<Segment> = Vec::new();
        for b in &doc.blocks {
            self.flatten_doc_block(b, &mut lines);
        }
        if lines.is_empty() {
            lines.push(Segment {
                kind: BlockKind::Paragraph,
                text: String::new(),
                attrs: Vec::new(),
            });
        }

        self.text.clear();
        self.attrs.clear();
        self.kinds.clear();
        for (i, seg) in lines.into_iter().enumerate() {
            if i > 0 {
                self.text.push('\n');
                self.attrs.push(CharAttr::PLAIN);
            }
            for (ch, a) in seg.text.chars().zip(seg.attrs.iter().copied()) {
                self.text.push(ch);
                self.attrs.push(a);
            }
            self.kinds.push(seg.kind);
        }
        self.cursor = self.text.len();
    }

    fn intern_link(&mut self, url: &str) -> u32 {
        if let Some(i) = self.links.iter().position(|u| u == url) {
            (i + 1) as u32
        } else {
            self.links.push(url.to_string());
            self.links.len() as u32
        }
    }

    // ---- command application ----------------------------------------------

    pub fn apply(&mut self, cmd: Command) {
        match cmd {
            Command::Insert(s) => {
                // Enter on an empty list/quote item exits the list (→ paragraph)
                // instead of creating another item.
                if s == "\n" && self.exit_block_on_empty() {
                    return;
                }
                let kind = if !s.is_empty() && s.chars().all(|c| c.is_alphanumeric()) {
                    EditKind::InsertWord
                } else {
                    EditKind::InsertBoundary
                };
                self.checkpoint(kind);
                self.insert(&s);
                if s == " " {
                    self.autoformat_line();
                }
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
                self.cursor = self.text.len();
                self.last_kind = EditKind::None;
            }
            Command::ToggleMark(m) => {
                self.checkpoint(EditKind::Mark);
                self.toggle_mark(m);
            }
            Command::SetColor(c) => {
                self.checkpoint(EditKind::Mark);
                self.set_color(c);
            }
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

    // ---- text mutation primitives (maintain the kinds invariant) ----------

    fn newlines_before(&self, pos: usize) -> usize {
        self.text[..pos].iter().filter(|c| **c == '\n').count()
    }

    fn raw_insert(&mut self, pos: usize, s: &str, attr: CharAttr) {
        let b = self.newlines_before(pos);
        let cont = continuation(self.kinds[b]);
        let mut at = pos;
        let mut split = 0usize;
        for ch in s.chars() {
            self.text.insert(at, ch);
            self.attrs.insert(at, attr);
            if ch == '\n' {
                self.kinds.insert(b + 1 + split, cont);
                split += 1;
            }
            at += 1;
        }
    }

    fn raw_remove(&mut self, a: usize, b: usize) {
        let removed = self.text[a..b].iter().filter(|c| **c == '\n').count();
        let start = self.newlines_before(a);
        self.text.drain(a..b);
        self.attrs.drain(a..b);
        for _ in 0..removed {
            if start + 1 < self.kinds.len() {
                self.kinds.remove(start + 1);
            }
        }
    }

    fn insert(&mut self, s: &str) {
        // Drop unprintable input: control chars (special keys arrive as these or
        // as private-use code points) — but keep newlines.
        let clean: String = s
            .chars()
            .filter(|c| *c == '\n' || (!c.is_control() && !is_private_use(*c)))
            .collect();
        if clean.is_empty() {
            return;
        }
        self.delete_selection();
        // Newly typed text never inherits a link.
        let mut attr = self.active;
        attr.link = 0;
        self.raw_insert(self.cursor, &clean, attr);
        self.cursor += clean.chars().count();
    }

    /// Markdown/Typst-style input rule: when a marker + space is typed at the
    /// start of a plain paragraph, promote the block and strip the marker.
    fn autoformat_line(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let (ls, _) = self.line_bounds();
        let marker_end = self.cursor - 1; // the just-typed space
        if marker_end < ls {
            return;
        }
        let bi = self.newlines_before(ls);
        if self.kinds[bi] != BlockKind::Paragraph {
            return;
        }
        let marker: String = self.text[ls..marker_end].iter().collect();
        let kind = match marker.as_str() {
            "#" | "=" => BlockKind::Heading(1),
            "##" | "==" => BlockKind::Heading(2),
            "###" | "===" => BlockKind::Heading(3),
            "-" | "*" | "+" => BlockKind::Bullet,
            "1." => BlockKind::Ordered,
            ">" => BlockKind::Quote,
            _ => return,
        };
        // Remove the marker and its trailing space.
        self.raw_remove(ls, self.cursor);
        self.cursor = ls;
        self.kinds[bi] = kind;
    }

    fn delete_backward(&mut self) {
        if self.delete_selection() {
            return;
        }
        // Backspace at the very start of a list/quote line un-bullets it (→
        // paragraph) rather than merging into the previous block.
        let bi = self.newlines_before(self.cursor);
        let (ls, _) = self.line_bounds();
        if self.cursor == ls && is_listish(self.kinds[bi]) {
            self.kinds[bi] = BlockKind::Paragraph;
            return;
        }
        if self.cursor > 0 {
            self.raw_remove(self.cursor - 1, self.cursor);
            self.cursor -= 1;
        }
    }

    /// If the caret sits on an empty list/quote block, convert it to a plain
    /// paragraph and report that the Enter was consumed.
    fn exit_block_on_empty(&mut self) -> bool {
        let bi = self.newlines_before(self.cursor);
        let (ls, le) = self.line_bounds();
        if ls == le && is_listish(self.kinds[bi]) {
            self.checkpoint(EditKind::Structure);
            self.kinds[bi] = BlockKind::Paragraph;
            self.last_kind = EditKind::None;
            true
        } else {
            false
        }
    }

    fn delete_forward(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor < self.text.len() {
            self.raw_remove(self.cursor, self.cursor + 1);
        }
    }

    fn delete_selection(&mut self) -> bool {
        if let Some((s, e)) = self.selection_range() {
            self.raw_remove(s, e);
            self.cursor = s;
            self.anchor = None;
            return true;
        }
        false
    }

    fn toggle_mark(&mut self, m: MarkSet) {
        if let Some((start, end)) = self.selection_range() {
            let all = self.attrs[start..end].iter().all(|x| x.marks.contains(m));
            for a in &mut self.attrs[start..end] {
                if all {
                    a.marks.remove(m);
                } else {
                    a.marks.insert(m);
                }
            }
        } else {
            self.active.marks.toggle(m);
        }
    }

    fn set_color(&mut self, c: Option<u32>) {
        if let Some((start, end)) = self.selection_range() {
            for a in &mut self.attrs[start..end] {
                a.color = c;
            }
        } else {
            self.active.color = c;
        }
    }

    fn make_link(&mut self, url: &str) {
        if let Some((start, end)) = self.selection_range() {
            let id = self.intern_link(url);
            for a in &mut self.attrs[start..end] {
                a.link = id;
                a.marks.insert(MarkSet::LINK);
            }
        }
    }

    fn set_block(&mut self, kind: BlockKind, toggle: bool) {
        let (s, e) = self.selection_range().unwrap_or((self.cursor, self.cursor));
        let bs = self.newlines_before(s);
        let be = self.newlines_before(e);
        let all_same = (bs..=be).all(|i| self.kinds[i] == kind);
        for i in bs..=be {
            self.kinds[i] = if toggle && all_same {
                BlockKind::Paragraph
            } else {
                kind
            };
        }
    }

    fn selection_range(&self) -> Option<(usize, usize)> {
        self.anchor
            .filter(|a| *a != self.cursor)
            .map(|a| (a.min(self.cursor), a.max(self.cursor)))
    }

    fn motion_target(&self, m: Motion) -> usize {
        match m {
            Motion::Left => self.cursor.saturating_sub(1),
            Motion::Right => (self.cursor + 1).min(self.text.len()),
            Motion::LineStart => self.line_bounds().0,
            Motion::LineEnd => self.line_bounds().1,
            Motion::Up | Motion::Down => self.cursor,
        }
    }

    fn line_bounds(&self) -> (usize, usize) {
        let start = self.text[..self.cursor]
            .iter()
            .rposition(|c| *c == '\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let end = self.text[self.cursor..]
            .iter()
            .position(|c| *c == '\n')
            .map(|i| self.cursor + i)
            .unwrap_or(self.text.len());
        (start, end)
    }

    // ---- undo / redo -------------------------------------------------------

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            attrs: self.attrs.clone(),
            kinds: self.kinds.clone(),
            links: self.links.clone(),
            cursor: self.cursor,
            anchor: self.anchor,
        }
    }

    fn restore(&mut self, s: Snapshot) {
        self.text = s.text;
        self.attrs = s.attrs;
        self.kinds = s.kinds;
        self.links = s.links;
        self.cursor = s.cursor;
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

    // ---- views -------------------------------------------------------------

    fn segments(&self) -> Vec<Segment> {
        let mut segs = Vec::new();
        let mut text = String::new();
        let mut attrs = Vec::new();
        let mut bi = 0usize;
        for (i, &ch) in self.text.iter().enumerate() {
            if ch == '\n' {
                segs.push(Segment {
                    kind: self.kinds[bi],
                    text: std::mem::take(&mut text),
                    attrs: std::mem::take(&mut attrs),
                });
                bi += 1;
            } else {
                text.push(ch);
                attrs.push(self.attrs[i]);
            }
        }
        segs.push(Segment {
            kind: *self.kinds.get(bi).unwrap_or(&BlockKind::Paragraph),
            text,
            attrs,
        });
        segs
    }

    /// Build layout runs (heading sizes, block prefixes, `'\n'` separators,
    /// colors) plus per-block prefix byte length for cursor translation.
    pub fn styled_runs(&self, base: f32) -> (Vec<Span>, Vec<usize>) {
        let segs = self.segments();
        let mut spans = Vec::new();
        let mut prefix_bytes = Vec::with_capacity(segs.len());
        let mut ordinal = 0usize;
        for (i, seg) in segs.iter().enumerate() {
            if i > 0 {
                spans.push(Span {
                    text: "\n".into(),
                    marks: MarkSet::empty(),
                    color: None,
                    size: base,
                });
            }
            if seg.kind == BlockKind::Ordered {
                ordinal += 1;
            } else {
                ordinal = 0;
            }
            let prefix = block_prefix(seg.kind, ordinal);
            prefix_bytes.push(prefix.len());
            if !prefix.is_empty() {
                spans.push(Span {
                    text: prefix,
                    marks: MarkSet::empty(),
                    color: None,
                    size: base,
                });
            }
            let size = block_size(seg.kind, base);
            let extra = block_extra_marks(seg.kind);
            let chars: Vec<char> = seg.text.chars().collect();
            let mut j = 0;
            while j < chars.len() {
                let (marks, color) = self.render_style(&seg.attrs, j, extra);
                let start = j;
                while j < chars.len() && self.render_style(&seg.attrs, j, extra) == (marks, color) {
                    j += 1;
                }
                spans.push(Span {
                    text: chars[start..j].iter().collect(),
                    marks,
                    color,
                    size,
                });
            }
        }
        (spans, prefix_bytes)
    }

    /// Contiguous ranges (in editable char indices) that need a drawn line:
    /// strikethrough for `STRIKE` marks, underline for links.
    pub fn decorations(&self) -> Vec<(usize, usize, Decoration)> {
        let mut out = Vec::new();
        collect_runs(&self.attrs, |a| a.marks.contains(MarkSet::STRIKE), Decoration::Strike, &mut out);
        collect_runs(&self.attrs, |a| a.link != 0, Decoration::Underline, &mut out);
        out
    }

    fn render_style(
        &self,
        attrs: &[CharAttr],
        i: usize,
        extra: MarkSet,
    ) -> (MarkSet, Option<[u8; 4]>) {
        let a = attrs.get(i).copied().unwrap_or(CharAttr::PLAIN);
        let mut marks = a.marks | extra;
        if a.link != 0 {
            marks.insert(MarkSet::LINK);
        }
        (marks, a.color.map(unpack_color))
    }

    /// Rebuild a structured `Document` from the editable buffer.
    pub fn to_document(&self) -> Document {
        let segs = self.segments();
        let mut blocks = Vec::new();
        let mut i = 0;
        while i < segs.len() {
            match segs[i].kind {
                BlockKind::Code => {
                    let mut code = String::new();
                    let mut first = true;
                    while i < segs.len() && segs[i].kind == BlockKind::Code {
                        if !first {
                            code.push('\n');
                        }
                        code.push_str(&segs[i].text);
                        first = false;
                        i += 1;
                    }
                    blocks.push(Block::CodeBlock { lang: None, code });
                }
                BlockKind::Bullet | BlockKind::Ordered => {
                    let ordered = segs[i].kind == BlockKind::Ordered;
                    let want = segs[i].kind;
                    let mut items = Vec::new();
                    while i < segs.len() && segs[i].kind == want {
                        items.push(vec![Block::Paragraph(self.seg_inlines(&segs[i]))]);
                        i += 1;
                    }
                    blocks.push(Block::List(List {
                        ordered,
                        tight: true,
                        items,
                    }));
                }
                BlockKind::Quote => {
                    let mut inner = Vec::new();
                    while i < segs.len() && segs[i].kind == BlockKind::Quote {
                        inner.push(Block::Paragraph(self.seg_inlines(&segs[i])));
                        i += 1;
                    }
                    blocks.push(Block::BlockQuote(inner));
                }
                BlockKind::Heading(level) => {
                    blocks.push(Block::Heading {
                        level,
                        content: self.seg_inlines(&segs[i]),
                    });
                    i += 1;
                }
                BlockKind::Divider => {
                    blocks.push(Block::ThematicBreak);
                    i += 1;
                }
                BlockKind::Paragraph => {
                    blocks.push(Block::Paragraph(self.seg_inlines(&segs[i])));
                    i += 1;
                }
            }
        }
        Document {
            blocks,
            meta: crate::model::DocMeta {
                origin: Some(self.format),
            },
        }
    }

    fn seg_inlines(&self, seg: &Segment) -> Vec<Inline> {
        // Group by serialization-relevant style (marks + link), ignoring color.
        let chars: Vec<char> = seg.text.chars().collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let a = seg.attrs.get(i).copied().unwrap_or(CharAttr::PLAIN);
            let key = (a.marks, a.link);
            let start = i;
            while i < chars.len() {
                let b = seg.attrs.get(i).copied().unwrap_or(CharAttr::PLAIN);
                if (b.marks, b.link) != key {
                    break;
                }
                i += 1;
            }
            let run: String = chars[start..i].iter().collect();
            if a.link != 0 {
                out.push(Inline::Styled {
                    marks: a.marks | MarkSet::LINK,
                    link: self.links.get((a.link - 1) as usize).cloned(),
                    content: vec![Inline::Text(run)],
                });
            } else if a.marks.is_empty() {
                out.push(Inline::Text(run));
            } else {
                out.push(Inline::Styled {
                    marks: a.marks,
                    link: None,
                    content: vec![Inline::Text(run)],
                });
            }
        }
        if out.is_empty() {
            out.push(Inline::Text(String::new()));
        }
        out
    }

    pub fn source(&self) -> String {
        backend_for(self.format)
            .serialize(&self.to_document())
            .unwrap_or_default()
    }

    // ---- loading helpers ---------------------------------------------------

    fn flatten_doc_block(&mut self, b: &Block, lines: &mut Vec<Segment>) {
        match b {
            Block::Paragraph(inl) => {
                lines.push(self.seg_from_inlines(BlockKind::Paragraph, inl))
            }
            Block::Heading { level, content } => {
                lines.push(self.seg_from_inlines(BlockKind::Heading(*level), content))
            }
            Block::List(list) => {
                let kind = if list.ordered {
                    BlockKind::Ordered
                } else {
                    BlockKind::Bullet
                };
                for item in &list.items {
                    let mut inl = Vec::new();
                    for b in item {
                        if let Block::Paragraph(p) = b {
                            inl.extend(p.iter().cloned());
                        }
                    }
                    lines.push(self.seg_from_inlines(kind, &inl));
                }
            }
            Block::BlockQuote(blocks) => {
                for b in blocks {
                    if let Block::Paragraph(p) = b {
                        lines.push(self.seg_from_inlines(BlockKind::Quote, p));
                    } else {
                        self.flatten_doc_block(b, lines);
                    }
                }
            }
            Block::CodeBlock { code, .. } => {
                for line in code.split('\n') {
                    lines.push(Segment {
                        kind: BlockKind::Code,
                        text: line.to_string(),
                        attrs: vec![CharAttr::PLAIN; line.chars().count()],
                    });
                }
            }
            Block::ThematicBreak => lines.push(Segment {
                kind: BlockKind::Divider,
                text: String::new(),
                attrs: Vec::new(),
            }),
            Block::Raw { src, .. } => lines.push(Segment {
                kind: BlockKind::Paragraph,
                text: src.clone(),
                attrs: vec![CharAttr::PLAIN; src.chars().count()],
            }),
        }
    }

    fn seg_from_inlines(&mut self, kind: BlockKind, inl: &[Inline]) -> Segment {
        let mut text = String::new();
        let mut attrs = Vec::new();
        self.push_inlines(inl, CharAttr::PLAIN, &mut text, &mut attrs);
        Segment { kind, text, attrs }
    }

    fn push_inlines(
        &mut self,
        inl: &[Inline],
        base: CharAttr,
        text: &mut String,
        attrs: &mut Vec<CharAttr>,
    ) {
        for i in inl {
            match i {
                Inline::Text(t) => {
                    for ch in t.chars() {
                        text.push(ch);
                        attrs.push(base);
                    }
                }
                Inline::Styled { marks, link, content } => {
                    let mut child = base;
                    child.marks |= *marks;
                    if let Some(url) = link {
                        child.link = self.intern_link(url);
                        child.marks.insert(MarkSet::LINK);
                    }
                    self.push_inlines(content, child, text, attrs);
                }
                Inline::LineBreak => {
                    text.push(' ');
                    attrs.push(base);
                }
                Inline::Image { alt, .. } => {
                    for ch in alt.chars() {
                        text.push(ch);
                        attrs.push(base);
                    }
                }
                Inline::Math { src, .. } | Inline::Raw { src, .. } => {
                    let mut a = base;
                    a.marks.insert(MarkSet::CODE);
                    for ch in src.chars() {
                        text.push(ch);
                        attrs.push(a);
                    }
                }
            }
        }
    }
}

// ---- block presentation helpers -------------------------------------------

fn is_listish(k: BlockKind) -> bool {
    matches!(k, BlockKind::Bullet | BlockKind::Ordered | BlockKind::Quote)
}

fn continuation(k: BlockKind) -> BlockKind {
    match k {
        BlockKind::Bullet => BlockKind::Bullet,
        BlockKind::Ordered => BlockKind::Ordered,
        BlockKind::Quote => BlockKind::Quote,
        BlockKind::Code => BlockKind::Code,
        _ => BlockKind::Paragraph,
    }
}

fn block_size(k: BlockKind, base: f32) -> f32 {
    match k {
        BlockKind::Heading(1) => base * 1.9,
        BlockKind::Heading(2) => base * 1.55,
        BlockKind::Heading(3) => base * 1.3,
        BlockKind::Heading(4) => base * 1.15,
        BlockKind::Heading(_) => base * 1.05,
        _ => base,
    }
}

fn block_extra_marks(k: BlockKind) -> MarkSet {
    match k {
        BlockKind::Heading(_) => MarkSet::BOLD,
        BlockKind::Code => MarkSet::CODE,
        _ => MarkSet::empty(),
    }
}

fn block_prefix(k: BlockKind, ordinal: usize) -> String {
    match k {
        BlockKind::Bullet => "•  ".to_string(),
        BlockKind::Ordered => format!("{}.  ", ordinal),
        BlockKind::Quote => "▌  ".to_string(),
        BlockKind::Divider => "─".repeat(40),
        _ => String::new(),
    }
}

fn collect_runs(
    attrs: &[CharAttr],
    pred: impl Fn(&CharAttr) -> bool,
    deco: Decoration,
    out: &mut Vec<(usize, usize, Decoration)>,
) {
    let mut i = 0;
    while i < attrs.len() {
        if pred(&attrs[i]) {
            let start = i;
            while i < attrs.len() && pred(&attrs[i]) {
                i += 1;
            }
            out.push((start, i, deco));
        } else {
            i += 1;
        }
    }
}

/// True for Unicode Private Use Area code points — Slint encodes special keys
/// (arrows, page-up, F-keys, …) in this range, which must never be inserted.
fn is_private_use(c: char) -> bool {
    let u = c as u32;
    (0xE000..=0xF8FF).contains(&u)
        || (0xF_0000..=0xF_FFFD).contains(&u)
        || (0x10_0000..=0x10_FFFD).contains(&u)
}

fn unpack_color(c: u32) -> [u8; 4] {
    [
        (c >> 24) as u8,
        (c >> 16) as u8,
        (c >> 8) as u8,
        c as u8,
    ]
}
