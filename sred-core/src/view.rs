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
    /// A colored under-line marking a misspelling (spellcheck).
    Squiggle([u8; 4]),
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

/// A syntax-highlight category for per-token coloring (Step D). The *category* is
/// theme-independent (so it lives in the text-keyed analysis cache); the concrete
/// RGBA is resolved at projection time via a host-supplied [`SynPalette`].
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum SynCat {
    Keyword,
    Function,
    Number,
    Str,
    Comment,
    Operator,
}

/// Host palette mapping each [`SynCat`] to an RGBA color, so per-token syntax
/// colors are themeable. Build one from your editor theme; [`SynPalette::DEFAULT`]
/// is a light-background default.
#[derive(Clone, Copy)]
pub struct SynPalette {
    pub keyword: [u8; 4],
    pub function: [u8; 4],
    pub number: [u8; 4],
    pub string: [u8; 4],
    pub comment: [u8; 4],
    pub operator: [u8; 4],
}

impl SynPalette {
    /// Light-background defaults (used when a host hasn't supplied a palette).
    pub const DEFAULT: SynPalette = SynPalette {
        keyword: [167, 29, 93, 255],   // magenta
        function: [0, 92, 197, 255],   // blue
        number: [0, 134, 109, 255],    // teal
        string: [3, 47, 98, 255],      // deep blue
        comment: [150, 152, 150, 255], // gray
        operator: [80, 60, 130, 255],  // violet
    };

    fn of(&self, cat: SynCat) -> [u8; 4] {
        match cat {
            SynCat::Keyword => self.keyword,
            SynCat::Function => self.function,
            SynCat::Number => self.number,
            SynCat::Str => self.string,
            SynCat::Comment => self.comment,
            SynCat::Operator => self.operator,
        }
    }
}

