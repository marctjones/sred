# MF1 / MF2 — Phase 2: block-level CommonMark & Typst (design + implementation)

> **STATUS: DONE — shipped in 0.3.0.** All four steps (A analysis/projection
> seam, B CommonMark block constructs, C Typst blocks via the syntax tree, D
> per-token color channel) are implemented in `sred-core/src/view.rs`, gated by
> `tests/commonmark.rs` + `tests/typst_blocks.rs`, perf-checked at ≈17 ms/4000
> lines. The sections below are the original plan, kept as design rationale.

**Cold-start brief.** Phase 1 (inline styling via real parsers) shipped in
`v0.3.0-alpha.1`. This document is the executable plan for **Phase 2**: the
*block-level* and cross-line constructs that the current per-line projection
can't handle, plus per-token color highlighting. A fresh session should be able
to start from "start phase 2 for mf1/mf2" using only this file.

---

## 1. Where things are now (read this first)

All styling lives in **`sred-core/src/view.rs`**. The pipeline, per keystroke:

```
Editor::render_view (api.rs)
  → styled_runs(text, format, base, caret_line, tokens, tokens_gen)   [view.rs]
      per source line:
        is this a fenced-code line?  → syntect colors (code_highlights, cached)
        else (prose):  STYLE_CACHE keyed (format,line,on_caret,base,tokens_gen)
            project_line(format, line, base, on_caret, code=false)  → (display, delta, size, extra)
            line_marks(format, display)                              → per-char MarkSet
            token colors                                             → per-char Option<color>
            emit Spans (split on (marks,color) runs)
  → decorations(text, format)  [STRIKE/LINK ranges, DECO_CACHE per line]
  → renderer.render_view  [layout.rs: incremental buffer + viewport raster]
```

**Phase 1 done (parser-driven INLINE):**
- `line_marks_md(line)` — parses the line with **pulldown-cmark** (`Options::ENABLE_STRIKETHROUGH`, `into_offset_iter()`), maps `Strong/Emphasis/Code/Strikethrough/Link` → `MarkSet`. Per-line.
- `line_marks_typst(line)` — parses with **typst-syntax** (`parse` → `LinkedNode`), walks the tree using the crate's own `highlight()` → `Tag`, maps `Strong/Emph/Raw/math/Link/Ref/Label/code-mode` → `MarkSet`. Per-line. (`typst_marks_walk`.)

**Still hand-rolled (the Phase-2 targets), all per-line:**
- `project_line_md` — block markers: ATX `#` headings, `> ` quote, `- /* /+ ` bullets (→ "• "), numbered. Computes the **delta** and hides markers off-caret.
- `project_line_typst` — `=`-run headings, `- ` bullet, `+ ` enum.
- `decorations` — only STRIKE/LINK from `line_marks` (so cross-line link defs etc. are invisible to it).

**The non-negotiable invariant:** `deltas[line] = (display leading bytes) − (source leading bytes)`, produced by `project_line`. `flat_to_render_cursor`/`render_cursor_to_flat` (layout.rs) use it for the caret/hit mapping. **Byte fidelity (`Editor::text()` round-trips) and cursor mapping depend ONLY on the projection/deltas, never on inline marks.** Inline marks are cosmetic. Phase 2 changes the *projection*, so it is delta-critical — every projection change needs a fidelity + delta test.

---

## 2. What Phase 2 must add

### CommonMark block-level (MF1 Phase 2)
Constructs that need cross-line / document context:
- **Setext headings** (`Title\n===` / `---`) — line N is a heading because of line N+1.
- **Reference links** `[text][ref]` resolved against `[ref]: url` defined elsewhere.
- **Nested lists & continuation**, loose-vs-tight, ordered start numbers.
- **Nested block quotes** (`> > x`).
- **Indented code blocks** (4-space / tab) — currently styled as prose.
- **HTML blocks**, **GFM tables**, **task list items** (`- [ ] x`).

### Typst block-level + per-token color (MF2 Phase 2)
- Drive `project_line_typst` from the **typst-syntax tree**: heading depth, list/enum/term markers, hiding — instead of the hand-rolled prefix checks.
- **Per-token COLORS** (the bigger one): map typst `Tag` (and CommonMark too) to *colors* (keyword/function/number/string/comment), not just the `CODE` mark. Requires a **color channel** alongside `MarkSet` in the styling pass (see §4.3).

---

## 3. The core architectural decision

Per-line styling can't see other lines. Phase 2 needs **document-level analysis**.
The key design that keeps performance and the Live-Preview model intact is to
**split the work into a caret-INDEPENDENT analysis and a caret-DEPENDENT projection:**

```
analyze(text, format) -> DocAnalysis          // doc-level parse; caret-INDEPENDENT; cacheable by text
    per line: { block: BlockKind, marker: Range<usize>(bytes to hide),
                size: f32, extra: MarkSet, inline: Vec<(Range,MarkSet)>,
                colors: Vec<(Range,Color)> }
project(DocAnalysis, caret_line) -> (spans, deltas)   // caret-DEPENDENT; cheap
    per line: reveal markers iff line == caret_line; else hide → compute delta;
              assemble spans from inline marks + colors
```

Why this split:
- The **expensive** part (full parse) is **caret-independent**, so moving the caret
  (a very common event) does NOT reparse — only re-projects (cheap). Cache
  `analyze()` by `text` hash; a *content* edit invalidates it (but pulldown /
  typst-syntax parse a 4000-line doc in ~1–2 ms, which the incremental buffer in
  layout.rs further shields — it still only rebuilds *changed* lines from the new
  spans).
- The **delta** computation stays in `project()` exactly as today (marker byte
  ranges come from the analysis), so the fidelity invariant is preserved by
  construction.

