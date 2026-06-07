//! #23 — multi-cursor with per-caret selections + "add caret at next match".

use sred_core::editor::Command;
use sred_core::{Editor, Format};

fn ed(src: &str) -> Editor {
    Editor::from_source(src, Format::Markdown)
}

#[test]
fn ctrl_d_selects_word_then_next_occurrence() {
    let mut e = ed("foo bar foo baz foo");
    e.core_mut().set_cursor(1); // inside the first "foo"
    e.apply(Command::AddCaretNextMatch); // first: select the word
    assert_eq!(e.selected_text(), "foo");
    assert!(
        !e.core_mut().has_multi_carets(),
        "first press only selects the word"
    );

    e.apply(Command::AddCaretNextMatch); // add the next "foo"
    assert!(e.core_mut().has_multi_carets());
    assert_eq!(e.core_mut().extra_selections().len(), 1);
    let (s, en) = e.core_mut().extra_selections()[0];
    assert_eq!(&e.text()[..][s..en], "foo");
    assert!(
        s >= 8,
        "next match is the second 'foo', not the one already selected"
    );
}

#[test]
fn typing_replaces_all_selected_occurrences() {
    let mut e = ed("foo bar foo baz foo");
    e.core_mut().set_cursor(0);
    e.apply(Command::AddCaretNextMatch); // select "foo" #1
    e.apply(Command::AddCaretNextMatch); // + "foo" #2
    e.apply(Command::AddCaretNextMatch); // + "foo" #3
    assert_eq!(e.core_mut().extra_selections().len(), 2);
    e.apply(Command::Insert("X".into())); // replace all three
    assert_eq!(e.text(), "X bar X baz X");
    // One undo reverts the whole multi-edit.
    e.apply(Command::Undo);
    assert_eq!(e.text(), "foo bar foo baz foo");
}

#[test]
fn backspace_deletes_all_selections() {
    let mut e = ed("ab ab ab");
    e.core_mut().set_cursor(0);
    e.apply(Command::AddCaretNextMatch); // "ab" #1
    e.apply(Command::AddCaretNextMatch); // "ab" #2
    e.apply(Command::AddCaretNextMatch); // "ab" #3
    e.apply(Command::DeleteBackward);
    assert_eq!(e.text(), "  ", "each 'ab' selection removed, spaces remain");
}

#[test]
fn point_carets_still_type_at_all_sites() {
    // Alt+click-style point carets (no selection) still work.
    let mut e = ed("a.b.c");
    e.core_mut().set_cursor(0);
    e.add_caret(2);
    e.add_caret(4);
    e.apply(Command::Insert("X".into()));
    assert_eq!(e.text(), "Xa.Xb.Xc");
}

#[test]
fn motion_collapses_multicursor() {
    let mut e = ed("foo foo");
    e.core_mut().set_cursor(0);
    e.apply(Command::AddCaretNextMatch);
    e.apply(Command::AddCaretNextMatch);
    assert!(e.core_mut().has_multi_carets());
    e.apply(Command::Move(sred_core::editor::Motion::Left));
    assert!(
        !e.core_mut().has_multi_carets(),
        "a motion collapses to one caret"
    );
}

#[test]
fn add_caret_wraps_and_stops_when_exhausted() {
    let mut e = ed("x y x");
    e.core_mut().set_cursor(0);
    e.apply(Command::AddCaretNextMatch); // select first "x"
    e.apply(Command::AddCaretNextMatch); // second "x"
    assert_eq!(e.core_mut().extra_selections().len(), 1);
    // No more new occurrences → no-op (count unchanged).
    e.apply(Command::AddCaretNextMatch);
    assert_eq!(
        e.core_mut().extra_selections().len(),
        1,
        "all occurrences already carets"
    );
}

#[test]
fn multicursor_edits_are_byte_lossless_via_undo() {
    let mut e = ed("the cat sat on the mat, the end");
    e.core_mut().set_cursor(0);
    for _ in 0..4 {
        e.apply(Command::AddCaretNextMatch); // word "the" + occurrences
    }
    e.apply(Command::Insert("THE".into()));
    assert!(e.text().contains("THE"));
    e.apply(Command::Undo);
    assert_eq!(e.text(), "the cat sat on the mat, the end");
}
