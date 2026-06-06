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
    Attrs, Buffer, Color, Cursor, Family, FontSystem, Metrics, Shaping, Style, SwashCache, Weight,
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
        }
    }

    fn build_buffer(&mut self, spans: &[Span], full_width: f32, theme: &Theme) -> Buffer {
        let metrics = Metrics::new(theme.font_size, theme.line_height);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        let shaping_w = (full_width - 2.0 * theme.margin_x).max(16.0);
        buffer.set_size(&mut self.font_system, Some(shaping_w), None);

        let default_attrs = Attrs::new();
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
        let buffer = self.build_buffer(spans, width as f32, theme);
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

        // A run is visible if any part of it falls inside the viewport band.
        let visible = |top: f32, lh: f32| -> bool {
            let y = top + theme.margin_y - sy;
            y + lh > 0.0 && y < height as f32
        };

        if let Some((s, e)) = selection {
            if s != e {
                let cs = flat_to_render_cursor(text, deltas, s);
                let ce = flat_to_render_cursor(text, deltas, e);
                for run in buffer.layout_runs() {
                    if !visible(run.line_top, run.line_height) {
                        continue;
                    }
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
                if s >= e {
                    continue;
                }
                let cs = flat_to_render_cursor(text, deltas, s);
                let ce = flat_to_render_cursor(text, deltas, e);
                for run in buffer.layout_runs() {
                    if !visible(run.line_top, run.line_height) {
                        continue;
                    }
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

        let fg = Color::rgba(theme.fg[0], theme.fg[1], theme.fg[2], theme.fg[3]);
        let (mx, my) = (theme.margin_x as i32, theme.margin_y as i32);
        let sy_i = sy as i32;
        buffer.draw(&mut self.font_system, &mut self.swash, fg, |x, y, w, h, color| {
            for dy in 0..h as i32 {
                for dx in 0..w as i32 {
                    let px = x + dx + mx;
                    let py = y + dy + my - sy_i;
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
                if !visible(run.line_top, run.line_height) {
                    continue;
                }
                if let Some((x, w)) = run.highlight(cs, ce) {
                    if w <= 0.0 {
                        continue;
                    }
                    let (frac, color) = match deco {
                        Decoration::Strike => (0.55, theme.fg),
                        Decoration::Underline => (0.86, theme.link),
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

fn attrs_for(marks: MarkSet, color: Option<[u8; 4]>, theme: &Theme) -> Attrs<'static> {
    let mut a = Attrs::new();
    if marks.contains(MarkSet::BOLD) {
        a = a.weight(Weight::BOLD);
    }
    if marks.contains(MarkSet::ITALIC) {
        a = a.style(Style::Italic);
    }
    if marks.contains(MarkSet::CODE) {
        a = a.family(Family::Monospace);
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
