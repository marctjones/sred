//! Slint integration for the sred editor.
//!
//! Exposes the generated `RichTextEditor` / `MainWindow` components and a [`run`]
//! entry point that wires an [`EditorCore`] + [`TextRenderer`] to the demo
//! window: forward the component's callbacks into `EditorCore`, push a
//! re-rasterized frame back after each edit, and bridge clipboard / link opening.

use std::cell::RefCell;
use std::rc::Rc;

use sred_core::{BlockKind, Command, EditorCore, Format, MarkSet, Motion, TextRenderer, Theme};

slint::include_modules!();

struct Controller {
    core: EditorCore,
    renderer: TextRenderer,
    width: u32,
    viewport_h: f32,
    clipboard: Option<arboard::Clipboard>,
}

impl Controller {
    fn new(src: &str, format: Format) -> Self {
        Controller {
            core: EditorCore::from_source(src, format),
            renderer: TextRenderer::new(),
            width: 820,
            viewport_h: 400.0,
            clipboard: arboard::Clipboard::new().ok(),
        }
    }

    fn clip_get(&mut self) -> Option<String> {
        self.clipboard.as_mut().and_then(|c| c.get_text().ok())
    }
    fn clip_set(&mut self, text: String) {
        if let Some(c) = self.clipboard.as_mut() {
            let _ = c.set_text(text);
        }
    }
}

/// Normalize a Ctrl-chord key to a lowercase letter, mapping control characters
/// (U+0001..=U+001A) back to 'a'..='z'.
fn normalize_chord(key: &str) -> String {
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => {
            let code = c as u32;
            if (1..=26).contains(&code) {
                ((b'a' + (code as u8 - 1)) as char).to_string()
            } else {
                c.to_lowercase().to_string()
            }
        }
        _ => key.to_string(),
    }
}

fn is_url(s: &str) -> bool {
    let s = s.trim();
    s.starts_with("http://") || s.starts_with("https://") || s.starts_with("www.")
}

/// Map a color command name to a packed `0xRRGGBBAA`, or `None` for "clear".
fn color_for(name: &str) -> Option<Option<u32>> {
    Some(match name {
        "color-red" => Some(0xCC2222FF),
        "color-green" => Some(0x1E8E3EFF),
        "color-blue" => Some(0x1A56D6FF),
        "color-orange" => Some(0xC8650AFF),
        "color-clear" => None,
        _ => return None,
    })
}

/// Core-only commands (no clipboard / IO).
fn apply_core_command(core: &mut EditorCore, name: &str) -> bool {
    let cmd = match name {
        "bold" => Command::ToggleMark(MarkSet::BOLD),
        "italic" => Command::ToggleMark(MarkSet::ITALIC),
        "code" => Command::ToggleMark(MarkSet::CODE),
        "strike" => Command::ToggleMark(MarkSet::STRIKE),
        "paragraph" => Command::SetBlock(BlockKind::Paragraph),
        "h1" => Command::ToggleBlock(BlockKind::Heading(1)),
        "h2" => Command::ToggleBlock(BlockKind::Heading(2)),
        "h3" => Command::ToggleBlock(BlockKind::Heading(3)),
        "bullet" => Command::ToggleBlock(BlockKind::Bullet),
        "ordered" => Command::ToggleBlock(BlockKind::Ordered),
        "quote" => Command::ToggleBlock(BlockKind::Quote),
        "codeblock" => Command::ToggleBlock(BlockKind::Code),
        "divider" => Command::SetBlock(BlockKind::Divider),
        "undo" => Command::Undo,
        "redo" => Command::Redo,
        "image" => Command::Insert("![alt](https://)".into()),
        "math" => Command::Insert("$x$".into()),
        "table" => Command::Insert("col a | col b\n---- | ----\nv1 | v2".into()),
        _ => return false,
    };
    core.apply(cmd);
    true
}

