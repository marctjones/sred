//! #17 + #15: caret geometry for multiple cursors, and range rects for overlays.

use sred_core::api::FragmentImage;
use sred_core::editor::Command;
use sred_core::{Editor, Format};

fn ed(src: &str) -> Editor {
    let mut e = Editor::from_source(src, Format::Markdown);
    e.set_viewport(800, 600.0);
    e
}

#[test]
fn frameout_has_one_caret_normally() {
    let mut e = ed("hello world");
    let out = e.render_view(true);
    assert_eq!(out.carets.len(), 1, "single cursor → one caret rect");
    assert_eq!(out.carets[0].x, out.caret.x);
    assert_eq!(out.carets[0].y, out.caret.y);
}

#[test]
fn frameout_reports_a_rect_per_cursor() {
    let mut e = ed("alpha beta gamma");
    e.core_mut().set_cursor(0);
    e.add_caret(6); // before "beta"
    e.add_caret(11); // before "gamma"
    let out = e.render_view(true);
    assert_eq!(out.carets.len(), 3, "three cursors → three caret rects");
    // Distinct x positions (same line, increasing columns).
    let xs: Vec<f32> = out.carets.iter().map(|c| c.x).collect();
    assert!(
        xs[0] < xs[1] && xs[1] < xs[2],
        "carets ordered left→right: {xs:?}"
    );
    // All on the first line → equal y.
    assert!(out
        .carets
        .iter()
        .all(|c| (c.y - out.carets[0].y).abs() < 0.5));
}

#[test]
fn caret_collapses_after_motion() {
    let mut e = ed("a.b");
    e.core_mut().set_cursor(0);
    e.add_caret(2);
    assert_eq!(e.render_view(true).carets.len(), 2);
    e.apply(Command::Move(sred_core::editor::Motion::Right)); // collapses
    assert_eq!(e.render_view(true).carets.len(), 1);
}

#[test]
fn rect_for_range_covers_a_span() {
    let mut e = ed("hello world");
    let rects = e.rect_for_range(0, 5); // "hello"
    assert_eq!(rects.len(), 1, "single-line span → one rect");
    assert!(rects[0].w > 0.0, "non-empty width");
    assert!(rects[0].h > 0.0);
    // A wider span is at least as wide.
    let wide = e.rect_for_range(0, 11);
    assert!(wide[0].w >= rects[0].w);
}

#[test]
fn fragment_overlay_geometry_end_to_end() {
    // Detect a math fragment, render it via a fake renderer, and get a rect to
    // overlay it — the full host-overlay path for #15.
    let mut e = ed("see $x^2$ here");
    e.set_fragment_renderer(Box::new(|src, _d, _fs| {
        Some(FragmentImage {
            width: 24,
            height: 18,
            rgba: vec![0; 24 * 18 * 4 * (src.len() / src.len())],
        })
    }));
    let frags = e.math_fragments();
    assert_eq!(frags.len(), 1);
    let img = e
        .render_fragment(&frags[0])
        .expect("renderer produces an image");
    assert_eq!((img.width, img.height), (24, 18));
    let rects = e.rect_for_range(frags[0].start, frags[0].end);
    assert!(
        !rects.is_empty(),
        "the $x^2$ span has a screen rect to overlay onto"
    );
    assert!(rects[0].w > 0.0);
}
