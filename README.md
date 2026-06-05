# sred — a rich-text editor component for Slint

WYSIWYG inline editing with **Markdown** and **Typst** backends, packaged as a
reusable Slint component. The hard parts (document model, text shaping, cursor,
hit-testing, backends) live in UI-free Rust; Slint is a thin render + input
surface.

> ### 🎯 Release target — **0.2.0: usable as [Noet](../notes)'s primary editor**
> sred is being driven toward replacing the raw-markdown editor in the Noet notes
> app. That requires a pivot to **byte-lossless, source-anchored editing** plus an
> extension API for domain tokens, theming, and scrolling. The plan lives in:
> - [`docs/DESIGN.md`](docs/DESIGN.md) — architecture, the source-anchored model, the extension API, sred↔Noet responsibility split.
> - [`docs/ROADMAP.md`](docs/ROADMAP.md) — milestones M1–M8 toward 0.2.0 with acceptance gates.
> - [`docs/IMPLEMENTATION_PLAN.md`](docs/IMPLEMENTATION_PLAN.md) — per-milestone task breakdown.
>
> The phased status below describes the **current 0.1.0** standalone editor; where
> it conflicts with the docs above, the docs win for 0.2 work.

## Workspace

| crate        | role |
|--------------|------|
| `sred-core`  | UI-free editor core: model, backends, editing engine, cosmic-text raster |
| `sred-slint` | the reusable `RichTextEditor` Slint component + Rust controller |
| `sred-demo`  | runnable demo app embedding the component |

```
sred-core/src
  model.rs      superset document AST (blocks + inline runs with a MarkSet bitset)
  format.rs     DocumentFormat trait + capability masks
  markdown.rs   CommonMark/GFM parse (pulldown-cmark) + direct serializer
  typst_fmt.rs  Typst markup parse (typst-syntax green tree) + serializer
  editor.rs     editing engine: commands, cursor, selection, marks
  layout.rs     cosmic-text shaping + RGBA rasterization + hit-testing
sred-slint
  ui/sred.slint RichTextEditor (reusable) + MainWindow (demo host)
  src/lib.rs    controller wiring EditorCore + TextRenderer to the component
```

## Run

```
cargo run -p sred-demo
```

Type to edit; click to place the caret, Shift-arrows to select, arrows (incl.
up/down) to move. Use the **menu bar** (Edit / Format / Paragraph / Insert) or
the **toolbar** for formatting; everything routes through one `command(string)`
dispatch in Rust. Switch the format dropdown to see the live source view
re-serialize to Markdown or Typst.

Feature coverage:
- **Inline marks:** bold, italic, inline code, strikethrough (Ctrl-B / Ctrl-I /
  Ctrl-E + toolbar/menu) — real WYSIWYG, survive save.
- **Block kinds:** body paragraph, H1–H3 (rendered at real heading sizes),
  bulleted list, numbered list, block quote, code block, divider — all survive
  the edit→save round-trip.
- **History:** undo/redo with typing-run coalescing (Ctrl-Z / Ctrl-Y).
- **Insert:** link / image / math / table insert editable source snippets.

## Testing

`cargo test` (20 tests). Three layers:

- **`sred-core` unit tests (12)** — headless logic: Markdown/Typst round-trip
  fixpoints, cross-format mark survival, block-structure survival, autoformat
  input rules, list-exit on Enter/Backspace, undo/redo coalescing, control/PUA
  input sanitizing, and the flat↔cursor / prefix-offset mapping.
- **`sred-slint` controller tests (7)** — the Rust glue without a UI: `run_named`
  command dispatch (bold/heading/list/undo/redo), color-is-view-only, plus pure
  helpers (`normalize_chord`, `is_url`, `color_for`).
- **`sred-slint` headless GUI test (1)** — drives the *real generated* `MainWindow`
  via `i-slint-backend-testing` (`build_window` wires everything without the event
  loop): invokes toolbar/menu commands and typed input, then asserts on the live
  `source-text` property. Covers command dispatch → core → render → property
  propagation, the typed autoformat path, and the Markdown→Typst format switch.

> The Slint backend is process-global (init once), so all UI assertions live in a
> single test; it passes under both parallel and `--test-threads=1`.

What's still **not** automated: actual pixel/raster output, pointer-drag
selection geometry, and clipboard round-trips (system clipboard) — these remain
manual via `cargo run -p sred-demo`.

## Status (phased plan)

- [x] **Phase 1** — model + Markdown + Typst backends, round-trip tests (`cargo test -p sred-core`)
- [x] **Phase 2** — cosmic-text layout + raster → Slint `Image`, renders a document
- [x] **Phase 3** — editing: insert/backspace/move/select/toggle-mark + cursor
- [x] **Geometry** — exact caret rect, selection highlight, click hit-testing and
      vertical motion all driven by cosmic-text (`layout.rs`); flat↔cursor mapping tested
- [x] **Phase 4** — block-aware buffer: headings/lists/quotes/code/dividers edited
      inline (real heading sizes, bullet/number/quote prefixes) and **survive
      edit→save** (tested in `block_structure_survives_edit_cycle`)
- [x] **Menu + toolbar** — full Markdown/Typst feature set via one `command()` dispatch
- [x] **Undo/redo** — snapshot history with typing-run coalescing (Ctrl-Z / Ctrl-Y), tested
- [ ] **Phase 5** — rich link/image/math editing (vs. literal snippets), Typst
      `Raw` chips, IME, rich/plain clipboard
- [ ] **Phase 6** — multi-line block spacing, dirty-region re-raster, GPU glyph path

## Known scaffold simplifications

These are deliberate and documented in-code; the *architecture* already accounts
for them:

- **Editable buffer is line-granular.** Each block is one editable line
  (`text: Vec<char>` + per-char `MarkSet` + per-block `BlockKind`); multi-line
  constructs (fenced code, multi-paragraph quotes) are consecutive same-kind
  blocks re-merged on save. Block structure now **does** survive edit→save. A
  production buffer would swap the `Vec<char>` for a `ropey::Rope` for large-doc
  performance — the `EditorCore` API (`apply` / `styled_runs` / `to_document`)
  wouldn't change.
- **Link/image/math are inserted as literal source snippets**, not yet edited as
  rich objects (the flat mark map has no per-run href storage). Phase 5.
- **Full re-raster per edit.** Dirty-region caching is Phase 6 — currently every
  edit re-shapes and re-rasterizes the whole document.

## Build & requirements

```
git clone https://github.com/marctjones/sred
cd sred
cargo build
cargo test
cargo run -p sred-demo   # launches the demo window
```

Pure Rust; no system deps beyond a working Slint backend (winit/femtovg on
desktop). A transitive Slint dependency (`typed-index-collections`) raised its
MSRV above the installed rustc 1.89, so `Cargo.lock` pins it to `3.3.0`; remove
the pin once on rustc ≥ 1.90.

## License

**GPL-3.0-only** (see [`LICENSE`](LICENSE)).

This matches [Slint](https://slint.dev), which is offered under
`GPL-3.0-only OR <commercial>`; sred uses Slint under its GPL terms, so the
combined work is GPL-3.0. If you need a non-GPL distribution you would need a
commercial Slint license and a separate arrangement for sred.

## Releases

- **v0.1.0** — standalone WYSIWYG editor: Markdown + Typst backends, block-aware
  inline editing, menu + toolbar, undo/redo, links/colors, 20 tests. Baseline
  before the 0.2 source-anchored rewrite (see `docs/ROADMAP.md`).