/// Full command dispatch, including clipboard / link / color actions.
fn run_named(c: &mut Controller, name: &str) -> bool {
    match name {
        "copy" => {
            let t = c.core.selected_text();
            if !t.is_empty() {
                c.clip_set(t);
            }
            true
        }
        "cut" => {
            let t = c.core.selected_text();
            if !t.is_empty() {
                c.clip_set(t);
                c.core.apply(Command::DeleteSelection);
            }
            true
        }
        "paste" => {
            if let Some(t) = c.clip_get() {
                c.core.apply(Command::Insert(t));
            }
            true
        }
        "selectall" => {
            c.core.apply(Command::SelectAll);
            true
        }
        "openlink" => {
            if let Some(url) = c.core.link_at_cursor() {
                let _ = open::that(url);
            }
            true
        }
        "link" => {
            if c.core.selection().is_some() {
                // Prefer a URL on the clipboard, else the selected text if it's a
                // URL, else a placeholder the user can refine in the source pane.
                let sel = c.core.selected_text();
                let clip = c.clip_get().unwrap_or_default();
                let target = if is_url(&clip) {
                    clip.trim().to_string()
                } else if is_url(&sel) {
                    sel.trim().to_string()
                } else {
                    "https://example.com".to_string()
                };
                c.core.apply(Command::Link(target));
            } else {
                c.core.apply(Command::Insert("[text](https://)".into()));
            }
            true
        }
        other => {
            if let Some(color) = color_for(other) {
                c.core.apply(Command::SetColor(color));
                true
            } else {
                apply_core_command(&mut c.core, other)
            }
        }
    }
}

/// Run the demo window.
pub fn run() -> Result<(), slint::PlatformError> {
    let initial = "# Welcome to sred\n\nA *rich text* editor with **markdown** and \
                   `typst` backends, rendered through Slint.\n\nDrag to select, then \
                   use the toolbar, menu, or Ctrl-B / Ctrl-I. Ctrl-C / X / V copy, cut, \
                   paste.\n\n```rust\nfn main() {\n    let greeting = \"hello\";\n    \
                   println!(\"{greeting}, world\");  // syntect highlighting\n}\n```\n";
    let window = build_window(initial, Format::Markdown)?;
    window.run()
}

/// Build and fully wire the editor window **without** starting the event loop,
/// so headless integration tests can drive it (and `run` can start it).
fn build_window(initial: &str, format: Format) -> Result<MainWindow, slint::PlatformError> {
    let window = MainWindow::new()?;
    let ctl = Rc::new(RefCell::new(Controller::new(initial, format)));
    refresh(&window, &ctl, true);

    macro_rules! after {
        ($w:ident, $ctl:ident) => {
            if let Some(win) = $w.upgrade() {
                refresh(&win, &$ctl, true);
            }
        };
    }

    // --- text input -------------------------------------------------------
    {
        let w = window.as_weak();
        let ctl = ctl.clone();
        window.on_insert_text(move |s| {
            ctl.borrow_mut().core.apply(Command::Insert(s.to_string()));
            after!(w, ctl);
        });
    }

    // --- special keys -----------------------------------------------------
    {
        let w = window.as_weak();
        let ctl = ctl.clone();
        window.on_special(move |name| {
            {
                let mut c = ctl.borrow_mut();
                match name.as_str() {
                    "up" | "down" => {
                        let down = name == "down";
                        let width = c.width;
                        let theme = Theme::default();
                        let (spans, pb) = c.core.styled_runs(theme.font_size);
                        let text = c.core.text();
                        let cur = c.core.cursor();
                        let idx = c.renderer.vertical(&spans, &text, &pb, width, &theme, cur, down);
                        c.core.set_cursor(idx);
                    }
                    other => {
                        let cmd = match other {
                            "backspace" => Some(Command::DeleteBackward),
                            "delete" => Some(Command::DeleteForward),
                            "left" => Some(Command::Move(Motion::Left)),
                            "right" => Some(Command::Move(Motion::Right)),
                            "home" => Some(Command::Move(Motion::LineStart)),
                            "end" => Some(Command::Move(Motion::LineEnd)),
                            "select-left" => Some(Command::Select(Motion::Left)),
                            "select-right" => Some(Command::Select(Motion::Right)),
                            _ => None,
                        };
                        if let Some(cmd) = cmd {
                            c.core.apply(cmd);
                        }
                    }
                }
            }
            after!(w, ctl);
        });
    }

    // --- Ctrl chords ------------------------------------------------------
    {
        let w = window.as_weak();
        let ctl = ctl.clone();
        window.on_chord(move |key| {
            // Under Ctrl, Slint may deliver a control character (e.g. U+0003 for
            // Ctrl-C) instead of the letter — normalize both forms to a letter.
            let norm = normalize_chord(&key);
            let name = match norm.as_str() {
                "b" => "bold",
                "i" => "italic",
                "e" | "`" => "code",
                "z" => "undo",
                "y" => "redo",
                "c" => "copy",
                "x" => "cut",
                "v" => "paste",
                "a" => "selectall",
                "k" => "link",
                _ => return false,
            };
            let handled = run_named(&mut ctl.borrow_mut(), name);
            if handled {
                after!(w, ctl);
            }
            handled
        });
    }

    // --- toolbar + menu (single command dispatch) -------------------------
    {
        let w = window.as_weak();
        let ctl = ctl.clone();
        window.on_command(move |name| {
            if run_named(&mut ctl.borrow_mut(), &name) {
                after!(w, ctl);
            }
        });
    }

    // --- pointer (click + drag selection + double-click word) -------------
    {
        let w = window.as_weak();
        let ctl = ctl.clone();
        window.on_pointer_down(move |x, y| {
            let idx = hit(&ctl, x, y);
            ctl.borrow_mut().core.set_cursor(idx);
            after!(w, ctl);
        });
    }
    {
        let w = window.as_weak();
        let ctl = ctl.clone();
        window.on_pointer_drag(move |x, y| {
            let idx = hit(&ctl, x, y);
            ctl.borrow_mut().core.extend_to(idx);
            after!(w, ctl);
        });
    }
    {
        let w = window.as_weak();
        let ctl = ctl.clone();
        window.on_pointer_double(move |x, y| {
            let idx = hit(&ctl, x, y);
            ctl.borrow_mut().core.select_word_at(idx);
            after!(w, ctl);
        });
    }

    // --- scroll (wheel / scrollbar) → re-render the new slice, no caret-follow -
    {
        let w = window.as_weak();
        let ctl = ctl.clone();
        window.on_scrolled(move |_y| {
            if let Some(win) = w.upgrade() {
                refresh(&win, &ctl, false);
            }
        });
    }

    // --- resize -----------------------------------------------------------
    {
        let w = window.as_weak();
        let ctl = ctl.clone();
        window.on_resized(move |width, height| {
            let px = width.max(0.0) as u32;
            if px < 80 {
                return;
            }
            let changed = {
                let mut c = ctl.borrow_mut();
                let changed = c.width != px || (c.viewport_h - height).abs() > 0.5;
                c.width = px;
                c.viewport_h = height.max(1.0);
                changed
            };
            if changed {
                after!(w, ctl);
            }
        });
    }

    // --- link URL editing -------------------------------------------------
    {
        let w = window.as_weak();
        let ctl = ctl.clone();
        window.on_apply_link_url(move |url| {
            {
                let mut c = ctl.borrow_mut();
                let u = url.trim().to_string();
                if c.core.selection().is_some() {
                    c.core.apply(Command::Link(u));
                } else {
                    c.core.update_link_at_cursor(&u);
                }
            }
            after!(w, ctl);
        });
    }

    // --- format switch ----------------------------------------------------
    {
        let w = window.as_weak();
        let ctl = ctl.clone();
        window.on_format_changed(move |f| {
            if let Some(fmt) = Format::from_str(&f) {
                ctl.borrow_mut().core.set_format(fmt);
                after!(w, ctl);
            }
        });
    }

    Ok(window)
}

