//! Native Typst renderer for sred's math/figure **fragment hook** (#22).
//!
//! `sred-core` deliberately bundles no compiler — it exposes
//! [`Editor::set_fragment_renderer`](sred_core::Editor::set_fragment_renderer),
//! and this *optional* crate fills that hook with the real Typst engine
//! (`typst` + `typst-render`, embedded fonts via `typst-kit`). Hosts that want
//! typeset math opt in; everyone else keeps `sred-core` light.
//!
//! ```ignore
//! let mut ed = sred_core::Editor::from_source(src, sred_core::Format::Typst);
//! ed.set_fragment_renderer(sred_typst::TypstRenderer::new().into_hook());
//! // now ed.render_fragment(&frag) compiles "$x^2$" → an RGBA image.
//! ```
//!
//! **Scope.** This renders *Typst* math (the fragment source is treated as Typst
//! markup). For Typst documents that's exact; Markdown `$…$` (LaTeX-flavored)
//! renders correctly only for expressions that are also valid Typst math.

use std::sync::Arc;

use sred_core::api::FragmentImage;
use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime};
use typst::layout::PagedDocument;
use typst::syntax::{FileId, Source, VirtualPath};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_kit::fonts::FontSlot;

/// The fragment-renderer hook type accepted by
/// [`Editor::set_fragment_renderer`](sred_core::Editor::set_fragment_renderer):
/// `(src, display, font_size) -> Option<image>`.
pub type FragmentHook = Box<dyn Fn(&str, bool, f32) -> Option<FragmentImage>>;

/// Shared, immutable Typst environment (standard library + embedded fonts). Built
/// once and cheaply shared across fragment compiles via `Arc`.
struct Env {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<FontSlot>,
}

impl Env {
    fn new() -> Self {
        let library = LazyHash::new(Library::builder().build());
        let mut searcher = typst_kit::fonts::FontSearcher::new();
        searcher.include_system_fonts(false);
        searcher.include_embedded_fonts(true);
        let fonts = searcher.search();
        Env {
            library,
            book: LazyHash::new(fonts.book),
            fonts: fonts.fonts,
        }
    }
}

/// A one-shot `World` over the shared [`Env`] plus a single in-memory source — one
/// per fragment compile. Self-contained (no imports / external files).
struct FragWorld<'a> {
    env: &'a Env,
    source: Source,
}

impl World for FragWorld<'_> {
    fn library(&self) -> &LazyHash<Library> {
        &self.env.library
    }
    fn book(&self) -> &LazyHash<FontBook> {
        &self.env.book
    }
    fn main(&self) -> FileId {
        self.source.id()
    }
    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.source.id() {
            Ok(self.source.clone())
        } else {
            Err(FileError::NotFound(id.vpath().as_rootless_path().into()))
        }
    }
    fn file(&self, id: FileId) -> FileResult<Bytes> {
        Err(FileError::NotFound(id.vpath().as_rootless_path().into()))
    }
    fn font(&self, index: usize) -> Option<Font> {
        self.fonts_get(index)
    }
    fn today(&self, _offset: Option<i64>) -> Option<Datetime> {
        None
    }
}

impl FragWorld<'_> {
    fn fonts_get(&self, index: usize) -> Option<Font> {
        self.env.fonts.get(index)?.get()
    }
}

/// Renders math/figure fragments with the Typst engine. Cheap to clone (shares
/// the [`Env`] via `Arc`); fonts + library are built once on `new`.
#[derive(Clone)]
pub struct TypstRenderer {
    env: Arc<Env>,
    /// Pixels per Typst point at which fragments are rasterized (sharpness).
    pixel_per_pt: f32,
}

impl Default for TypstRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl TypstRenderer {
    /// Build the renderer (loads the standard library + embedded fonts once).
    pub fn new() -> Self {
        TypstRenderer {
            env: Arc::new(Env::new()),
            pixel_per_pt: 2.0,
        }
    }

