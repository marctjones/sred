//! View builder for the source-anchored editor.
//!
//! Given the raw markdown text, produce a styled *projection* without modifying
//! it: per-run font size + marks ([`styled_runs`]), strike/underline ranges
//! ([`decorations`]), and link lookup ([`link_at`]). Markers (`#`, `**`, `- `)
//! stay in the text and are styled in place. The concatenation of all emitted
//! span text equals the input exactly, so cursor offsets map 1:1 (prefix bytes
//! are all zero — nothing is injected).
//!
//! Inline styling is intentionally pragmatic (non-nested common cases). Byte
//! fidelity does not depend on it — the buffer is the source of truth — so the
//! scanner can be refined without risk.

use std::collections::HashMap;
use std::ops::Range;

use crate::model::{Format, MarkSet};

/// Per-line syntax-highlight colors for fenced code blocks, keyed by line index
/// (char-column ranges → RGBA). Empty unless the `syntax-highlight` feature is on.
type CodeHighlights = HashMap<usize, Vec<(usize, usize, [u8; 4])>>;

/// A contiguous run of identically-styled text for layout.
#[derive(Clone)]
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
    /// A filled chip background (RGBA) behind a token range — drawn before glyphs.
    Chip([u8; 4]),
}

/// Build styled layout runs + per-line prefix bytes (always 0 here — markers are
/// real text, nothing is injected).
/// One match of a host-registered token within a line (char offsets + the value
/// the host cares about, e.g. the project/person/tag name or the URL).
pub struct TokenMatch {
    pub start: usize,
    pub end: usize,
    pub value: String,
}

/// A host-registered inline token kind (e.g. `[[wikilink]]`, `#tag`, url). The
/// matcher finds occurrences in a line; matched chars render in `fg`.
pub struct TokenSpec {
    pub id: String,
    pub fg: [u8; 4],
    /// Optional chip background (RGBA) behind the matched text.
    pub bg: Option<[u8; 4]>,
    pub matcher: Box<dyn Fn(&str) -> Vec<TokenMatch>>,
}

// ---- doc-level analysis (caret-INDEPENDENT) -------------------------------
//
// Phase 2 splits styling into a caret-independent `analyze()` (one whole-document
// parse, cacheable by text) and a cheap caret-dependent `project()` per line.
// The expensive parse therefore does NOT re-run on a caret move (a very common
// event); only the projection of the two lines whose caret-state flipped does.
// The byte-delta — the fidelity/cursor-mapping invariant — is produced entirely
// in `project()` from the marker byte range that `analyze()` records, so it stays
// correct by construction.

/// Caret-independent per-line facts produced by [`analyze`]. Everything a full
/// parse determines that does NOT depend on where the caret is.
#[derive(Clone)]
struct LineInfo {
    /// Leading SOURCE bytes that form the hideable block marker (`"## "`, `"- "`,
    /// `"> "`, or the whole ```` ``` ```` fence line). Replaced by `repl` when the
    /// line is projected off-caret; shown verbatim on the caret line. `0` ⇒ no
    /// hideable marker (paragraphs, numbered lists, code interiors).
    marker_len: usize,
    /// What the hidden marker is replaced by: `""` to drop it, `"• "` for bullets.
    repl: &'static str,
    /// Whole-line font-size multiplier (headings scale up); `1.0` otherwise. Kept
    /// base-independent so the analysis cache survives a font-size change.
    scale: f32,
    /// Marks applied to every char of the line (heading `BOLD`; code `CODE`).
    extra: MarkSet,
    /// Code line (fence delimiter or interior): colors come from the syntax
    /// highlighter, inline markup isn't parsed, and host tokens aren't matched.
    is_code: bool,
    /// Per-SOURCE-char inline marks (markup-parser output). `CODE`-only for code.
    inline: Vec<MarkSet>,
    /// Per-SOURCE-char colors (syntect for code; syntax palette for prose in
    /// Step D). `None` where uncolored.
    colors: Vec<Option<[u8; 4]>>,
    /// Hash of all of the above + the source text: the [`project`] cache seed.
    /// Captures every cross-line dependency (fence state, future setext/refs) so
    /// a per-line project cache stays correct even when a line's rendering depends
    /// on its neighbors.
    digest: u64,
}

