//! Perceived-performance probes. Ignored by default (timing, not correctness).
//!
//! Run alone (ONE cargo at a time) for credible numbers:
//!   cargo test -p sred-core --release --test perf_probe -- --ignored --nocapture
//!
//! The question these answer: after Tier 2 (viewport rendering) made raster +
//! upload flat, does per-keystroke cost still grow with document length? If yes,
//! shaping + the markdown scan are the remaining O(doc) cost and the next target.

use sred_core::{Command, Editor, Format};
use std::time::Instant;

fn paragraph(n: usize) -> String {
    // Mixed markdown so styled_runs/decorations do realistic work.
    let mut s = String::new();
    for i in 0..n {
        match i % 5 {
            0 => s.push_str(&format!("# Heading {i}\n")),
            1 => s.push_str("- a bullet with **bold** and `code` and a [link](http://x)\n"),
            2 => s.push_str("> a quoted line with some _emphasis_ in it\n"),
            _ => s.push_str("The quick brown fox jumps over the lazy dog every day.\n"),
        }
    }
    s
}

#[test]
#[ignore]
fn keystroke_cost_vs_doc_length() {
    println!("\n--- per-keystroke cost (apply Insert + render_view follow) ---");
    for &n in &[10usize, 100, 500, 2000, 4000] {
        let body = paragraph(n);
        let mut e = Editor::from_source(&body, Format::Markdown);
        e.set_viewport(800, 600.0);
        e.render_view(true); // warm caches (syntect, swash)

        let iters = 30;
        let t = Instant::now();
        for _ in 0..iters {
            e.apply(Command::Insert("x".into()));
            let _ = e.render_view(true);
        }
        let per = t.elapsed().as_micros() as f64 / iters as f64;
        println!("doc {n:5} lines: {per:9.1} µs/keystroke");
    }
    println!("(linear growth ⇒ shaping/scan dominate; flat ⇒ already viewport-bound)");
}

#[test]
#[ignore]
fn stage_breakdown_4000() {
    // Run with: SRED_PERF=1 cargo test -p sred-core --release --test perf_probe \
    //   stage_breakdown_4000 -- --ignored --nocapture
    for &n in &[10usize, 100, 1000, 4000] {
        let body = paragraph(n);
        let mut e = Editor::from_source(&body, Format::Markdown);
        e.set_viewport(800, 600.0);
        e.render_view(true); // warm
        println!("\n=== one keystroke, {n}-line doc (stages) ===");
        for _ in 0..2 {
            e.apply(Command::Insert("x".into()));
            let t = Instant::now();
            let _ = e.render_view(true);
            println!(
                "TOTAL render_view {:>8.1}µs",
                t.elapsed().as_micros() as f64
            );
        }
    }
}

#[test]
#[ignore]
fn fulldoc_vs_viewport_render() {
    println!("\n--- render() full-doc vs render_view() viewport, by doc length ---");
    for &n in &[100usize, 1000, 4000] {
        let body = paragraph(n);
        let mut e = Editor::from_source(&body, Format::Markdown);
        e.set_viewport(800, 600.0);
        e.render(true);
        e.render_view(true);

        let iters = 30;
        let t1 = Instant::now();
        for _ in 0..iters {
            let _ = e.render(true);
        }
        let full = t1.elapsed().as_micros() as f64 / iters as f64;

        let t2 = Instant::now();
        for _ in 0..iters {
            let _ = e.render_view(true);
        }
        let view = t2.elapsed().as_micros() as f64 / iters as f64;

        println!(
            "doc {n:5} lines: render()={full:9.1} µs  render_view()={view:9.1} µs  ratio {:.2}x",
            full / view
        );
    }
}
