//! Layout + rasterization via `cosmic-text`.
//!
//! Styled spans (with per-run font size for headings) are shaped and line-broken
//! by cosmic-text, then rasterized to an RGBA buffer the Slint layer shows as an
//! `Image`. cosmic-text is also the source of truth for geometry: caret rect,
//! selection highlight, click hit-testing, and vertical motion.
//!
//! The rendered buffer contains non-editable block prefixes (bullets, quote
//! bars), so cursor positions are translated between the editable buffer and the
//! rendered buffer using `prefix_bytes` (the leading non-editable byte count per
//! block/line).

use cosmic_text::{
    Attrs, AttrsList, Buffer, BufferLine, Color, Cursor, Family, FontSystem, LineEnding, Metrics,
    Shaping, Style, SwashCache, Weight,
};

use crate::editor::{Decoration, Span};
use crate::model::MarkSet;

pub struct Theme {
    pub font_size: f32,
    pub line_height: f32,
    pub margin_x: f32,
    pub margin_y: f32,
    pub fg: [u8; 4],
    pub bg: [u8; 4],
    pub link: [u8; 4],
    pub code: [u8; 4],
    pub selection: [u8; 4],
    /// Per-token syntax-highlight palette (Step D): keyword / function / number /
    /// string / comment / operator. Drives Typst code-mode coloring (and fenced
    /// code where syntect is unavailable). Host-overridable.
    pub syn_keyword: [u8; 4],
    pub syn_function: [u8; 4],
    pub syn_number: [u8; 4],
    pub syn_string: [u8; 4],
    pub syn_comment: [u8; 4],
    pub syn_operator: [u8; 4],
    /// Body font family. `None` ⇒ cosmic-text's default sans. The named family
    /// must be loaded into the renderer's `FontSystem` (host responsibility).
    pub font_family: Option<String>,
    /// Monospace/code font family. `None` ⇒ the generic monospace family.
    pub code_font_family: Option<String>,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            font_size: 18.0,
            line_height: 26.0,
            margin_x: 16.0,
            margin_y: 16.0,
            fg: [33, 33, 33, 255],
            bg: [253, 253, 250, 255],
            link: [30, 100, 220, 255],
            code: [140, 40, 110, 255],
            selection: [120, 170, 255, 90],
            syn_keyword: [167, 29, 93, 255],
            syn_function: [0, 92, 197, 255],
            syn_number: [0, 134, 109, 255],
            syn_string: [3, 47, 98, 255],
            syn_comment: [150, 152, 150, 255],
            syn_operator: [80, 60, 130, 255],
            font_family: None,
            code_font_family: None,
        }
    }
}

pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
pub struct Caret {
    pub x: f32,
    pub y: f32,
    pub h: f32,
}

pub struct RenderOut {
    pub frame: Frame,
    pub caret: Caret,
    /// Full document height in px. Equals `frame.height` here; see [`ViewOut`]
    /// for the viewport-rendering variant.
    pub doc_height: u32,
}

/// Output of [`TextRenderer::render_viewport`]: a viewport-sized frame plus the
/// scroll the renderer actually resolved to (after clamping + caret-follow).
pub struct ViewOut {
    /// Viewport-sized RGBA image (host shows it at a fixed position).
    pub frame: Frame,
    /// Caret rectangle in **viewport-relative** coordinates.
    pub caret: Caret,
    /// The scroll offset actually used (input clamped, then caret-follow).
    pub scroll_y: f32,
    /// Full scrollable document height in px (for the host's scrollbar).
    pub doc_height: u32,
}

pub struct TextRenderer {
    font_system: FontSystem,
    swash: SwashCache,
    /// Persistent buffer for the viewport path: re-used across keystrokes so we
    /// only rebuild the lines that actually changed (cosmic-text's
    /// `set_rich_text` otherwise rebuilds every line's text + AttrsList — the
    /// dominant per-keystroke cost on long notes).
    cache_buf: Option<Buffer>,
    /// Per-line content signature matching `cache_buf.lines`. A line is rebuilt
    /// only when its signature changes.
    cache_sigs: Vec<u64>,
    /// Signature of (width, font metrics): any change forces a full rebuild.
    cache_key: u64,
}

impl Default for TextRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl TextRenderer {
    pub fn new() -> Self {
        TextRenderer {
            font_system: FontSystem::new(),
            swash: SwashCache::new(),
            cache_buf: None,
            cache_sigs: Vec::new(),
            cache_key: 0,
        }
    }

    fn build_buffer(&mut self, spans: &[Span], full_width: f32, theme: &Theme) -> Buffer {
        let metrics = Metrics::new(theme.font_size, theme.line_height);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        let shaping_w = (full_width - 2.0 * theme.margin_x).max(16.0);
        buffer.set_size(&mut self.font_system, Some(shaping_w), None);

        let default_attrs = base_attrs(theme);
        let rich: Vec<(&str, Attrs)> = spans
            .iter()
            .map(|s| {
                let attrs =
                    attrs_for(s.marks, s.color, theme).metrics(Metrics::new(s.size, s.size * 1.4));
                (s.text.as_str(), attrs)
            })
            .collect();
        buffer.set_rich_text(
            &mut self.font_system,
            rich,
            &default_attrs,
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);
        buffer
    }