impl LineInfo {
    fn new(
        line: &str,
        marker_len: usize,
        repl: &'static str,
        scale: f32,
        extra: MarkSet,
        is_code: bool,
        inline: Vec<MarkSet>,
        colors: Vec<Option<[u8; 4]>>,
    ) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        line.hash(&mut h);
        marker_len.hash(&mut h);
        repl.hash(&mut h);
        scale.to_bits().hash(&mut h);
        extra.bits().hash(&mut h);
        is_code.hash(&mut h);
        for m in &inline {
            m.bits().hash(&mut h);
        }
        for c in &colors {
            c.hash(&mut h);
        }
        let digest = h.finish();
        LineInfo { marker_len, repl, scale, extra, is_code, inline, colors, digest }
    }
}

/// Whole-document analysis: one [`LineInfo`] per source line.
struct DocAnalysis {
    lines: Vec<LineInfo>,
}

thread_local! {
    // Whole-document analysis, keyed by (format, text). A content edit misses and
    // re-parses (~1–2 ms for a 4000-line doc); a caret move is a pure cache hit.
    static ANALYSIS_CACHE: std::cell::RefCell<HashMap<u64, std::rc::Rc<DocAnalysis>>> =
        std::cell::RefCell::new(HashMap::new());
    // Per-line projected spans + delta, keyed by `project_key`. A line's
    // projection is a pure function of its `LineInfo`, caret-state, base size and
    // token generation, so a caret move only reprojects the two lines whose
    // caret-state flipped. Bounded; cleared wholesale when large.
    static PROJECT_CACHE: std::cell::RefCell<HashMap<u64, (Vec<Span>, i32)>> =
        std::cell::RefCell::new(HashMap::new());
}

fn analysis_key(format: Format, text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (format as u8).hash(&mut h);
    text.hash(&mut h);
    h.finish()
}

/// Cache key for one line's projection: its analysis `digest` (which already
/// folds in the source text + every cross-line dependency) plus the caret-state,
/// base font size and token generation that `project` also reads.
fn project_key(digest: u64, on_caret: bool, base: f32, tokens_gen: u64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    digest.hash(&mut h);
    on_caret.hash(&mut h);
    base.to_bits().hash(&mut h);
    tokens_gen.hash(&mut h);
    h.finish()
}

fn analyze_cached(text: &str, format: Format) -> std::rc::Rc<DocAnalysis> {
    let key = analysis_key(format, text);
    ANALYSIS_CACHE.with(|c| {
        if let Some(hit) = c.borrow().get(&key) {
            return hit.clone();
        }
        let computed = std::rc::Rc::new(analyze(text, format));
        let mut m = c.borrow_mut();
        if m.len() > 64 {
            m.clear();
        }
        m.insert(key, computed.clone());
        computed
    })
}

/// One whole-document parse → per-line caret-independent facts. Tracks fenced
/// code across lines (so a code line's classification depends on context, which
/// is why this is doc-level), highlights code once, and classifies each prose
/// line's block marker + inline marks.
fn analyze(text: &str, format: Format) -> DocAnalysis {
    let highlights = code_highlights(text);
    let mut lines = Vec::new();
    let mut in_fence = false;
    for (li, line) in text.split('\n').enumerate() {
        let is_fence = line.trim_start().starts_with("```");
        let interior = in_fence && !is_fence;
        if is_fence {
            in_fence = !in_fence;
        }
        let info = if is_fence || interior {
            // Fence delimiter hides fully off-caret (marker = whole line); an
            // interior line is always shown (no marker).
            let n = line.chars().count();
            let marker_len = if is_fence { line.len() } else { 0 };
            let colors = line_code_colors(highlights.get(&li), n);
            LineInfo::new(line, marker_len, "", 1.0, MarkSet::CODE, true, vec![MarkSet::empty(); n], colors)
        } else {
            analyze_prose(format, line)
        };
        lines.push(info);
    }
    DocAnalysis { lines }
}