impl Default for SynPalette {
    fn default() -> Self {
        SynPalette::DEFAULT
    }
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

/// A line's hideable block marker. Off the caret line, `src[start..start+len]` is
/// replaced by `repl` (the leading `src[..start]` indentation is always shown);
/// on the caret line the marker is shown verbatim. The byte delta is therefore
/// `repl.len() − len`, independent of indentation.
#[derive(Clone, Copy)]
struct Marker {
    /// Byte offset where the hideable marker begins (indentation before it stays).
    start: usize,
    /// Byte length of the hideable marker (`"## "`, `"- "`, `"> "`, `"- [ ] "`, or
    /// a whole ```` ``` ```` fence / setext-underline line).
    len: usize,
    /// Substitute shown when hidden: `""` to drop it, `"• "`/checkbox to replace.
    repl: &'static str,
}

impl Marker {
    /// No hideable marker (paragraphs, numbered lists, code interiors, tables).
    const NONE: Marker = Marker {
        start: 0,
        len: 0,
        repl: "",
    };
    /// A leading marker (no indentation) of `len` bytes shown as `repl` when hidden.
    fn lead(len: usize, repl: &'static str) -> Marker {
        Marker {
            start: 0,
            len,
            repl,
        }
    }
}

/// Caret-independent per-line facts produced by [`analyze`]. Everything a full
/// parse determines that does NOT depend on where the caret is.
#[derive(Clone)]
struct LineInfo {
    /// The hideable block marker (Live Preview), or [`Marker::NONE`].
    marker: Marker,
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
    /// Per-SOURCE-char concrete colors (syntect for fenced code). `None` where
    /// uncolored; prose syntax coloring goes through `syn` instead so it stays
    /// themeable.
    colors: Vec<Option<[u8; 4]>>,
    /// Per-SOURCE-char syntax category (Step D), resolved to a color via the
    /// host [`SynPalette`] at projection time. `None` where uncategorized.
    syn: Vec<Option<SynCat>>,
    /// Hash of all of the above + the source text: the [`project`] cache seed.
    /// Captures every cross-line dependency (fence state, setext, refs) so a
    /// per-line project cache stays correct even when a line's rendering depends
    /// on its neighbors.
    digest: u64,
}

impl LineInfo {
    #[allow(clippy::too_many_arguments)]
    fn new(
        line: &str,
        marker: Marker,
        scale: f32,
        extra: MarkSet,
        is_code: bool,
        inline: Vec<MarkSet>,
        colors: Vec<Option<[u8; 4]>>,
        syn: Vec<Option<SynCat>>,
    ) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        line.hash(&mut h);
        marker.start.hash(&mut h);
        marker.len.hash(&mut h);
        marker.repl.hash(&mut h);
        scale.to_bits().hash(&mut h);
        extra.bits().hash(&mut h);
        is_code.hash(&mut h);
        // Sparse hashing: per-char vecs are mostly empty for prose, so hash only
        // the populated entries (with their index) + the length. This keeps the
        // digest O(marked chars) instead of O(line length) — the dominant cost
        // when rebuilding every line's info each keystroke (#18).
        inline.len().hash(&mut h);
        for (i, m) in inline.iter().enumerate() {
            if !m.is_empty() {
                i.hash(&mut h);
                m.bits().hash(&mut h);
            }
        }
        colors.len().hash(&mut h);
        for (i, c) in colors.iter().enumerate() {
            if let Some(c) = c {
                i.hash(&mut h);
                c.hash(&mut h);
            }
        }
        syn.len().hash(&mut h);
        for (i, s) in syn.iter().enumerate() {
            if let Some(s) = s {
                i.hash(&mut h);
                s.hash(&mut h);
            }
        }
        let digest = h.finish();
        LineInfo {
            marker,
            scale,
            extra,
            is_code,
            inline,
            colors,
            syn,
            digest,
        }
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
/// base font size, token generation and syntax palette that `project` also reads.
fn project_key(digest: u64, on_caret: bool, base: f32, tokens_gen: u64, syn: &SynPalette) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    digest.hash(&mut h);
    on_caret.hash(&mut h);
    base.to_bits().hash(&mut h);
    tokens_gen.hash(&mut h);
    for c in [
        syn.keyword,
        syn.function,
        syn.number,
        syn.string,
        syn.comment,
        syn.operator,
    ] {
        c.hash(&mut h);
    }
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
/// is why this is doc-level), highlights code once, and — for Markdown — runs a
/// single whole-document `pulldown-cmark` pass ([`scan_md`]) so cross-line
/// constructs (setext headings, indented code, reference links, tables) and all
/// inline marks come from the reference parser.
fn analyze(text: &str, format: Format) -> DocAnalysis {
    let perf = std::env::var_os("SRED_PERF").is_some();
    let t0 = std::time::Instant::now();
    let highlights = code_highlights(text);
    let raw: Vec<&str> = text.split('\n').collect();
    // Fence membership first: a ``` delimiter / its interior is code regardless
    // of what the block parser thinks, and is hidden/shown by caret state below.
    let mut is_fence = vec![false; raw.len()];
    let mut interior = vec![false; raw.len()];
    {
        let mut in_fence = false;
        for (i, line) in raw.iter().enumerate() {
            is_fence[i] = line.trim_start().starts_with("```");
            interior[i] = in_fence && !is_fence[i];
            if is_fence[i] {
                in_fence = !in_fence;
            }
        }
    }
    let md = matches!(format, Format::Markdown).then(|| scan_md(text, &raw, &is_fence, &interior));
    let typ = matches!(format, Format::Typst).then(|| scan_typst(text, &raw));
    let t_parse = t0.elapsed();

    // Consume the per-line facts (move their vecs into `LineInfo` rather than
    // clone) by advancing an iterator in lockstep with the lines (#18).
    let mut md_it = md.map(|v| v.into_iter());
    let mut typ_it = typ.map(|v| v.into_iter());
    let mut lines = Vec::with_capacity(raw.len());
    for (li, &line) in raw.iter().enumerate() {
        let md_f = md_it
            .as_mut()
            .map(|it| it.next().expect("one fact per line"));
        let typ_f = typ_it
            .as_mut()
            .map(|it| it.next().expect("one fact per line"));
        let info = if is_fence[li] || interior[li] {
            // Fence delimiter hides fully off-caret (marker = whole line); an
            // interior line is always shown (no marker). Facts (if any) are unused.
            let n = line.chars().count();
            let marker = if is_fence[li] {
                Marker::lead(line.len(), "")
            } else {
                Marker::NONE
            };
            let colors = line_code_colors(highlights.get(&li), n);
            LineInfo::new(
                line,
                marker,
                1.0,
                MarkSet::CODE,
                true,
                vec![MarkSet::empty(); n],
                colors,
                vec![None; n],
            )
        } else if let Some(f) = md_f {
            analyze_prose_md(line, f)
        } else if let Some(f) = typ_f {
            analyze_prose_typst(line, f)
        } else {
            unreachable!("format is Markdown or Typst")
        };
        lines.push(info);
    }
    if perf {
        eprintln!(
            "SRED_PERF analyze: parse={:?} build={:?} lines={}",
            t_parse,
            t0.elapsed() - t_parse,
            raw.len()
        );
    }
    DocAnalysis { lines }
}

/// Build a prose Markdown line's [`LineInfo`] from its parser-derived facts:
/// setext headings/underlines and indented code override the per-line block
/// classification; everything else uses [`classify_block_md`] for the marker and
/// the whole-document inline marks for styling.
fn analyze_prose_md(line: &str, f: MdLineFacts) -> LineInfo {
    let n = line.chars().count();
    if f.setext_underline {
        // The `===` / `---` line: hide it fully off-caret (it only existed to mark
        // the line above as a heading); show it verbatim on the caret line.
        return LineInfo::new(
            line,
            Marker::lead(line.len(), ""),
            1.0,
            MarkSet::empty(),
            false,
            vec![MarkSet::empty(); n],
            vec![None; n],
            vec![None; n],
        );
    }
    if f.indented_code {
        return LineInfo::new(
            line,
            Marker::NONE,
            1.0,
            MarkSet::CODE,
            true,
            vec![MarkSet::empty(); n],
            vec![None; n],
            vec![None; n],
        );
    }
    let (marker, scale, extra) = if let Some(level) = f.setext_level {
        // Setext title: the text stays (no marker), but scales up + bolds.
        (Marker::NONE, heading_scale(level as usize), MarkSet::BOLD)
    } else {
        classify_block_md(line)
    };
    // Markdown prose carries no per-token syntax colors (code blocks colour via
    // syntect in `colors`); only Typst code-mode does. Move `inline` in (no clone).
    LineInfo::new(
        line,
        marker,
        scale,
        extra,
        false,
        f.inline,
        vec![None; n],
        vec![None; n],
    )
}

fn analyze_prose_typst(line: &str, f: TypstLineFacts) -> LineInfo {
    let n = line.chars().count();
    LineInfo::new(
        line,
        f.marker,
        f.scale,
        f.extra,
        false,
        f.inline,
        vec![None; n],
        f.syn,
    )
}

/// Per-line output of the whole-document Typst scan ([`scan_typst`]).
#[derive(Clone)]
struct TypstLineFacts {
    /// Per-SOURCE-char inline marks (from the typst-syntax highlighter).
    inline: Vec<MarkSet>,
    /// Per-SOURCE-char syntax category for code-mode coloring (Step D).
    syn: Vec<Option<SynCat>>,
    /// The line's hideable block marker (heading `=`-run, `- ` list, `/ ` term),
    /// or [`Marker::NONE`] (paragraphs, `+ ` enums kept visible).
    marker: Marker,
    /// Heading font-size multiplier; `1.0` otherwise.
    scale: f32,
    /// Whole-line marks (heading `BOLD`).
    extra: MarkSet,
}

/// One whole-document `typst-syntax` parse → per-line inline marks + block
/// markers, read from the real grammar tree (Step C). Heading depth comes from
/// the `HeadingMarker` length; `ListItem`/`EnumItem`/`TermItem` are recognised by
/// their marker nodes; marker byte ranges (and thus deltas) come from node
/// ranges, so nested/indented markers keep their indentation.
fn scan_typst(text: &str, lines: &[&str]) -> Vec<TypstLineFacts> {
    use typst_syntax::{parse, LinkedNode};

    let mut facts: Vec<TypstLineFacts> = lines
        .iter()
        .map(|l| TypstLineFacts {
            inline: vec![MarkSet::empty(); l.chars().count()],
            syn: vec![None; l.chars().count()],
            marker: Marker::NONE,
            scale: 1.0,
            extra: MarkSet::empty(),
        })
        .collect();

    let mut starts = Vec::with_capacity(lines.len());
    {
        let mut off = 0usize;
        for l in lines {
            starts.push(off);
            off += l.len() + 1;
        }
    }
    let line_of = |byte: usize| -> usize {
        match starts.binary_search(&byte) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        }
    };

