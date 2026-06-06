# Changelog

All notable changes to sred. Versions follow the milestones in `docs/ROADMAP.md`
(target: **0.2.0** = usable as the primary editor for [Noet](../notes)).

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