    /// Build (or incrementally update) the persistent viewport buffer, rebuilding
    /// only the lines whose content changed since the last call. Identical layout
    /// to [`build_buffer`] (same text + attrs + metrics + line ending), so caret /
    /// hit / delta geometry are unaffected — verified by
    /// `incremental_matches_full_render`.
    ///
    /// Returns the buffer by value (taken out of `self`); the caller must store it
    /// back into `self.cache_buf` after use.
    fn build_buffer_cached(&mut self, spans: &[Span], full_width: f32, theme: &Theme) -> Buffer {
        let shaping_w = (full_width - 2.0 * theme.margin_x).max(16.0);
        let key = theme_key(theme, shaping_w);
        let lines = split_lines(spans);

        // Full rebuild if metrics/width changed, the line count changed (Enter /
        // delete-line / paste), or there's no cached buffer yet. Line splitting
        // must match cosmic-text's: structural divergence falls back here safely.
        // A full rebuild is only needed when there's no buffer yet or the render
        // parameters (width / metrics) changed. Line insertions/deletions are
        // handled incrementally below.
        if self.cache_buf.is_none() || self.cache_key != key {
            let buffer = self.build_buffer(spans, full_width, theme);
            self.cache_sigs = lines.iter().map(|l| l.sig).collect();
            self.cache_key = key;
            return buffer;
        }

        let mut buffer = self.cache_buf.take().unwrap();
        // Defensive: if our cached line count ever disagrees with the buffer's
        // actual line count (e.g. a Unicode paragraph separator we don't split
        // on slipped in), fall back to a full rebuild rather than mis-splice.
        if buffer.lines.len() != self.cache_sigs.len() {
            let nb = self.build_buffer(spans, full_width, theme);
            self.cache_sigs = lines.iter().map(|l| l.sig).collect();
            self.cache_key = key;
            return nb;
        }

        // Diff old vs new line signatures by common prefix + suffix, then splice
        // only the changed middle. This keeps single-line edits O(1) AND makes
        // line insert/delete (Enter, Backspace-join, paste) cost O(changed lines
        // + tail shift) instead of rebuilding the whole document.
        let new_sigs: Vec<u64> = lines.iter().map(|l| l.sig).collect();
        let (ol, nl) = (self.cache_sigs.len(), new_sigs.len());
        let mut p = 0;
        while p < ol && p < nl && self.cache_sigs[p] == new_sigs[p] {
            p += 1;
        }
        let mut s = 0;
        while s < ol - p && s < nl - p && self.cache_sigs[ol - 1 - s] == new_sigs[nl - 1 - s] {
            s += 1;
        }
        if p == ol && p == nl {
            // Nothing changed (e.g. a scroll-only re-render) — buffer is current.
            return buffer;
        }
        let rebuilt: Vec<BufferLine> = (p..nl - s)
            .map(|i| make_buffer_line(&lines[i], theme))
            .collect();
        buffer.lines.splice(p..ol - s, rebuilt);
        self.cache_sigs
            .splice(p..ol - s, new_sigs[p..nl - s].iter().copied());
        // Re-layout: unchanged lines hit cosmic-text's shape cache (cheap); only
        // the spliced lines actually re-shape.
        buffer.shape_until_scroll(&mut self.font_system, false);
        buffer
    }

    fn doc_height(buffer: &Buffer, theme: &Theme) -> u32 {
        let bottom = buffer
            .layout_runs()
            .map(|r| r.line_top + r.line_height)
            .fold(0.0f32, f32::max);
        (bottom.max(theme.line_height) + 2.0 * theme.margin_y).ceil() as u32
    }

