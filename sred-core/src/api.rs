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
    /// Primary caret rectangle (same coordinate space as `carets[0]`).
    pub caret: Caret,
    /// All caret rectangles (primary + any secondary multi-cursors), in the same
    /// coordinate space as `caret`. Usually one element; draw each as a bar.
    pub carets: Vec<Caret>,
    /// Suggested vertical scroll offset in px after caret-follow.
    pub scroll_y: f32,
    /// Document height in px (for the scroll container / scrollbar).
    pub doc_height: u32,
}

/// A screen-space rectangle (e.g. where to overlay a rendered math fragment).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
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
    /// Host spellcheck callback (text → misspelled char ranges) + a cache keyed by
    /// text hash so it only re-runs when the document changes.
    spellcheck: Option<Box<dyn Fn(&str) -> Vec<(usize, usize)>>>,
    spell_cache: Option<(u64, Vec<(usize, usize)>)>,
    /// Find/replace match highlights (display char ranges) + the current match.
    search_hits: Vec<(usize, usize)>,
    search_current: Option<usize>,
    /// Host renderer for math fragments (`src`, `display`, `font_size`) → image,
    /// plus a cache so an unchanged fragment isn't recompiled.
    fragment_renderer: Option<Box<dyn Fn(&str, bool, f32) -> Option<FragmentImage>>>,
    fragment_cache: std::collections::HashMap<(String, bool, u32), Option<FragmentImage>>,
}

