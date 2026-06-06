//! v0.6.0 — power editing: find/replace, multiple cursors, auto-pairs, spellcheck.

use sred_core::editor::{Command, Motion, SearchOpts};
use sred_core::{Editor, Format};

fn ed(src: &str) -> Editor {
    Editor::from_source(src, Format::Markdown)
}

// ---- find / replace --------------------------------------------------------

#[test]
fn find_all_basic_and_case() {
    let e = ed("Cat cat CAT");
    let ci = SearchOpts { case_sensitive: false, whole_word: false };
    assert_eq!(e.find("cat", ci).len(), 3, "case-insensitive finds all three");
    let cs = SearchOpts { case_sensitive: true, whole_word: false };
    assert_eq!(e.find("cat", cs), vec![(4, 7)], "case-sensitive finds only the lowercase one");
}

#[test]
fn find_whole_word_only() {
    let e = ed("cat category cat");
    let ww = SearchOpts { case_sensitive: false, whole_word: true };
    assert_eq!(e.find("cat", ww), vec![(0, 3), (13, 16)], "skips 'category'");
}

#[test]
fn replace_all_is_single_lossless_edit() {
    let mut e = ed("a foo b foo c");
    let n = e.replace_all("foo", "X", SearchOpts::default());
    assert_eq!(n, 2);
    assert_eq!(e.text(), "a X b X c");
    e.apply(Command::Undo);
    assert_eq!(e.text(), "a foo b foo c", "one undo reverts all replacements");
}

#[test]
fn find_next_wraps() {
    let mut e = ed("x..x..x");
    let o = SearchOpts::default();
    assert_eq!(e.core_mut().find_next(1, "x", o), Some((3, 4)));
    assert_eq!(e.core_mut().find_next(5, "x", o), Some((6, 7)));
    assert_eq!(e.core_mut().find_next(7, "x", o), Some((0, 1)), "wraps to the top");
}

// ---- multiple cursors ------------------------------------------------------

#[test]
fn multi_caret_insert_types_at_all() {
    let mut e = ed("a.b.c");
    e.core_mut().set_cursor(0);
    e.add_caret(2); // before 'b'
    e.add_caret(4); // before 'c'
    assert_eq!(e.carets(), vec![0, 2, 4]);
    e.apply(Command::Insert("X".into()));
    assert_eq!(e.text(), "Xa.Xb.Xc");
    assert!(e.carets().len() == 3, "carets persist across multi-insert");
}

#[test]
fn multi_caret_backspace_deletes_at_all() {
    let mut e = ed("aX.bX.cX");
    // carets after each X: positions 2, 5, 8
    e.core_mut().set_cursor(2);
    e.add_caret(5);
    e.add_caret(8);
    e.apply(Command::DeleteBackward);
    assert_eq!(e.text(), "a.b.c");
}

#[test]
fn motion_collapses_multi_carets() {
    let mut e = ed("a.b");
    e.core_mut().set_cursor(0);
    e.add_caret(2);
    e.apply(Command::Move(Motion::Right));
    assert!(!e.core_mut().has_multi_carets(), "a motion collapses to the primary caret");
}

// ---- auto-pairs ------------------------------------------------------------

#[test]
fn auto_pair_inserts_closer_and_caret_inside() {
    let mut e = ed("");
    e.apply(Command::Insert("(".into()));
    assert_eq!(e.text(), "()");
    assert_eq!(e.core_mut().cursor(), 1, "caret sits between the delimiters");
    // typing the matching closer steps over it instead of duplicating
    e.apply(Command::Insert(")".into()));
    assert_eq!(e.text(), "()");
    assert_eq!(e.core_mut().cursor(), 2);
}

#[test]
fn auto_pair_wraps_selection() {
    let mut e = ed("word");
    e.apply(Command::SelectAll);
    e.apply(Command::Insert("[".into()));
    assert_eq!(e.text(), "[word]");
}

#[test]
fn auto_pairs_can_be_disabled() {
    let mut e = ed("");
    e.set_auto_pairs(false);
    e.apply(Command::Insert("(".into()));
    assert_eq!(e.text(), "(", "no auto-close when disabled");
}

// ---- spellcheck ------------------------------------------------------------

#[test]
fn word_at_returns_word_under_caret() {
    // word_at needs a render-mapped hit; instead verify the underlying word range.
    let mut e = ed("hello wrld here");
    let (s, en) = e.core_mut().word_at(7); // inside "wrld"
    let w: String = "hello wrld here".chars().skip(s).take(en - s).collect();
    assert_eq!(w, "wrld");
}

#[test]
fn spellchecker_callback_is_registered() {
    // The checker runs during render; here we just confirm registration compiles
    // and a render with a checker set does not panic.
    let mut e = ed("teh cat");
    e.set_viewport(400, 300.0);
    e.set_spellchecker(Box::new(|text: &str| {
        // flag the literal "teh"
        text.match_indices("teh")
            .map(|(i, _)| (text[..i].chars().count(), text[..i].chars().count() + 3))
            .collect()
    }));
    let _ = e.render_view(false); // must not panic; squiggle decoration emitted
}