    /// Render to an RGBA frame, painting selection behind the glyphs and
    /// returning the exact caret rectangle.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        spans: &[Span],
        text: &str,
        deltas: &[i32],
        decorations: &[(usize, usize, Decoration)],
        width: u32,
        theme: &Theme,
        cursor: usize,
        selection: Option<(usize, usize)>,
    ) -> RenderOut {
        let buffer = self.build_buffer(spans, width as f32, theme);
        let height = Self::doc_height(&buffer, theme);

        let mut rgba = vec![0u8; (width * height * 4) as usize];
        for px in rgba.chunks_exact_mut(4) {
            px.copy_from_slice(&theme.bg);
        }

        if let Some((s, e)) = selection {
            if s != e {
                let cs = flat_to_render_cursor(text, deltas, s);
                let ce = flat_to_render_cursor(text, deltas, e);
                for run in buffer.layout_runs() {
                    if let Some((x, w)) = run.highlight(cs, ce) {
                        fill_rect(
                            &mut rgba,
                            width,
                            height,
                            x + theme.margin_x,
                            run.line_top + theme.margin_y,
                            w.max(2.0),
                            run.line_height,
                            theme.selection,
                        );
                    }
                }
            }
        }

        // token chip backgrounds (behind the glyphs)
        for &(s, e, deco) in decorations {
            if let Decoration::Chip(color) = deco {
                if s >= e {
                    continue;
                }
                let cs = flat_to_render_cursor(text, deltas, s);
                let ce = flat_to_render_cursor(text, deltas, e);
                for run in buffer.layout_runs() {
                    if let Some((x, w)) = run.highlight(cs, ce) {
                        fill_rect(
                            &mut rgba,
                            width,
                            height,
                            x + theme.margin_x - 1.0,
                            run.line_top + theme.margin_y,
                            w.max(2.0) + 2.0,
                            run.line_height,
                            color,
                        );
                    }
                }
            }
        }

        let fg = Color::rgba(theme.fg[0], theme.fg[1], theme.fg[2], theme.fg[3]);
        let (mx, my) = (theme.margin_x as i32, theme.margin_y as i32);
        buffer.draw(&mut self.font_system, &mut self.swash, fg, |x, y, w, h, color| {
            for dy in 0..h as i32 {
                for dx in 0..w as i32 {
                    let px = x + dx + mx;
                    let py = y + dy + my;
                    if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                        continue;
                    }
                    let idx = ((py as u32 * width + px as u32) * 4) as usize;
                    blend(&mut rgba[idx..idx + 4], color);
                }
            }
        });

        // strikethrough / underline lines (cosmic-text draws neither)
        for &(s, e, deco) in decorations {
            if s >= e {
                continue;
            }
            let cs = flat_to_render_cursor(text, deltas, s);
            let ce = flat_to_render_cursor(text, deltas, e);
            for run in buffer.layout_runs() {
                if let Some((x, w)) = run.highlight(cs, ce) {
                    if w <= 0.0 {
                        continue;
                    }
                    let (frac, color) = match deco {
                        Decoration::Strike => (0.55, theme.fg),
                        Decoration::Underline => (0.86, theme.link),
                        Decoration::Squiggle(c) => (0.92, c),
                        Decoration::Chip(_) => continue, // drawn in the pre-glyph pass
                    };
                    let thick = (run.line_height * 0.06).max(1.5);
                    fill_rect(
                        &mut rgba,
                        width,
                        height,
                        x + theme.margin_x,
                        run.line_top + theme.margin_y + run.line_height * frac,
                        w,
                        thick,
                        color,
                    );
                }
            }
        }

        let mut caret = self.caret_geom(&buffer, text, deltas, cursor, theme);
        caret.x += theme.margin_x;
        caret.y += theme.margin_y;
        RenderOut {
            frame: Frame {
                width,
                height,
                rgba,
            },
            caret,
            doc_height: height,
        }
    }

    /// Render only the visible slice `[scroll_y, scroll_y + viewport_h)` of the
    /// document into a **viewport-sized** frame, so per-keystroke allocation and
    /// GPU upload are flat regardless of document length.
    ///
    /// The buffer is still shaped in full (geometry/caret/hit stay identical to
    /// [`render`]); only rasterization is bounded. Caret `y` is returned in
    /// viewport-relative coordinates. `doc_height` carries the full scrollable
    /// height for the host's scrollbar.
    ///
    /// When `follow` is set the scroll is nudged so the caret stays on screen,
    /// using the same buffer (one shaping pass) so the rasterized slice always
    /// matches the resolved scroll — typing at the bottom can never paint a
    /// stale slice.
    #[allow(clippy::too_many_arguments)]
    pub fn render_viewport(
        &mut self,
        spans: &[Span],
        text: &str,
        deltas: &[i32],
        decorations: &[(usize, usize, Decoration)],
        width: u32,
        viewport_h: u32,
        scroll_y: f32,
        follow: bool,
        theme: &Theme,
        cursor: usize,
        selection: Option<(usize, usize)>,
    ) -> ViewOut {
        let buffer = self.build_buffer_cached(spans, width as f32, theme);
        let doc_height = Self::doc_height(&buffer, theme);
        let height = viewport_h.max(1);

        // Doc-space caret first, so caret-follow can pick the slice before we
        // rasterize it.
        let mut doc_caret = self.caret_geom(&buffer, text, deltas, cursor, theme);
        doc_caret.x += theme.margin_x;
        doc_caret.y += theme.margin_y;

        let max_scroll = doc_height.saturating_sub(height) as f32;
        let mut sy = scroll_y.clamp(0.0, max_scroll);
        if follow {
            let pad = doc_caret.h.min(height as f32 * 0.3);
            if doc_caret.y - pad < sy {
                sy = doc_caret.y - pad;
            } else if doc_caret.y + doc_caret.h + pad > sy + height as f32 {
                sy = doc_caret.y + doc_caret.h + pad - height as f32;
            }
            sy = sy.clamp(0.0, max_scroll);
        }

        let mut rgba = vec![0u8; (width * height * 4) as usize];
        for px in rgba.chunks_exact_mut(4) {
            px.copy_from_slice(&theme.bg);
        }

        // Collect the visible runs ONCE. Every pass below (selection, chips,
        // glyphs, strike/underline) iterates this small set instead of
        // re-scanning all of the document's runs per decoration — the key to
        // keeping a keystroke's cost tied to the viewport, not the doc length.
        let vis: Vec<_> = buffer
            .layout_runs()
            .filter(|r| {
                let y = r.line_top + theme.margin_y - sy;
                y + r.line_height > 0.0 && y < height as f32
            })
            .collect();

        // Restrict decorations to the visible source span so the decoration loops
        // are bounded by what's on screen, not by the whole document's markup.
        let vis_src = visible_source_range(&vis, text, deltas);
        let dec_overlaps = |s: usize, e: usize| s < vis_src.1 && e > vis_src.0;

        if let Some((s, e)) = selection {
            if s != e {
                let cs = flat_to_render_cursor(text, deltas, s);
                let ce = flat_to_render_cursor(text, deltas, e);
                for run in &vis {
                    if let Some((x, w)) = run.highlight(cs, ce) {
                        fill_rect(
                            &mut rgba,
                            width,
                            height,
                            x + theme.margin_x,
                            run.line_top + theme.margin_y - sy,
                            w.max(2.0),
                            run.line_height,
                            theme.selection,
                        );
                    }
                }
            }
        }

        // token chip backgrounds (behind the glyphs)
        for &(s, e, deco) in decorations {
            if let Decoration::Chip(color) = deco {
                if s >= e || !dec_overlaps(s, e) {
                    continue;
                }
                let cs = flat_to_render_cursor(text, deltas, s);
                let ce = flat_to_render_cursor(text, deltas, e);
                for run in &vis {
                    if let Some((x, w)) = run.highlight(cs, ce) {
                        fill_rect(
                            &mut rgba,
                            width,
                            height,
                            x + theme.margin_x - 1.0,
                            run.line_top + theme.margin_y - sy,
                            w.max(2.0) + 2.0,
                            run.line_height,
                            color,
                        );
                    }
                }
            }
        }

        // Rasterize glyphs from the visible runs only (mirrors `Buffer::draw`'s
        // placement) so swash doesn't rasterize the whole document each keystroke.
        let fg = Color::rgba(theme.fg[0], theme.fg[1], theme.fg[2], theme.fg[3]);
        let (mx, my) = (theme.margin_x as i32, theme.margin_y as i32);
        let sy_i = sy as i32;
        for run in &vis {
            let line_y = run.line_y as i32;
            for glyph in run.glyphs.iter() {
                let pg = glyph.physical((0.0, 0.0), 1.0);
                let glyph_color = glyph.color_opt.unwrap_or(fg);
                self.swash.with_pixels(
                    &mut self.font_system,
                    pg.cache_key,
                    glyph_color,
                    |gx, gy, color| {
                        let px = pg.x + gx + mx;
                        let py = line_y + pg.y + gy + my - sy_i;
                        if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                            return;
                        }
                        let idx = ((py as u32 * width + px as u32) * 4) as usize;
                        blend(&mut rgba[idx..idx + 4], color);
                    },
                );
            }
        }

        // strikethrough / underline lines (cosmic-text draws neither)
        for &(s, e, deco) in decorations {
            if s >= e || !dec_overlaps(s, e) {
                continue;
            }
            let cs = flat_to_render_cursor(text, deltas, s);
            let ce = flat_to_render_cursor(text, deltas, e);
            for run in &vis {
                if let Some((x, w)) = run.highlight(cs, ce) {
                    if w <= 0.0 {
                        continue;
                    }
                    let (frac, color) = match deco {
                        Decoration::Strike => (0.55, theme.fg),
                        Decoration::Underline => (0.86, theme.link),
                        Decoration::Squiggle(c) => (0.92, c),
                        Decoration::Chip(_) => continue,
                    };
                    let thick = (run.line_height * 0.06).max(1.5);
                    fill_rect(
                        &mut rgba,
                        width,
                        height,
                        x + theme.margin_x,
                        run.line_top + theme.margin_y - sy + run.line_height * frac,
                        w,
                        thick,
                        color,
                    );
                }
            }
        }

        let caret = Caret {
            x: doc_caret.x,
            y: doc_caret.y - sy,
            h: doc_caret.h,
        };
        // Return the persistent buffer for reuse on the next keystroke.
        self.cache_buf = Some(buffer);
        ViewOut {
            frame: Frame {
                width,
                height,
                rgba,
            },
            caret,
            scroll_y: sy,
            doc_height,
        }
    }

    fn caret_geom(
        &self,
        buffer: &Buffer,
        text: &str,
        deltas: &[i32],
        cursor: usize,
        theme: &Theme,
    ) -> Caret {
        let c = flat_to_render_cursor(text, deltas, cursor);
        let mut last_bottom = 0.0;
        for run in buffer.layout_runs() {
            if run.line_i == c.line {
                // `highlight` returns None at line start / on empty lines — fall
                // back to x=0 rather than dropping the caret into the corner.
                let x = run.highlight(c, c).map(|(x, _)| x).unwrap_or(0.0);
                return Caret {
                    x,
                    y: run.line_top,
                    h: run.line_height,
                };
            }
            last_bottom = run.line_top + run.line_height;
        }
        // Line beyond the last laid-out run (e.g. a trailing empty line).
        Caret {
            x: 0.0,
            y: last_bottom,
            h: theme.line_height,
        }
    }

    /// Document-space caret rects for many char offsets, in one buffer build
    /// (for multiple cursors). Margins included, like the primary caret.
    pub fn caret_rects(
        &mut self,
        spans: &[Span],
        text: &str,
        deltas: &[i32],
        width: u32,
        theme: &Theme,
        offsets: &[usize],
    ) -> Vec<Caret> {
        let buffer = self.build_buffer(spans, width as f32, theme);
        offsets
            .iter()
            .map(|&o| {
                let mut c = self.caret_geom(&buffer, text, deltas, o, theme);
                c.x += theme.margin_x;
                c.y += theme.margin_y;
                c
            })
            .collect()
    }

    /// Document-space rects covering the char range `[start, end)` (one per visual
    /// line it spans). Used to position overlays (e.g. rendered math fragments).
    pub fn range_rects(
        &mut self,
        spans: &[Span],
        text: &str,
        deltas: &[i32],
        width: u32,
        theme: &Theme,
        start: usize,
        end: usize,
    ) -> Vec<(f32, f32, f32, f32)> {
        let buffer = self.build_buffer(spans, width as f32, theme);
        let cs = flat_to_render_cursor(text, deltas, start);
        let ce = flat_to_render_cursor(text, deltas, end);
        let mut out = Vec::new();
        for run in buffer.layout_runs() {
            if let Some((x, w)) = run.highlight(cs, ce) {
                if w > 0.0 {
                    out.push((
                        x + theme.margin_x,
                        run.line_top + theme.margin_y,
                        w,
                        run.line_height,
                    ));
                }
            }
        }
        out
    }

    pub fn hit(
        &mut self,
        spans: &[Span],
        text: &str,
        deltas: &[i32],
        width: u32,
        theme: &Theme,
        x: f32,
        y: f32,
    ) -> usize {
        let buffer = self.build_buffer(spans, width as f32, theme);
        let bx = (x - theme.margin_x).max(0.0);
        let by = (y - theme.margin_y).max(0.0);
        match buffer.hit(bx, by) {
            Some(cursor) => render_cursor_to_flat(text, deltas, cursor),
            None => text.chars().count(),
        }
    }

    pub fn vertical(
        &mut self,
        spans: &[Span],
        text: &str,
        deltas: &[i32],
        width: u32,
        theme: &Theme,
        cursor: usize,
        down: bool,
    ) -> usize {
        let buffer = self.build_buffer(spans, width as f32, theme);
        let caret = self.caret_geom(&buffer, text, deltas, cursor, theme);
        let target_y = if down {
            caret.y + caret.h * 1.5
        } else {
            caret.y - caret.h * 0.5
        };
        match buffer.hit(caret.x, target_y.max(0.0)) {
            Some(c) => render_cursor_to_flat(text, deltas, c),
            None => cursor,
        }
    }
}

