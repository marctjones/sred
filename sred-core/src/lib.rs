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

pub mod api;
pub mod editor;
pub mod format;
pub mod layout;
pub mod markdown;
pub mod model;
pub mod typst_fmt;
pub mod view;

pub use api::{Editor, FrameOut};
pub use view::{TokenMatch, TokenSpec};
pub use editor::{BlockKind, Command, Decoration, EditorCore, Motion, Span};
pub use format::{backend_for, Caps, DocumentFormat, FormatError};
pub use layout::{Frame, RenderOut, TextRenderer, Theme, ViewOut};
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
        let (spans, _) = view::styled_runs(src, Format::Markdown, 16.0, 0, &[], 0);
        let colors: std::collections::HashSet<[u8; 4]> =
            spans.iter().filter_map(|s| s.color).collect();
        assert!(
            colors.len() >= 2,
            "expected multiple syntect colors in a code block, got {}",
            colors.len()
        );
    }

    #[test]
    fn live_preview_hides_markers_off_caret_line() {
        let src = "para\n## Heading\n- item";
        let (spans, deltas) = view::styled_runs(src, Format::Markdown, 16.0, 0, &[], 0); // caret line 0
        let display: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert!(display.contains("Heading"));
        assert!(
            !display.contains("## Heading"),
            "heading marker should be hidden off the caret line: {display:?}"
        );
        assert!(display.contains("• item"), "bullet should be substituted: {display:?}");
        assert_eq!(deltas[1], -3, "hidden '## ' → delta -3");
        assert_eq!(deltas[2], 2, "'- ' → '• ' → delta +2");
    }

    #[test]
    fn live_preview_reveals_markers_on_caret_line() {
        let src = "## Heading";
        let (spans, deltas) = view::styled_runs(src, Format::Markdown, 16.0, 0, &[], 0); // caret on line 0
        let display: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert!(display.contains("## Heading"), "caret line shows the raw marker: {display:?}");
        assert_eq!(deltas[0], 0);
    }

    #[test]
    fn markdown_inline_via_commonmark() {
        // Per-display-char marks for a single (revealed) line.
        fn marks_of(line: &str) -> Vec<MarkSet> {
            let (spans, _) = view::styled_runs(line, Format::Markdown, 16.0, 0, &[], 0);
            spans.iter().flat_map(|s| s.text.chars().map(move |_| s.marks)).collect()
        }
        // Nesting: **b _i_** → 'b' bold, 'i' bold+italic (the hand-rolled scanner
        // couldn't nest reliably).  chars: * * b ␠ _ i _ * *
        let m = marks_of("**b _i_**");
        assert!(m[2].contains(MarkSet::BOLD), "b should be bold");
        assert!(
            m[5].contains(MarkSet::BOLD) && m[5].contains(MarkSet::ITALIC),
            "i should be bold+italic"
        );
        // Code span protects emphasis: `*x*` → x is code, NOT italic.
        let m = marks_of("`*x*`");
        assert!(m[2].contains(MarkSet::CODE) && !m[2].contains(MarkSet::ITALIC));
        // Inline link.
        assert!(marks_of("see [t](u)").iter().any(|x| x.contains(MarkSet::LINK)));
        // GFM strikethrough.
        assert!(marks_of("~~gone~~")[2].contains(MarkSet::STRIKE));
        // Unmatched delimiter is not emphasis (CommonMark).
        assert!(!marks_of("a **b c")[3].contains(MarkSet::BOLD));
    }

    #[test]
    fn typst_live_preview_projects_markup() {
        // Typst headings use '=' (level = count); '- ' is a bullet.
        let src = "para\n= Heading\n- item";
        let (spans, deltas) = view::styled_runs(src, Format::Typst, 16.0, 0, &[], 0); // caret line 0
        let display: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert!(display.contains("Heading"));
        assert!(
            !display.contains("= Heading"),
            "typst heading marker should hide off the caret line: {display:?}"
        );
        assert!(display.contains("• item"), "typst '- ' should become a bullet: {display:?}");
        assert_eq!(deltas[1], -2, "hidden '= ' → delta -2");
        assert_eq!(deltas[2], 2, "'- ' → '• ' → delta +2");
    }

    #[test]
    fn typst_inline_marks_bold_italic_math() {
        // On the caret line (markers shown): *strong*, _emph_, $math$.
        let (spans, _) = view::styled_runs("see *b* _i_ $x^2$", Format::Typst, 16.0, 0, &[], 0);
        assert!(spans.iter().any(|s| s.marks.contains(MarkSet::BOLD)), "typst *…* is bold");
        assert!(spans.iter().any(|s| s.marks.contains(MarkSet::ITALIC)), "typst _…_ is italic");
        assert!(spans.iter().any(|s| s.marks.contains(MarkSet::CODE)), "typst $…$ styled like code");
    }

    #[test]
    fn typst_full_via_typst_syntax() {
        fn marks_of(line: &str) -> Vec<MarkSet> {
            let (spans, _) = view::styled_runs(line, Format::Typst, 16.0, 0, &[], 0);
            spans.iter().flat_map(|s| s.text.chars().map(move |_| s.marks)).collect()
        }
        // Nested strong+emph: *b _i_*  → chars: * b ␠ _ i _ *
        let m = marks_of("*b _i_*");
        assert!(m[1].contains(MarkSet::BOLD), "b should be bold");
        assert!(
            m[4].contains(MarkSet::BOLD) && m[4].contains(MarkSet::ITALIC),
            "i should be bold+italic"
        );
        // Code mode (#let …) is recognized and styled like code.
        assert!(marks_of("#let x = 1").iter().any(|x| x.contains(MarkSet::CODE)));
        // A reference @target is link-styled.
        assert!(marks_of("see @intro here").iter().any(|x| x.contains(MarkSet::LINK)));
    }

    #[test]
    fn typst_and_markdown_differ_on_single_asterisk() {
        // Same source, format-dependent: '*x*' is STRONG in Typst, EMPHASIS in
        // Markdown. This is why the styling cache key must include the format.
        let (ts, _) = view::styled_runs("*x*", Format::Typst, 16.0, 0, &[], 0);
        let (md, _) = view::styled_runs("*x*", Format::Markdown, 16.0, 0, &[], 0);
        let ts_x = ts.iter().find(|s| s.text.contains('x')).unwrap();
        let md_x = md.iter().find(|s| s.text.contains('x')).unwrap();
        assert!(ts_x.marks.contains(MarkSet::BOLD) && !ts_x.marks.contains(MarkSet::ITALIC));
        assert!(md_x.marks.contains(MarkSet::ITALIC) && !md_x.marks.contains(MarkSet::BOLD));
    }

    #[test]
    fn tokens_color_matched_chars() {
        let spec = view::TokenSpec {
            id: "x".into(),
            fg: [10, 20, 30, 255],
            bg: None,
            matcher: Box::new(|_line: &str| {
                vec![view::TokenMatch { start: 4, end: 11, value: "Project".into() }]
            }),
        };
        let (spans, _) = view::styled_runs(
            "see [[Project]] x", Format::Markdown, 16.0, 0, std::slice::from_ref(&spec), 0,
        );
        assert!(
            spans.iter().any(|s| s.color == Some([10, 20, 30, 255])),
            "registered token color should appear in the rendered spans"
        );
    }

    #[test]
    fn styling_cache_matches_fresh_computation() {
        // The per-line styling cache must produce byte-identical spans + deltas +
        // decorations to a from-scratch (cache-cleared) computation — across
        // edits, caret moves, tokens, and a code fence (code lines aren't cached).
        fn spans_eq(a: &[view::Span], b: &[view::Span]) -> bool {
            a.len() == b.len()
                && a.iter().zip(b).all(|(x, y)| {
                    x.text == y.text && x.marks == y.marks && x.color == y.color && x.size == y.size
                })
        }
        let spec = view::TokenSpec {
            id: "p".into(),
            fg: [9, 8, 7, 255],
            bg: None,
            matcher: Box::new(|line: &str| {
                line.match_indices("[[P]]")
                    .map(|(i, _)| view::TokenMatch {
                        start: line[..i].chars().count(),
                        end: line[..i].chars().count() + 5,
                        value: "P".into(),
                    })
                    .collect()
            }),
        };
        let seq = [
            "# Title\n- a\n> quote\nplain **bold** ~~x~~ [[P]]\n```rust\nlet a=1;\n```\ntail",
            "# Title\n- aZ\n> quote\nplain **bold** ~~x~~ [[P]]\n```rust\nlet a=1;\n```\ntail",
            "# TitleQ\n- aZ\n> quote\nplain **bold** ~~x~~ [[P]]\n```rust\nlet a=12;\n```\ntail",
        ];
        for tokens_gen in [0u64, 7] {
            let toks: &[view::TokenSpec] =
                if tokens_gen == 0 { &[] } else { std::slice::from_ref(&spec) };
            for caret in [0usize, 1, 4, 5] {
                for t in seq {
                    let inc = view::styled_runs(t, Format::Markdown, 16.0, caret, toks, tokens_gen);
                    let inc_d = view::decorations(t, Format::Markdown);
                    view::clear_style_cache();
                    let fresh = view::styled_runs(t, Format::Markdown, 16.0, caret, toks, tokens_gen);
                    let fresh_d = view::decorations(t, Format::Markdown);
                    assert!(
                        spans_eq(&inc.0, &fresh.0) && inc.1 == fresh.1,
                        "styled cache != fresh (gen={tokens_gen} caret={caret} t={t:?})"
                    );
                    assert_eq!(inc_d, fresh_d, "deco cache != fresh (t={t:?})");
                }
            }
        }
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