/// An RGBA image a host renders for a math/figure fragment (e.g. via the Typst
/// engine), to overlay on the editor at the fragment's position.
#[derive(Clone)]
pub struct FragmentImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Whether the host background is dark (Rec. 601 luma < 50%), so fenced-code
/// highlighting can pick a matching syntect theme and stay legible (#21).
fn theme_is_dark(theme: &Theme) -> bool {
    let [r, g, b, _] = theme.bg;
    let luma = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    luma < 128.0
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
            spellcheck: None,
            spell_cache: None,
            search_hits: Vec::new(),
            search_current: None,
            fragment_renderer: None,
            fragment_cache: std::collections::HashMap::new(),
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
                        out.push((
                            line_start + m.start,
                            line_start + m.end,
                            Decoration::Chip(bg),
                        ));
                    }
                }
            }
            line_start += n + 1;
        }
        out
    }

    fn styled_with(&self, text: &str) -> (Vec<Span>, Vec<i32>) {
        let palette = crate::view::SynPalette {
            keyword: self.theme.syn_keyword,
            function: self.theme.syn_function,
            number: self.theme.syn_number,
            string: self.theme.syn_string,
            comment: self.theme.syn_comment,
            operator: self.theme.syn_operator,
        };
        crate::view::styled_runs_with(
            text,
            self.core.format(),
            self.theme.font_size,
            self.core.cursor_line(),
            &self.tokens,
            self.tokens_gen,
            &palette,
            theme_is_dark(&self.theme),
        )
    }

    fn styled(&self) -> (Vec<Span>, Vec<i32>) {
        self.styled_with(&self.core.text())
    }

    // ---- editing -----------------------------------------------------------

    pub fn apply(&mut self, cmd: Command) {
        self.core.apply(cmd);
    }

    /// Add a secondary caret (multi-cursor). Insert/Backspace then apply at all
    /// carets; any other command collapses them. `carets()` lists them for drawing.
    pub fn add_caret(&mut self, idx: usize) {
        self.core.add_caret(idx);
    }
    /// Add a secondary caret at a pointer position (e.g. Alt+click).
    pub fn add_caret_at(&mut self, x: f32, y: f32) {
        let idx = self.hit(x, y);
        self.core.add_caret(idx);
    }
    pub fn clear_extra_carets(&mut self) {
        self.core.clear_extra_carets();
    }
    pub fn carets(&self) -> Vec<usize> {
        self.core.carets()
    }
    /// Enable/disable automatic bracket & quote pairing (on by default).
    pub fn set_auto_pairs(&mut self, on: bool) {
        self.core.set_auto_pairs(on);
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
        // Pointer interaction cancels any IME composition, so hit-testing runs on
        // the clean buffer text (no injected preedit to map around).
        self.core.clear_preedit();
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
    /// Triple-click selects the whole line/paragraph under the pointer.
    pub fn triple_click(&mut self, x: f32, y: f32) {
        let idx = self.hit(x, y);
        self.core.select_line_at(idx);
    }
    /// Drop the current selection's text at the pointer (drag-and-drop move).
    pub fn drop_selection_at(&mut self, x: f32, y: f32) {
        let idx = self.hit(x, y);
        self.core.move_selection_to(idx);
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

    /// Page up/down: move the caret by ~one viewport of lines (and let the next
    /// render's caret-follow scroll to it).
    pub fn page(&mut self, down: bool) {
        let rows = (self.viewport_h / self.theme.line_height).floor().max(1.0) as usize;
        // One row of overlap for context, like most editors.
        for _ in 0..rows.saturating_sub(1).max(1) {
            self.move_vertical(down);
        }
    }

    // ---- clipboard (host owns the system clipboard; these are the hooks) ------

    /// Selected text for Copy (empty if no selection).
    pub fn copy(&self) -> String {
        self.core.copy()
    }
    /// Selected text for Cut; also deletes the selection.
    pub fn cut(&mut self) -> String {
        self.core.cut()
    }
    /// Insert clipboard text at the caret (replaces any selection).
    pub fn paste(&mut self, text: &str) {
        self.core.paste(text);
    }

    // ---- IME / preedit (host forwards platform composition events) ----------

    /// Set/replace the in-flight IME composition (`caret` = caret char-offset
    /// within `text`). The preedit is shown underlined and is NOT in `text()`.
    pub fn set_preedit(&mut self, text: &str, caret: usize) {
        self.core.set_preedit(text, caret);
    }
    /// Commit the composition (or `text`) as a real edit.
    pub fn commit_preedit(&mut self, text: &str) {
        self.core.commit_preedit(text);
    }
    /// Cancel the composition.
    pub fn clear_preedit(&mut self) {
        self.core.clear_preedit();
    }
    pub fn has_preedit(&self) -> bool {
        self.core.has_preedit()
    }

    // ---- accessibility ------------------------------------------------------

    /// Host-agnostic accessibility snapshot (map onto AccessKit or your backend).
    pub fn a11y(&self) -> crate::editor::A11ySnapshot {
        self.core.a11y()
    }

    // ---- find / replace -----------------------------------------------------

    /// All matches of `query` as char ranges (host drives the find UI).
    pub fn find(&self, query: &str, opts: crate::editor::SearchOpts) -> Vec<(usize, usize)> {
        self.core.find_all(query, opts)
    }
    /// Replace every match, returning the count (one undoable edit).
    pub fn replace_all(
        &mut self,
        query: &str,
        with: &str,
        opts: crate::editor::SearchOpts,
    ) -> usize {
        self.core.replace_all(query, with, opts)
    }
    /// Highlight a set of match ranges (e.g. from [`find`](Self::find)); `current`
    /// indexes the active match (drawn more strongly). Pass an empty slice to clear.
    pub fn set_search_highlights(&mut self, hits: &[(usize, usize)], current: Option<usize>) {
        self.search_hits = hits.to_vec();
        self.search_current = current;
    }

    // ---- spellcheck ---------------------------------------------------------

    /// Register a host spellchecker (`text → misspelled char ranges`). It re-runs
    /// only when the document text changes (cached by content hash). Misspellings
    /// render with a colored squiggle (`Theme`-independent red by default).
    pub fn set_spellchecker(&mut self, checker: Box<dyn Fn(&str) -> Vec<(usize, usize)>>) {
        self.spellcheck = Some(checker);
        self.spell_cache = None;
    }
    pub fn clear_spellchecker(&mut self) {
        self.spellcheck = None;
        self.spell_cache = None;
    }
    /// The word range + text under a point — for a host "correct this word" menu.
    pub fn word_at(&mut self, x: f32, y: f32) -> Option<(std::ops::Range<usize>, String)> {
        let idx = self.hit(x, y);
        let (s, e) = self.core.word_at(idx);
        if s == e {
            return None;
        }
        let text = self.core.text();
        let word: String = text.chars().skip(s).take(e - s).collect();
        Some((s..e, word))
    }

    // ---- rendered fragments (math / figures) -------------------------------

    /// Register a host renderer that compiles a math/figure fragment (`src`,
    /// `display`, `font_size`) to an RGBA image (e.g. via the Typst engine). The
    /// editor caches results by `(src, display, font_size)`, so a host can call
    /// [`render_fragment`](Self::render_fragment) every frame cheaply.
    ///
    /// sred does not bundle a compiler (by design — see DESIGN.md); the host
    /// supplies it. Position the overlay with [`rect_for_range`](Self::rect_for_range)
    /// over the fragment's `(start, end)`.
    pub fn set_fragment_renderer(
        &mut self,
        renderer: Box<dyn Fn(&str, bool, f32) -> Option<FragmentImage>>,
    ) {
        self.fragment_renderer = Some(renderer);
        self.fragment_cache.clear();
    }

    /// Whether a fragment renderer is registered (so hosts can skip the math scan).
    pub fn has_fragment_renderer(&self) -> bool {
        self.fragment_renderer.is_some()
    }

    /// Math fragments in the current document (char ranges + delimited source +
    /// display flag), for a host to render and overlay.
    pub fn math_fragments(&self) -> Vec<crate::view::MathFragment> {
        crate::view::math_fragments(&self.core.text(), self.core.format())
    }

    /// Render one fragment to an image via the registered renderer (cached).
    /// Returns `None` if no renderer is set or it declined the fragment.
    pub fn render_fragment(&mut self, frag: &crate::view::MathFragment) -> Option<FragmentImage> {
        let renderer = self.fragment_renderer.as_ref()?;
        let key = (
            frag.src.clone(),
            frag.display,
            self.theme.font_size.to_bits(),
        );
        if let Some(hit) = self.fragment_cache.get(&key) {
            return hit.clone();
        }
        let img = renderer(&frag.src, frag.display, self.theme.font_size);
        self.fragment_cache.insert(key, img.clone());
        img
    }

    /// Screen-space rects covering a char range `[start, end)`, in the same
    /// coordinates as [`render_view`](Self::render_view)'s frame/caret (viewport
    /// relative, scroll already subtracted) — one rect per visual line. Use it to
    /// overlay a rendered fragment over its source span, or for any range UI.
    pub fn rect_for_range(&mut self, start: usize, end: usize) -> Vec<Rect> {
        let text = self.core.display_text();
        let (spans, deltas) = self.styled_with(&text);
        self.renderer
            .range_rects(&spans, &text, &deltas, self.width, &self.theme, start, end)
            .into_iter()
            .map(|(x, y, w, h)| Rect {
                x,
                y: y - self.scroll_y,
                w,
                h,
            })
            .collect()
    }

    /// Caret rects (primary + secondary multi-cursors) in document space, in one
    /// buffer build. Empty extra carets → just the primary `out.caret` is used.
    fn caret_rects_doc(&mut self, text: &str) -> Vec<Caret> {
        let offsets = self.core.carets();
        let (spans, deltas) = self.styled_with(text);
        self.renderer
            .caret_rects(&spans, text, &deltas, self.width, &self.theme, &offsets)
    }

    /// Misspelling squiggle + search-highlight + secondary-selection decorations.
    fn aux_decorations(&mut self, text: &str) -> Vec<(usize, usize, Decoration)> {
        let mut out = Vec::new();
        // Secondary multi-cursor selections, highlighted like the primary one
        // (the primary selection is drawn by the renderer's selection pass).
        for (s, e) in self.core.extra_selections() {
            out.push((s, e, Decoration::Chip(self.theme.selection)));
        }
        // Search highlights: pale chip for matches, selection color for the current.
        for (i, &(s, e)) in self.search_hits.iter().enumerate() {
            let color = if Some(i) == self.search_current {
                self.theme.selection
            } else {
                let c = self.theme.selection;
                [c[0], c[1], c[2], (c[3] / 2).max(30)]
            };
            out.push((s, e, Decoration::Chip(color)));
        }
        // Spellcheck squiggles (cached by text hash).
        if let Some(check) = &self.spellcheck {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            text.hash(&mut h);
            let key = h.finish();
            if self.spell_cache.as_ref().map(|(k, _)| *k) != Some(key) {
                self.spell_cache = Some((key, check(text)));
            }
            if let Some((_, ranges)) = &self.spell_cache {
                for &(s, e) in ranges {
                    out.push((s, e, Decoration::Squiggle([200, 40, 40, 255])));
                }
            }
        }
        out
    }

    /// Underline decoration for the active IME preedit (display char range).
    fn preedit_decoration(&self) -> Option<(usize, usize, Decoration)> {
        self.core
            .preedit_range()
            .map(|(s, e)| (s, e, Decoration::Underline))
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
        // Render the *display* text (buffer with any IME preedit injected); the
        // saved text() stays clean. Computed once and shared across the pipeline.
        let text = self.core.display_text();
        let (spans, deltas) = self.styled_with(&text);
        let mut decorations = crate::view::decorations(&text, self.core.format());
        decorations.extend(self.token_decorations_with(&text));
        decorations.extend(self.preedit_decoration());
        decorations.extend(self.aux_decorations(&text));
        let cursor = self.core.display_cursor();
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
        // Multi-cursor: extra caret rects (document space, same as `out.caret`).
        let carets = if self.core.has_multi_carets() {
            self.caret_rects_doc(&text)
        } else {
            vec![out.caret]
        };
        FrameOut {
            frame: out.frame,
            caret: out.caret,
            carets,
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
        let text = self.core.display_text();
        let (spans, deltas) = self.styled_with(&text);
        let mut decorations = crate::view::decorations(&text, self.core.format());
        decorations.extend(self.token_decorations_with(&text));
        decorations.extend(self.preedit_decoration());
        decorations.extend(self.aux_decorations(&text));
        let cursor = self.core.display_cursor();
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
        // Multi-cursor: extra caret rects, viewport-relative (scroll subtracted),
        // matching `out.caret`.
        let carets = if self.core.has_multi_carets() {
            self.caret_rects_doc(&text)
                .into_iter()
                .map(|mut c| {
                    c.y -= out.scroll_y;
                    c
                })
                .collect()
        } else {
            vec![out.caret]
        };
        FrameOut {
            frame: out.frame,
            caret: out.caret,
            carets,
            scroll_y: out.scroll_y,
            doc_height: out.doc_height,
        }
    }
}