// ---- flat-index <-> rendered-cursor mapping -------------------------------

/// Editable char index → (logical line, byte offset within the line's *editable*
/// text).
fn linecol(text: &str, char_idx: usize) -> (usize, usize) {
    let mut line = 0usize;
    let mut chars = 0usize;
    let mut byte_in_line = 0usize;
    for ch in text.chars() {
        if chars == char_idx {
            return (line, byte_in_line);
        }
        if ch == '\n' {
            line += 1;
            byte_in_line = 0;
        } else {
            byte_in_line += ch.len_utf8();
        }
        chars += 1;
    }
    (line, byte_in_line)
}

/// Inverse of [`linecol`]: (line, editable byte offset) → editable char index.
fn flat_of(text: &str, target_line: usize, target_byte: usize) -> usize {
    let mut line = 0usize;
    let mut chars = 0usize;
    let mut byte_in_line = 0usize;
    for ch in text.chars() {
        if line == target_line && byte_in_line >= target_byte {
            return chars;
        }
        if ch == '\n' {
            if line == target_line {
                return chars; // target byte past end of line
            }
            line += 1;
            byte_in_line = 0;
        } else {
            byte_in_line += ch.len_utf8();
        }
        chars += 1;
    }
    chars
}

/// Source (line, byte) → display Cursor: `display_byte = src_byte + delta`.
fn flat_to_render_cursor(text: &str, deltas: &[i32], char_idx: usize) -> Cursor {
    let (line, byte) = linecol(text, char_idx);
    let d = deltas.get(line).copied().unwrap_or(0);
    let display_byte = (byte as i32 + d).max(0) as usize;
    Cursor::new(line, display_byte)
}