fn hit(ctl: &Rc<RefCell<Controller>>, x: f32, y: f32) -> usize {
    let mut c = ctl.borrow_mut();
    let width = c.width;
    let theme = Theme::default();
    let (spans, pb) = c.core.styled_runs(theme.font_size);
    let text = c.core.text();
    c.renderer.hit(&spans, &text, &pb, width, &theme, x, y)
}

/// Re-rasterize the **visible slice** (viewport-bounded rendering) and push the
/// frame + caret + source into the window. `follow` keeps the caret on screen
/// after edits/clicks; pass `false` for plain scrolling so the view doesn't snap
/// back to the caret. Caret-follow + scroll clamping happen inside
/// `render_viewport`, which returns the resolved scroll.
fn refresh(window: &MainWindow, ctl: &Rc<RefCell<Controller>>, follow: bool) {
    let mut c = ctl.borrow_mut();
    let width = c.width;
    let vp_h = c.viewport_h;
    let theme = Theme::default();
    let (spans, prefix_bytes) = c.core.styled_runs(theme.font_size);
    let decorations = c.core.decorations();
    let text = c.core.text();
    let cursor = c.core.cursor();
    let selection = c.core.selection();
    let scroll_in = window.get_scroll_y();

    let out = c.renderer.render_viewport(
        &spans,
        &text,
        &prefix_bytes,
        &decorations,
        width,
        vp_h as u32,
        scroll_in,
        follow,
        &theme,
        cursor,
        selection,
    );

    // Viewport-sized frame + viewport-relative caret + resolved scroll.
    let img = rgba_to_image(&out.frame.rgba, out.frame.width, out.frame.height);
    window.set_frame(img);
    window.set_doc_height(out.doc_height as f32);
    window.set_caret_x(out.caret.x);
    window.set_caret_y(out.caret.y);
    window.set_caret_h(out.caret.h);
    // Keep the caret visible even while a selection is active so it never "disappears".
    window.set_caret_on(true);
    window.set_scroll_y(out.scroll_y);
    window.set_source_text(c.core.source().into());
    // Show the URL of the link under the caret (if any) in the toolbar field.
    window.set_link_url(c.core.link_at_cursor().unwrap_or_default().into());
}

