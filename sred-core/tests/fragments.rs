//! #15 — math fragment detection + the host fragment-renderer hook & cache.

use sred_core::api::FragmentImage;
use sred_core::{Editor, Format};
use std::cell::Cell;
use std::rc::Rc;

#[test]
fn detects_markdown_inline_and_display_math() {
    let e = Editor::from_source("a $x^2$ b\n\n$$E=mc^2$$\n", Format::Markdown);
    let frags = e.math_fragments();
    assert_eq!(frags.len(), 2);
    assert_eq!(frags[0].src, "$x^2$");
    assert!(!frags[0].display, "inline math");
    assert_eq!(frags[1].src, "$$E=mc^2$$");
    assert!(frags[1].display, "display math");
}

#[test]
fn detects_typst_inline_and_block_math() {
    let e = Editor::from_source("inline $x^2$ and $ E = m c^2 $\n", Format::Typst);
    let frags = e.math_fragments();
    assert_eq!(frags.len(), 2);
    assert!(!frags[0].display, "$x^2$ is inline");
    assert!(frags[1].display, "$ … $ with spaces is block/display");
}

#[test]
fn fragment_ranges_are_char_indices() {
    // Multibyte before the math so byte≠char offsets would diverge.
    let e = Editor::from_source("é $x$\n", Format::Markdown);
    let f = &e.math_fragments()[0];
    // "é " = 2 chars, then "$x$"
    assert_eq!(f.start, 2);
    assert_eq!(f.end, 5);
}

#[test]
fn fragment_renderer_is_called_and_cached() {
    let calls = Rc::new(Cell::new(0u32));
    let c = calls.clone();
    let mut e = Editor::from_source("$x^2$ and $x^2$\n", Format::Markdown);
    e.set_fragment_renderer(Box::new(move |src, _display, _fs| {
        c.set(c.get() + 1);
        Some(FragmentImage {
            width: src.len() as u32,
            height: 10,
            rgba: vec![0; src.len() * 40],
        })
    }));
    let frags = e.math_fragments();
    assert_eq!(frags.len(), 2);
    let a = e.render_fragment(&frags[0]).unwrap();
    assert_eq!(a.width, 5, "image sized from the fragment");
    // Second fragment has identical src → served from cache, renderer not called again.
    let _ = e.render_fragment(&frags[1]);
    let _ = e.render_fragment(&frags[0]);
    assert_eq!(
        calls.get(),
        1,
        "identical fragments compile once (cached by src)"
    );
}

#[test]
fn render_fragment_is_none_without_a_renderer() {
    let mut e = Editor::from_source("$x$\n", Format::Markdown);
    let f = e.math_fragments()[0].clone();
    assert!(e.render_fragment(&f).is_none());
}