/// Display Cursor → source char index: `src_byte = display_byte − delta`.
fn render_cursor_to_flat(text: &str, deltas: &[i32], cursor: Cursor) -> usize {
    let d = deltas.get(cursor.line).copied().unwrap_or(0);
    let src_byte = (cursor.index as i32 - d).max(0) as usize;
    flat_of(text, cursor.line, src_byte)
}

// ---- raster helpers --------------------------------------------------------

/// The base attributes for unstyled text: the host body font family if set.
fn base_attrs(theme: &Theme) -> Attrs<'_> {
    let mut a = Attrs::new();
    if let Some(f) = &theme.font_family {
        a = a.family(Family::Name(f));
    }
    a
}

fn attrs_for<'a>(marks: MarkSet, color: Option<[u8; 4]>, theme: &'a Theme) -> Attrs<'a> {
    let mut a = base_attrs(theme);
    if marks.contains(MarkSet::BOLD) {
        a = a.weight(Weight::BOLD);
    }
    if marks.contains(MarkSet::ITALIC) {
        a = a.style(Style::Italic);
    }
    if marks.contains(MarkSet::CODE) {
        a = a.family(
            theme
                .code_font_family
                .as_deref()
                .map(Family::Name)
                .unwrap_or(Family::Monospace),
        );
        a = a.color(rgba(theme.code));
    }
    if marks.contains(MarkSet::LINK) {
        a = a.color(rgba(theme.link));
    }
    // An explicit view color wins over mark-derived colors.
    if let Some(c) = color {
        a = a.color(rgba(c));
    }
    a
}

fn rgba(c: [u8; 4]) -> Color {
    Color::rgba(c[0], c[1], c[2], c[3])
}

// ---- incremental buffer support -------------------------------------------

/// One display line, split out of the flat span list: its style runs (borrowed,
/// no allocation) plus a content signature used to detect changes cheaply.
struct LineParts<'a> {
    runs: Vec<(&'a str, MarkSet, Option<[u8; 4]>, f32)>,
    sig: u64,
}