fn rgba_to_image(rgba: &[u8], width: u32, height: u32) -> slint::Image {
    let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(width, height);
    buf.make_mut_bytes().copy_from_slice(rgba);
    slint::Image::from_rgba8(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- pure helper unit tests (no backend needed) ----------------------

    #[test]
    fn chord_normalization() {
        assert_eq!(normalize_chord("\u{3}"), "c"); // Ctrl-C delivered as control char
        assert_eq!(normalize_chord("C"), "c"); // uppercase
        assert_eq!(normalize_chord("b"), "b");
    }

    #[test]
    fn url_detection() {
        assert!(is_url("https://example.com"));
        assert!(is_url("  http://x.io "));
        assert!(is_url("www.x.io"));
        assert!(!is_url("just text"));
    }

    #[test]
    fn color_name_mapping() {
        assert_eq!(color_for("color-clear"), Some(None));
        assert!(matches!(color_for("color-red"), Some(Some(_))));
        assert_eq!(color_for("not-a-color"), None);
    }

    // ---- controller-level workflow tests (core + dispatch, no UI) ---------

    fn controller(initial: &str) -> Controller {
        Controller::new(initial, Format::Markdown)
    }

    #[test]
    fn workflow_select_all_then_bold() {
        let mut c = controller("hello world");
        c.core.apply(Command::SelectAll);
        assert!(run_named(&mut c, "bold"));
        assert!(
            c.core.source().contains("**hello world**"),
            "got: {}",
            c.core.source()
        );
    }

    #[test]
    fn workflow_heading_and_list_via_dispatch() {
        let mut c = controller("Title");
        c.core.apply(Command::SelectAll);
        run_named(&mut c, "h2");
        assert!(c.core.source().starts_with("## Title"), "got: {}", c.core.source());

        let mut c = controller("item");
        c.core.apply(Command::SelectAll);
        run_named(&mut c, "bullet");
        assert!(c.core.source().contains("- item"), "got: {}", c.core.source());
    }

    #[test]
    fn workflow_undo_redo_via_dispatch() {
        let mut c = controller("x");
        c.core.apply(Command::SelectAll);
        run_named(&mut c, "bold");
        assert!(c.core.source().contains("**x**"));
        run_named(&mut c, "undo");
        assert!(!c.core.source().contains("**x**"), "undo failed: {}", c.core.source());
        run_named(&mut c, "redo");
        assert!(c.core.source().contains("**x**"), "redo failed: {}", c.core.source());
    }

    #[test]
    fn workflow_color_is_view_only() {
        // Coloring must not change the serialized source.
        let mut c = controller("hi");
        c.core.apply(Command::SelectAll);
        run_named(&mut c, "color-red");
        assert_eq!(c.core.source().trim(), "hi");
    }

    // ---- end-to-end headless GUI test (drives the generated component) ----
    // The Slint backend is process-global and may only be initialised once, so
    // all UI assertions live in a single test. Run with `--test-threads=1`.

    #[test]
    fn gui_end_to_end_workflows() {
        i_slint_backend_testing::init_no_event_loop();

        // (1) Toolbar/menu command path: select-all + bold updates the source pane.
        let w = build_window("hello", Format::Markdown).unwrap();
        w.invoke_command("selectall".into());
        w.invoke_command("bold".into());
        assert!(
            w.get_source_text().contains("**hello**"),
            "command dispatch → source pane failed: {:?}",
            w.get_source_text()
        );

        // (2) Typed-input path: autoformat a heading via the insert-text callback.
        let w2 = build_window("", Format::Markdown).unwrap();
        for ch in "# Heading".chars() {
            w2.invoke_insert_text(ch.to_string().into());
        }
        assert!(
            w2.get_source_text().starts_with("# Heading"),
            "autoformat through the UI failed: {:?}",
            w2.get_source_text()
        );

        // (3) Source-anchored invariant: switching the view format must NOT
        // rewrite the source — the raw text is the buffer (byte-lossless).
        let w3 = build_window("# T", Format::Markdown).unwrap();
        assert_eq!(w3.get_source_text().as_str(), "# T");
        w3.invoke_format_changed("typst".into());
        assert_eq!(
            w3.get_source_text().as_str(),
            "# T",
            "format switch must not mutate the source"
        );
    }
}