/// Classify one prose line: its hideable block marker + scale + whole-line marks
/// (`classify_block`), then its inline markup (`line_marks`). Both are functions
/// of the line alone in Step A; cross-line constructs are added in Steps B/C.
fn analyze_prose(format: Format, line: &str) -> LineInfo {
    let (marker_len, repl, scale, extra) = classify_block(format, line);
    let inline = line_marks(format, line);
    let n = inline.len();
    LineInfo::new(line, marker_len, repl, scale, extra, false, inline, vec![None; n])
}

/// Project one line for display (caret-DEPENDENT, cheap): hide/substitute its
/// block marker unless it's the caret line, map the caret-independent inline
/// marks + colors into display-char space, overlay host token colors, and emit
/// spans. Returns the spans and the byte delta `display_leading − source_leading`.
fn project_line(info: &LineInfo, line: &str, on_caret: bool, base: f32, tokens: &[TokenSpec]) -> (Vec<Span>, i32) {
    let size = base * info.scale;
    let reveal = on_caret || info.marker_len == 0;
    let (display, delta) = if reveal {
        (line.to_string(), 0)
    } else {
        let mut d = String::with_capacity(info.repl.len() + line.len() - info.marker_len);
        d.push_str(info.repl);
        d.push_str(&line[info.marker_len..]);
        (d, info.repl.len() as i32 - info.marker_len as i32)
    };

    let disp_n = display.chars().count();
    let mut marks = vec![MarkSet::empty(); disp_n];
    let mut colors: Vec<Option<[u8; 4]>> = vec![None; disp_n];
    if reveal {
        // Marker shown verbatim: source ↔ display chars line up 1:1.
        for i in 0..disp_n {
            marks[i] = info.inline[i] | info.extra;
            colors[i] = info.colors[i];
        }
    } else {
        // Hidden: [0..repl_chars) is the substitute (carries only `extra`), the
        // rest are source chars past the marker, shifted into display space.
        let repl_chars = info.repl.chars().count();
        let marker_src_chars = line[..info.marker_len].chars().count();
        for slot in marks.iter_mut().take(repl_chars) {
            *slot = info.extra;
        }
        for k in 0..disp_n.saturating_sub(repl_chars) {
            let si = marker_src_chars + k;
            marks[repl_chars + k] = info.inline[si] | info.extra;
            colors[repl_chars + k] = info.colors[si];
        }
    }

    // Host tokens are matched on the display text and override syntax colors
    // (precedence: token > syntax > mark-derived). Code lines aren't tokenized.
    if !info.is_code && !tokens.is_empty() {
        for spec in tokens {
            for m in (spec.matcher)(&display) {
                for slot in colors.iter_mut().take(m.end.min(disp_n)).skip(m.start.min(disp_n)) {
                    *slot = Some(spec.fg);
                }
            }
        }
    }

    let chars: Vec<char> = display.chars().collect();
    let mut out = Vec::new();
    let mut j = 0;
    while j < disp_n {
        let mk = marks[j];
        let col = colors[j];
        let start = j;
        while j < disp_n && marks[j] == mk && colors[j] == col {
            j += 1;
        }
        out.push(Span { text: chars[start..j].iter().collect(), marks: mk, color: col, size });
    }
    (out, delta)
}

fn project_cached(
    info: &LineInfo,
    line: &str,
    on_caret: bool,
    base: f32,
    tokens: &[TokenSpec],
    tokens_gen: u64,
) -> (Vec<Span>, i32) {
    let key = project_key(info.digest, on_caret, base, tokens_gen);
    PROJECT_CACHE.with(|c| {
        if let Some(hit) = c.borrow().get(&key) {
            return hit.clone();
        }
        let computed = project_line(info, line, on_caret, base, tokens);
        let mut m = c.borrow_mut();
        if m.len() > 16384 {
            m.clear();
        }
        m.insert(key, computed.clone());
        computed
    })
}

