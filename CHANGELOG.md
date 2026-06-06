# Changelog

All notable changes to sred. Versions follow the milestones in `docs/ROADMAP.md`
(target: **0.2.0** = usable as the primary editor for [Noet](../notes)).

## [0.4.0] — 2026-06-06

**Milestone: editing parity & quick wins.** Rounds out the everyday editing
surface and finishes CommonMark block coverage, on the way to a general-purpose
editor component. All edits stay byte-lossless.

### Added
- **Word & document motion** (#6): `Motion::WordLeft/WordRight/DocStart/DocEnd`
  plus `Command::DeleteWordBackward/DeleteWordForward` (Ctrl+←/→, Ctrl+Home/End,
  Ctrl+Backspace/Delete). Char-class word boundaries; Unicode-aware.
- **Portable clipboard contract** (#7): `Editor`/`EditorCore` gain
  `copy()`/`cut()`/`paste()` so any host (not just Slint) gets copy/cut/paste
  for free. `sred-slint` now dispatches through it.
- **Host-selectable font family** (#8): `Theme.font_family` / `code_font_family`
  → cosmic-text `Family::Name`; folded into the layout cache key so a font change
  invalidates correctly. (Custom fonts are loaded into the host `FontSystem`.)
- **Pointer & selection polish** (#10): triple-click line/paragraph selection
  (`triple_click`), drag-and-drop move of a selection (`drop_selection_at` /
  `EditorCore::move_selection_to`), and `page(down)` (PageUp/PageDown).
- **CommonMark HTML blocks** (#14): raw HTML blocks render as code (source kept,
  delta 0), via the existing whole-document `scan_md` pass.

### Tests
- `tests/editing_v04.rs` (8): word/doc motion, word-delete, clipboard round-trip,
  line selection, drag-drop move, Unicode word motion.
- `tests/commonmark.rs`: + HTML-block case.

## [0.3.0] — 2026-06-06

**Milestone: parser-driven styling, inline *and* block-level.** Rolls up the
`0.3.0-alpha.*` line. All Markdown/Typst styling is now driven by the real
parsers (`pulldown-cmark`, `typst-syntax`) — inline (alpha.1) plus the
block-level / cross-line constructs below (alpha.2) — with per-token syntax
colors, while keeping byte fidelity, exact cursor mapping, Live Preview, and flat
per-keystroke performance.

### Added — Phase 2: block-level CommonMark & Typst + per-token colors

Styling is now split into a caret-**independent** whole-document `analyze()`
(one parse, cached by text) and a cheap caret-**dependent** `project()` per line.
A caret move no longer reparses — only the two lines whose caret-state flipped
re-project. The byte delta (the fidelity/cursor invariant) is produced entirely
in `project()` from the marker byte range `analyze()` records.

- **Step A — analysis/projection seam.** `LineInfo`/`DocAnalysis`; per-line
  `STYLE_CACHE` replaced by `ANALYSIS_CACHE` (by text) + `PROJECT_CACHE` (by a
  per-line digest that folds in every cross-line dependency). Pure refactor.
- **Step B — CommonMark block constructs (MF1)**, from one whole-document
  `pulldown-cmark` pass: **setext headings** (underline hidden), **indented code
  blocks** (lazy continuations excluded), **task lists** (`- [ ] `/`- [x] ` →
  `☐ `/`☑ `), **GFM tables** (header bold, pipes code), **reference links**
  (`[text][ref]` resolved against a definition elsewhere), and **nested
  lists/quotes** (indentation preserved; nested `> > ` collapses). New
  `Marker { start, len, repl }` keeps indentation before a marker and an exact
  delta.
- **Step C — Typst blocks via `typst-syntax` tree (MF2).** Heading depth, list /
  enum / term markers read from the grammar tree (`scan_typst`); marker byte
  ranges (hence deltas) come from node ranges, so nested markers keep their
  indentation. Enum `+ ` kept visible; term `/ ` hidden.
- **Step D — per-token color channel (MF2).** Theme-independent `SynCat`
  categories from the typst highlighter, resolved to RGBA at projection time via
  a host `SynPalette` (precedence token > syntax > mark). `Theme` gains a syntax
  palette; `Editor` calls the new `view::styled_runs_with`. `layout.rs` untouched
  (colors flow through the already-hashed `Span.color`).
- Tests: new `tests/commonmark.rs` (13) and `tests/typst_blocks.rs` (11), each
  asserting display + byte delta + marks/colors and a byte-lossless caret-line
  round-trip. perf_probe ≈ 17 ms warm at 4000 lines (no regression; one
  whole-doc parse beats 4000 per-line parses).

## [0.3.0-alpha.1] — 2026-06-06

### Changed — parser-driven inline styling (full CommonMark + full Typst, inline)
- **MF1 — Markdown inline via `pulldown-cmark`.** `line_marks_md` now uses the
  CommonMark reference parser (+ GFM strikethrough) instead of a hand-rolled
  scanner: spec-correct nesting, delimiter matching, code-span protection, and
  links. Replaces `apply_pair`/`apply_single`/`apply_links_marks`.
- **MF2 — Typst inline via `typst-syntax`.** `line_marks_typst` walks
  `typst-syntax`'s `LinkedNode` tree using the crate's own `highlight()`/`Tag`
  categorizer: strong/emph/raw, math, refs/labels, and `#`-code-mode tokens.
  Replaces the Level-1 hand-rolled recognizer.
- Both reuse already-present workspace deps; only the inline recognizer changed.
  Block-marker projection + byte-delta/cursor mapping are unchanged (inline marks
  are cosmetic). Per-line styling cache keeps keystroke cost flat (4000-line note
  ≈ 16 ms warm).
- Tests: `markdown_inline_via_commonmark`, `typst_full_via_typst_syntax`.

### Roadmap
- `docs/ROADMAP.md`: MF1/MF2 **block-level** constructs (reference links, setext,
  nested lists, Typst block markers + per-token colors) scoped as Phase 2; added a
  **Future file formats** section (plaintext markup vs DOCX/ODP, comments before
  redlining).

## [0.2.0] — 2026-06-06

**Milestone: sred is usable as [Noet](../notes)'s primary editor.** Rolls up the
`0.2.0-alpha.*` line. sred is now a **byte-lossless, source-anchored** Markdown /
Typst editor with Obsidian-style **Live Preview**, an embeddable `Editor` facade,
a domain-token extension API, host theming, and **flat per-keystroke performance**
(a keystroke — including Enter/paste — stays under one 60 fps frame regardless of
note length). Noet consumes it as the sole note editor.

Highlights since 0.1.0 (see the `alpha.*` entries below for detail):
- **Source-anchored core** — the raw text *is* the buffer (`ropey::Rope`); `text()`
  round-trips byte-for-byte (fidelity corpus test). Replaces the 0.1 structured
  model that normalized on save.
- **Live Preview** — caret-aware hiding of block markers + code fences.
- **Embeddable `Editor` facade** + **domain-token API** (fg colors + chip
  backgrounds; `token_at` click resolution) + host `Theme` + scrolling.
- **Flat performance** — viewport-bounded rendering, incremental persistent
  cosmic-text buffer, prefix/suffix line splice, per-line styling cache.
- **Level-1 Typst** markup live-preview; `Editor::can_undo()/can_redo()`.

### Known limitations (targeted for 0.3.0)
- Inline styling is **pragmatic** (common Markdown + Level-1 Typst markup), and
  decoupled from fidelity. **Spec-complete CommonMark (`pulldown-cmark`) and full
  Typst (`typst-syntax`)** are the 0.3.0 milestone.
- Rendered Typst (math layout, figures) needs the Typst compiler (out of scope
  for the inline editor; hosts can keep a compiled preview).
- No IME / accessibility on the image surface yet; `Theme` has no font-family.

## [0.2.0-alpha.6] — 2026-06-06

### Added — Level 1 Typst markup live-preview
- The styling layer is now **format-aware** (it previously applied Markdown rules
  to Typst notes). Typst notes get inline WYSIWYG markup styling: `=`-run headings
  (marker hidden off the caret line), `- ` bullets, single-asterisk `*strong*`,
  `_emph_`, and `` `raw` `` / `$math$` styled like code.
- Styling/decoration caches are keyed by format (same text styles differently per
  format — e.g. `*x*` is strong in Typst, emphasis in Markdown).
- Rendered Typst (math layout, figures) still needs the Typst compiler — see
  `docs/ROADMAP.md` "Post-0.2.0 — full format support (parser-driven)" for the
  plan to reach full CommonMark (`pulldown-cmark`) and full Typst (`typst-syntax`).

### Added — API
- `Editor::can_undo()` / `can_redo()` so hosts can enable/disable Undo/Redo UI.

## [0.2.0-alpha.5] — 2026-06-06

### Performance — flat per-keystroke cost (snappy at any document size)
A keystroke (including Enter / paste) is now under one 60 fps frame regardless of
note length — measured warm: 100 lines ~2 ms, 1000 ~7 ms, 4000 ~13 ms (was
~1900 ms at 4000 before this line of work).

- **Incremental persistent buffer** — reuse one cosmic-text `Buffer` across
  renders; rebuild only changed lines instead of `set_rich_text` over the whole
  document each keystroke (that rebuild was the dominant cost; shaping itself was
  already cached by cosmic-text).
- **Line-splice updates** — a prefix/suffix signature diff splices `BufferLine`s,
  so line insert/delete (Enter, Backspace-join, paste) is O(changed lines), not
  O(doc). Removes the multi-hundred-ms stall on long notes.
- **Viewport-bounded raster** — rasterize glyphs from visible runs only, and run
  selection/chip/strike-underline passes over the visible runs + the on-screen
  source range (was re-scanning every run per decoration).
- **Per-line styling cache** — `styled_runs` + `decorations` memoize the markdown
  scan per line; only changed lines re-scan. `Span` now derives `Clone`.
- All byte-identical to the non-cached / full-rebuild paths — guarded by
  `incremental_*` (layout) and `styling_cache_matches_fresh_computation` (view)
  tests. New `sred-core/tests/perf_probe.rs` (ignored) measures keystroke cost.
- `Editor::register_token` / `clear_tokens` bump a token generation that
  invalidates the styling cache for token-colored lines.

## [0.2.0-alpha.4] — 2026-06-06

### Performance
- **Viewport-bounded rendering** (Tier 2) — new `TextRenderer::render_viewport`
  and `Editor::render_view(follow)` rasterize only the visible slice into a
  **viewport-sized** frame, so per-keystroke allocation and GPU upload are flat
  regardless of document length (the throughput fix for long notes). The buffer
  is still shaped in full (caret/hit/geometry unchanged) and cosmic-text's scroll
  API is avoided, so the blank-render class that bit the earlier attempt can't
  recur. Caret-follow is folded into the same shaping pass — the rasterized slice
  always matches the resolved scroll.
- New `ViewOut` type (frame + viewport-relative caret + resolved `scroll_y` +
  full `doc_height`). `render()`/`RenderOut` are unchanged; full-doc hosts keep
  working and opt into the viewport path via `render_view`.
- Test-gated: `viewport_render_{shows_content_at_top,reflects_edits_in_view,
  cost_is_flat_in_doc_length,scrolls_to_show_lower_content,caret_is_viewport_relative}`.

### Added
- Re-export `RenderOut` and `ViewOut` from the crate root.

## [0.2.0-alpha.3] — 2026-06-06

### Performance
- **Memoized syntect highlighting per code block** (Tier 3) — unchanged fenced
  code blocks are no longer re-highlighted on every keystroke (thread-local cache
  keyed by `(lang, body)` content hash).
- Added `docs/PERF.md` — per-keystroke cost diagnosis and the perceived-snappiness
  plan (Tier 1: defer off-screen work [host-side, done in Noet]; Tier 2:
  viewport-bounded rendering; Tier 3: syntect cache / buffer reuse).

## [0.2.0-alpha.2] — 2026-06-05

### Performance
- Facade `render()` computes the source text once and shares it across the
  style/decoration/token passes (was cloned 4× per keystroke).

## [0.2.0-alpha.1] — 2026-06-05

The "embeddable" milestone: sred became a **byte-lossless, source-anchored,
Live-Preview** markdown editor with a clean embedding API. Noet now consumes it.

### Added
- **Embeddable `sred_core::Editor` facade** (`api.rs`) — bundles the whole
  per-keystroke pipeline (style → decorate → rasterize → caret-follow) behind a
  few calls: `from_source`/`text`/`set_text` (byte-lossless), `set_theme`,
  `set_viewport`, `apply(Command)`, `click`/`drag`/`double_click`,
  `move_vertical`, `scroll_by`/`scroll_to`, and `render(follow) -> FrameOut`.
- **Domain-token extension API** — `register_token(TokenSpec { id, fg, bg,
  matcher })` colors `[[wikilink]]`/`#tag`/`@mention`/url (with optional **chip
  backgrounds**), and `token_at(x, y) -> Option<(id, value)>` resolves a click to
  its token for host filter/open-url routing.
- **Live Preview** — caret-aware hiding of block markers (`#`, `-`, `>`) and
  ` ``` ` code fences off the caret line; markers reappear on the line being
  edited. Source stays byte-lossless.
- **Syntect code-block highlighting** behind the `syntax-highlight` feature
  (pure-Rust `fancy-regex`, off by default).
- **Scrolling** — mouse-wheel + draggable scrollbar + caret-follow autoscroll;
  symmetric 16px margins on all four sides.
- Docs: `docs/DESIGN.md`, `docs/ROADMAP.md`, `docs/IMPLEMENTATION_PLAN.md`,
  `docs/INTEGRATION.md` (how to embed in a host).

### Changed
- **Editor core re-architected to source-anchored** — the raw markdown text *is*
  the buffer (`ropey::Rope`); edits splice it, so `text()` round-trips
  byte-for-byte (corpus test: CRLF, trailing whitespace, blank runs, nested
  lists, HTML, tables, todo lines, unicode). Replaces the old structured model
  that normalized markdown on save.
- Toolbar actions now write real markers into the source (`Bold` → `**…**`,
  `H2` → `## `).

### Fixed
- Pointer coordinates are document-space; the facade no longer double-counts the
  scroll offset (clicking while scrolled placed the caret on the wrong line).

### Known limitations
- Per-keystroke render is whole-document (viewport-bounded rendering is planned;
  a first attempt is parked on the `m2-viewport` branch).
- No IME / accessibility on the image-rendered surface yet.
- `Theme` has no font-family field (uses cosmic-text's default fonts).

## [0.1.0] — 2026-06-05

Initial public release: standalone WYSIWYG editor with Markdown + Typst backends,
block-aware inline editing, menu + toolbar, undo/redo, links/colors. Structured
model with reconstructive save (superseded by the 0.2 source-anchored core).

[0.2.0-alpha.1]: https://github.com/marctjones/sred/releases/tag/v0.2.0-alpha.1
[0.1.0]: https://github.com/marctjones/sred/releases/tag/v0.1.0
[0.2.0-alpha.2]: https://github.com/marctjones/sred/releases/tag/v0.2.0-alpha.2
[0.2.0-alpha.3]: https://github.com/marctjones/sred/releases/tag/v0.2.0-alpha.3
[0.2.0-alpha.4]: https://github.com/marctjones/sred/releases/tag/v0.2.0-alpha.4
[0.2.0-alpha.5]: https://github.com/marctjones/sred/releases/tag/v0.2.0-alpha.5
[0.2.0-alpha.6]: https://github.com/marctjones/sred/releases/tag/v0.2.0-alpha.6
[0.2.0]: https://github.com/marctjones/sred/releases/tag/v0.2.0
[0.3.0-alpha.1]: https://github.com/marctjones/sred/releases/tag/v0.3.0-alpha.1