    /// Set the rasterization scale (pixels per Typst point; default `2.0`).
    pub fn with_pixel_per_pt(mut self, ppp: f32) -> Self {
        self.pixel_per_pt = ppp.max(0.1);
        self
    }

    /// Compile + rasterize one fragment to an RGBA image, or `None` if it doesn't
    /// compile. `display` selects block vs inline math; `font_size` is in points.
    pub fn render(&self, src: &str, display: bool, font_size: f32) -> Option<FragmentImage> {
        let inner = src.trim().trim_matches('$').trim();
        if inner.is_empty() {
            return None;
        }
        let body = if display {
            format!("$ {inner} $")
        } else {
            format!("${inner}$")
        };
        // Auto-sized, margin-free, transparent page so the raster is a tight box
        // the host can overlay.
        let doc_src = format!(
            "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n\
             #set text(size: {font_size}pt)\n{body}\n"
        );
        let id = FileId::new_fake(VirtualPath::new("fragment.typ"));
        let world = FragWorld {
            env: &self.env,
            source: Source::new(id, doc_src),
        };

        let doc: PagedDocument = typst::compile(&world).output.ok()?;
        let page = doc.pages.first()?;
        let pixmap = typst_render::render(page, self.pixel_per_pt);
        Some(pixmap_to_image(&pixmap))
    }

    /// Boxed closure for [`Editor::set_fragment_renderer`](sred_core::Editor::set_fragment_renderer).
    pub fn into_hook(self) -> FragmentHook {
        Box::new(move |src, display, font_size| self.render(src, display, font_size))
    }
}

/// Convert a tiny-skia (premultiplied) pixmap into straight-alpha RGBA bytes,
/// matching what hosts expect (`egui::ColorImage::from_rgba_unmultiplied`, etc.).
fn pixmap_to_image(p: &tiny_skia::Pixmap) -> FragmentImage {
    let (w, h) = (p.width(), p.height());
    let mut rgba = Vec::with_capacity((w as usize) * (h as usize) * 4);
    for px in p.pixels() {
        let c = px.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    FragmentImage {
        width: w,
        height: h,
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_inline_math_to_a_nonempty_image() {
        let r = TypstRenderer::new();
        let img = r.render("$x^2$", false, 16.0).expect("compiles");
        assert!(img.width > 0 && img.height > 0, "produced a sized image");
        assert_eq!(
            img.rgba.len(),
            (img.width as usize) * (img.height as usize) * 4
        );
        // Some pixel is non-transparent (the glyphs were drawn).
        assert!(img.rgba.chunks(4).any(|p| p[3] > 0), "has visible glyphs");
    }

    #[test]
    fn display_math_is_larger_than_inline() {
        let r = TypstRenderer::new();
        let inline = r.render("E = m c^2", false, 16.0).unwrap();
        let display = r.render("E = m c^2", true, 16.0).unwrap();
        // Block/display math is laid out larger than the same inline expression.
        assert!(display.height >= inline.height);
    }

    #[test]
    fn larger_font_size_yields_a_larger_image() {
        let r = TypstRenderer::new();
        let small = r.render("$x$", false, 12.0).unwrap();
        let big = r.render("$x$", false, 48.0).unwrap();
        assert!(big.height > small.height, "font size scales the raster");
    }

    #[test]
    fn empty_and_garbage_fragments_are_handled() {
        let r = TypstRenderer::new();
        assert!(r.render("$$", false, 16.0).is_none(), "empty math → None");
        // A nonsense expression that fails to compile returns None, not a panic.
        let _ = r.render("$ #@! broken (((", true, 16.0);
    }

    #[test]
    fn into_hook_plugs_into_the_editor() {
        use sred_core::{Editor, Format};
        let mut ed = Editor::from_source("inline $x^2$ here", Format::Typst);
        ed.set_fragment_renderer(TypstRenderer::new().into_hook());
        let frags = ed.math_fragments();
        assert_eq!(frags.len(), 1);
        let img = ed
            .render_fragment(&frags[0])
            .expect("rendered via the hook");
        assert!(img.width > 0 && img.height > 0);
    }
}
