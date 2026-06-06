# Changelog

All notable changes to sred. Versions follow the milestones in `docs/ROADMAP.md`
(target: **0.2.0** = usable as the primary editor for [Noet](../notes)).

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
