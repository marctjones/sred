# Performance — diagnosis & plan (perceived snappiness first)

**Goal:** typing feels *instant* and scrolling stays smooth, from the user's
point of view. Not raw throughput — perceived latency (keypress → character
visible) and absence of jank.

## 1. Where the cost is (the per-keystroke pipeline)

Every keystroke runs this, **synchronously, before the frame paints**:

| Step | Cost | Notes |
|---|---|---|
| `EditorCore.apply(Insert)` — rope splice | **O(edit)** | negligible |
| `text()` clone (+ Noet `set_current_body`) | **O(doc)** | whole-string copies |
| `view::styled_runs` + `decorations` + token scan | **O(doc)** | markdown re-scanned; **syntect re-highlights *all* code blocks** |
| `build_buffer` → `set_rich_text(all spans)` | **O(doc)** | **cosmic-text re-shapes the *whole document*** (HarfBuzz — the expensive part) |
| allocate + rasterize a **full-document-height** RGBA image | **O(doc px)** | tall image = millions of px |
| copy RGBA → `SharedPixelBuffer` + **GPU upload** | **O(doc px)** | a fresh texture every keystroke |
| Noet `invoke_note_edited` — re-parse entities + rebuild preview blocks + arm autosave | **O(doc)** | runs **on the keystroke path** |

**The core problem:** the user sees ~30–50 lines (the viewport), but the pipeline
redoes the **entire document** on every keystroke. Two independent axes:

- **Axis A — long-note lag:** shaping + raster + upload scale with note length.
  A 2000-line note pays 2000 lines of work to show one new character in the
  visible 40.
- **Axis B — short-note "not snappy":** even a tiny note can't paint the new
  character until sred renders *and* Noet's entity/preview/autosave pipeline runs
  synchronously. That's **off-screen work blocking the visible update**.

These need different fixes, and the cheap one targets the symptom the user
actually reported ("not slow, but not snappy").

## 2. Fixes, ranked by *perceived* win per unit of risk

### Tier 1 — get the off-screen work off the keystroke path  *(do first; low risk)*
The typed character should paint with **only the visible render** in the critical
path. Everything the user doesn't need *this instant* gets deferred:

- **Noet (host):** debounce `invoke_note_edited`'s heavy half — entity recompute
  + markdown-preview-block rebuild — by ~80–120 ms (autosave is already
  debounced). Keep the `text()` mirror immediate; defer the rest. The chips and
  preview update a beat after you stop typing; the *character* appears instantly.
- **sred:** skip `decorations`/token/syntect passes when nothing they affect is
  on screen is a Tier-2 concern; for Tier 1, the win is host-side deferral.

This alone likely makes short/medium notes feel snappy, and it's a small,
test-gateable change.

### Tier 2 — viewport-bounded rendering  *(the throughput fix for long notes)* — **DONE**
Rasterize + alloc + upload **only the visible slice**, not the whole document, so
per-keystroke allocation and GPU upload are **flat regardless of note length**.

- Shipped as `TextRenderer::render_viewport` + `Editor::render_view` (sred,
  v0.2.0-alpha.4) and the Noet display switch (viewport-sized image at a fixed
  position; pointer coords add `scroll-y` to reach document space; scroll drives
  a host re-render of the new slice).
- **Deliberately conservative on the part that broke before:** the buffer is
  still shaped in full (so caret/hit/geometry are byte-identical to the full-doc
  path) and we *avoid cosmic-text's `set_scroll` API* — only rasterization is
  bounded. This sidesteps the blank-render class entirely. Folding caret-follow
  into the same shaping pass guarantees the rasterized slice always matches the
  resolved scroll (typing at the bottom can't paint a stale frame).
- **Test-gated** against the prior regression: headless tests assert the visible
  frame has ink at the top, *changes* when a character is typed, is the *same
  size* for a 10-line and a 4000-line doc, and maps scroll + caret correctly.
- *Not yet done (future):* bounding the **shaping** too (persistent `Buffer` with
  per-line shape cache, or cosmic-text scroll-shaping) would also make shaping
  flat in doc length; today shaping is still O(doc) while alloc/upload are flat.

### Tier 3 — incremental polish  *(after Tiers 1–2)*
- **Cache syntect** highlights per code-block by content hash (only re-highlight
  changed blocks).
- **Reuse the RGBA scratch buffer** and consider a **persistent cosmic `Buffer`**
  updated per changed line (so unchanged lines keep their cached shaping).
- **Coalesce renders:** if keystrokes arrive faster than a render completes,
  render once for the batch.

## 2b. What the measurements actually showed (and what we did)

Instrumenting the per-keystroke pipeline (`sred-core/tests/perf_probe.rs`,
release) overturned the initial guess that *shaping* was the bottleneck:

| Stage (4000-line note, before) | cost | reality |
|---|---|---|
| `shape_until_scroll` | <1 ms | cosmic-text already caches shaping globally |
| `set_rich_text` | ~1500 ms | **rebuilding every line's text+AttrsList each keystroke** |
| `buffer.draw` raster | ~700 ms | rasterized *every* glyph, then discarded off-screen ones |
| decoration loops | ~500 ms | O(decorations × all runs) |
| `styled_runs` + `decorations` | ~40 ms | whole-document markdown re-scan |

Fixes, all byte-identical to the naive path and test-gated:

1. **Incremental persistent buffer** (`TextRenderer.cache_buf`) — reuse one
   `Buffer`; a per-line signature diff rebuilds only changed lines via
   `BufferLine::reset_new`.
2. **Prefix/suffix line splice** — line insert/delete updates `buffer.lines`
   with `Vec::splice` instead of a full rebuild, so Enter/paste are O(changed).
3. **Visible-run raster** — draw glyphs from visible runs only; selection/chip/
   strike passes iterate the visible runs + on-screen source range.
4. **Per-line styling cache** — `styled_runs`/`decorations` memoize the markdown
   scan per line (key: text, on-caret, base size, token generation).

Result (warm keystroke, including Enter/paste): **100 lines ≈ 2 ms, 1000 ≈ 7 ms,
4000 ≈ 13 ms** — under one 60 fps frame (16.7 ms) at every realistic size.

The residual is cheap O(doc) assembly (~3 µs/line: whole-doc span clone +
signature hash + `layout_runs` iteration). Driving it to true O(1) would require
a *changed-lines* facade contract (pass only edited lines, never materialize
whole-document spans) — worthwhile only beyond ~10k-line single notes.

## 3. Principle

Perceived snappiness comes from **doing only the work the user can see, now, and
deferring everything else.** Tier 1 stops blocking the paint on off-screen work;
Tier 2 makes the visible work independent of document size. Measure with a
keystroke timer (`Instant` around `apply`+`render`, and around Noet's
`rich_after_edit`) once the build environment is healthy, but the structure above
is what dictates the order of work.
