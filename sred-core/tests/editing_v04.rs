//! v0.4.0 — editing parity: word/document motion + word delete, the portable
//! clipboard contract, line selection, and drag-and-drop move. All byte-lossless.

use sred_core::editor::{Command, Motion};
use sred_core::{Editor, Format};

fn ed(src: &str) -> Editor {
    Editor::from_source(src, Format::Markdown)
}

#[test]
fn word_left_right_motion() {
    let mut e = ed("alpha beta gamma");
    e.core_mut().set_cursor(0);
    e.apply(Command::Move(Motion::WordRight));
    assert_eq!(e.core_mut().cursor(), 5, "word-right lands after 'alpha'");
    e.apply(Command::Move(Motion::WordRight));
    assert_eq!(e.core_mut().cursor(), 10, "after 'beta'");
    e.apply(Command::Move(Motion::WordLeft));
    assert_eq!(e.core_mut().cursor(), 6, "word-left to start of 'beta'");
}

#[test]
fn document_start_end_motion() {
    let mut e = ed("one\ntwo\nthree");
    e.core_mut().set_cursor(5);
    e.apply(Command::Move(Motion::DocStart));
    assert_eq!(e.core_mut().cursor(), 0);
    e.apply(Command::Move(Motion::DocEnd));
    assert_eq!(e.core_mut().cursor(), "one\ntwo\nthree".chars().count());
}

#[test]
fn delete_word_backward_and_forward() {
    let mut e = ed("alpha beta gamma");
    // caret after "alpha beta " (char 11) → delete-word-backward removes "beta"
    e.core_mut().set_cursor(11);
    e.apply(Command::DeleteWordBackward);
    assert_eq!(e.text(), "alpha gamma");
    // caret at 0 → delete-word-forward removes "alpha"
    e.core_mut().set_cursor(0);
    e.apply(Command::DeleteWordForward);
    assert_eq!(e.text(), " gamma");
}

#[test]
fn select_word_motion_extends() {
    let mut e = ed("alpha beta");
    e.core_mut().set_cursor(0);
    e.apply(Command::Select(Motion::WordRight));
    assert_eq!(e.selected_text(), "alpha");
}

#[test]
fn clipboard_copy_cut_paste_round_trip() {
    let mut e = ed("hello world");
    e.core_mut().set_cursor(0);
    e.apply(Command::Select(Motion::WordRight)); // select "hello"
    let copied = e.copy();
    assert_eq!(copied, "hello");
    assert_eq!(e.text(), "hello world", "copy does not mutate");

    let cut = e.cut();
    assert_eq!(cut, "hello");
    assert_eq!(e.text(), " world", "cut removes the selection");

    e.apply(Command::Move(Motion::DocEnd));
    e.paste("!!");
    assert_eq!(e.text(), " world!!");
}

#[test]
fn line_selection_then_replace() {
    let mut e = ed("first line\nsecond line\nthird");
    e.core_mut().select_line_at(13); // somewhere in the 2nd line
    assert_eq!(e.selected_text(), "second line\n");
}

#[test]
fn drag_and_drop_move_is_byte_lossless() {
    let mut e = ed("AB-CD");
    // select "AB"
    e.core_mut().set_cursor(0);
    e.apply(Command::Select(Motion::Right));
    e.apply(Command::Select(Motion::Right));
    assert_eq!(e.selected_text(), "AB");
    // drop it at the end (char index 5)
    e.core_mut().move_selection_to(5);
    assert_eq!(e.text(), "-CDAB");
}

#[test]
fn word_motion_handles_unicode() {
    let mut e = ed("héllo wörld");
    e.core_mut().set_cursor(0);
    e.apply(Command::Move(Motion::WordRight));
    assert_eq!(e.core_mut().cursor(), 5, "accented word counted by chars");
}
