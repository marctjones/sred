//! v0.5.0 — IME/preedit composition and the accessibility snapshot.

use sred_core::editor::Command;
use sred_core::{Editor, Format};

fn ed(src: &str) -> Editor {
    Editor::from_source(src, Format::Markdown)
}

#[test]
fn preedit_is_not_in_text_but_is_in_display() {
    let mut e = ed("ab");
    e.core_mut().set_cursor(1); // between a|b
    e.set_preedit("ni", 2);
    assert_eq!(e.text(), "ab", "preedit must NOT be in the saved text");
    assert_eq!(e.core_mut().display_text(), "anib", "preedit shows in the rendered text");
    assert_eq!(e.core_mut().preedit_range(), Some((1, 3)));
    assert_eq!(e.core_mut().display_cursor(), 3, "caret sits after the 2-char preedit");
    assert!(e.has_preedit());
}

#[test]
fn commit_preedit_becomes_real_text() {
    let mut e = ed("ab");
    e.core_mut().set_cursor(1);
    e.set_preedit("ni", 2);
    e.commit_preedit("你"); // candidate chosen
    assert_eq!(e.text(), "a你b");
    assert!(!e.has_preedit());
}

#[test]
fn clear_preedit_cancels_composition() {
    let mut e = ed("ab");
    e.core_mut().set_cursor(1);
    e.set_preedit("ni", 2);
    e.clear_preedit();
    assert_eq!(e.text(), "ab");
    assert_eq!(e.core_mut().display_text(), "ab");
    assert!(!e.has_preedit());
}

#[test]
fn any_command_cancels_preedit() {
    let mut e = ed("ab");
    e.core_mut().set_cursor(1);
    e.set_preedit("ni", 2);
    e.apply(Command::Insert("X".into()));
    assert_eq!(e.text(), "aXb", "the committed key wins; preedit is dropped");
    assert!(!e.has_preedit());
}

#[test]
fn preedit_replaces_selection_on_start() {
    let mut e = ed("hello");
    e.apply(Command::SelectAll);
    e.set_preedit("ni", 2);
    assert_eq!(e.text(), "", "starting composition removes the selection from the buffer");
    e.commit_preedit("你");
    assert_eq!(e.text(), "你");
}

#[test]
fn a11y_snapshot_reflects_value_and_selection() {
    let mut e = ed("hello world");
    let snap = e.a11y();
    assert_eq!(snap.value, "hello world");
    assert!(snap.multiline);
    assert_eq!((snap.selection_start, snap.selection_end), (snap.caret, snap.caret));

    e.core_mut().set_cursor(0);
    e.apply(Command::Select(sred_core::editor::Motion::WordRight)); // select "hello"
    let snap = e.a11y();
    assert_eq!(snap.selection_start, 0);
    assert_eq!(snap.selection_end, 5);
}
