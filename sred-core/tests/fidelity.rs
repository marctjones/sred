//! M1 gate: byte-lossless round-trips and diff-minimal edits.
//!
//! The source-anchored editor must return exactly what it was given, and an edit
//! must only change the bytes it touches. These properties are what make sred
//! safe to save Noet's plain-markdown notes.

use sred_core::{Command, EditorCore, Format, Motion};

/// A spread of real-world markdown shapes that a structured/reconstructive editor
/// would silently normalize.
const CORPUS: &[&str] = &[
    "",
    "plain text with no trailing newline",
    "trailing newline\n",
    "two trailing newlines\n\n",
    "windows line endings\r\nsecond line\r\n",
    "trailing spaces here   \nand a tab\there\n",
    "# ATX heading\n\nbody\n",
    "Setext heading\n==============\n\nbody\n",
    "- bullet a\n- bullet b\n  - nested\n    - deeper\n",
    "* star bullet\n+ plus bullet\n",
    "1. one\n2. two\n10. ten\n",
    "> quote line 1\n> quote line 2\n>\n> after blank quote\n",
    "```rust\nfn main() {\n    println!(\"hi {}\", x);\n}\n```\n",
    "~~~\nalt fence\n~~~\n",
    "emphasis: *one* _two_ **three** __four__ ~~strike~~ `code`\n",
    "a link [text](https://example.com/path?q=1&z=2) and a bare https://x.io\n",
    "an ![image](img.png) inline\n",
    "<div class=\"raw\">\n  <span>HTML block</span>\n</div>\n",
    "| a | b |\n| - | - |\n| 1 | 2 |\n",
    "footnote ref[^1]\n\n[^1]: the note\n",
    "wikilink [[Workstream]] +[[Project]] @[[Person]] @bare #tag\n",
    "TODO(do) ship it @[[Marc]] +[[Sred]] due:2026-07-01 [#A] repeat:1w jira:SR-12\n",
    "mixed\n\n\n\nmany blank lines\n",
    "unicode: café — 日本語 — \u{1F600}\n",
];

#[test]
fn corpus_roundtrips_verbatim() {
    for (i, src) in CORPUS.iter().enumerate() {
        let ed = EditorCore::from_source(src, Format::Markdown);
        assert_eq!(
            &ed.text(),
            src,
            "case #{i} must round-trip byte-for-byte\n--- expected ---\n{src:?}\n--- got ---\n{:?}",
            ed.text()
        );
    }
}

#[test]
fn set_text_then_text_is_identity() {
    let mut ed = EditorCore::new(Format::Markdown);
    for src in CORPUS {
        ed.set_text(src);
        assert_eq!(&ed.text(), src);
    }
}

#[test]
fn cursor_moves_never_mutate_text() {
    for src in CORPUS {
        let mut ed = EditorCore::from_source(src, Format::Markdown);
        ed.set_cursor(0);
        for _ in 0..(src.chars().count() + 5) {
            ed.apply(Command::Move(Motion::Right));
        }
        ed.apply(Command::Move(Motion::LineStart));
        ed.apply(Command::Move(Motion::LineEnd));
        assert_eq!(&ed.text(), src, "navigation must not change the buffer");
    }
}

#[test]
fn insert_only_touches_the_insertion_point() {
    let src = "alpha\nbeta\ngamma\ndelta\n";
    let mut ed = EditorCore::from_source(src, Format::Markdown);
    // place caret at the start of "gamma" (after "alpha\nbeta\n" = 11 chars)
    let at = "alpha\nbeta\n".chars().count();
    ed.set_cursor(at);
    ed.apply(Command::Insert("XYZ".into()));
    assert_eq!(ed.text(), "alpha\nbeta\nXYZgamma\ndelta\n");
}

#[test]
fn delete_only_touches_the_deletion_range() {
    let src = "keep1\nDROP\nkeep2\n";
    let mut ed = EditorCore::from_source(src, Format::Markdown);
    let at = "keep1\n".chars().count();
    ed.set_cursor(at);
    for _ in 0..4 {
        ed.apply(Command::Select(Motion::Right)); // select "DROP"
    }
    ed.apply(Command::DeleteSelection);
    assert_eq!(ed.text(), "keep1\n\nkeep2\n");
}

#[test]
fn typing_a_full_line_is_lossless() {
    // Simulate a human typing markdown character-by-character; the result is the
    // exact string typed (no normalization, no marker stripping).
    let typed = "## Heading **bold** and a [link](http://x)";
    let mut ed = EditorCore::new(Format::Markdown);
    for ch in typed.chars() {
        ed.apply(Command::Insert(ch.to_string()));
    }
    assert_eq!(ed.text(), typed);
}
