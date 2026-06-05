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

/// Build styled layout runs + per-line prefix bytes (always 0 here — markers are
/// real text, nothing is injected).
pub fn styled_runs(text: &str, _format: Format, base: f32) -> (Vec<Span>, Vec<usize>) {
    let lines: Vec<&str> = text.split('\n').collect();
    let highlights = code_highlights(text);
    let mut spans = Vec::new();
    let mut prefix_bytes = Vec::with_capacity(lines.len());
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
        prefix_bytes.push(0);

        let is_fence = line.trim_start().starts_with("```");
        let line_is_code = in_fence || is_fence;
        let (size, extra) = if line_is_code {
            (base, MarkSet::CODE)
        } else {
            heading_style(line, base)
        };
        if is_fence {
            in_fence = !in_fence;
        }

        let chars: Vec<char> = line.chars().collect();
        let charmarks: Vec<MarkSet> = if line_is_code {
            vec![MarkSet::CODE; chars.len()]
        } else {
            let mut m = line_marks(line);
            for x in &mut m {
                *x |= extra;
            }
            m
        };
        // Per-char syntax-highlight colors for code lines (empty when the
        // feature is off, so code falls back to the uniform CODE color).
        let charcolors: Vec<Option<[u8; 4]>> = if line_is_code {
            line_code_colors(highlights.get(&li), chars.len())
        } else {
            vec![None; chars.len()]
        };

        let mut j = 0;
        while j < chars.len() {
            let mk = charmarks[j];
            let col = charcolors[j];
            let start = j;
            while j < chars.len() && charmarks[j] == mk && charcolors[j] == col {
                j += 1;
            }
            spans.push(Span {
                text: chars[start..j].iter().collect(),
                marks: mk,
                color: col,
                size,
            });
        }
    }

    (spans, prefix_bytes)
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

/// Strike (`~~…~~`) and underline (links) ranges in global char offsets.
pub fn decorations(text: &str, _format: Format) -> Vec<(usize, usize, Decoration)> {
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
            let m = line_marks(line);
            push_runs(&m, MarkSet::STRIKE, Decoration::Strike, global, &mut out);
            push_runs(&m, MarkSet::LINK, Decoration::Underline, global, &mut out);
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

/// Per-character marks for one line (non-nested common cases).
fn line_marks(line: &str) -> Vec<MarkSet> {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut marks = vec![MarkSet::empty(); n];
    let mut code = vec![false; n];

    // inline code `...`
    let mut i = 0;
    while i < n {
        if chars[i] == '`' {
            if let Some(j) = (i + 1..n).find(|&k| chars[k] == '`') {
                for k in i..=j {
                    marks[k].insert(MarkSet::CODE);
                    code[k] = true;
                }
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }

    apply_pair(&chars, &code, &mut marks, "**", MarkSet::BOLD);
    apply_pair(&chars, &code, &mut marks, "~~", MarkSet::STRIKE);
    apply_single(&chars, &code, &mut marks, '*', MarkSet::ITALIC);
    apply_single(&chars, &code, &mut marks, '_', MarkSet::ITALIC);
    apply_links_marks(&chars, &code, &mut marks);
    marks
}

fn slice_eq(chars: &[char], at: usize, pat: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    at + p.len() <= chars.len() && chars[at..at + p.len()] == p[..]
}

fn apply_pair(chars: &[char], code: &[bool], marks: &mut [MarkSet], pat: &str, bit: MarkSet) {
    let plen = pat.chars().count();
    let n = chars.len();
    let mut i = 0;
    while i + plen <= n {
        if !code[i] && slice_eq(chars, i, pat) {
            // find closing
            let mut j = i + plen;
            let mut close = None;
            while j + plen <= n {
                if !code[j] && slice_eq(chars, j, pat) {
                    close = Some(j);
                    break;
                }
                j += 1;
            }
            if let Some(c) = close {
                for k in i..c + plen {
                    marks[k].insert(bit);
                }
                i = c + plen;
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

fn apply_links_marks(chars: &[char], _code: &[bool], marks: &mut [MarkSet]) {
    let line: String = chars.iter().collect();
    for lk in links_in_line(&line) {
        for k in lk.start..=lk.end.min(marks.len().saturating_sub(1)) {
            marks[k].insert(MarkSet::LINK);
        }
    }
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
    use syntect::easy::HighlightLines;
    use syntect::highlighting::{Theme, ThemeSet};
    use syntect::parsing::SyntaxSet;
    use syntect::util::LinesWithEndings;

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
        let syntax = ps
            .find_syntax_by_token(lang)
            .unwrap_or_else(|| ps.find_syntax_plain_text());
        let mut hl = HighlightLines::new(syntax, theme);
        for (k, line) in LinesWithEndings::from(&body).enumerate() {
            let line_idx = body_start + k;
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
                out.insert(line_idx, spans);
            }
        }
        i = j; // resume at the closing fence (or end)
    }
    out
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