pub fn styled_runs(
    text: &str,
    format: Format,
    base: f32,
    caret_line: usize,
    tokens: &[TokenSpec],
    tokens_gen: u64,
) -> (Vec<Span>, Vec<i32>) {
    let analysis = analyze_cached(text, format);
    let mut spans = Vec::new();
    // Per-line byte delta: (display leading bytes) − (source leading bytes).
    // 0 on the caret line and on paragraphs/code; negative where a marker is
    // hidden (headings/quotes); positive where a bullet glyph replaces "- ".
    let mut deltas: Vec<i32> = Vec::with_capacity(analysis.lines.len());

    for (li, (info, line)) in analysis.lines.iter().zip(text.split('\n')).enumerate() {
        if li > 0 {
            spans.push(Span {
                text: "\n".into(),
                marks: MarkSet::empty(),
                color: None,
                size: base,
            });
        }
        let on_caret = li == caret_line;
        let (line_spans, delta) = project_cached(info, line, on_caret, base, tokens, tokens_gen);
        deltas.push(delta);
        spans.extend(line_spans);
    }

    (spans, deltas)
}

/// Test/maintenance hook: drop the styling caches (so a fresh computation can be
/// compared against the incremental one).
#[cfg(test)]
pub(crate) fn clear_style_cache() {
    ANALYSIS_CACHE.with(|c| c.borrow_mut().clear());
    PROJECT_CACHE.with(|c| c.borrow_mut().clear());
    DECO_CACHE.with(|c| c.borrow_mut().clear());
}

/// Bullet substitution glyph: U+2022 + space (4 bytes, 2 chars). Replaces a
/// 2-byte `"- "`/`"* "`/`"+ "` marker, so the byte delta is `+2`.
const BULLET: &str = "• ";

/// Classify a prose line's hideable block marker → `(marker_len, repl, scale,
/// extra)`, dispatched by format. `marker_len == 0` means nothing is hidden
/// (paragraphs, numbered lists). See [`LineInfo`] for field meanings.
fn classify_block(format: Format, line: &str) -> (usize, &'static str, f32, MarkSet) {
    match format {
        Format::Typst => classify_block_typst(line),
        _ => classify_block_md(line),
    }
}

fn classify_block_md(line: &str) -> (usize, &'static str, f32, MarkSet) {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
        return (hashes + 1, "", heading_scale(hashes), MarkSet::BOLD); // "### "
    }
    if line.starts_with("> ") {
        return (2, "", 1.0, MarkSet::empty());
    }
    for marker in ["- ", "* ", "+ "] {
        if line.starts_with(marker) {
            return (2, BULLET, 1.0, MarkSet::empty());
        }
    }
    let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && line[digits..].starts_with(". ") {
        // Numbered markers carry meaning; keep them visible (no projection).
        return (0, "", 1.0, MarkSet::empty());
    }
    (0, "", 1.0, MarkSet::empty())
}

/// Typst block-marker classification (Level 1). Headings use `=` runs; `- ` is an
/// unordered list, `+ ` an enum (kept visible like Markdown's numbered lists).
fn classify_block_typst(line: &str) -> (usize, &'static str, f32, MarkSet) {
    let eqs = line.chars().take_while(|c| *c == '=').count();
    if (1..=6).contains(&eqs) && line[eqs..].starts_with(' ') {
        return (eqs + 1, "", heading_scale(eqs), MarkSet::BOLD); // "== "
    }
    if line.starts_with("- ") {
        return (2, BULLET, 1.0, MarkSet::empty());
    }
    if line.starts_with("+ ") {
        // Typst enum marker — keep visible (carries auto-numbering meaning).
        return (0, "", 1.0, MarkSet::empty());
    }
    (0, "", 1.0, MarkSet::empty())
}

/// Heading font-size multiplier by level (1 = largest).
fn heading_scale(level: usize) -> f32 {
    match level {
        1 => 1.9,
        2 => 1.55,
        3 => 1.3,
        4 => 1.15,
        _ => 1.05,
    }
}

fn line_code_colors(
    ranges: Option<&Vec<(usize, usize, [u8; 4])>>,
    len: usize,
) -> Vec<Option<[u8; 4]>> {
    let mut out = vec![None; len];
    if let Some(ranges) = ranges {
        for &(s, e, c) in ranges {
            for slot in out.iter_mut().take(e.min(len)).skip(s.min(len)) {
                *slot = Some(c);
            }
        }
    }
    out
}

