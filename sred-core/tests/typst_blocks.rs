//! Phase-2 Typst *block-level* constructs (MF2 Step C), driven by the
//! typst-syntax tree: heading depth, list / enum / term markers, nesting.
//!
//! Asserts the Live-Preview projection (display + per-line byte delta). The byte
//! delta is the fidelity/cursor invariant: `display_leading − source_leading`.

use sred_core::view::{self, Span};
use sred_core::{Format, MarkSet};

fn render(src: &str, caret: usize) -> (Vec<String>, Vec<i32>, Vec<Span>) {
    let (spans, deltas) = view::styled_runs(src, Format::Typst, 16.0, caret, &[], 0);
    let joined: String = spans.iter().map(|s| s.text.as_str()).collect();
    let lines = joined.split('\n').map(str::to_string).collect();
    (lines, deltas, spans)
}

#[test]
fn heading_depth_from_marker_run() {
    let src = "para\n= H1\n=== H3\n";
    let (disp, deltas, spans) = render(src, 0); // caret line 0, headings project
    assert_eq!(disp[1], "H1", "'= ' hidden off-caret");
    assert_eq!(deltas[1], -2, "'= ' (2 bytes) hidden");
    assert_eq!(disp[2], "H3", "'=== ' hidden off-caret");
    assert_eq!(deltas[2], -4, "'=== ' (4 bytes) hidden");
    // Level 1 scales larger than level 3.
    let h1 = spans.iter().find(|s| s.text.contains("H1")).unwrap().size;
    let h3 = spans.iter().find(|s| s.text.contains("H3")).unwrap().size;
    assert!(h1 > h3 && h3 > 16.0, "deeper heading is smaller but still > base ({h1} vs {h3})");
}

#[test]
fn heading_reveals_on_caret_line() {
    let src = "= Title\nbody\n";
    let (disp, deltas, _) = render(src, 0);
    assert_eq!(disp[0], "= Title", "marker shown on the caret line");
    assert_eq!(deltas[0], 0);
}

#[test]
fn list_item_becomes_bullet() {
    let src = "para\n- item\n";
    let (disp, deltas, _) = render(src, 0);
    assert_eq!(disp[1], "• item");
    assert_eq!(deltas[1], 2, "'- ' → '• ' ⇒ +2");
}

#[test]
fn nested_list_preserves_indentation() {
    let src = "- top\n  - nested\n";
    let (disp, deltas, _) = render(src, 0); // caret on line 0; line 1 projects
    assert_eq!(disp[1], "  • nested", "nested marker keeps its indentation");
    assert_eq!(deltas[1], 2);
}

#[test]
fn enum_marker_stays_visible() {
    // Typst enums auto-number; like Markdown numbered lists we keep the marker.
    let src = "para\n+ one\n+ two\n";
    let (disp, deltas, _) = render(src, 0);
    assert_eq!(disp[1], "+ one", "enum marker is not hidden");
    assert_eq!(deltas[1], 0);
    assert_eq!(deltas[2], 0);
}

#[test]
fn term_list_hides_marker() {
    let src = "para\n/ Term: definition\n";
    let (disp, deltas, _) = render(src, 0);
    assert_eq!(disp[1], "Term: definition", "'/ ' term marker hidden off-caret");
    assert_eq!(deltas[1], -2);
}

#[test]
fn inline_marks_still_apply_via_tree() {
    // Block markers from the tree don't disturb inline styling.
    let src = "para\n= A *strong* title\n";
    let (_, _, spans) = render(src, 1); // caret on heading line (revealed)
    assert!(
        spans.iter().any(|s| s.text.contains("strong") && s.marks.contains(MarkSet::BOLD)),
        "strong inside a heading should still be bold"
    );
}

#[test]
fn typst_blocks_round_trip_byte_lossless() {
    let src = "= H1\n- a\n  - b\n+ e\n/ T: d\npara *x*\n";
    let nlines = src.split('\n').count();
    for caret in 0..nlines {
        let (disp, _, _) = render(src, caret);
        let line = src.split('\n').nth(caret).unwrap();
        assert_eq!(disp[caret], line, "caret line {caret} must show source verbatim");
    }
}
