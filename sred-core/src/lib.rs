//! `sred-core` — a format-agnostic rich-text editor core.
//!
//! Layers (all UI-free):
//! - [`model`]    — the superset document AST (blocks + inline runs with marks).
//! - [`format`]   — the `DocumentFormat` backend trait + capability masks.
//! - [`markdown`] / [`typst_fmt`] — the two backends.
//! - [`editor`]   — the editing engine (commands, cursor, selection, marks).
//! - [`layout`]   — cosmic-text shaping + rasterization to an RGBA frame.

pub mod editor;
pub mod format;
pub mod layout;
pub mod markdown;
pub mod model;
pub mod typst_fmt;

pub use editor::{BlockKind, Command, Decoration, EditorCore, Motion, Span};
pub use format::{backend_for, Caps, DocumentFormat, FormatError};
pub use layout::{Frame, TextRenderer, Theme};
pub use model::{Block, Document, Format, Inline, List, MarkSet};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_roundtrip_stable() {
        let src = "# Title\n\nHello **bold** and *italic* and `code`.\n";
        let md = markdown::Markdown;
        let doc = md.parse(src).unwrap();
        let out = md.serialize(&doc).unwrap();
        // Re-parsing the output yields the same model (stability).
        let doc2 = md.parse(&out).unwrap();
        assert_eq!(doc, doc2, "serialize→parse must be a fixpoint\n--- out ---\n{out}");
    }

    #[test]
    fn markdown_has_expected_structure() {
        let md = markdown::Markdown;
        let doc = md.parse("# H\n\ntext\n").unwrap();
        assert!(matches!(doc.blocks[0], Block::Heading { level: 1, .. }));
        assert!(matches!(doc.blocks[1], Block::Paragraph(_)));
    }

    #[test]
    fn typst_roundtrip_stable() {
        let src = "= Title\n\nHello *bold* and _italic_.\n";
        let ty = typst_fmt::Typst;
        let doc = ty.parse(src).unwrap();
        let out = ty.serialize(&doc).unwrap();
        let doc2 = ty.parse(&out).unwrap();
        assert_eq!(doc, doc2, "typst serialize→parse fixpoint\n--- out ---\n{out}");
    }

    #[test]
    fn cross_format_bold_survives() {
        // Markdown in, Typst out: the bold mark must carry across the model.
        let doc = markdown::Markdown.parse("a **b** c\n").unwrap();
        let typ = typst_fmt::Typst.serialize(&doc).unwrap();
        assert!(typ.contains("*b*"), "expected typst strong, got: {typ}");
    }

    #[test]
    fn block_structure_survives_edit_cycle() {
        // Open a doc with a heading + two bullets, edit nothing structural, save:
        // the structure must come back out.
        let src = "# Title\n\n- one\n- two\n";
        let mut ed = EditorCore::from_source(src, Format::Markdown);
        let out = ed.source();
        let doc = markdown::Markdown.parse(&out).unwrap();
        assert!(matches!(doc.blocks[0], Block::Heading { level: 1, .. }));
        assert!(
            matches!(&doc.blocks[1], Block::List(l) if !l.ordered && l.items.len() == 2),
            "expected a 2-item bullet list, got: {:?}",
            doc.blocks
        );
        // Now type at the end and make the current block a heading.
        ed.apply(Command::Insert("!".into()));
        ed.apply(Command::ToggleBlock(BlockKind::Heading(2)));
        let doc2 = markdown::Markdown.parse(&ed.source()).unwrap();
        assert!(
            doc2.blocks.iter().any(|b| matches!(b, Block::Heading { level: 2, .. })),
            "expected an H2 after ToggleBlock, got: {:?}",
            doc2.blocks
        );
    }

    #[test]
    fn autoformat_promotes_markers() {
        // "# " -> H1, "- " -> bullet, "= " -> H1 (typst style)
        for (marker, want_h) in [("# ", 1u8), ("## ", 2), ("=== ", 3)] {
            let mut ed = EditorCore::new(Format::Markdown);
            for ch in marker.chars() {
                ed.apply(Command::Insert(ch.to_string()));
            }
            ed.apply(Command::Insert("Title".into()));
            let doc = ed.to_document();
            assert!(
                matches!(doc.blocks[0], Block::Heading { level, .. } if level == want_h),
                "marker {marker:?} should give H{want_h}, got {:?}",
                doc.blocks
            );
        }
        let mut ed = EditorCore::new(Format::Markdown);
        for ch in "- item".chars() {
            ed.apply(Command::Insert(ch.to_string()));
        }
        assert!(matches!(&ed.to_document().blocks[0], Block::List(l) if !l.ordered));
    }

    #[test]
    fn enter_and_backspace_exit_list() {
        // Build "- a" then Enter (new empty bullet) then Enter again → exits list.
        let mut ed = EditorCore::new(Format::Markdown);
        for ch in "- a".chars() {
            ed.apply(Command::Insert(ch.to_string()));
        }
        ed.apply(Command::Insert("\n".into())); // new empty bullet
        ed.apply(Command::Insert("\n".into())); // empty → exit to paragraph
        let doc = ed.to_document();
        assert!(
            matches!(&doc.blocks[0], Block::List(l) if l.items.len() == 1),
            "first block should remain a 1-item list, got {:?}",
            doc.blocks
        );
        assert!(
            doc.blocks.iter().any(|b| matches!(b, Block::Paragraph(_))),
            "exiting the list should leave a paragraph, got {:?}",
            doc.blocks
        );

        // Backspace at the start of a bullet un-bullets it.
        let mut ed = EditorCore::new(Format::Markdown);
        for ch in "- hi".chars() {
            ed.apply(Command::Insert(ch.to_string()));
        }
        ed.apply(Command::Move(Motion::LineStart));
        ed.apply(Command::DeleteBackward);
        assert!(
            matches!(&ed.to_document().blocks[0], Block::Paragraph(_)),
            "backspace at bullet start should produce a paragraph, got {:?}",
            ed.to_document().blocks
        );
    }

    #[test]
    fn insert_drops_control_and_pua() {
        let mut ed = EditorCore::new(Format::Markdown);
        ed.apply(Command::Insert("a\u{1b}b\u{f700}c\u{7}".into())); // esc, PUA arrow, bell
        assert_eq!(ed.text(), "abc");
    }

    #[test]
    fn editor_undo_redo_coalesces_word() {
        let mut ed = EditorCore::new(Format::Markdown);
        // Type a word char-by-char (coalesces into one undo group), then a space
        // + second word (a new group).
        for ch in "foo".chars() {
            ed.apply(Command::Insert(ch.to_string()));
        }
        ed.apply(Command::Insert(" ".into()));
        for ch in "bar".chars() {
            ed.apply(Command::Insert(ch.to_string()));
        }
        assert_eq!(ed.text(), "foo bar");
        ed.apply(Command::Undo); // undo "bar"
        assert_eq!(ed.text(), "foo ");
        ed.apply(Command::Undo); // undo " "
        assert_eq!(ed.text(), "foo");
        ed.apply(Command::Undo); // undo "foo"
        assert_eq!(ed.text(), "");
        ed.apply(Command::Redo);
        assert_eq!(ed.text(), "foo");
        // A fresh edit clears the redo stack.
        ed.apply(Command::Insert("X".into()));
        assert!(!ed.can_redo());
    }

    #[test]
    fn editor_typing_and_marks() {
        let mut ed = EditorCore::new(Format::Markdown);
        ed.apply(Command::Insert("hello".into()));
        assert_eq!(ed.text(), "hello");
        // select "hello" and bold it
        ed.apply(Command::Move(Motion::LineStart));
        for _ in 0..5 {
            ed.apply(Command::Select(Motion::Right));
        }
        ed.apply(Command::ToggleMark(MarkSet::BOLD));
        let out = ed.source();
        assert!(out.contains("**hello**"), "got: {out}");
    }
}