    let bytes = text.as_bytes();
    let root = parse(text);
    walk_typst(
        &LinkedNode::new(&root),
        text,
        bytes,
        &starts,
        &line_of,
        &mut facts,
    );
    facts
}

fn walk_typst(
    node: &typst_syntax::LinkedNode,
    text: &str,
    bytes: &[u8],
    starts: &[usize],
    line_of: &impl Fn(usize) -> usize,
    facts: &mut [TypstLineFacts],
) {
    use typst_syntax::SyntaxKind;
    let r = node.range();
    // Block marker (the leading construct on its line).
    let k = node.kind();
    if matches!(
        k,
        SyntaxKind::HeadingMarker
            | SyntaxKind::ListMarker
            | SyntaxKind::EnumMarker
            | SyntaxKind::TermMarker
    ) {
        let li = line_of(r.start);
        let col = r.start - starts[li];
        // Only the first marker on a line is the block marker (outer of nested).
        if facts[li].marker.len == 0
            && facts[li].extra.is_empty()
            && col_is_leading(text, starts[li], r.start)
        {
            let mlen = r.end - r.start;
            let space = usize::from(bytes.get(r.end) == Some(&b' '));
            match k {
                SyntaxKind::HeadingMarker => {
                    facts[li].marker = Marker {
                        start: col,
                        len: mlen + space,
                        repl: "",
                    };
                    facts[li].scale = heading_scale(mlen);
                    facts[li].extra = MarkSet::BOLD;
                }
                SyntaxKind::ListMarker => {
                    facts[li].marker = Marker {
                        start: col,
                        len: mlen + space,
                        repl: BULLET,
                    };
                }
                SyntaxKind::TermMarker => {
                    facts[li].marker = Marker {
                        start: col,
                        len: mlen + space,
                        repl: "",
                    };
                }
                // Enum `+ ` markers carry auto-numbering meaning → kept visible.
                _ => {}
            }
        }
    }
    // Inline mark + syntax category, clipped per line (a node may span lines).
    let bit = typst_inline_bit(node);
    let cat = typst_syn_cat(node);
    if bit.is_some() || cat.is_some() {
        let mut b = r.start;
        while b < r.end {
            let li = line_of(b);
            let lstart = starts[li];
            let lend = lstart + facts_line_len(text, lstart);
            let seg_end = r.end.min(lend);
            if seg_end > b {
                let cs = text[lstart..b].chars().count();
                let ce = text[lstart..seg_end].chars().count();
                if let Some(bit) = bit {
                    for slot in facts[li].inline[cs..ce].iter_mut() {
                        slot.insert(bit);
                    }
                }
                if let Some(cat) = cat {
                    for slot in facts[li].syn[cs..ce].iter_mut() {
                        *slot = Some(cat);
                    }
                }
            }
            b = (lend + 1).max(b + 1);
        }
    }
    for child in node.children() {
        walk_typst(&child, text, bytes, starts, line_of, facts);
    }
}

/// True when the bytes between the line start and `pos` are only spaces (so the
/// marker is genuinely the line's leading construct, not mid-line content).
fn col_is_leading(text: &str, line_start: usize, pos: usize) -> bool {
    text[line_start..pos].bytes().all(|b| b == b' ')
}

/// Byte length of the line starting at `line_start` (up to the next '\n' or EOF).
fn facts_line_len(text: &str, line_start: usize) -> usize {
    text[line_start..].split('\n').next().map_or(0, str::len)
}

/// Per-line output of the whole-document Markdown scan ([`scan_md`]).
#[derive(Clone)]
struct MdLineFacts {
    /// Per-SOURCE-char inline marks (strong/emph/code/strike/link incl. reference
    /// links), plus table styling (header bold, pipes code).
    inline: Vec<MarkSet>,
    /// `Some(level)` when this line is the *title* of a setext heading.
    setext_level: Option<u8>,
    /// This line is a setext `===`/`---` underline (hidden off-caret).
    setext_underline: bool,
    /// This line renders as code (indented code block or raw HTML block): CODE
    /// mark, no marker hiding, source kept verbatim.
    indented_code: bool,
}

/// One whole-document `pulldown-cmark` pass (the CommonMark reference parser, with
/// GFM strikethrough/tables/task-lists). Produces per-line inline marks and the
/// cross-line block facts (setext, indented code, tables) that a per-line scan
/// can't see. Reference links resolve here for free — the parser sees the whole
/// document, so `[text][ref]` against a `[ref]: url` defined elsewhere is a LINK.
///
/// Fenced-code lines (tracked separately, authoritatively, by [`analyze`]) are
/// left blank here; the marker hiding + deltas for ATX headings, quotes and lists
/// stay in [`classify_block_md`]. This pass only contributes inline marks and the
/// setext/indented-code/table classifications.
fn scan_md(text: &str, lines: &[&str], is_fence: &[bool], interior: &[bool]) -> Vec<MdLineFacts> {
    use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag};

