# sred — Implementation Plan (0.1.0 → 0.2.0)

Task-level breakdown per milestone from `ROADMAP.md`. Each task notes the files
touched and the tests that gate it. "core" = `sred-core`, "slint" = `sred-slint`,
"noet" = `~/Projects/notes`.

---

## M1 — Source-anchored core 

The pivot. Build it behind the existing `EditorCore` public surface so the Slint
layer barely changes (`styled_runs`, `apply`, `text`, `selection` keep their
shapes; their *implementation* changes).

1. **Buffer.** Replace `text: Vec<char>` + `attrs` + `kinds` with a raw markdown
   buffer (`ropey::Rope`) + caret/selection as char offsets. Delete the
   `BlockKind`/`CharAttr`/`Snapshot`-of-structure machinery. *(core/src/editor.rs)*
2. **Edit primitives.** `insert`, `delete`, `delete_selection` splice the rope.
   Undo/redo snapshots the rope + caret (cheap with rope clones / or an op-log).
3. **Source-editing commands.** Re-express every command as a source transform:
   - bold/italic/code/strike → wrap selection with `**`/`*`/`` ` ``/`~~`
     (toggle = detect + unwrap if already wrapped);
   - headings/lists/quote/code-block → insert/remove the line-leading marker;
   - autoformat (`# `, `- `, `1. `, `> `, `= `) → already inserts real markers;
     keep, but now it's the *only* representation;
   - list-exit on empty item → delete the marker run.
   *(core/src/editor.rs)*
