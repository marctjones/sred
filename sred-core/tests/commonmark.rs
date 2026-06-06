//! Phase-2 CommonMark *block-level* constructs (MF1): setext headings, indented
//! code, task lists, GFM tables, reference links, nested lists/quotes.
//!
//! Each case asserts the Live-Preview projection (display text + per-line byte
//! delta) and/or the inline marks. The byte delta is the fidelity/cursor-mapping
//! invariant: `delta = display_leading_bytes − source_leading_bytes`.

use sred_core::view::{self, Span};
use sred_core::{Format, MarkSet};

/// Render markdown with the caret on `caret_line`; return (display lines, deltas,
/// spans). Display is the concatenation of all span text, re-split on '\n'.
fn render(src: &str, caret: usize) -> (Vec<String>, Vec<i32>, Vec<Span>) {
    let (spans, deltas) = view::styled_runs(src, Format::Markdown, 16.0, caret, &[], 0);
    let joined: String = spans.iter().map(|s| s.text.as_str()).collect();
    let lines = joined.split('\n').map(str::to_string).collect();
    (lines, deltas, spans)
}

/// Marks covering the (first) span whose text contains `needle`.
fn marks_of<'a>(spans: &'a [Span], needle: &str) -> MarkSet {
    spans
        .iter()
        .find(|s| s.text.contains(needle))
        .map(|s| s.marks)
        .unwrap_or(MarkSet::empty())
}

#[test]
fn setext_heading_styles_title_and_hides_underline() {
    // Caret on the body line, so the title + underline both project (off-caret).
    let src = "Title\n===\n\nbody\n";
    let (disp, deltas, spans) = render(src, 3);
    assert_eq!(disp[0], "Title", "setext title text stays");
    assert_eq!(deltas[0], 0, "title line has no marker → delta 0");
    assert_eq!(disp[1], "", "the === underline is hidden off-caret");
    assert_eq!(deltas[1], -3, "hidden '===' → delta −3");
    // The title is heading-styled (scaled above the 16.0 base + bold).
    let title = spans.iter().find(|s| s.text.contains("Title")).unwrap();
    assert!(title.size > 16.0, "setext title should scale up, got {}", title.size);
    assert!(title.marks.contains(MarkSet::BOLD), "setext title should be bold");
}

#[test]
fn setext_underline_reveals_on_caret_line() {
    let src = "Title\n===\n\nbody\n";
    let (disp, deltas, _) = render(src, 1); // caret on the underline line
    assert_eq!(disp[1], "===", "underline shows verbatim on the caret line");
    assert_eq!(deltas[1], 0);
}

#[test]
fn setext_h2_uses_dashes() {
    let src = "Sub\n---\nbody\n";
    let (disp, deltas, spans) = render(src, 2);
    assert_eq!(disp[0], "Sub");
    assert_eq!(disp[1], "", "--- underline hidden");
    assert_eq!(deltas[1], -3);
    assert!(spans.iter().find(|s| s.text.contains("Sub")).unwrap().size > 16.0);
}

#[test]
fn task_list_items_render_checkboxes() {
    let src = "- [ ] todo\n- [x] done\n";
    let (disp, deltas, _) = render(src, 9); // caret off both items
    assert_eq!(disp[0], "☐ todo", "unchecked → ballot box");
    assert_eq!(disp[1], "☑ done", "checked → ballot box with check");
    // "- [ ] " (6 bytes) → "☐ " (4 bytes) ⇒ delta −2.
    assert_eq!(deltas[0], -2);
    assert_eq!(deltas[1], -2);
}

#[test]
fn task_list_reveals_source_on_caret_line() {
    let src = "- [ ] todo\n";
    let (disp, deltas, _) = render(src, 0);
    assert_eq!(disp[0], "- [ ] todo");
    assert_eq!(deltas[0], 0);
}

#[test]
fn indented_code_block_is_styled_as_code() {
    // Indented code can't interrupt a paragraph; it follows a blank line.
    let src = "para\n\n    let x = 1;\n    let y = 2;\n\nback\n";
    let (disp, deltas, spans) = render(src, 0);
    assert_eq!(disp[2], "    let x = 1;", "indented code is shown verbatim");
    assert_eq!(deltas[2], 0, "indented code has no marker → delta 0");
    assert!(
        marks_of(&spans, "let x = 1;").contains(MarkSet::CODE),
        "indented code line should carry the CODE mark"
    );
    // A normal paragraph line nearby is NOT code.
    assert!(!marks_of(&spans, "back").contains(MarkSet::CODE));
}

