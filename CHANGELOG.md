# Changelog

All notable changes to sred. Versions follow the milestones in `docs/ROADMAP.md`
(target: **0.2.0** = usable as the primary editor for [Noet](../notes)).

## [0.7.0] — 2026-06-07

**Milestone: general-purpose editor-component hardening.** Rolls up the
`0.7.0-alpha.*` line into a stable release. Together with 0.4.0–0.6.0, sred is now
a genuinely reusable Rust text-editor component — not just Noet's Markdown/Typst
editor. Byte fidelity, Live Preview, and the per-line keystroke cost are intact;
clippy is clean and `cargo fmt` normalized.

Highlights since 0.6.0 (see the `alpha.*` entries below for detail):
- **Rendered math/figure fragments** — detection (`math_fragments`), a host
  renderer hook + cache (`set_fragment_renderer`/`render_fragment`), and overlay
  geometry (`rect_for_range`); wired end-to-end in the egui adapter. No compiler
  bundled (the host supplies one, by design).
- **Multiple-cursor rendering** — `FrameOut.carets`; egui draws all carets (+
  Alt+click to add) and the Slint component draws them via a `CaretBox` repeater.
- **Slint host fixes** — relayout no longer recurses (`Recursion detected`); an
  accessibility shadow exposes the document to screen readers.
- **List editing** — Tab/Shift-Tab indent/outdent.

### Stability
- `cargo fmt` clean; zero clippy warnings (`cargo clippy --workspace --all-targets`).
- 129 tests green in debug **and** release; 115 green with `--features
  syntax-highlight`.
- Per-keystroke cost at 10–2000 lines matches the v0.3.0 baseline (no algorithmic
  regression in the hot path). The whole-document re-parse at 4000+ lines is the
  next perf target — see the per-block analyze-cache follow-up.

## [0.7.0-alpha.3] — 2026-06-07

### Added — offset/range geometry; multi-cursor rendering (#17); fragment overlay (#15)

A new renderer geometry primitive (`TextRenderer::caret_rects` / `range_rects`)
finishes the two remaining items:

- **Multiple-cursor rendering (#17).** `FrameOut.carets: Vec<Caret>` now reports a
  bar per cursor. The egui adapter draws them all and adds **Alt+click** to drop a
  secondary caret; the Slint component draws them via a `CaretBox` repeater
  (`carets` model), so both host adapters show real multi-cursors. The extra
  buffer build happens only when more than one caret exists.
- **Rendered-fragment overlay (#15).** `Editor::rect_for_range(start, end)` returns
  screen-space rects (viewport-relative) for a span, so a host overlays a rendered
  math image precisely over its source. The egui adapter wires the full path
  end-to-end: detect → `render_fragment` (cached) → `rect_for_range` → blit. sred
  still bundles no compiler (by design — the host supplies it); true inline
  *reflow* around a fragment's box remains an optional future enhancement.

### Tests
- `tests/geometry.rs` (5): one caret normally, a rect per cursor (ordered, same
  line), collapse after motion, `rect_for_range` covers a span, and the math
  overlay path end-to-end.

## [0.7.0-alpha.2] — 2026-06-06

### Fixed / Added — long-standing Slint-host + list gaps (#1, #2, #3)

- **#1 — Slint relayout no longer recurses.** The reusable `RichTextEditor`
  reported its viewport size from the Flickable's `changed`/`init` geometry
  handlers, which fire *during* the layout flush and re-entered Slint's property
  system → `Recursion detected` (SIGABRT) in any host that relays it out. Now the
  size is reported from a post-layout `Timer`, the component's `preferred-width/
  height` are pinned to `0px`, and `scope`/`flick` fill explicitly. Regression
  test `relayout_does_not_recurse` advances time to force relayout. Behaviour
  matches the workaround documented in `docs/INTEGRATION.md`.
- **#2 — accessibility shadow in the Slint host.** The bitmap surface is opaque
  to assistive tech; the component now mirrors the document into `doc-text` and
  exposes it as `accessible-role: text` + `accessible-value`, so screen readers
  can read the content. (Core already provides `Editor::a11y()` since 0.5.0, and
  the egui adapter feeds AccessKit.) IME through the Slint image surface remains
  limited by Slint; core + the egui adapter support composition.
- **#3 — list editing: Tab/Shift-Tab indent & outdent.** `Command::Indent` /
  `Outdent` shift the selected lines (or caret line) by one level — nesting list
  items, indenting plain lines — wired to Tab/Shift-Tab in both the Slint and egui
  adapters. (Nested bullet/ordered *rendering* with indentation already shipped in
  0.3.0; `tests/lists.rs` pins it down. Ordered markers stay visible by design.)

### Tests
- `sred-slint`: `relayout_does_not_recurse`, `editor_exposes_text_to_accessibility`.
- `tests/lists.rs` (6): nested bullet/ordered rendering, indent/outdent (+ undo,
  multi-line, column-0 floor, byte-lossless).

## [0.7.0-alpha.1] — 2026-06-06

### Added — rendered math/figure fragment architecture (#15, partial)

The component-level building blocks for rendered Typst math/figures (the heavy
compiler + true inline interleaving remain a follow-up):

- **Fragment detection**: `view::math_fragments(text, format)` →
  `Vec<MathFragment>` (char range + delimited source + display flag). Markdown
  `$…$`/`$$…$$` via pulldown's math extension; Typst `$…$` equations via the
  syntax tree (block flag from the grammar). `Editor::math_fragments()`.
- **Host renderer hook + cache**: `Editor::set_fragment_renderer(cb)` where the
  host compiles `(src, display, font_size)` → `FragmentImage` (RGBA); results are
  cached by `(src, display, font_size)`. `Editor::render_fragment(frag)`.

sred deliberately does **not** bundle the Typst compiler. Remaining (tracked as a
follow-up on #15): overlay positioning geometry (`rect_for_range`) and true inline
interleaving of rendered fragments into the cosmic-text layout.

### Tests
- `tests/fragments.rs` (5): markdown + typst inline/display detection, char-index
  ranges, renderer call + caching, none-without-renderer.

## [0.6.0] — 2026-06-06

**Milestone: power editing & conveniences.**

### Added
- **Find / replace** (#9): `Editor::find` / `replace_all` and
  `EditorCore::find_next` / `find_prev` / `replace_range`, with `SearchOpts`
  (case-sensitive, whole-word). `replace_all` is one undoable, byte-lossless edit.
  `set_search_highlights` draws matches (pale chip; current match in the selection
  color).
- **Multiple cursors** (#11): `Editor::add_caret` / `clear_extra_carets` /
  `carets()`. Insert and Backspace apply at every caret in one transaction; any
  other command collapses to the primary. *(Editing model + accessor; drawing the
  secondary carets in the host adapters is a follow-up.)*
- **Auto-pairs & bracket matching** (#12): typing `([{`/`` ` ``/`"` inserts the
  closer with the caret inside, wraps a selection, and types over a matching
  closer. Toggle with `Editor::set_auto_pairs`.
- **Spellcheck hooks** (#13): `Editor::set_spellchecker(cb)` (host callback,
  re-run only when text changes) draws a red squiggle (`Decoration::Squiggle`)
  under flagged ranges; `Editor::word_at(x, y)` returns the word for a correction
  menu.

### Tests
- `tests/power_v06.rs` (12): find (case/whole-word/wrap), single-edit replace-all
  + undo, multi-caret insert/backspace + collapse, auto-pair insert/over/wrap/
  disable, `word_at`, spellchecker render.

## [0.5.0] — 2026-06-06

**Milestone: accessibility, international input & ecosystem reach.** Makes sred a
genuine general-purpose component, validated against a second host.

### Added
- **IME / preedit composition** (#4): `Editor::set_preedit/commit_preedit/
  clear_preedit/has_preedit`. The preedit is kept **out of the rope** — `text()`
  stays byte-lossless mid-composition — and injected only into the rendered
  `display_text()`, shown underlined (`preedit_range()`). Any committed command or
  pointer interaction cancels the composition.
- **Accessibility snapshot** (#5): host-agnostic `Editor::a11y() -> A11ySnapshot`
  (value, caret, selection, multiline). Hosts map it onto their a11y backend; the
  egui adapter feeds it to egui's `WidgetInfo` (which drives AccessKit).
- **egui host adapter** (#16): new `sred-egui` crate (`SredWidget`) — depends only
  on `sred-core`'s public API. Blits the RGBA frame to an egui texture, forwards
  keyboard/IME/pointer/clipboard events, draws the caret, and reports
  accessibility. Proves the core is host-agnostic (Slint was the only host
  before). Input mapping is factored into a headless-testable `apply_event`.

### Tests
- `tests/ime_a11y.rs` (6): preedit not in `text()` but in `display_text()`,
  commit/clear/cancel, selection-replace, a11y snapshot.
- `sred-egui` unit tests (4): text insert, IME preedit→commit, word motion/delete
  via keys, select-all + undo via the command modifier.

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