thread_local! {
    // Per-prose-line strike/underline ranges (line-relative char offsets), keyed
    // by line text. Lets a keystroke re-scan only changed lines. Code lines are
    // never cached. Bounded; cleared wholesale when large.
    static DECO_CACHE: std::cell::RefCell<HashMap<u64, Vec<(usize, usize, Decoration)>>> =
        std::cell::RefCell::new(HashMap::new());
}

fn deco_key(format: Format, line: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (format as u8).hash(&mut h);
    line.hash(&mut h);
    h.finish()
}

/// Strike (`~~…~~`) and underline (links) ranges in global char offsets.
/// (Typst markup carries neither in Level-1 projection, so this yields nothing
/// for Typst.)
pub fn decorations(text: &str, format: Format) -> Vec<(usize, usize, Decoration)> {
    let mut out = Vec::new();
    let mut global = 0usize;
    let mut in_fence = false;
    for line in text.split('\n') {
        let is_fence = line.trim_start().starts_with("```");
        let line_is_code = in_fence || is_fence;
        if is_fence {
            in_fence = !in_fence;
        }
        if !line_is_code {
            let key = deco_key(format, line);
            // Cached ranges are line-relative; shift them by the line's global
            // char offset.
            let rel = DECO_CACHE.with(|c| {
                if let Some(hit) = c.borrow().get(&key) {
                    return hit.clone();
                }
                let m = line_marks(format, line);
                let mut v = Vec::new();
                push_runs(&m, MarkSet::STRIKE, Decoration::Strike, 0, &mut v);
                push_runs(&m, MarkSet::LINK, Decoration::Underline, 0, &mut v);
                let mut mm = c.borrow_mut();
                if mm.len() > 16384 {
                    mm.clear();
                }
                mm.insert(key, v.clone());
                v
            });
            for &(s, e, d) in &rel {
                out.push((global + s, global + e, d));
            }
        }
        global += line.chars().count() + 1; // + '\n'
    }
    out
}

/// If the caret is inside a `[text](url)` link, return the URL's char range and
/// the URL string.
pub fn link_at(text: &str, cursor: usize) -> Option<(Range<usize>, String)> {
    let mut global = 0usize;
    for line in text.split('\n') {
        let n = line.chars().count();
        if cursor >= global && cursor <= global + n {
            let local = cursor - global;
            for lk in links_in_line(line) {
                if local >= lk.start && local <= lk.end {
                    return Some((
                        (global + lk.url_start)..(global + lk.url_end),
                        lk.url,
                    ));
                }
            }
        }
        global += n + 1;
    }
    None
}

// ---- inline scanner --------------------------------------------------------

/// Per-character inline marks for one line, dispatched by format.
fn line_marks(format: Format, line: &str) -> Vec<MarkSet> {
    match format {
        Format::Typst => line_marks_typst(line),
        _ => line_marks_md(line),
    }
}

/// Markdown inline marks, parsed by `pulldown-cmark` (the CommonMark reference
/// parser, + GFM strikethrough) rather than a hand-rolled scanner — so nesting,
/// mismatched delimiters, code-span protection, and links follow the spec.
///
/// Operates on the line's *display* text (block markers already projected away),
/// so emphasis/strong/code/strike/link spans line up with the rendered glyphs.
/// Inline parsing is intra-line; cross-line constructs (reference-link
/// definitions, setext headings) are a block-level / Phase-2 concern.
fn line_marks_md(line: &str) -> Vec<MarkSet> {
    use pulldown_cmark::{Event, Options, Parser, Tag};
    let n = line.chars().count();
    let mut marks = vec![MarkSet::empty(); n];
    if line.is_empty() {
        return marks;
    }
    // Map source byte offset (at char boundaries) → char index.
    let mut b2c = vec![n; line.len() + 1];
    for (ci, (bi, _)) in line.char_indices().enumerate() {
        b2c[bi] = ci;
    }
    b2c[line.len()] = n;

    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    for (ev, range) in Parser::new_ext(line, opts).into_offset_iter() {
        let bit = match ev {
            Event::Start(Tag::Strong) => MarkSet::BOLD,
            Event::Start(Tag::Emphasis) => MarkSet::ITALIC,
            Event::Start(Tag::Strikethrough) => MarkSet::STRIKE,
            Event::Start(Tag::Link { .. }) => MarkSet::LINK,
            Event::Code(_) => MarkSet::CODE,
            _ => continue,
        };
        let (cs, ce) = (b2c[range.start.min(line.len())], b2c[range.end.min(line.len())]);
        for slot in marks.iter_mut().take(ce.min(n)).skip(cs) {
            slot.insert(bit);
        }
    }
    marks
}

