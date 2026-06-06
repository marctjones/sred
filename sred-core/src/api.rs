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
use crate::view::{Decoration, Span, TokenSpec};

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
    tokens: Vec<TokenSpec>,
    /// Bumped whenever the token set changes, so the per-line styling cache
    /// (keyed partly on this) invalidates token-colored lines.
    tokens_gen: u64,
}

/// Globally-unique generation source for token sets, so caches can't confuse two
/// editors' token configurations.
fn next_tokens_gen() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static GEN: AtomicU64 = AtomicU64::new(1);
    GEN.fetch_add(1, Ordering::Relaxed)
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
            tokens: Vec::new(),
            tokens_gen: 0,
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

    // ---- domain tokens (host extension) -----------------------------------

    /// Register an inline token kind (e.g. `[[wikilink]]`, `#tag`, url). Matched
    /// chars render in `spec.fg`; use [`token_at`](Self::token_at) on click.
    pub fn register_token(&mut self, spec: TokenSpec) {
        self.tokens.push(spec);
        self.tokens_gen = next_tokens_gen();
    }
    pub fn clear_tokens(&mut self) {
        self.tokens.clear();
        self.tokens_gen = next_tokens_gen();
    }

    /// The token under a viewport point, if any: `(id, value)` — route this to
    /// your filter / open-url handler.
    pub fn token_at(&mut self, x: f32, y: f32) -> Option<(String, String)> {
        let idx = self.hit(x, y);
        let text = self.core.text();
        let mut line_start = 0usize;
        for line in text.split('\n') {
            let n = line.chars().count();
            if idx <= line_start + n {
                let col = idx - line_start;
                for spec in &self.tokens {
                    for m in (spec.matcher)(line) {
                        if col >= m.start && col < m.end {
                            return Some((spec.id.clone(), m.value));
                        }
                    }
                }
                return None;
            }
            line_start += n + 1;
        }
        None
    }

    /// Chip-background decorations for registered tokens that set `bg` (global
    /// source-char ranges). Foreground coloring is applied in `styled_runs`.
    fn token_decorations_with(&self, text: &str) -> Vec<(usize, usize, Decoration)> {
        if self.tokens.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut line_start = 0usize;
        for line in text.split('\n') {
            let n = line.chars().count();
            for spec in &self.tokens {
                if let Some(bg) = spec.bg {
                    for m in (spec.matcher)(line) {
                        out.push((line_start + m.start, line_start + m.end, Decoration::Chip(bg)));
                    }
                }
            }
            line_start += n + 1;
        }
        out
    }

    fn styled_with(&self, text: &str) -> (Vec<Span>, Vec<i32>) {
        crate::view::styled_runs(
            text,
            self.core.format(),
            self.theme.font_size,
            self.core.cursor_line(),
            &self.tokens,
            self.tokens_gen,
        )
    }

    fn styled(&self) -> (Vec<Span>, Vec<i32>) {
        self.styled_with(&self.core.text())
    }

    // ---- editing -----------------------------------------------------------

    pub fn apply(&mut self, cmd: Command) {
        self.core.apply(cmd);
    }

    /// Whether an undo / redo step is available — for enabling/disabling host
    /// toolbar buttons or menu items.
    pub fn can_undo(&self) -> bool {
        self.core.can_undo()
    }
    pub fn can_redo(&self) -> bool {
        self.core.can_redo()
    }

    // ---- pointer input ----------------------------------------------------
    //
    // Coordinates are in **document space** — the full-frame coordinate system,
    // matching what a `TouchArea` inside a scroll container (Flickable) reports
    // (its `mouse-y` already includes the scroll offset). Do NOT add `scroll_y`.

    fn hit(&mut self, x: f32, y: f32) -> usize {
        let (spans, deltas) = self.styled();
        let text = self.core.text();
        self.renderer
            .hit(&spans, &text, &deltas, self.width, &self.theme, x, y)
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
        let (spans, deltas) = self.styled();
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
        // Compute the source text once and share it across the whole pipeline
        // (was cloned 4× per render).
        let text = self.core.text();
        let (spans, deltas) = self.styled_with(&text);
        let mut decorations = crate::view::decorations(&text, self.core.format());
        decorations.extend(self.token_decorations_with(&text));
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

    /// Viewport-bounded render: rasterizes **only the visible slice**, so the
    /// returned image is viewport-sized and the per-keystroke alloc + GPU upload
    /// are flat regardless of document length. Caret-follow is applied inside
    /// the same shaping pass (the rasterized slice always matches the resolved
    /// scroll — typing at the bottom can't paint a stale frame).
    ///
    /// Unlike [`render`](Self::render), in the returned [`FrameOut`]:
    /// - `frame` is **viewport-sized** — display it at a *fixed* position, not
    ///   inside a scrolled `Flickable` content area.
    /// - `caret` is in **viewport-relative** coordinates (already minus scroll).
    /// - `doc_height` is the full scrollable height (size your scrollbar to it);
    ///   scroll by calling [`scroll_by`](Self::scroll_by) / [`scroll_to`] and
    ///   re-rendering.
    pub fn render_view(&mut self, follow: bool) -> FrameOut {
        let text = self.core.text();
        let (spans, deltas) = self.styled_with(&text);
        let mut decorations = crate::view::decorations(&text, self.core.format());
        decorations.extend(self.token_decorations_with(&text));
        let cursor = self.core.cursor();
        let selection = self.core.selection();

        let out = self.renderer.render_viewport(
            &spans,
            &text,
            &deltas,
            &decorations,
            self.width,
            self.viewport_h as u32,
            self.scroll_y,
            follow,
            &self.theme,
            cursor,
            selection,
        );
        self.scroll_y = out.scroll_y;
        FrameOut {
            frame: out.frame,
            caret: out.caret,
            scroll_y: out.scroll_y,
            doc_height: out.doc_height,
        }
    }
}
