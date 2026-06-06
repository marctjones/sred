# sred — a rich-text editor component for Slint

WYSIWYG inline editing with **Markdown** and **Typst** backends, packaged as a
reusable Slint component. The hard parts (document model, text shaping, cursor,
hit-testing, backends) live in UI-free Rust; Slint is a thin render + input
surface.

> ### ✅ **0.2.0 — released: [Noet](../notes)'s primary editor**
> sred is a **byte-lossless, source-anchored** editor: the raw Markdown/Typst text
> *is* the buffer, so `text()` round-trips byte-for-byte. It renders inline
> (Obsidian-style **Live Preview** — markers hidden off the caret line), embeds via
> a small `Editor` facade, exposes a **domain-token** extension API, and has
> **flat per-keystroke performance** (a keystroke — including Enter/paste — stays
> under one 60 fps frame regardless of note length). Noet consumes it as the sole
> note editor today.
>
> **0.3.0:** styling is driven entirely by the real parsers.
> *Inline* and *block-level* CommonMark — setext headings,
> indented code, task lists, GFM tables, reference links, nested lists/quotes —
> plus Typst blocks read from the `typst-syntax` tree and per-token syntax colors.
> See [`docs/ROADMAP.md`](docs/ROADMAP.md) → "Post-0.2.0 — full format support"
> and [`docs/MF_PHASE2.md`](docs/MF_PHASE2.md).
> Architecture + the sred↔Noet split: [`docs/DESIGN.md`](docs/DESIGN.md);
> embedding guide: [`docs/INTEGRATION.md`](docs/INTEGRATION.md); performance
> diagnosis: [`docs/PERF.md`](docs/PERF.md).

## Workspace

| crate        | role |
|--------------|------|
| `sred-core`  | UI-free editor core: model, backends, editing engine, cosmic-text raster |
| `sred-slint` | the reusable `RichTextEditor` Slint component + Rust controller |
| `sred-egui`  | egui host adapter (`SredWidget`) — depends only on `sred-core` |
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

`cargo test` (40+ tests). Layers:

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
- [x] **Performance** — flat per-keystroke cost: incremental persistent cosmic-text
      buffer + line-splice + viewport-bounded raster + per-line styling cache
- [x] **Live Preview + viewport rendering** — markers hidden off-caret; viewport-sized
      frames via `render_view`
- [x] **0.3.0** — full CommonMark (`pulldown-cmark`) + full Typst (`typst-syntax`)
      styling, inline + block-level, with per-token syntax colors
- [x] **0.4.0** — editing parity: word/document motion + word-delete, portable
      clipboard contract, host font family, triple-click/drag-drop/page, HTML blocks
- [x] **0.5.0** — IME/preedit + accessibility snapshot + a non-Slint (`sred-egui`)
      adapter (egui feeds AccessKit)
- [ ] **0.6.0** — find/replace, multiple cursors, auto-pairs, spellcheck hooks
      ([milestones](https://github.com/marctjones/sred/milestones))

## Embedding (the reusable surface)

Depend on **`sred-core`** (UI-free, Slint-version-independent) and drive one
`Editor`:

```rust
use sred_core::{Editor, Command, Format};
let mut ed = Editor::from_source(&note_body, Format::Markdown);
ed.set_theme(my_theme); ed.set_viewport(w, h);
ed.apply(Command::Insert("x".into()));        // or click/drag/scroll/…
let out = ed.render_view(/* follow caret */ true); // viewport-sized RGBA + caret + scroll
backend.save(ed.text());                      // byte-lossless
```

Register domain tokens (`[[wikilink]]`/`#tag`/…) with colors + chip backgrounds
via `register_token`, and resolve clicks with `token_at`. Full guide:
[`docs/INTEGRATION.md`](docs/INTEGRATION.md). [Noet](../notes) consumes sred this
way today.

## 0.2.0 — what's in it (see `CHANGELOG.md`)

- ✅ **Source-anchored core** — byte-lossless `text()` (fidelity corpus test).
- ✅ **Live Preview** — caret-aware hiding of block markers + ``` ` ``` fences.
- ✅ **Embeddable `Editor` facade** + **domain-token extension API** (fg + chip bg).
- ✅ **Flat per-keystroke performance** — incremental persistent buffer +
  line-splice updates + viewport-bounded rasterization + per-line styling cache.
  A keystroke (including Enter/paste) is under one 60 fps frame at any note length
  (4000 lines ≈ 13 ms, was ~1.9 s). See [`docs/PERF.md`](docs/PERF.md).
- ✅ **Parser-driven styling** (0.3.0) — inline + block-level CommonMark via
  `pulldown-cmark` (setext, indented code, task lists, GFM tables, reference links,
  nested lists/quotes) and Typst blocks via the `typst-syntax` tree, with
  per-token syntax colors. Caret-independent `analyze()` + cheap `project()`.
- ✅ **Syntect** code highlighting (`syntax-highlight` feature); scrolling
  (wheel + scrollbar + caret-follow); `can_undo()`/`can_redo()`.

### Known limitations
- **Styling is decoupled from fidelity** (the buffer is the source of truth); the
  parser-driven marks/colors are cosmetic. HTML blocks are a remaining CommonMark
  nicety; thematic breaks aren't yet specially styled.
- **Rendered Typst** (math layout, figures) needs the Typst compiler — out of
  scope for the inline editor; hosts can keep a compiled preview.
- **No IME / accessibility** on the image surface yet; `Theme` has no font-family.

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
