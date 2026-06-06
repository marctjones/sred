//! High-level embeddable editor facade.
//!
//! `sred-core` is UI-free; a host (e.g. Noet) embeds the editor by owning an
//! [`Editor`], forwarding input events to it, and pushing the [`Frame`] it
//! produces into a host image (a Slint `Image`, an `egui` texture, …). This
//! bundles the whole per-keystroke pipeline (style → decorate → rasterize →
//! caret-follow) that would otherwise be reimplemented per host.
//!
//! Typical host loop:
//! ```ignore
//! let mut ed = Editor::from_source(&note_body, Format::Markdown);
//! ed.set_theme(my_theme);                 // host palette + scale
//! ed.set_viewport(width_px, height_px);   // on resize
//! // on a key:   ed.apply(Command::Insert("x".into()));
//! // on a click: ed.click(x, y);
//! let out = ed.render(true);              // -> RGBA frame + caret + scroll
//! // host: show out.frame as an image at out.scroll_y, draw the caret rect.
//! // persist with ed.text() — byte-lossless.
//! ```

use crate::editor::{Command, EditorCore};
use crate::layout::{Caret, Frame, TextRenderer, Theme};
use crate::model::Format;

/// One render's output for the host to display.
pub struct FrameOut {
    /// Full-document RGBA image (host shows it in a scroll container).
    pub frame: Frame,
    /// Caret rectangle in document coordinates.
    pub caret: Caret,
    /// Suggested vertical scroll offset in px after caret-follow.
    pub scroll_y: f32,
    /// Document height in px (for the scroll container / scrollbar).
    pub doc_height: u32,
}

/// An embeddable editor: owns the source buffer, the renderer, the theme, and
/// the viewport/scroll state.
pub struct Editor {
    core: EditorCore,
    renderer: TextRenderer,
    theme: Theme,
    width: u32,
    viewport_h: f32,
    scroll_y: f32,
}

impl Editor {
    pub fn new(format: Format) -> Self {
        Editor {
            core: EditorCore::new(format),
            renderer: TextRenderer::new(),
            theme: Theme::default(),
            width: 800,
            viewport_h: 600.0,
            scroll_y: 0.0,
        }
    }

    /// Load source text verbatim (byte-lossless).
    pub fn from_source(src: &str, format: Format) -> Self {
        let mut e = Editor::new(format);
        e.core = EditorCore::from_source(src, format);
        e
    }

    // ---- content (byte-lossless) ------------------------------------------

    /// Exactly what was loaded/typed — persist this.
    pub fn text(&self) -> String {
        self.core.text()
    }
    pub fn set_text(&mut self, src: &str) {
        self.core.set_text(src);
    }
    pub fn selected_text(&self) -> String {
        self.core.selected_text()
    }
    pub fn link_at_cursor(&self) -> Option<String> {
        self.core.link_at_cursor()
    }
    pub fn update_link_at_cursor(&mut self, url: &str) -> bool {
        self.core.update_link_at_cursor(url)
    }
    /// Escape hatch for advanced hosts that need the underlying engine.
    pub fn core_mut(&mut self) -> &mut EditorCore {
        &mut self.core
    }

    // ---- configuration -----------------------------------------------------

    /// Host palette + font scale (build a [`Theme`] from your app's colors).
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }
    pub fn theme(&self) -> &Theme {
        &self.theme
    }
    /// Editor viewport size in physical px (call on resize).
    pub fn set_viewport(&mut self, width: u32, height: f32) {
        self.width = width.max(1);
        self.viewport_h = height.max(1.0);
    }

    // ---- editing -----------------------------------------------------------

    pub fn apply(&mut self, cmd: Command) {
        self.core.apply(cmd);
    }

    // ---- pointer input (x,y in px within the viewport) --------------------

    fn hit(&mut self, x: f32, y: f32) -> usize {
        let (spans, deltas) = self.core.styled_runs(self.theme.font_size);
        let text = self.core.text();
        // viewport y -> document y
        let doc_y = y + self.scroll_y;
        self.renderer
            .hit(&spans, &text, &deltas, self.width, &self.theme, x, doc_y)
    }

    pub fn click(&mut self, x: f32, y: f32) {
        let idx = self.hit(x, y);
        self.core.set_cursor(idx);
    }
    pub fn drag(&mut self, x: f32, y: f32) {
        let idx = self.hit(x, y);
        self.core.extend_to(idx);
    }
    pub fn double_click(&mut self, x: f32, y: f32) {
        let idx = self.hit(x, y);
        self.core.select_word_at(idx);
    }

    /// Vertical caret motion (Up/Down) — uses layout to find the column.
    pub fn move_vertical(&mut self, down: bool) {
        let (spans, deltas) = self.core.styled_runs(self.theme.font_size);
        let text = self.core.text();
        let cur = self.core.cursor();
        let idx =
            self.renderer
                .vertical(&spans, &text, &deltas, self.width, &self.theme, cur, down);
        self.core.set_cursor(idx);
    }

    // ---- scrolling ---------------------------------------------------------

    /// Scroll by a pixel delta (e.g. mouse wheel); clamped on the next render.
    pub fn scroll_by(&mut self, dy: f32) {
        self.scroll_y = (self.scroll_y + dy).max(0.0);
    }
    pub fn scroll_to(&mut self, y: f32) {
        self.scroll_y = y.max(0.0);
    }
    pub fn scroll_y(&self) -> f32 {
        self.scroll_y
    }

    // ---- render ------------------------------------------------------------

    /// Rasterize the document and (if `follow`) nudge the scroll to keep the
    /// caret on screen. Call after any input; push `FrameOut` to your UI.
    pub fn render(&mut self, follow: bool) -> FrameOut {
        let (spans, deltas) = self.core.styled_runs(self.theme.font_size);
        let decorations = self.core.decorations();
        let text = self.core.text();
        let cursor = self.core.cursor();
        let selection = self.core.selection();

        let out = self.renderer.render(
            &spans,
            &text,
            &deltas,
            &decorations,
            self.width,
            &self.theme,
            cursor,
            selection,
        );

        let doc_h = out.frame.height as f32;
        let max_scroll = (doc_h - self.viewport_h).max(0.0);
        let pad = out.caret.h.min(self.viewport_h * 0.3);
        let mut scroll = self.scroll_y.clamp(0.0, max_scroll);
        if follow {
            if out.caret.y - pad < scroll {
                scroll = out.caret.y - pad;
            } else if out.caret.y + out.caret.h + pad > scroll + self.viewport_h {
                scroll = out.caret.y + out.caret.h + pad - self.viewport_h;
            }
        }
        self.scroll_y = scroll.clamp(0.0, max_scroll);

        let doc_height = out.frame.height;
        FrameOut {
            frame: out.frame,
            caret: out.caret,
            scroll_y: self.scroll_y,
            doc_height,
        }
    }
}
