//! `sred-core` — a UI-free, source-anchored rich-text editor core.
//!
//! Layers:
//! - [`editor`]   — source-anchored editing engine: the raw markdown text is the
//!                  buffer; edits splice it, so `text()` is byte-lossless.
//! - [`view`]     — styles the raw markdown in place (runs, decorations, links).
//! - [`layout`]   — cosmic-text shaping + rasterization to an RGBA frame.
//! - [`model`]    — shared types (`Format`, `MarkSet`) + the document AST used by
//!                  the standalone backends.
//! - [`format`] / [`markdown`] / [`typst_fmt`] — document backends (used by the
//!                  demo and tests; not on the source-anchored editing path).

pub mod editor;
pub mod format;
pub mod layout;
pub mod markdown;
pub mod model;
pub mod typst_fmt;
pub mod view;

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

    // ---- source-anchored editor (byte-lossless) --------------------------

    #[test]
    fn fidelity_roundtrip_verbatim() {
        // Loading then reading back must be byte-for-byte identical.
        let src = "# Title\n\n- one\n- two\n\n```rust\nfn main() {}\n```\n\n> quote with **bold**\n\nmixed _emph_ and `code` and a [link](https://x.io).\n";
        let ed = EditorCore::from_source(src, Format::Markdown);
        assert_eq!(ed.text(), src, "open→text must be verbatim");
    }

    #[test]
    fn edit_is_diff_minimal() {
        // Inserting in the middle changes only the touched bytes.
        let src = "line one\nline two\nline three\n";
        let mut ed = EditorCore::from_source(src, Format::Markdown);
        ed.set_cursor(0);
        for _ in 0..("line one".len()) {
            ed.apply(Command::Move(Motion::Right));
        }
        ed.apply(Command::Insert("!".into()));
        assert_eq!(ed.text(), "line one!\nline two\nline three\n");
    }

    #[test]
    fn toggle_bold_writes_markers_in_source() {
        let mut ed = EditorCore::from_source("hello world", Format::Markdown);
        ed.apply(Command::SelectAll);
        ed.apply(Command::ToggleMark(MarkSet::BOLD));
        assert_eq!(ed.text(), "**hello world**");
        // toggling again unwraps
        ed.apply(Command::SelectAll);
        ed.apply(Command::ToggleMark(MarkSet::BOLD));
        assert_eq!(ed.text(), "hello world");
    }

    #[test]
    fn set_block_writes_real_markers() {
        let mut ed = EditorCore::from_source("Title", Format::Markdown);
        ed.apply(Command::ToggleBlock(BlockKind::Heading(2)));
        assert_eq!(ed.text(), "## Title");
        // toggling H2 off returns to a plain paragraph
        ed.apply(Command::ToggleBlock(BlockKind::Heading(2)));
        assert_eq!(ed.text(), "Title");

        let mut ed = EditorCore::from_source("item", Format::Markdown);
        ed.apply(Command::ToggleBlock(BlockKind::Bullet));
        assert_eq!(ed.text(), "- item");
    }

    #[test]
    fn enter_continues_then_exits_list() {
        let mut ed = EditorCore::from_source("- a", Format::Markdown);
        ed.set_cursor(ed.len()); // caret at end of "- a"
        ed.apply(Command::Insert("\n".into())); // continue → "- a\n- "
        assert_eq!(ed.text(), "- a\n- ");
        ed.apply(Command::Insert("\n".into())); // empty item → exit (drop marker)
        assert_eq!(ed.text(), "- a\n");
    }

    #[test]
    fn make_link_wraps_selection() {
        let mut ed = EditorCore::from_source("click", Format::Markdown);
        ed.apply(Command::SelectAll);
        ed.apply(Command::Link("https://x.io".into()));
        assert_eq!(ed.text(), "[click](https://x.io)");
        ed.set_cursor(2); // inside the link text
        assert_eq!(ed.link_at_cursor().as_deref(), Some("https://x.io"));
        assert!(ed.update_link_at_cursor("https://y.io"));
        assert_eq!(ed.text(), "[click](https://y.io)");
    }

    #[test]
    fn color_command_is_noop_on_source() {
        let mut ed = EditorCore::from_source("hi", Format::Markdown);
        ed.apply(Command::SelectAll);
        ed.apply(Command::SetColor(Some(0xCC2222FF)));
        assert_eq!(ed.text(), "hi", "color must not touch the source");
    }

    #[test]
    #[cfg(feature = "syntax-highlight")]
    fn fenced_code_block_gets_syntect_colors() {
        // A highlighted code block should produce more than one foreground color.
        let src = "```rust\nfn main() { let x = 1; }\n```\n";
        let (spans, _) = view::styled_runs(src, Format::Markdown, 16.0);
        let colors: std::collections::HashSet<[u8; 4]> =
            spans.iter().filter_map(|s| s.color).collect();
        assert!(
            colors.len() >= 2,
            "expected multiple syntect colors in a code block, got {}",
            colors.len()
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
