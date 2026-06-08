//! #24 — built-in fragment compositing must be pixel-identical to a host running
//! the overlay loop itself (`math_fragments` → `render_fragment` → `rect_for_range`
//! → blit). That parity is the whole point: a host (Noet, sred-egui) can flip
//! `set_fragment_overlay(true)` and delete its blit code with zero visual change.
//!
//! The `blit_fragment` / `sample_bilinear` copies below are deliberately the same
//! algorithm sred uses internally — they stand in for the host's own overlay code.

use sred_core::api::FragmentImage;
use sred_core::view::MathFragment;
use sred_core::{Editor, Format};

/// Deterministic fragment image: a gradient with a left transparent margin (so the
/// alpha-skip and blend paths are both exercised). Same input → same pixels.
fn gradient(w: u32, h: u32) -> FragmentImage {
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            rgba[i] = (x * 255 / w.max(1)) as u8;
            rgba[i + 1] = (y * 255 / h.max(1)) as u8;
            rgba[i + 2] = 128;
            // Left ~20% fully transparent, rest opaque-ish ramp.
            rgba[i + 3] = if x < w / 5 {
                0
            } else {
                ((x * 255 / w.max(1)) as u8).max(40)
            };
        }
    }
    FragmentImage {
        width: w,
        height: h,
        rgba,
    }
}

#[allow(clippy::type_complexity)]
fn renderer() -> Box<dyn Fn(&str, bool, f32) -> Option<FragmentImage>> {
    // Display math gets a taller image than inline, like a real Typst raster.
    Box::new(|_src: &str, display: bool, _size: f32| {
        Some(if display {
            gradient(120, 60)
        } else {
            gradient(48, 24)
        })
    })
}

fn editor(body: &str) -> Editor {
    let mut e = Editor::from_source(body, Format::Markdown);
    e.set_viewport(800, 600.0);
    e.set_fragment_renderer(renderer());
    e
}

// ---- host-side overlay (the code a host would otherwise maintain) -------------

fn host_composite(e: &mut Editor, frame: &mut [u8], fw: u32, fh: u32) {
    let frags: Vec<MathFragment> = e.math_fragments();
    for frag in &frags {
        let Some(img) = e.render_fragment(frag) else {
            continue;
        };
        if img.width == 0 || img.height == 0 || img.rgba.is_empty() {
            continue;
        }
        let aspect = img.width as f32 / img.height.max(1) as f32;
        for r in e.rect_for_range(frag.start, frag.end) {
            blit(frame, fw, fh, &img, r.x, r.y, r.h * aspect, r.h);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn blit(
    frame: &mut [u8],
    fw: u32,
    fh: u32,
    img: &FragmentImage,
    dx: f32,
    dy: f32,
    tw: f32,
    th: f32,
) {
    if tw < 1.0 || th < 1.0 {
        return;
    }
    let (dx0, dy0) = (dx.round() as i32, dy.round() as i32);
    let (tw_i, th_i) = (tw.round() as i32, th.round() as i32);
    for ty in 0..th_i {
        let oy = dy0 + ty;
        if oy < 0 || oy >= fh as i32 {
            continue;
        }
        let sy = ((ty as f32 + 0.5) / th) * img.height as f32 - 0.5;
        for tx in 0..tw_i {
            let ox = dx0 + tx;
            if ox < 0 || ox >= fw as i32 {
                continue;
            }
            let sx = ((tx as f32 + 0.5) / tw) * img.width as f32 - 0.5;
            let [sr, sg, sb, sa] = sample(img, sx, sy);
            if sa == 0 {
                continue;
            }
            let di = ((oy as usize) * (fw as usize) + ox as usize) * 4;
            let a = sa as f32 / 255.0;
            frame[di] = (sr as f32 * a + frame[di] as f32 * (1.0 - a)).round() as u8;
            frame[di + 1] = (sg as f32 * a + frame[di + 1] as f32 * (1.0 - a)).round() as u8;
            frame[di + 2] = (sb as f32 * a + frame[di + 2] as f32 * (1.0 - a)).round() as u8;
            frame[di + 3] = 255;
        }
    }
}

fn sample(img: &FragmentImage, x: f32, y: f32) -> [u8; 4] {
    let x = x.clamp(0.0, (img.width - 1) as f32);
    let y = y.clamp(0.0, (img.height - 1) as f32);
    let (x0, y0) = (x.floor() as u32, y.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(img.width - 1), (y0 + 1).min(img.height - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let px = |xx: u32, yy: u32| -> [u8; 4] {
        let i = ((yy * img.width + xx) * 4) as usize;
        [
            img.rgba[i],
            img.rgba[i + 1],
            img.rgba[i + 2],
            img.rgba[i + 3],
        ]
    };
    let (p00, p10, p01, p11) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
    let mut out = [0u8; 4];
    for c in 0..4 {
        let top = p00[c] as f32 + (p10[c] as f32 - p00[c] as f32) * fx;
        let bot = p01[c] as f32 + (p11[c] as f32 - p01[c] as f32) * fx;
        out[c] = (top + (bot - top) * fy).round().clamp(0.0, 255.0) as u8;
    }
    out
}

// ---- tests -------------------------------------------------------------------

#[test]
fn overlay_matches_manual_composite() {
    let body = "Energy is $E=mc^2$ in every note, and a display block:\n\n$$ \\sum_{i=0}^n i $$\n\nmore text after.\n";

    // Manual: render with overlay OFF, then composite via the public API.
    let mut m = editor(body);
    let off = m.render_view(true);
    let (fw, fh) = (off.frame.width, off.frame.height);
    let mut reference = off.frame.rgba.clone();
    host_composite(&mut m, &mut reference, fw, fh);

    // Built-in: overlay ON in a single render_view call.
    let mut b = editor(body);
    b.set_fragment_overlay(true);
    let on = b.render_view(true);

    assert_eq!((on.frame.width, on.frame.height), (fw, fh), "frame size");
    assert!(
        on.frame.rgba != off.frame.rgba,
        "overlay drew nothing — math not detected or not composited"
    );
    assert!(
        on.frame.rgba == reference,
        "built-in overlay differs from the manual host composite"
    );
}

#[test]
fn overlay_off_by_default_is_unchanged() {
    let body = "Energy is $E=mc^2$ today.\n";
    // A registered renderer alone must not change render_view; only the flag does.
    let mut with = editor(body);
    let a = with.render_view(true);

    let mut without = Editor::from_source(body, Format::Markdown);
    without.set_viewport(800, 600.0);
    let b = without.render_view(true);

    assert!(
        a.frame.rgba == b.frame.rgba,
        "renderer-without-overlay changed the frame"
    );
}

#[test]
fn overlay_noop_without_math() {
    let body = "Plain note with no math at all, just prose.\n";
    let mut off = editor(body);
    let a = off.render_view(true);

    let mut on = editor(body);
    on.set_fragment_overlay(true);
    let b = on.render_view(true);

    assert!(
        a.frame.rgba == b.frame.rgba,
        "overlay altered a math-free note"
    );
}

#[test]
fn overlay_noop_without_renderer() {
    let body = "Energy is $E=mc^2$ today.\n";
    let mut plain = Editor::from_source(body, Format::Markdown);
    plain.set_viewport(800, 600.0);
    let a = plain.render_view(true);

    let mut flagged = Editor::from_source(body, Format::Markdown);
    flagged.set_viewport(800, 600.0);
    flagged.set_fragment_overlay(true); // on, but no renderer registered
    let b = flagged.render_view(true);

    assert!(
        a.frame.rgba == b.frame.rgba,
        "overlay drew without a renderer"
    );
}