/// Split the flat spans into per-display-line run lists, splitting on `\n` (the
/// same hard break cosmic-text uses for typical LF markdown). Unchanged lines
/// cost only a hash — no string allocation.
fn split_lines(spans: &[Span]) -> Vec<LineParts<'_>> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut out: Vec<LineParts> = Vec::new();
    let mut runs: Vec<(&str, MarkSet, Option<[u8; 4]>, f32)> = Vec::new();
    let mut hasher = DefaultHasher::new();

    for s in spans {
        let mut rest = s.text.as_str();
        loop {
            match rest.find('\n') {
                Some(nl) => {
                    let (head, tail) = (&rest[..nl], &rest[nl + 1..]);
                    if !head.is_empty() {
                        head.hash(&mut hasher);
                        s.marks.bits().hash(&mut hasher);
                        s.color.hash(&mut hasher);
                        s.size.to_bits().hash(&mut hasher);
                        runs.push((head, s.marks, s.color, s.size));
                    }
                    out.push(LineParts { runs: std::mem::take(&mut runs), sig: hasher.finish() });
                    hasher = DefaultHasher::new();
                    rest = tail;
                }
                None => {
                    if !rest.is_empty() {
                        rest.hash(&mut hasher);
                        s.marks.bits().hash(&mut hasher);
                        s.color.hash(&mut hasher);
                        s.size.to_bits().hash(&mut hasher);
                        runs.push((rest, s.marks, s.color, s.size));
                    }
                    break;
                }
            }
        }
    }
    // Flush the trailing segment, matching `str::lines()` / cosmic-text's
    // BidiParagraphs: a trailing '\n' does NOT create a final empty line, but an
    // entirely empty document is still one (empty) line.
    if !runs.is_empty() || out.is_empty() {
        out.push(LineParts { runs, sig: hasher.finish() });
    }
    out
}

/// Source character range `[start, end)` spanned by the visible runs, used to
/// skip decorations that aren't on screen. Display line index == source line
/// index (1:1; long lines wrap into multiple runs sharing a `line_i`).
fn visible_source_range(vis: &[cosmic_text::LayoutRun], text: &str, deltas: &[i32]) -> (usize, usize) {
    let _ = deltas;
    if vis.is_empty() {
        return (0, usize::MAX);
    }
    let min_line = vis.iter().map(|r| r.line_i).min().unwrap();
    let max_line = vis.iter().map(|r| r.line_i).max().unwrap();
    (flat_of(text, min_line, 0), flat_of(text, max_line + 1, 0))
}

/// Build a cosmic-text `BufferLine` (unshaped) for one display line, matching
/// exactly what `set_rich_text` would produce for it (same text + AttrsList +
/// metrics + line ending) so layout/caret/hit geometry are unaffected.
fn make_buffer_line(lp: &LineParts, theme: &Theme) -> BufferLine {
    let default_attrs = base_attrs(theme);
    let mut text = String::new();
    let mut attrs_list = AttrsList::new(&default_attrs);
    for &(t, marks, color, size) in &lp.runs {
        let start = text.len();
        text.push_str(t);
        let end = text.len();
        if start < end {
            let attrs = attrs_for(marks, color, theme).metrics(Metrics::new(size, size * 1.4));
            attrs_list.add_span(start..end, &attrs);
        }
    }
    BufferLine::new(text, LineEnding::default(), attrs_list, Shaping::Advanced)
}

/// Signature of the render parameters that, when changed, require a full buffer
/// rebuild rather than a per-line update.
fn theme_key(theme: &Theme, shaping_w: f32) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    theme.font_size.to_bits().hash(&mut h);
    theme.line_height.to_bits().hash(&mut h);
    theme.font_family.hash(&mut h);
    theme.code_font_family.hash(&mut h);
    shaping_w.to_bits().hash(&mut h);
    h.finish()
}

#[allow(clippy::too_many_arguments)]
fn fill_rect(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [u8; 4],
) {
    let x0 = x.floor().max(0.0) as u32;
    let y0 = y.floor().max(0.0) as u32;
    let x1 = ((x + w).ceil() as u32).min(width);
    let y1 = ((y + h).ceil() as u32).min(height);
    let src = Color::rgba(color[0], color[1], color[2], color[3]);
    for py in y0..y1 {
        for px in x0..x1 {
            let idx = ((py * width + px) * 4) as usize;
            blend(&mut rgba[idx..idx + 4], src);
        }
    }
}

fn blend(dst: &mut [u8], src: Color) {
    let sa = src.a() as u32;
    if sa == 0 {
        return;
    }
    let inv = 255 - sa;
    dst[0] = ((src.r() as u32 * sa + dst[0] as u32 * inv) / 255) as u8;
    dst[1] = ((src.g() as u32 * sa + dst[1] as u32 * inv) / 255) as u8;
    dst[2] = ((src.b() as u32 * sa + dst[2] as u32 * inv) / 255) as u8;
    dst[3] = 255;
}

#[cfg(test)]
mod tests {
    use super::{flat_of, flat_to_render_cursor, linecol, render_cursor_to_flat};
    use super::{Span, TextRenderer, Theme};
    use crate::model::MarkSet;

    fn spans_for(text: &str) -> Vec<Span> {
        vec![Span {
            text: text.to_string(),
            marks: MarkSet::empty(),
            color: None,
            size: 18.0,
        }]
    }