/// Typst inline marks (MF2), driven by `typst-syntax`'s own highlighter — the
/// official Typst parser + `highlight()` categorizer — so strong/emph/raw/math,
/// references/labels, and code-mode constructs (`#let`, function calls, numbers,
/// strings, keywords) all follow the real grammar.
///
/// Operates on the line's *display* text. Block-level marker hiding + deltas stay
/// in `project_line_typst`; this only assigns inline styling, so it can't affect
/// the byte-delta/cursor mapping.
fn line_marks_typst(line: &str) -> Vec<MarkSet> {
    use typst_syntax::{parse, LinkedNode};
    let n = line.chars().count();
    let mut marks = vec![MarkSet::empty(); n];
    if line.is_empty() {
        return marks;
    }
    let mut b2c = vec![n; line.len() + 1];
    for (ci, (bi, _)) in line.char_indices().enumerate() {
        b2c[bi] = ci;
    }
    b2c[line.len()] = n;
    let root = parse(line);
    typst_marks_walk(&LinkedNode::new(&root), &b2c, n, &mut marks);
    marks
}

fn typst_marks_walk(node: &typst_syntax::LinkedNode, b2c: &[usize], n: usize, marks: &mut [MarkSet]) {
    use typst_syntax::{highlight, SyntaxKind, Tag};
    // `highlight()` returns None for the `Equation` node (its `$` + inner tokens
    // are tagged separately); style the whole math span like code instead.
    let bit = if node.kind() == SyntaxKind::Equation {
        Some(MarkSet::CODE)
    } else {
        match highlight(node) {
            Some(Tag::Strong) => Some(MarkSet::BOLD),
            Some(Tag::Emph) => Some(MarkSet::ITALIC),
            Some(Tag::Raw) | Some(Tag::MathDelimiter) | Some(Tag::MathOperator) => Some(MarkSet::CODE),
            Some(Tag::Link) | Some(Tag::Ref) | Some(Tag::Label) => Some(MarkSet::LINK),
            Some(Tag::Keyword)
            | Some(Tag::Function)
            | Some(Tag::Number)
            | Some(Tag::String)
            | Some(Tag::Interpolated)
            | Some(Tag::Operator)
            | Some(Tag::Punctuation) => Some(MarkSet::CODE),
            // Heading / ListMarker / Comment / Escape / Error: block-level or not
            // styled inline.
            _ => None,
        }
    };
    if let Some(bit) = bit {
        let r = node.range();
        let cs = *b2c.get(r.start).unwrap_or(&n);
        let ce = *b2c.get(r.end).unwrap_or(&n);
        for slot in marks.iter_mut().take(ce.min(n)).skip(cs.min(n)) {
            slot.insert(bit);
        }
    }
    for child in node.children() {
        typst_marks_walk(&child, b2c, n, marks);
    }
}

struct LinkSpan {
    start: usize,
    end: usize, // inclusive last char index
    url_start: usize,
    url_end: usize, // exclusive
    url: String,
}