4. **View builder.** New module `core/src/view.rs`: scan the buffer → `Vec<Span>`
   (text + derived marks + size + color) **plus** a map from screen position to
   source offset. Reuse the block rules; derive inline marks from `**`,`*`,`` ` ``,
   `~~`, `[..](..)`. Markers stay visible (styled). *(replaces the old
   `styled_runs` body; keep the signature)*
5. **Layout/raster.** `layout.rs` is mostly unchanged (it already consumes
   `Span` + prefix bytes); adapt prefix handling now that markers are inline.
6. **Fidelity test suite** *(core)*: a `tests/fidelity.rs` with a corpus of real-
   world markdown (incl. todo lines, nested lists, fenced code, HTML, CRLF,
   trailing spaces, mixed emphasis). Assert `set_text(x); text()==x`; assert a
   scripted edit touches only expected bytes; property-test random edits never
   corrupt untouched lines.

**Gate:** fidelity suite green; existing 12 core tests ported/green.

---

## M2 — Scrolling & viewport 

1. **Viewport state** in the bridge: scroll-y, visible height. *(slint)*
2. **cosmic-text viewport**: pass scroll + visible height to the buffer; only
   shape/raster the visible line range; pad scrollbar via total height.
   *(core/src/layout.rs — use `Buffer::set_scroll` / shape-until-scroll bounds)*
3. **Caret-follow**: after an edit/caret move, adjust scroll-y so the caret rect
   is within the viewport. *(slint bridge + a `caret_rect` already returned)*
4. **Wheel + drag coexistence**: keep the `Flickable` non-interactive for
   selection; handle wheel via a `scroll-event` callback that adjusts scroll-y.
   *(slint ui/sred.slint + bridge)*

**Gate:** 5k-line note scroll test (headless: set text, move caret to end, assert
caret rect within viewport); latency micro-bench flat vs length.

---

## M3 — Theming & scale hooks 

1. **Theme inputs** on `RichTextEditor`: `fg,bg,accent,selection,code,link` as
   `color` props, `scale: float`, `dark: bool`. *(slint ui)*
2. **Bridge → renderer**: build `layout::Theme` from those props each refresh
   (replace `Theme::default()` call sites). *(slint/src/lib.rs)*
3. **Scale**: multiply font-size/line-height/margins by `scale`. *(core/layout)*

**Gate:** headless test sets theme props + scale, asserts rendered frame differs
and caret height tracks scale.

---

## M4 — Embeddable component + version alignment 

1. **Version pin**: workspace `slint = "1.13"`, `slint-build = "1.13"`,
   `i-slint-backend-testing = "1.13"`; fix any 1.16-only usage. *(Cargo.toml)*
2. **De-MenuBar the component**: the reusable `RichTextEditor` has no `MenuBar`;
   move the demo's menu into an in-window panel (mirroring Noet's `MenuPanel`).
   *(slint ui)*
3. **`sred::Editor` binding** *(new slint/src/editor.rs)*: a struct that owns the
   `Controller` and exposes the §4 API as plain Rust methods + setters for
   callbacks (`on_changed`, `on_token_activated`, `on_block_action`). Refactor
   `build_window` to use it.
4. **Slint library export**: document/configure the library path so a host does
   `import { RichTextEditor } from "@sred"` (slint-build `library_paths`).
5. **Embedding example** *(new sred-demo or examples/embed)*: a ~30-line external
   window embedding the component.

**Gate:** embedding example builds & round-trips text on Slint 1.13; workspace
clean.

---

## M5 — Inline-token extension API 

1. **`TokenSpec`** in core: `{ id, matcher: Matcher, style: TokenStyle,
   clickable }`, `Matcher = Regex(String) | Fn`. A registry on `EditorCore`.
   *(core)* — avoid a hard `regex` dep in core if possible; accept precompiled
   match ranges from the host, or feature-gate `regex`.
2. **View builder** consumes the registry: emit chip spans (bg + ink + range +
   token id/value) alongside markdown-derived marks. *(core/view.rs, layout.rs
   chip backgrounds — reuse selection-rect fill)*
3. **Hit-testing** maps a click on a chip → `(id, value, range)`; bridge emits
   `on_token_activated`. *(core hit + slint callback)*
4. **Finalize** `insert_text`, `selected_text`, `selection` on the binding.

**Gate:** headless test registers a `[[…]]` token, sets text with one, asserts a
chip span exists and a simulated click reports the value; `**bold**` still styles.

---

## M6 — Block-widget hooks + todo affordance 

1. **`BlockWidgetSpec`** `{ id, line_matcher: Fn(&str)->Option<State>, on_action }`
   registry. *(core)*
2. **Slot reservation**: lines with a widget reserve leading space; the renderer
   leaves a gutter; the bridge places a small Slint overlay (checkbox) there and
   forwards clicks to `on_block_action(line_no, action)`. *(core layout gutter +
   slint overlay)*
3. **Reference todo widget** wired in the demo to prove the hook.

**Gate:** headless test with a todo-line matcher asserts a widget slot at the
right line and that clicking emits `on_block_action` with the line number;
source bytes unchanged.

---

## M7 — Accessibility & headless-test parity 

1. **A11y**: set `accessible-role: text-editor`/`accessible-label` on the editor
   surface. *(slint ui)*
2. **Port GUI tests** to `i-slint-backend-testing` 1.13 (`ElementHandle`,
   `find_by_*`, `mock_single_click`, `mock_elapsed_time`, `init_no_event_loop`).
3. **Test helpers** crate-public so Noet can reuse (e.g. `type_into_editor`,
   `editor_text`).

**Gate:** a headless test locates the editor by `ElementHandle`, types via
simulated input, asserts `text()`.

---

## M8 — Integration hardening in Noet (`0.2.0`, release gate) — work in `notes`

1. **Depend on sred**: add `sred-core`/`sred-slint` to `notes` workspace; import
   `RichTextEditor` into `app.slint`.
2. **Beta toggle**: a `WYSIWYG (beta)` switch beside `Preview`; when on, the edit
   pane renders `RichTextEditor` bound to `current-body`; when off, the existing
   `TextEdit`. *(notes/crates/gui/ui/app.slint)*
3. **Register tokens** with Noet's regexes + Theme tint/ink:
   `wikilink/project/person/tag/url`; route `on_token_activated` →
   `filter-entity`/`open-url`. *(notes/crates/gui/src/main.rs)*
4. **Todo block widget**: register the `TODO|DOING|DONE(kind)` matcher; on cycle,
   rewrite the line with Noet's `format_todo_line()`. *(noet)*
5. **Wire editing flows**: `on_changed` → existing autosave debounce +
   `save_note`; entity chips recompute via `note_entities(text())`; format
   buttons + entity pickers use `insert_text`; →Todo/→Note use `selected_text`.
6. **Typst**: if `effective_kind == typst`, keep raw `TextEdit` + image preview
   (don't mount sred). *(noet)*
7. **Fidelity gate**: a Noet test that loads every note in a fixture vault,
   mounts sred, reads `text()`, and asserts equality with the file bytes.
8. **Perf pass**: dirty-region raster if the viewport bound from M2 isn't enough.
9. **Promote**: make sred the default once the release criteria in `ROADMAP.md`
   M8 hold; keep raw mode as fallback.

**Gate:** the `ROADMAP.md` M8 release criteria.

---

## Cross-cutting

- Keep `sred-core` UI-free and Slint-version-independent (it already is) — only
  `sred-slint` pins to 1.13.
- Every milestone keeps `cargo test` green and the `sred-demo` runnable as the
  manual proving ground.
- Update top-level `README.md` status table as milestones land.