    // Count pixels that differ from the background — a proxy for "is anything
    // actually drawn?". Catches the blank-render regression directly.
    fn ink(rgba: &[u8], bg: [u8; 4]) -> usize {
        rgba.chunks_exact(4).filter(|p| p[..] != bg[..]).count()
    }

    #[test]
    fn viewport_render_shows_content_at_top() {
        let mut r = TextRenderer::new();
        let theme = Theme::default();
        let text = "Hello world\nsecond line\nthird line";
        let spans = spans_for(text);
        let deltas = vec![0; 3];
        let out =
            r.render_viewport(&spans, text, &deltas, &[], 400, 200, 0.0, false, &theme, 0, None);
        assert_eq!(out.frame.height, 200, "frame must be viewport-sized");
        assert!(
            ink(&out.frame.rgba, theme.bg) > 50,
            "top-of-document viewport must actually paint glyphs (blank-render guard)"
        );
    }

    #[test]
    fn viewport_render_reflects_edits_in_view() {
        let mut r = TextRenderer::new();
        let theme = Theme::default();
        let before = "abc";
        let after = "abcXYZ";
        let r1 = r.render_viewport(&spans_for(before), before, &[0], &[], 400, 120, 0.0, false, &theme, 0, None);
        let r2 = r.render_viewport(&spans_for(after), after, &[0], &[], 400, 120, 0.0, false, &theme, 0, None);
        assert!(
            r1.frame.rgba != r2.frame.rgba,
            "typing a character must change the visible frame"
        );
        assert!(ink(&r2.frame.rgba, theme.bg) > ink(&r1.frame.rgba, theme.bg));
    }

    #[test]
    fn viewport_render_cost_is_flat_in_doc_length() {
        // The whole point of Tier 2: the rendered frame (and thus alloc/upload)
        // does not grow with document length.
        let mut r = TextRenderer::new();
        let theme = Theme::default();
        let short: String = "line\n".repeat(10);
        let long: String = "line\n".repeat(4000);
        let s = r.render_viewport(&spans_for(&short), &short, &[0; 11], &[], 400, 200, 0.0, false, &theme, 0, None);
        let l = r.render_viewport(&spans_for(&long), &long, &[0; 4001], &[], 400, 200, 0.0, false, &theme, 0, None);
        assert_eq!(s.frame.rgba.len(), l.frame.rgba.len(), "frame size must be flat");
        assert!(l.doc_height > s.doc_height, "doc_height still reflects full length");
        assert!(l.doc_height > 4000 * 10, "long doc reports a tall scroll range");
    }

    #[test]
    fn viewport_render_scrolls_to_show_lower_content() {
        // Distinct content top vs bottom; scrolling must change what's painted.
        let mut r = TextRenderer::new();
        let theme = Theme::default();
        let mut text = String::new();
        for i in 0..200 {
            text.push_str(&format!("row{i}\n"));
        }
        let deltas = vec![0; 201];
        let top = r.render_viewport(&spans_for(&text), &text, &deltas, &[], 400, 200, 0.0, false, &theme, 0, None);
        let down = r.render_viewport(&spans_for(&text), &text, &deltas, &[], 400, 200, 1500.0, false, &theme, 0, None);
        assert!(top.frame.rgba != down.frame.rgba, "scrolling must change the frame");
        assert!(ink(&down.frame.rgba, theme.bg) > 50, "scrolled viewport still paints");
    }

    // The incremental persistent-buffer path (reused renderer) must produce a
    // byte-identical frame + caret to a fresh full rebuild. This is the safety
    // net for the perf optimization: any layout divergence shows up as a pixel
    // diff here.
    fn assert_inc_eq_full(seq: &[&str], final_cursor: usize) {
        let theme = Theme::default();
        let (w, h) = (400u32, 300u32);
        let last = *seq.last().unwrap();
        let dl = vec![0i32; last.matches('\n').count() + 1];

        // Renderer A: replays the whole edit sequence (incremental after step 1).
        let mut a = TextRenderer::new();
        for (i, t) in seq.iter().enumerate() {
            let d = vec![0i32; t.matches('\n').count() + 1];
            let cur = if i + 1 == seq.len() { final_cursor } else { 0 };
            a.render_viewport(&spans_for(t), t, &d, &[], w, h, 0.0, false, &theme, cur, None);
        }
        let inc = a.render_viewport(&spans_for(last), last, &dl, &[], w, h, 0.0, false, &theme, final_cursor, None);

        // Renderer B: fresh, renders only the final state (full rebuild).
        let mut b = TextRenderer::new();
        let full = b.render_viewport(&spans_for(last), last, &dl, &[], w, h, 0.0, false, &theme, final_cursor, None);

        assert_eq!(inc.doc_height, full.doc_height, "doc_height diverged for {seq:?}");
        assert!((inc.caret.y - full.caret.y).abs() < 0.01 && (inc.caret.x - full.caret.x).abs() < 0.01,
            "caret diverged for {seq:?}: inc=({},{}) full=({},{})", inc.caret.x, inc.caret.y, full.caret.x, full.caret.y);
        assert!(inc.frame.rgba == full.frame.rgba, "incremental frame != full-rebuild frame for {seq:?}");
    }

    #[test]
    fn incremental_edit_one_line_matches_full() {
        assert_inc_eq_full(&["alpha\nbravo\ncharlie\ndelta", "alpha\nbravoX\ncharlie\ndelta"], 6);
    }