#[test]
fn lazy_continuation_is_not_indented_code() {
    // 4-space indent right after a paragraph line is a lazy continuation, not code.
    let src = "para\n    still para\n";
    let (_, _, spans) = render(src, 0);
    assert!(
        !marks_of(&spans, "still para").contains(MarkSet::CODE),
        "indented line continuing a paragraph must not become code"
    );
}

#[test]
fn reference_link_resolves_against_definition() {
    // Reference link defined on another line — only a whole-document parse sees it.
    let src = "see [text][ref] here\n\n[ref]: https://example.com\n";
    let (_, _, spans) = render(src, 4);
    assert!(
        spans.iter().any(|s| s.text.contains("text") && s.marks.contains(MarkSet::LINK)),
        "reference link text should carry the LINK mark"
    );
}

#[test]
fn collapsed_reference_link_resolves() {
    let src = "see [ref] here\n\n[ref]: https://example.com\n";
    let (_, _, spans) = render(src, 4);
    assert!(
        spans.iter().any(|s| s.text.contains("ref") && s.marks.contains(MarkSet::LINK)),
        "collapsed reference link should carry the LINK mark"
    );
}

#[test]
fn gfm_table_bolds_header_and_codes_pipes() {
    let src = "| a | b |\n| --- | --- |\n| 1 | 2 |\n";
    let (disp, deltas, spans) = render(src, 9);
    // Source kept verbatim (no marker hiding anywhere in the table).
    assert_eq!(disp[0], "| a | b |");
    assert_eq!(deltas.iter().all(|&d| d == 0), true, "tables keep source → all deltas 0");
    // Header cell text is bold; pipe separators are code-marked.
    assert!(marks_of(&spans, "a").contains(MarkSet::BOLD), "header cell bold");
    assert!(
        spans.iter().any(|s| s.text.contains('|') && s.marks.contains(MarkSet::CODE)),
        "table pipes should be code-marked"
    );
}

#[test]
fn nested_bullet_preserves_indentation() {
    let src = "- top\n  - nested\n";
    let (disp, deltas, _) = render(src, 0); // caret on line 0, line 1 projects
    assert_eq!(disp[1], "  • nested", "indentation kept; '- ' → '• '");
    assert_eq!(deltas[1], 2, "'- ' (2 bytes) → '• ' (4 bytes) ⇒ +2, indent unaffected");
}

#[test]
fn nested_blockquote_collapses_markers() {
    let src = "> > deep\nplain\n";
    let (disp, deltas, _) = render(src, 1); // caret off the quote line
    assert_eq!(disp[0], "deep", "both '> ' levels hidden");
    assert_eq!(deltas[0], -4, "two '> ' markers (4 bytes) hidden");
}

#[test]
fn html_block_is_styled_as_code() {
    let src = "para\n\n<div class=\"x\">\n  <p>hi</p>\n</div>\n\nback\n";
    let (disp, deltas, spans) = render(src, 0);
    assert_eq!(disp[2], "<div class=\"x\">", "HTML block kept verbatim");
    assert_eq!(deltas[2], 0, "HTML block has no marker \u{2192} delta 0");
    assert!(
        marks_of(&spans, "<div").contains(MarkSet::CODE),
        "HTML block line should carry the CODE mark"
    );
}

#[test]
fn block_constructs_round_trip_byte_lossless() {
    // The caret line always shows its source verbatim, so the full document is
    // recoverable by visiting every line as the caret line.
    let src = "Title\n===\n- [ ] a\n  - b\n> > q\n\n    code\n| x | y |\n| - | - |\n";
    let nlines = src.split('\n').count();
    for caret in 0..nlines {
        let (disp, _, _) = render(src, caret);
        let line = src.split('\n').nth(caret).unwrap();
        assert_eq!(disp[caret], line, "caret line {caret} must show source verbatim");
    }
}