**Performance contract to honor (do NOT regress):** keystroke under one 60 fps
frame at 4000 lines. Today ~16 ms. A whole-doc parse per keystroke is acceptable
if measured ≤ a few ms; otherwise add per-block caching (hash each top-level block's
source → cache its analysis). Re-run `cargo test -p sred-core --release --test
perf_probe stage_breakdown_4000 -- --ignored --nocapture` after each step.

---

## 4. Implementation plan (incremental, test-gated)

Do these in order; each is independently shippable as a `0.3.0-alpha.N`.

### Step A — introduce the doc-level analysis seam (no behavior change)
1. Add `struct DocAnalysis { lines: Vec<LineInfo> }` and `LineInfo { block, marker_bytes, size, extra, inline: Vec<(Range<usize>, MarkSet)> }` in view.rs.
2. Implement `analyze_md(text) -> DocAnalysis` that reproduces **exactly today's behavior** but from a single pulldown-cmark `into_offset_iter()` pass over the whole document (track block Start/End to know each line's block kind + marker range; collect inline ranges per line). Implement `analyze_typst(text)` similarly from one typst-syntax parse of the whole doc.
3. Rewrite `styled_runs` to call `analyze()` then `project()` per line. **Gate:** all existing tests still pass byte-identically (the `*_matches_fresh` / live-preview / fidelity tests are the guard). Keep the per-line `STYLE_CACHE` *or* replace with an `analyze()`-by-text-hash cache; measure both.
   - This step is pure refactor — same output, new seam. Land it before adding constructs.

### Step B — CommonMark block constructs (MF1 Phase 2)
Add to `analyze_md`, one construct per commit, each with a live-preview + delta + fidelity test:
- Setext headings (mark line N heading, hide the `===`/`---` line).
- Indented code blocks (style as code; no marker hiding).
- Task list items `- [ ]` (render a checkbox glyph; delta like bullets).
- GFM tables (style pipes/headers; keep source).
- Reference links: first pass collects `[ref]: url`; resolve `[text][ref]` / `[ref]` to LINK.
- Nested lists/quotes: indentation-aware bullet/quote projection.
**Gate per construct:** a markdown sample → expected (display, deltas, marks); plus the byte-lossless `text()` round-trip.

### Step C — Typst block via typst-syntax (MF2 Phase 2)
Rewrite `project_line_typst` (now `analyze_typst`) to read heading depth (`Heading`/`HeadingMarker`), `ListItem`/`EnumItem`/`TermItem` markers from the tree, computing marker byte ranges + deltas from node ranges. **Gate:** existing typst tests + new term-list/nested cases + fidelity.

### Step D — per-token COLOR channel (MF2 Phase 2, also benefits code)
1. Extend the analysis to carry `colors: Vec<(Range, [u8;4])>` per line (from typst `Tag`→color and optionally CommonMark).
2. Thread colors into `style_prose_line`'s span assembly (it already merges a `charcolors` vec for tokens + code; feed analysis colors into the same vec, precedence: token > syntax color > mark-derived).
3. Add a **theme palette** for syntax categories on `layout::Theme` (keyword/function/number/string/comment/operator), so colors are host-themeable.
**Gate:** a typst `#let f(x) = 1` sample yields distinct keyword/function/number colors; markdown unaffected unless opted in.

---

## 5. Invariants & risks (the "don't break these")

- **Byte fidelity:** `Editor::text()` must round-trip. The corpus/fidelity tests in `sred-core` are the gate. Any projection change is delta-critical.
- **Delta correctness:** every new block construct must set `marker_bytes` so the off-caret `delta = displayed_leading − source_leading` is exact, or the caret/hit mapping drifts. Add a delta assertion per construct.
- **Live Preview:** markers hidden off-caret, revealed on the caret line — keep the caret-dependent projection cheap and correct (caret move must not reparse).
- **Performance:** stay under ~16 ms/keystroke at 4000 lines. Measure with `perf_probe`. If the whole-doc parse is too slow, add per-block caching keyed by block source hash.
- **Incremental buffer (layout.rs) is downstream and unaffected** — it diffs the spans `styled_runs` returns, so it keeps working regardless of how spans are produced. Don't touch `build_buffer_cached`.
- **Cross-line constructs break the per-line `STYLE_CACHE` key** (a line's rendering now depends on neighbors). Either switch to an `analyze()`-by-text cache or key block-level entries by block content. Decide in Step A and write a `*_matches_fresh` test.

## 6. Acceptance gates for "Phase 2 done"
- A curated subset of the **CommonMark spec examples** render with correct inline+block marks (add `sred-core/tests/commonmark.rs`, ignored-by-default if large).
- Typst: headings/lists/term-lists/refs/code-mode/math all styled; per-token colors visible in the demo (`cargo run -p sred-demo`, switch format to Typst).
- All fidelity + delta + `*_matches_fresh` tests green; `perf_probe` shows no regression past ~one frame at 4000 lines.
- README/CHANGELOG/ROADMAP updated; tag `v0.3.0` when block-level is complete.

## 7. Pointers
- Code: `sred-core/src/view.rs` (all styling), `sred-core/src/layout.rs` (downstream buffer/raster — leave alone), `sred-core/src/api.rs` (facade).
- Parsers already deps: `pulldown-cmark` 0.13, `typst-syntax` 0.14 (its `highlight()`/`Tag` is in that crate's `highlight.rs`).
- Phase-1 reference commits: `20111dd` (MF1 inline), `44d337d` (MF2 inline).
- Perf method + numbers: `docs/PERF.md`. Roadmap context: `docs/ROADMAP.md` → "Post-0.2.0".