fn links_in_line(line: &str) -> Vec<LinkSpan> {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if chars[i] == '[' {
            if let Some(rb) = (i + 1..n).find(|&k| chars[k] == ']') {
                if rb + 1 < n && chars[rb + 1] == '(' {
                    if let Some(rp) = (rb + 2..n).find(|&k| chars[k] == ')') {
                        let url: String = chars[rb + 2..rp].iter().collect();
                        out.push(LinkSpan {
                            start: i,
                            end: rp,
                            url_start: rb + 2,
                            url_end: rp,
                            url,
                        });
                        i = rp + 1;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    out
}

// ---- syntax highlighting (syntect, feature-gated) -------------------------

/// Scan fenced code blocks (```lang … ```) and return per-line highlight colors.
/// No-op (empty) unless the `syntax-highlight` feature is enabled.
#[cfg(not(feature = "syntax-highlight"))]
fn code_highlights(_text: &str) -> CodeHighlights {
    CodeHighlights::new()
}

#[cfg(feature = "syntax-highlight")]
fn code_highlights(text: &str) -> CodeHighlights {
    use std::sync::OnceLock;
    use syntect::highlighting::{Theme, ThemeSet};
    use syntect::parsing::SyntaxSet;

    static RES: OnceLock<(SyntaxSet, Theme)> = OnceLock::new();
    let (ps, theme) = RES.get_or_init(|| {
        let ps = SyntaxSet::load_defaults_newlines();
        let mut ts = ThemeSet::load_defaults();
        // A light theme to match the default light background.
        let theme = ts
            .themes
            .remove("InspiredGitHub")
            .unwrap_or_else(|| ts.themes.values().next().cloned().unwrap());
        (ps, theme)
    });

    let mut out = CodeHighlights::new();
    let lines: Vec<&str> = text.split('\n').collect();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        let lang = trimmed.strip_prefix("```").map(str::trim);
        let Some(lang) = lang.filter(|l| !l.is_empty()) else {
            i += 1;
            continue;
        };
        // Collect the block body until the closing fence.
        let body_start = i + 1;
        let mut j = body_start;
        while j < lines.len() && !lines[j].trim_start().starts_with("```") {
            j += 1;
        }
        let body = lines[body_start..j].join("\n");
        // Cached per block by content — unchanged code blocks aren't re-highlighted.
        for (rel, spans) in highlight_block(ps, theme, lang, &body) {
            out.insert(body_start + rel, spans);
        }
        i = j; // resume at the closing fence (or end)
    }
    out
}

/// Syntax-highlight one fenced code block → per-line column ranges (relative to
/// the block start), memoized by `(lang, body)` content hash so re-rendering an
/// unchanged block is free.
#[cfg(feature = "syntax-highlight")]
fn highlight_block(
    ps: &syntect::parsing::SyntaxSet,
    theme: &syntect::highlighting::Theme,
    lang: &str,
    body: &str,
) -> Vec<(usize, Vec<(usize, usize, [u8; 4])>)> {
    use std::cell::RefCell;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use syntect::easy::HighlightLines;
    use syntect::util::LinesWithEndings;

    thread_local! {
        static HL_CACHE: RefCell<HashMap<u64, Vec<(usize, Vec<(usize, usize, [u8; 4])>)>>> =
            RefCell::new(HashMap::new());
    }

    let mut hasher = DefaultHasher::new();
    lang.hash(&mut hasher);
    body.hash(&mut hasher);
    let key = hasher.finish();
    if let Some(cached) = HL_CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return cached;
    }

    let syntax = ps
        .find_syntax_by_token(lang)
        .unwrap_or_else(|| ps.find_syntax_plain_text());
    let mut hl = HighlightLines::new(syntax, theme);
    let mut block = Vec::new();
    for (rel, line) in LinesWithEndings::from(body).enumerate() {
        let Ok(ranges) = hl.highlight_line(line, ps) else {
            continue;
        };
        let mut col = 0usize;
        let mut spans = Vec::new();
        for (style, piece) in ranges {
            let n = piece.trim_end_matches(['\n', '\r']).chars().count();
            if n > 0 {
                let c = style.foreground;
                spans.push((col, col + n, [c.r, c.g, c.b, 255]));
            }
            col += piece.chars().count();
        }
        if !spans.is_empty() {
            block.push((rel, spans));
        }
    }

    HL_CACHE.with(|c| {
        let mut m = c.borrow_mut();
        if m.len() > 256 {
            m.clear(); // simple bound; blocks re-highlight lazily
        }
        m.insert(key, block.clone());
    });
    block
}

fn push_runs(
    marks: &[MarkSet],
    bit: MarkSet,
    deco: Decoration,
    global: usize,
    out: &mut Vec<(usize, usize, Decoration)>,
) {
    let mut i = 0;
    while i < marks.len() {
        if marks[i].contains(bit) {
            let start = i;
            while i < marks.len() && marks[i].contains(bit) {
                i += 1;
            }
            out.push((global + start, global + i, deco));
        } else {
            i += 1;
        }
    }
}