    #[test]
    fn incremental_trailing_newline_matches_full() {
        // Real markdown ends with '\n'; the trailing newline must NOT create a
        // phantom line (this case previously panicked on a line-count mismatch).
        assert_inc_eq_full(&["alpha\nbravo\ncharlie\n", "alpha\nbravoX\ncharlie\n"], 6);
        assert_inc_eq_full(&["x\n\ny\n", "x\n\nyy\n"], 0);
    }

    #[test]
    fn incremental_edit_first_and_last_line_matches_full() {
        assert_inc_eq_full(
            &["one\ntwo\nthree", "One\ntwo\nthree", "One\ntwo\nthreeee"],
            0,
        );
    }

    #[test]
    fn incremental_line_count_change_then_edit_matches_full() {
        // step 2 changes the line count (structural full rebuild), step 3 edits
        // a line (incremental on the rebuilt buffer).
        assert_inc_eq_full(
            &["a\nb\nc", "a\nb\nc\nd", "a\nb\nc\nD"],
            0,
        );
    }

    #[test]
    fn incremental_line_insert_delete_matches_full() {
        // The prefix/suffix splice must match a full rebuild for every shape of
        // line-count change: middle insert, middle delete, Enter-split,
        // Backspace-join, and append past a trailing newline.
        assert_inc_eq_full(&["a\nb\nc\nd", "a\nb\nX\nc\nd"], 0); // insert in middle
        assert_inc_eq_full(&["a\nb\nc\nd", "a\nd"], 0); // delete middle lines
        assert_inc_eq_full(&["hello world", "hello\nworld"], 0); // Enter split
        assert_inc_eq_full(&["hello\nworld", "helloworld"], 0); // backspace join
        assert_inc_eq_full(&["a\nb\nc\n", "a\nb\nc\nd"], 8); // append past trailing newline
        assert_inc_eq_full(&["# H\nbody\n- x\n- y", "# H\nbody\n- x\n- mid\n- y"], 0); // insert bullet
    }

    #[test]
    fn incremental_heading_size_change_matches_full() {
        // A line whose font size changes (heading toggling) must re-layout right.
        let theme = Theme::default();
        let (w, h) = (400u32, 300u32);
        let plain = vec![Span { text: "title\nbody".into(), marks: MarkSet::empty(), color: None, size: 18.0 }];
        let heading = vec![
            Span { text: "title".into(), marks: MarkSet::empty(), color: None, size: 30.0 },
            Span { text: "\nbody".into(), marks: MarkSet::empty(), color: None, size: 18.0 },
        ];
        let d = vec![0i32; 2];
        let mut a = TextRenderer::new();
        a.render_viewport(&plain, "title\nbody", &d, &[], w, h, 0.0, false, &theme, 0, None);
        let inc = a.render_viewport(&heading, "title\nbody", &d, &[], w, h, 0.0, false, &theme, 0, None);
        let mut b = TextRenderer::new();
        let full = b.render_viewport(&heading, "title\nbody", &d, &[], w, h, 0.0, false, &theme, 0, None);
        assert_eq!(inc.doc_height, full.doc_height, "heading height diverged");
        assert!(inc.frame.rgba == full.frame.rgba, "heading incremental frame != full");
    }

    #[test]
    fn viewport_caret_is_viewport_relative() {
        let mut r = TextRenderer::new();
        let theme = Theme::default();
        let mut text = String::new();
        for i in 0..100 {
            text.push_str(&format!("row{i}\n"));
        }
        let deltas = vec![0; 101];
        // Caret at the document end; scroll past the bottom (clamps to max) so
        // the final line — and the caret — sit inside the viewport.
        let n = text.chars().count();
        let out = r.render_viewport(&spans_for(&text), &text, &deltas, &[], 400, 200, 50_000.0, false, &theme, n, None);
        assert!(
            out.caret.y >= -theme.line_height && out.caret.y <= 200.0 + theme.line_height,
            "caret y should be within (near) the viewport, got {}",
            out.caret.y
        );
    }

    #[test]
    fn linecol_roundtrip_multiline() {
        let text = "ab\ncдe\n\nx";
        let n = text.chars().count();
        for i in 0..=n {
            let (l, b) = linecol(text, i);
            assert_eq!(flat_of(text, l, b), i, "roundtrip failed at {i}");
        }
    }

    #[test]
    fn signed_delta_maps_both_ways() {
        // line 0: a hidden heading "## " → display shorter by 3 (delta −3)
        // line 1: a hidden bullet "- " → display "• " longer by 2 (delta +2)
        let text = "## Title\n- item";
        let deltas = vec![-3i32, 2];
        // source cursor at the 'T' of Title (char index 3) → display byte 0
        let c0 = flat_to_render_cursor(text, &deltas, 3);
        assert_eq!((c0.line, c0.index), (0, 0));
        assert_eq!(render_cursor_to_flat(text, &deltas, c0), 3);
        // source cursor at the 'i' of item (char index 9+2=... ) → display byte +2
        let item_i = "## Title\n- ".chars().count(); // start of "item"
        let c1 = flat_to_render_cursor(text, &deltas, item_i);
        assert_eq!(c1.line, 1);
        assert_eq!(c1.index, 4); // "- " src byte 2 + delta 2 = display byte 4 ("• ")
        assert_eq!(render_cursor_to_flat(text, &deltas, c1), item_i);
    }
}
