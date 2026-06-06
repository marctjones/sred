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

thread_local! {
    // Per-prose-line styled spans + delta, keyed by `prose_key`. Lets a keystroke
    // re-style only the line(s) that changed instead of re-scanning the whole
    // document. Code lines are never cached (their highlight colors depend on
    // block context). Bounded; cleared wholesale when large.
    static STYLE_CACHE: std::cell::RefCell<HashMap<u64, (Vec<Span>, i32)>> =
        std::cell::RefCell::new(HashMap::new());
}

/// Cache key for a prose line's styling. Includes everything its spans depend
/// on: the format (Markdown vs Typst projection), the text, whether it's the
/// caret line (markers shown vs hidden), the base font size, and the host's
/// token generation (bumped when tokens change).
fn prose_key(format: Format, line: &str, on_caret: bool, base: f32, tokens_gen: u64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (format as u8).hash(&mut h);
    line.hash(&mut h);
    on_caret.hash(&mut h);
    base.to_bits().hash(&mut h);
    tokens_gen.hash(&mut h);
    h.finish()
}

/// Style one non-code (prose) line: project block markers (Live Preview), scan
/// inline marks, apply token colors, and emit spans. A pure function of its
/// inputs, so its result is safe to memoize in `STYLE_CACHE`.
fn style_prose_line(
    format: Format,
    line: &str,
    base: f32,
    on_caret: bool,
    tokens: &[TokenSpec],
) -> (Vec<Span>, i32) {
    let (display, delta, size, extra) = project_line(format, line, base, on_caret, false);
    let chars: Vec<char> = display.chars().collect();
    let mut charmarks = line_marks(format, &display);
    for x in &mut charmarks {
        *x |= extra;
    }
    let mut charcolors: Vec<Option<[u8; 4]>> = vec![None; chars.len()];
    if !tokens.is_empty() {
        for spec in tokens {
            for m in (spec.matcher)(&display) {
                for slot in charcolors
                    .iter_mut()
                    .take(m.end.min(chars.len()))
                    .skip(m.start.min(chars.len()))
                {
                    *slot = Some(spec.fg);
                }
            }
        }
    }
    let mut out = Vec::new();
    let mut j = 0;
    while j < chars.len() {
        let mk = charmarks[j];
        let col = charcolors[j];
        let start = j;
        while j < chars.len() && charmarks[j] == mk && charcolors[j] == col {
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

pub fn styled_runs(
    text: &str,
    format: Format,
    base: f32,
    caret_line: usize,
    tokens: &[TokenSpec],
    tokens_gen: u64,
) -> (Vec<Span>, Vec<i32>) {
    let lines: Vec<&str> = text.split('\n').collect();
    let highlights = code_highlights(text);
    let mut spans = Vec::new();
    // Per-line byte delta: (display leading bytes) − (source leading bytes).
    // 0 on the caret line and on paragraphs/code; negative where a marker is
    // hidden (headings/quotes); positive where a bullet glyph replaces "- ".
    let mut deltas: Vec<i32> = Vec::with_capacity(lines.len());
    let mut in_fence = false;

    for (li, line) in lines.iter().enumerate() {
        if li > 0 {
            spans.push(Span {
                text: "\n".into(),
                marks: MarkSet::empty(),
                color: None,
                size: base,
            });
        }

        let is_fence = line.trim_start().starts_with("```");
        let interior = in_fence && !is_fence; // a code-content line (not a delimiter)
        if is_fence {
            in_fence = !in_fence;
        }
        let on_caret = li == caret_line;
        let line_is_code = is_fence || interior;

        if line_is_code {
            // Code lines: highlight colors depend on block context, so they are
            // recomputed (the underlying syntect highlight is itself cached).
            let (display, delta) = if is_fence {
                if on_caret {
                    (line.to_string(), 0)
                } else {
                    (String::new(), -(line.len() as i32))
                }
            } else {
                (line.to_string(), 0)
            };
            deltas.push(delta);
            let chars: Vec<char> = display.chars().collect();
            let charcolors = line_code_colors(highlights.get(&li), chars.len());
            let mut j = 0;
            while j < chars.len() {
                let col = charcolors[j];
                let start = j;
                while j < chars.len() && charcolors[j] == col {
                    j += 1;
                }
                spans.push(Span {
                    text: chars[start..j].iter().collect(),
                    marks: MarkSet::CODE,
                    color: col,
                    size: base,
                });
            }
        } else {
            // Prose lines: memoize per (format, text, on-caret, base, token-gen).
            let key = prose_key(format, line, on_caret, base, tokens_gen);
            let (line_spans, delta) = STYLE_CACHE.with(|c| {
                if let Some(hit) = c.borrow().get(&key) {
                    return hit.clone();
                }
                let computed = style_prose_line(format, line, base, on_caret, tokens);
                let mut m = c.borrow_mut();
                if m.len() > 16384 {
                    m.clear();
                }
                m.insert(key, computed.clone());
                computed
            });
            deltas.push(delta);
            spans.extend(line_spans);
        }
    }

    (spans, deltas)
}

/// Test/maintenance hook: drop the styling caches (so a fresh computation can be
/// compared against the incremental one).
#[cfg(test)]
pub(crate) fn clear_style_cache() {
    STYLE_CACHE.with(|c| c.borrow_mut().clear());
    DECO_CACHE.with(|c| c.borrow_mut().clear());
}

/// Project a source line to its displayed form. On a revealed line (caret line /
/// code / paragraph) the display equals the source. Otherwise leading block
/// markers are hidden (headings/quotes) or substituted (bullets → "• "), and the
/// returned delta is `display_leading_bytes − source_leading_bytes`.
fn project_line(
    format: Format,
    line: &str,
    base: f32,
    reveal: bool,
    line_is_code: bool,
) -> (String, i32, f32, MarkSet) {
    match format {
        Format::Typst => project_line_typst(line, base, reveal, line_is_code),
        _ => project_line_md(line, base, reveal, line_is_code),
    }
}

fn project_line_md(line: &str, base: f32, reveal: bool, line_is_code: bool) -> (String, i32, f32, MarkSet) {
    if line_is_code {
        return (line.to_string(), 0, base, MarkSet::CODE);
    }
    let (size, extra) = heading_style(line, base);
    if reveal {
        return (line.to_string(), 0, size, extra);
    }
    // hidden line: project the leading marker
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
        let m = hashes + 1; // "### "
        return (line[m..].to_string(), -(m as i32), size, extra);
    }
    if let Some(rest) = line.strip_prefix("> ") {
        return (rest.to_string(), -2, base, MarkSet::empty());
    }
    const BULLET: &str = "• "; // U+2022 + space = 4 bytes
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(marker) {
            let display = format!("{BULLET}{rest}");
            let delta = BULLET.len() as i32 - marker.len() as i32; // 4 − 2 = +2
            return (display, delta, base, MarkSet::empty());
        }
    }
    let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && line[digits..].starts_with(". ") {
        // keep numbered markers visible (they carry meaning); no projection
        return (line.to_string(), 0, base, MarkSet::empty());
    }
    (line.to_string(), 0, base, MarkSet::empty())
}

/// Typst block-marker projection (Level 1). Headings use `=` runs; `- ` is an
/// unordered list, `+ ` an enum (kept visible like Markdown's numbered lists).
fn project_line_typst(line: &str, base: f32, reveal: bool, line_is_code: bool) -> (String, i32, f32, MarkSet) {
    if line_is_code {
        return (line.to_string(), 0, base, MarkSet::CODE);
    }
    // Heading: one or more leading '=' then a space (level = count of '=').
    let eqs = line.chars().take_while(|c| *c == '=').count();
    let (size, extra) = if (1..=6).contains(&eqs) && line[eqs..].starts_with(' ') {
        let scale = match eqs {
            1 => 1.9,
            2 => 1.55,
            3 => 1.3,
            4 => 1.15,
            _ => 1.05,
        };
        (base * scale, MarkSet::BOLD)
    } else {
        (base, MarkSet::empty())
    };
    if reveal {
        return (line.to_string(), 0, size, extra);
    }
    if (1..=6).contains(&eqs) && line[eqs..].starts_with(' ') {
        let m = eqs + 1; // "== "
        return (line[m..].to_string(), -(m as i32), size, extra);
    }
    const BULLET: &str = "• "; // U+2022 + space = 4 bytes
    if let Some(rest) = line.strip_prefix("- ") {
        let display = format!("{BULLET}{rest}");
        return (display, BULLET.len() as i32 - 2, base, MarkSet::empty());
    }
    if line.strip_prefix("+ ").is_some() {
        // Typst enum marker — keep visible (carries auto-numbering meaning).
        return (line.to_string(), 0, base, MarkSet::empty());
    }
    (line.to_string(), 0, base, MarkSet::empty())
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

fn heading_style(line: &str, base: f32) -> (f32, MarkSet) {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
        let scale = match hashes {
            1 => 1.9,
            2 => 1.55,
            3 => 1.3,
            4 => 1.15,
            _ => 1.05,
        };
        (base * scale, MarkSet::BOLD)
    } else {
        (base, MarkSet::empty())
    }
}

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

/// Typst inline marks (Level 1): raw `` `…` ``, math `$…$` (styled like code),
/// strong `*…*` (single asterisk, unlike Markdown's `**`), emphasis `_…_`.
fn line_marks_typst(line: &str) -> Vec<MarkSet> {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut marks = vec![MarkSet::empty(); n];
    let mut code = vec![false; n];

    // Raw and math are "code-protected" so emphasis scanning ignores their guts.
    scan_pairs_single(&chars, '`', &mut marks, &mut code, MarkSet::CODE);
    scan_pairs_single(&chars, '$', &mut marks, &mut code, MarkSet::CODE);

    apply_single(&chars, &code, &mut marks, '*', MarkSet::BOLD);
    apply_single(&chars, &code, &mut marks, '_', MarkSet::ITALIC);
    marks
}

/// Mark `delim … delim` pairs on one line with `bit`, protecting their contents
/// from later emphasis scanning (`code[k] = true`). Used for `` ` `` and `$`.
fn scan_pairs_single(chars: &[char], delim: char, marks: &mut [MarkSet], code: &mut [bool], bit: MarkSet) {
    let n = chars.len();
    let mut i = 0;
    while i < n {
        if chars[i] == delim {
            if let Some(j) = (i + 1..n).find(|&k| chars[k] == delim) {
                for k in i..=j {
                    marks[k].insert(bit);
                    code[k] = true;
                }
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
}

fn apply_single(chars: &[char], code: &[bool], marks: &mut [MarkSet], delim: char, bit: MarkSet) {
    let n = chars.len();
    let is_part_of_double = |k: usize| {
        (k + 1 < n && chars[k + 1] == delim) || (k > 0 && chars[k - 1] == delim)
    };
    let mut i = 0;
    while i < n {
        if !code[i] && chars[i] == delim && !is_part_of_double(i) && !marks[i].contains(bit) {
            let mut j = i + 1;
            let mut close = None;
            while j < n {
                if !code[j] && chars[j] == delim && !is_part_of_double(j) {
                    close = Some(j);
                    break;
                }
                j += 1;
            }
            if let Some(c) = close {
                for k in i..=c {
                    marks[k].insert(bit);
                }
                i = c + 1;
                continue;
            }
        }
        i += 1;
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
