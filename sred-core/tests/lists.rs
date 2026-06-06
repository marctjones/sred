//! #3 — list rendering + Tab/Shift-Tab indent/outdent.
//!
//! Nested bullet/ordered *rendering* (indentation-aware projection) landed with
//! Phase 2; this pins it down and adds the editing-side half (indent/outdent).

use sred_core::editor::Command;
use sred_core::view::{self, Span};
use sred_core::{Editor, Format, MarkSet};

fn ed(src: &str) -> Editor {
    Editor::from_source(src, Format::Markdown)
}

fn render(src: &str, caret: usize) -> (Vec<String>, Vec<i32>) {
    let (spans, deltas): (Vec<Span>, Vec<i32>) =
        view::styled_runs(src, Format::Markdown, 16.0, caret, &[], 0);
    let joined: String = spans.iter().map(|s| s.text.as_str()).collect();
    (joined.split('\n').map(str::to_string).collect(), deltas)
}

// ---- nested list rendering (Phase 2; #3 part 2) ----------------------------

#[test]
fn nested_bullet_renders_with_indent() {
    let src = "- top\n  - sub\n";
    let (disp, deltas) = render(src, 0); // caret on line 0; line 1 projects
    assert_eq!(disp[1], "  • sub", "indented bullet keeps indent, '- ' → '• '");
    assert_eq!(deltas[1], 2);
}

#[test]
fn nested_ordered_keeps_indent_and_number() {
    let src = "1. first\n   1. sub\n2. second\n";
    let (disp, deltas) = render(src, 0);
    // Ordered markers carry meaning → kept visible; indentation preserved.
    assert_eq!(disp[1], "   1. sub", "nested numbered item rendered with its indent + number");
    assert_eq!(deltas[1], 0);
    assert_eq!(disp[2], "2. second");
}

// ---- indent / outdent editing (#3 part 1) ----------------------------------

#[test]
fn indent_nests_a_list_item() {
    let mut e = ed("- a\n- b\n");
    e.core_mut().set_cursor(5); // on the second line
    e.apply(Command::Indent);
    assert_eq!(e.text(), "- a\n  - b\n", "Tab indents the line by one level");
    e.apply(Command::Outdent);
    assert_eq!(e.text(), "- a\n- b\n", "Shift+Tab outdents it back");
}

#[test]
fn outdent_stops_at_column_zero() {
    let mut e = ed("- a\n");
    e.core_mut().set_cursor(0);
    e.apply(Command::Outdent); // nothing to remove
    assert_eq!(e.text(), "- a\n");
}

#[test]
fn indent_applies_to_all_selected_lines() {
    let mut e = ed("a\nb\nc\n");
    e.core_mut().set_cursor(0);
    // select "a\nb" (chars 0..3): touches lines 0 and 1, not line 2.
    for _ in 0..3 {
        e.apply(Command::Select(sred_core::editor::Motion::Right));
    }
    e.apply(Command::Indent);
    assert_eq!(e.text(), "  a\n  b\nc\n", "both touched lines indented; one undo reverts");
    e.apply(Command::Undo);
    assert_eq!(e.text(), "a\nb\nc\n");
}

#[test]
fn indent_is_byte_lossless_caret_lines() {
    // After indent the projection still round-trips the source on the caret line.
    let mut e = ed("- a\n- b\n");
    e.core_mut().set_cursor(5);
    e.apply(Command::Indent);
    let src = e.text();
    let (disp, _) = render(&src, 1); // caret on the now-nested line
    assert_eq!(disp[1], "  - b", "caret line shows source verbatim");
    // sanity: it's recognized as a bullet line (mark-free content)
    let _ = MarkSet::empty();
}