    let mut facts: Vec<MdLineFacts> = lines
        .iter()
        .map(|l| MdLineFacts {
            inline: vec![MarkSet::empty(); l.chars().count()],
            setext_level: None,
            setext_underline: false,
            indented_code: false,
        })
        .collect();

    // Byte offset of each line's start in `text`, for mapping parser byte ranges
    // back to (line, char-column).
    let mut starts = Vec::with_capacity(lines.len());
    {
        let mut off = 0usize;
        for l in lines {
            starts.push(off);
            off += l.len() + 1; // + '\n'
        }
    }
    let line_of = |byte: usize| -> usize {
        match starts.binary_search(&byte) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        }
    };
    // Apply `bit` over a (possibly multi-line) byte range, clipped per line.
    let apply = |facts: &mut [MdLineFacts], s: usize, e: usize, bit: MarkSet| {
        let mut b = s;
        while b < e {
            let li = line_of(b);
            let lstart = starts[li];
            let lend = lstart + lines[li].len(); // exclusive of the '\n'
            let seg_end = e.min(lend);
            if seg_end > b {
                let cs = lines[li][..b - lstart].chars().count();
                let ce = lines[li][..seg_end - lstart].chars().count();
                for slot in facts[li].inline[cs..ce].iter_mut() {
                    slot.insert(bit);
                }
            }
            b = (lend + 1).max(b + 1); // advance to next line (skip the '\n')
        }
    };

    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_TASKLISTS);

    let mut tables: Vec<(usize, usize)> = Vec::new(); // (first line, last line)
    for (ev, r) in Parser::new_ext(text, opts).into_offset_iter() {
        match ev {
            Event::Start(Tag::Strong) => apply(&mut facts, r.start, r.end, MarkSet::BOLD),
            Event::Start(Tag::Emphasis) => apply(&mut facts, r.start, r.end, MarkSet::ITALIC),
            Event::Start(Tag::Strikethrough) => apply(&mut facts, r.start, r.end, MarkSet::STRIKE),
            Event::Start(Tag::Link { .. }) => apply(&mut facts, r.start, r.end, MarkSet::LINK),
            Event::Code(_) => apply(&mut facts, r.start, r.end, MarkSet::CODE),
            Event::Start(Tag::Heading { level, .. }) => {
                // A setext heading's source spans >1 line (title + underline);
                // an ATX heading is single-line (handled by classify_block_md).
                let l0 = line_of(r.start);
                let l1 = line_of(r.end.saturating_sub(1));
                if l1 > l0 {
                    facts[l0].setext_level = Some(level as u8);
                    for f in &mut facts[l0 + 1..=l1] {
                        f.setext_underline = true;
                    }
                }
            }
            Event::Start(Tag::CodeBlock(CodeBlockKind::Indented))
            | Event::Start(Tag::HtmlBlock) => {
                // Indented code and raw HTML blocks both render as code (source
                // kept, no marker hiding → delta 0).
                let l0 = line_of(r.start);
                let hi = line_of(r.end.saturating_sub(1)).min(lines.len().saturating_sub(1));
                for f in &mut facts[l0..=hi] {
                    f.indented_code = true;
                }
            }
            Event::Start(Tag::Table(_)) => {
                tables.push((line_of(r.start), line_of(r.end.saturating_sub(1))));
            }
            _ => {}
        }
    }

    // Table styling: bold the header row, code the column separators. Source is
    // kept (no marker), so this is purely cosmetic inline marking.
    for (l0, l1) in tables {
        for li in l0..=l1.min(lines.len().saturating_sub(1)) {
            if is_fence[li] || interior[li] {
                continue;
            }
            for (ci, c) in lines[li].chars().enumerate() {
                if li == l0 {
                    facts[li].inline[ci].insert(MarkSet::BOLD);
                }
                if c == '|' {
                    facts[li].inline[ci].insert(MarkSet::CODE);
                }
            }
        }
    }

    facts
}

