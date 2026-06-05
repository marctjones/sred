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
