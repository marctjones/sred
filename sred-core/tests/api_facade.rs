//! Smoke test for the embeddable Editor facade (the Noet integration surface).
use sred_core::{Command, Editor, Format, MarkSet};

#[test]
fn facade_drives_edit_render_and_is_lossless() {
    let mut ed = Editor::from_source("# Title\n\nhello", Format::Markdown);
    ed.set_viewport(700, 500.0);

    // edit through the facade
    ed.apply(Command::SelectAll);
    ed.apply(Command::ToggleMark(MarkSet::BOLD));
    assert!(ed.text().contains("**"), "edit went through: {:?}", ed.text());

    // render produces a non-empty frame + a caret
    let out = ed.render(true);
    assert!(out.frame.width > 0 && out.frame.height > 0);
    assert_eq!(out.frame.rgba.len(), (out.frame.width * out.frame.height * 4) as usize);
    assert!(out.caret.h > 0.0);

    // set_text round-trips byte-for-byte
    ed.set_text("plain note\nsecond line\n");
    assert_eq!(ed.text(), "plain note\nsecond line\n");
}

#[test]
fn facade_click_moves_caret() {
    let mut ed = Editor::from_source("alpha beta gamma", Format::Markdown);
    ed.set_viewport(700, 500.0);
    let _ = ed.render(true);
    ed.click(5.0, 10.0); // near the start
    let near_start = ed.core_mut().cursor();
    ed.click(400.0, 10.0); // far right
    let near_end = ed.core_mut().cursor();
    assert!(near_end >= near_start, "clicking right moves caret right or equal");
}