/// Project one line for display (caret-DEPENDENT, cheap): hide/substitute its
/// block marker unless it's the caret line, map the caret-independent inline
/// marks + colors into display-char space, overlay host token colors, and emit
/// spans. Returns the spans and the byte delta `display_leading − source_leading`.
fn project_line(
    info: &LineInfo,
    line: &str,
    on_caret: bool,
    base: f32,
    tokens: &[TokenSpec],
    palette: &SynPalette,
) -> (Vec<Span>, i32) {
    let size = base * info.scale;
    let m = info.marker;
    let reveal = on_caret || m.len == 0;
    let (display, delta) = if reveal {
        (line.to_string(), 0)
    } else {
        let mut d = String::with_capacity(m.start + m.repl.len() + line.len() - (m.start + m.len));
        d.push_str(&line[..m.start]); // indentation, shown verbatim
        d.push_str(m.repl);
        d.push_str(&line[m.start + m.len..]);
        (d, m.repl.len() as i32 - m.len as i32)
    };

    let disp_n = display.chars().count();
    let mut marks = vec![MarkSet::empty(); disp_n];
    let mut colors: Vec<Option<[u8; 4]>> = vec![None; disp_n];
    let mut syn: Vec<Option<SynCat>> = vec![None; disp_n];
    if reveal {
        // Marker shown verbatim: source ↔ display chars line up 1:1.
        for i in 0..disp_n {
            marks[i] = info.inline[i] | info.extra;
            colors[i] = info.colors[i];
            syn[i] = info.syn[i];
        }
    } else {
        // Hidden: [indent] [repl] [source past the marker]. The indentation maps
        // 1:1 from source; the repl substitute carries only `extra`; the tail is
        // the source after the marker, shifted into display space.
        let indent_chars = line[..m.start].chars().count();
        let repl_chars = m.repl.chars().count();
        let marker_src_chars = line[m.start..m.start + m.len].chars().count();
        for k in 0..indent_chars {
            marks[k] = info.inline[k] | info.extra;
            colors[k] = info.colors[k];
            syn[k] = info.syn[k];
        }
        for slot in marks.iter_mut().skip(indent_chars).take(repl_chars) {
            *slot = info.extra;
        }
        let head = indent_chars + repl_chars;
        for k in 0..disp_n.saturating_sub(head) {
            let si = indent_chars + marker_src_chars + k;
            marks[head + k] = info.inline[si] | info.extra;
            colors[head + k] = info.colors[si];
            syn[head + k] = info.syn[si];
        }
    }

    // Resolve syntax categories to colors where no concrete color is set yet
    // (precedence: token > concrete/syntax > mark-derived). Code lines colour via
    // `colors` (syntect); prose typst code-mode colours via `syn` + palette.
    for i in 0..disp_n {
        if colors[i].is_none() {
            if let Some(cat) = syn[i] {
                colors[i] = Some(palette.of(cat));
            }
        }
    }

    // Host tokens are matched on the display text and override syntax colors.
    // Code lines aren't tokenized.
    if !info.is_code && !tokens.is_empty() {
        for spec in tokens {
            for m in (spec.matcher)(&display) {
                for slot in colors
                    .iter_mut()
                    .take(m.end.min(disp_n))
                    .skip(m.start.min(disp_n))
                {
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
        out.push(Span {
            text: chars[start..j].iter().collect(),
            marks: mk,
            color: col,
            size,
        });
    }
    (out, delta)
}

#[allow(clippy::too_many_arguments)]
fn project_cached(
    info: &LineInfo,
    line: &str,
    on_caret: bool,
    base: f32,
    tokens: &[TokenSpec],
    tokens_gen: u64,
    palette: &SynPalette,
) -> (Vec<Span>, i32) {
    let key = project_key(info.digest, on_caret, base, tokens_gen, palette);
    PROJECT_CACHE.with(|c| {
        if let Some(hit) = c.borrow().get(&key) {
            return hit.clone();
        }
        let computed = project_line(info, line, on_caret, base, tokens, palette);
        let mut m = c.borrow_mut();
        if m.len() > 16384 {
            m.clear();
        }
        m.insert(key, computed.clone());
        computed
    })
}

/// Style the document with the default syntax palette. See [`styled_runs_with`]
/// to supply a host palette for themeable per-token colors.
pub fn styled_runs(
    text: &str,
    format: Format,
    base: f32,
    caret_line: usize,
    tokens: &[TokenSpec],
    tokens_gen: u64,
) -> (Vec<Span>, Vec<i32>) {
    styled_runs_with(
        text,
        format,
        base,
        caret_line,
        tokens,
        tokens_gen,
        &SynPalette::DEFAULT,
    )
}

/// As [`styled_runs`], but with a host-supplied [`SynPalette`] for per-token
/// syntax colors (Step D).
#[allow(clippy::too_many_arguments)]
pub fn styled_runs_with(
    text: &str,
    format: Format,
    base: f32,
    caret_line: usize,
    tokens: &[TokenSpec],
    tokens_gen: u64,
    palette: &SynPalette,
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
        let (line_spans, delta) =
            project_cached(info, line, on_caret, base, tokens, tokens_gen, palette);
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
/// Task-list checkbox glyphs (each + a trailing space = 4 bytes, 2 chars).
/// Replace the 6-byte `"- [ ] "` / `"- [x] "` marker (byte delta `−2`).
const TASK_OPEN: &str = "☐ "; // U+2610 BALLOT BOX
const TASK_DONE: &str = "☑ "; // U+2611 BALLOT BOX WITH CHECK

/// Leading-space indentation of a line, in bytes (= chars, spaces are ASCII).
fn indent_width(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// Classify a prose Markdown line's hideable block marker → `(marker, scale,
/// extra)`. Indentation before the marker is preserved (nested lists/quotes);
/// numbered markers stay visible. Setext/indented-code are decided in [`scan_md`].
fn classify_block_md(line: &str) -> (Marker, f32, MarkSet) {
    let indent = indent_width(line);
    let body = &line[indent..];
    // ATX heading (CommonMark allows up to 3 leading spaces).
    let hashes = body.chars().take_while(|c| *c == '#').count();
    if indent <= 3 && (1..=6).contains(&hashes) && body[hashes..].starts_with(' ') {
        return (
            Marker::lead(indent + hashes + 1, ""),
            heading_scale(hashes),
            MarkSet::BOLD,
        );
    }
    // Task-list items (must be checked before the plain bullet, which is a prefix).
    for (mk, repl) in [
        ("- [ ] ", TASK_OPEN),
        ("- [x] ", TASK_DONE),
        ("- [X] ", TASK_DONE),
    ] {
        if body.starts_with(mk) {
            return (
                Marker {
                    start: indent,
                    len: mk.len(),
                    repl,
                },
                1.0,
                MarkSet::empty(),
            );
        }
    }
    // Block quote(s): hide the whole leading `> ` run (collapses nested quotes).
    if body.starts_with("> ") {
        let mut len = 0;
        while body[len..].starts_with("> ") {
            len += 2;
        }
        return (
            Marker {
                start: indent,
                len,
                repl: "",
            },
            1.0,
            MarkSet::empty(),
        );
    }
    // Unordered list item.
    for marker in ["- ", "* ", "+ "] {
        if body.starts_with(marker) {
            return (
                Marker {
                    start: indent,
                    len: 2,
                    repl: BULLET,
                },
                1.0,
                MarkSet::empty(),
            );
        }
    }
    let digits = body.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && body[digits..].starts_with(". ") {
        // Numbered markers carry meaning; keep them visible (no projection).
        return (Marker::NONE, 1.0, MarkSet::empty());
    }
    (Marker::NONE, 1.0, MarkSet::empty())
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
                    return Some(((global + lk.url_start)..(global + lk.url_end), lk.url));
                }
            }
        }
        global += n + 1;
    }
    None
}

// ---- math fragments (rendered-fragment architecture, #15) ------------------

/// A math span detected in the source: its char range, the delimited source
/// (`$…$` / `$$…$$`), and whether it is display (block) math. A host renders it
/// to an image via [`crate::Editor::set_fragment_renderer`] and overlays it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathFragment {
    pub start: usize,
    pub end: usize,
    pub src: String,
    pub display: bool,
}

