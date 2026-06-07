//! Smoke test for the embeddable Editor facade (the Noet integration surface).
use sred_core::{Command, Editor, Format, MarkSet};

#[test]
fn facade_drives_edit_render_and_is_lossless() {
    let mut ed = Editor::from_source("# Title\n\nhello", Format::Markdown);
    ed.set_viewport(700, 500.0);

    // edit through the facade
    ed.apply(Command::SelectAll);
    ed.apply(Command::ToggleMark(MarkSet::BOLD));
    assert!(
        ed.text().contains("**"),
        "edit went through: {:?}",
        ed.text()
    );

    // render produces a non-empty frame + a caret
    let out = ed.render(true);
    assert!(out.frame.width > 0 && out.frame.height > 0);
    assert_eq!(
        out.frame.rgba.len(),
        (out.frame.width * out.frame.height * 4) as usize
    );
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
    assert!(
        near_end >= near_start,
        "clicking right moves caret right or equal"
    );
}

#[test]
fn click_is_document_space_and_scroll_independent() {
    // A click at a fixed document-y must resolve to the same position whether or
    // not the view is scrolled (the host's Flickable reports document coords).
    let body = (0..40)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut ed = Editor::from_source(&body, Format::Markdown);
    ed.set_viewport(400, 120.0); // small viewport → scrollable
    let _ = ed.render(true);

    ed.click(20.0, 200.0);
    let c_unscrolled = ed.core_mut().cursor();

    ed.scroll_by(150.0);
    let _ = ed.render(false);
    ed.click(20.0, 200.0); // same document-y
    let c_scrolled = ed.core_mut().cursor();

    assert_eq!(
        c_unscrolled, c_scrolled,
        "document-space click must not change with scroll (no double-count)"
    );
}

#[test]
fn token_chip_background_is_painted() {
    use sred_core::{TokenMatch, TokenSpec};
    let mut ed = Editor::from_source("see [[Project]] here", Format::Markdown);
    ed.set_viewport(700, 300.0);
    let bg = [220u8, 245, 230, 255];
    ed.register_token(TokenSpec {
        id: "wikilink".into(),
        fg: [31, 122, 68, 255],
        bg: Some(bg),
        matcher: Box::new(|line: &str| {
            let chars: Vec<char> = line.chars().collect();
            let mut out = Vec::new();
            let mut i = 0;
            while i + 1 < chars.len() {
                if chars[i] == '[' && chars[i + 1] == '[' {
                    if let Some(c) = (i + 2..chars.len().saturating_sub(1))
                        .find(|&k| chars[k] == ']' && chars[k + 1] == ']')
                    {
                        out.push(TokenMatch {
                            start: i,
                            end: c + 2,
                            value: chars[i + 2..c].iter().collect(),
                        });
                        i = c + 2;
                        continue;
                    }
                }
                i += 1;
            }
            out
        }),
    });
    let out = ed.render(true);
    let painted = out.frame.rgba.chunks_exact(4).any(|p| p == bg);
    assert!(
        painted,
        "registered token chip background should be painted in the frame"
    );
}