/// Detect math fragments in the document (Markdown `$…$`/`$$…$$` via the parser's
/// math extension; Typst `$…$` equations via the syntax tree, with the block flag
/// from the grammar). Char ranges, left to right.
pub fn math_fragments(text: &str, format: Format) -> Vec<MathFragment> {
    match format {
        Format::Typst => math_fragments_typst(text),
        _ => math_fragments_md(text),
    }
}

fn byte_to_char(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())].chars().count()
}

fn math_fragments_md(text: &str) -> Vec<MathFragment> {
    use pulldown_cmark::{Event, Options, Parser};
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_MATH);
    let mut out = Vec::new();
    for (ev, r) in Parser::new_ext(text, opts).into_offset_iter() {
        let display = match ev {
            Event::InlineMath(_) => false,
            Event::DisplayMath(_) => true,
            _ => continue,
        };
        out.push(MathFragment {
            start: byte_to_char(text, r.start),
            end: byte_to_char(text, r.end),
            src: text[r.clone()].to_string(),
            display,
        });
    }
    out
}

fn math_fragments_typst(text: &str) -> Vec<MathFragment> {
    use typst_syntax::{ast, parse, SyntaxKind, SyntaxNode};
    fn walk(node: &SyntaxNode, off: usize, text: &str, out: &mut Vec<MathFragment>) {
        if node.kind() == SyntaxKind::Equation {
            let display = node
                .cast::<ast::Equation>()
                .map(|e| e.block())
                .unwrap_or(false);
            let end = off + node.len();
            out.push(MathFragment {
                start: byte_to_char(text, off),
                end: byte_to_char(text, end),
                src: text[off..end.min(text.len())].to_string(),
                display,
            });
            return; // don't descend into nested equation tokens
        }
        let mut child_off = off;
        for child in node.children() {
            walk(child, child_off, text, out);
            child_off += child.len();
        }
    }
    let root = parse(text);
    let mut out = Vec::new();
    walk(&root, 0, text, &mut out);
    out
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
        let (cs, ce) = (
            b2c[range.start.min(line.len())],
            b2c[range.end.min(line.len())],
        );
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

/// Inline mark for one typst node, from the crate's own `highlight()` categorizer
/// (plus the `Equation` special case). Shared by the per-line decorator pass
/// ([`typst_marks_walk`]) and the whole-document [`scan_typst`] pass.
fn typst_inline_bit(node: &typst_syntax::LinkedNode) -> Option<MarkSet> {
    use typst_syntax::{highlight, SyntaxKind, Tag};
    // `highlight()` returns None for the `Equation` node (its `$` + inner tokens
    // are tagged separately); style the whole math span like code instead.
    if node.kind() == SyntaxKind::Equation {
        return Some(MarkSet::CODE);
    }
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
}

/// Per-token syntax *category* for one typst node (Step D), from the crate's
/// `highlight()` categorizer. Theme-independent — the concrete color is resolved
/// later via [`SynPalette`]. Only code-mode/math tokens get a category; markup
/// (strong/emph/links) is styled by marks instead.
fn typst_syn_cat(node: &typst_syntax::LinkedNode) -> Option<SynCat> {
    use typst_syntax::{highlight, Tag};
    match highlight(node)? {
        Tag::Keyword => Some(SynCat::Keyword),
        Tag::Function => Some(SynCat::Function),
        Tag::Number => Some(SynCat::Number),
        Tag::String => Some(SynCat::Str),
        Tag::Comment => Some(SynCat::Comment),
        Tag::Operator => Some(SynCat::Operator),
        _ => None,
    }
}

fn typst_marks_walk(
    node: &typst_syntax::LinkedNode,
    b2c: &[usize],
    n: usize,
    marks: &mut [MarkSet],
) {
    if let Some(bit) = typst_inline_bit(node) {
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
