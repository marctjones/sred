# sred — Design (targeting use as Noet's primary editor)

Status: design for **sred 0.2.0**. Supersedes the standalone-editor assumptions
in the top-level `README.md` where they conflict.

## 1. Goal

Make sred's `RichTextEditor` usable as the **primary editing surface** in
[Noet](../../notes) (`~/Projects/notes`), a Rust + Slint notes app that stores
notes as **plain markdown files with YAML frontmatter**. Today Noet edits notes
in a raw-markdown `TextEdit` with a side preview; sred should replace that raw
editor with an inline, richly-styled, *byte-lossless* editor.

## 2. The constraint that drives the whole design: byte-lossless round-trips

Noet's source of truth is the raw note body, saved **verbatim**:

- `read_note()` reads the body as-is; `write_note()` writes it back unchanged
  (`notes/crates/core/src/backend/vault.rs`).
- The editor binds `TextEdit.text <=> current-body`; autosave calls
  `save_note(id, title, body)` with the raw string — no normalization
  (`notes/crates/gui/src/main.rs`).
- Notes contain hand-authored markdown plus domain syntax sred doesn't model:
  `[[wikilinks]]`, `+[[project]]`, `@[[Person]]`/`@name`, `#tag`, URLs, and
  strict **todo lines** (`TODO(kind) … @[[P]] +[[Proj]] due:YYYY-MM-DD [#A]
  repeat:1w jira:KEY`) with a canonical serializer `format_todo_line()`.

**sred today cannot do this.** Its `EditorCore` keeps a *structured* model
(`Vec<EditBlock>` + per-char attributes + a block-kind vector) and **reconstructs
markdown on save**. Reconstruction normalizes: emphasis marker choice (`*`/`_`),
list markers (`-`/`*`/`+`), blank-line counts, indentation, trailing whitespace,
setext vs ATX headings, raw HTML, and every domain token would degrade to plain
text. Saving a real note would silently rewrite the user's file.

Guaranteeing byte-identical reconstruction from a structured model is, for
arbitrary hand-written markdown, effectively impossible. Therefore:

> **Key decision: sred pivots from a "structured model + reconstructive save"
> editor to a *source-anchored* editor — the raw markdown text IS the buffer, and
> the rich view is a decorated projection of it.**

This is the model used by Obsidian Live Preview / CodeMirror-markdown: the
document is the markdown string; parsing produces *decorations* and *inline
widgets* layered over the source; editing splices the raw string. Unedited
regions stay byte-identical by construction, and edits produce minimal diffs.

## 3. Architecture (0.2 target)

```
            ┌──────────────────────────────────────────────────────────┐
 Host (Noet)│  registers token kinds, theme, block-widgets;            │
            │  reads text(); handles changed / token_activated /       │
            │  block_action; owns frontmatter, autosave, todo encode   │
            └───────────────▲───────────────────────┬──────────────────┘
                            │  sred::Editor (binding) │
        ┌───────────────────┴─────────────────────────▼───────────────┐
        │ sred-slint   RichTextEditor.slint  (no MenuBar; themable)     │
        │              + Editor binding (set_text/text/insert/select/   │
        │              on_changed/on_token_activated/on_block_action)   │
        └───────────────────────────▲──────────────────────────────────┘
        ┌───────────────────────────┴──────────────────────────────────┐
        │ sred-core (UI-free)                                            │
        │  ┌───────────────┐  ┌──────────────────┐  ┌────────────────┐  │
        │  │ Source buffer │  │ View builder     │  │ Layout/raster  │  │
        │  │ = raw md text │─▶│ parse → decoration│─▶│ (cosmic-text)  │  │
        │  │ (rope)        │  │ + inline widgets  │  │ + widget slots │  │
        │  └───────────────┘  │ + extension tokens│  └────────────────┘  │
        │   edits splice raw  └──────────────────┘                       │
        └───────────────────────────────────────────────────────────────┘
```

### 3.1 Editing substrate: lean on cosmic-text's `Edit`/`Editor`

We already depend on cosmic-text for shaping/layout/raster. It *also* ships a
full editing layer (`Edit` trait, `Editor`, `ViEditor`, `SyntaxEditor`) that
owns a `Buffer` and provides cursor, selection, motions (`Action`), scroll,
clipboard copy/delete, IME, "draw with cursor + selection," and undo/redo
(`ViEditor` via `cosmic-undo-2`). **Rather than reinvent those, the editing
substrate is cosmic-text's `Editor`** — less code, better-tested
motions/bidi/IME, scroll-to-cursor for free.

- **The buffer is cosmic-text's `Buffer`** (owned by an `Editor`) — *not* a
  custom `ropey` rope. The buffer text *is* the raw markdown.
- **Byte-lossless, proven.** cosmic-text tracks a per-line `LineEnding`
  (Lf/CrLf/Cr/…). `text()` reconstructs from `lines + endings`; an empirical
  spike (`tests/cosmic_fidelity_spike.rs`) confirms byte-for-byte round-trips
  across the corpus (CRLF, trailing whitespace, blank runs, unicode, todo lines,
  code fences). The one edge — an empty buffer reads back as `"\n"` — is a
  one-line guard. `tests/fidelity.rs` stays the gate.
- **All edits go through `Editor` actions** (insert/delete/motion/selection),
  which splice the buffer text. Toolbar source-transforms ("Bold" → wrap with
  `**…**`, "H1" → insert `# `) are expressed as insert/delete actions, so the
  markers are really in the text.
- This **supersedes M1's interim `ropey` buffer** (a correct stepping stone that
  proved the source-anchored model). M1's conceptual win — *source-anchored,
  byte-lossless* — stands; only the storage moves to cosmic-text.

> **What stays ours (and always will):** cosmic-text knows nothing about
> markdown. The *markdown → visual styling* (headings, bold, chips), the
> *decorations* (strike/underline), the *extension tokens* (wikilinks/tags), the
> *block widgets* (todo checkbox), and *byte-exact text in/out* are sred's job —
> re-derived from the source and pushed onto the buffer as per-line attrs after
> each edit. That's the part that makes sred a *markdown* editor, not a plain box.

### 3.1a Strip-and-replace audit (stop maintaining our own)

Concrete map of what we delete and what `cosmic-text` provides instead. The rule:
**anything that is "generic text editor plumbing" is cosmic-text's; only
markdown-specific behavior is ours.**

| Our code (today) | Replace with cosmic-text | Verb |
|---|---|---|
| `ropey::Rope` buffer + `insert`/`delete` primitives | `Editor`-owned `Buffer` + `Action::Insert/Backspace/Delete`, `insert_string` | **strip** |
| cursor/anchor state, `set_cursor`/`extend_to`/`select_word_at`, `selection_range` | `Editor` cursor + `Selection` + `Action::Click/DoubleClick/Drag{x,y}` | **strip** |
| motions: `motion_target`, `word_range`, `Motion::LineStart/End`, `layout::vertical()` up/down hack | `Action::Motion(cosmic Motion::Left/Right/Up/Down/Home/End/LeftWord/RightWord/…)` (bidi + word + vertical correct) | **strip** |
| pointer hit-testing: `layout::hit`, bridge `hit()` | `Action::Click/Drag{x,y}` (Editor maps px→cursor) | **strip** |
| scrolling (the reverted manual viewport/`follow_scroll`) | `Action::Scroll{lines}` + `Buffer::shape_until_cursor` (scroll-to-cursor) | **strip** |
| flat↔cursor mapping (`linecol`/`flat_of`/`flat_to_render_cursor`/`render_cursor_to_flat`) + `prefix_bytes` | obsolete — source-anchored has no injected prefixes; the cursor *is* a cosmic `Cursor` | **strip** |
| manual selection-highlight fill + `caret_geom` drawing | `Editor::draw(text, cursor, selection, selected_text colors, F)` draws glyphs + cursor + selection in one pass | **strip** |
| undo/redo full-text `Snapshot` vecs | `Editor::start_change`/`finish_change` → `Change` (delta) stack | **strip** |
| `text()` = `rope.to_string()` | reconstruct from `Buffer.lines` (`text()` + `ending().as_str()`), empty guard | **adapt** |
| clipboard editing half (`selected_text`, delete) | `Editor::copy_selection()` / `delete_selection()` (OS transport still `arboard`) | **adapt** |
| `view.rs` markdown scanner → `Vec<Span>` for `set_rich_text` | keep the scanner, but emit **per-line `AttrsList`** applied via `BufferLine::set_attrs_list` (restyle without changing text) | **keep, adapt** |
| decoration draw (strike/underline) via `LayoutRun::highlight` | keep (cosmic draws neither) | **keep** |
| source transforms (bold→`**…**`, headings/list markers, link wrap, Enter list-continue/exit) | keep — markdown semantics, expressed via `Editor` insert/delete actions | **keep** |
| extension tokens / block widgets (todo checkbox, chips) | keep — ours (future) | **keep** |
| rasterize to `slint::Image` + Slint component/bridge | keep — Slint can't consume an `Editor` | **keep** |

Net: `editor.rs` collapses from a ~640-line hand-rolled engine to a thin wrapper
(`Editor` + a `Change` undo stack + our source-transform helpers); `layout.rs`
loses the flat-mapping (~80 lines) and the manual selection/caret raster (~40
lines). We stop maintaining cursor/selection/motion/scroll/undo entirely.

### 3.2 View builder (parse → decorations)
- An **incremental block + inline scanner** (reuse Noet's lightweight block
  rules where sensible; do *not* pull in a heavyweight AST) maps source spans to:
  - **line styles** — heading size, blockquote bar, list marker, code-block mono;
  - **inline decorations** — bold/italic/code/strike ranges (from `**`, `*`,
    `` ` ``, `~~`), link ranges;
  - **extension tokens** — host-registered matchers (see §4) producing colored
    "chips" with a `kind`/`value` and a click target;
  - **block widgets** — host-registered line matchers producing an interactive
    widget slot (e.g. a checkbox on todo lines).
- Decorations never rewrite text; they are an overlay keyed by source range.
- **Marker visibility** is staged: 0.2 ships *visible markers, richly styled*
  ("live-preview-lite" — `# `, `**` stay but headings are big and bold text is
  bold, entities are chips). Hidden-marker reveal-on-caret is post-0.2 (§7).

### 3.3 Layout / raster
- cosmic-text shapes the source text with per-run `Attrs` derived from
  decorations (size, weight, style, color, mono). Strike/underline and chip
  backgrounds are drawn by the rasterizer (as today). Block-widget slots reserve
  space and are drawn/hit-tested as overlays.
- Caret, selection highlight, and hit-testing already come from cosmic-text
  (`sred-core/src/layout.rs`); they keep working against the raw text offsets.

## 4. Extension API (what sred exposes so Noet can adapt it)

The component is configured by the host; sred ships **no** domain knowledge.

### 4.1 Content & editing
- `set_text(&str)` / `text() -> String` — byte-lossless.
- `insert_text(&str)` — insert at caret (entity pickers, format buttons).
- `selected_text() -> String`, `selection() -> Option<(usize,usize)>` — for
  →Todo/→Note.
- `on_changed(cb)` — fires after any edit (drives autosave + entity recompute).
- Built-in editing commands (bold/italic/headings/lists/quote/code/undo/redo/
  clipboard) operate on the source and remain available.

### 4.2 Inline token extensions
Host registers token kinds:
```
TokenSpec { id: &str,
            matcher: Regex | fn(&str)->ranges,
            style: { fg, bg, underline, bold },
            clickable: bool }
```
- sred renders matches as styled chips inline with the source.
- Clicking emits `on_token_activated(id, value, range)`.
- Noet registers: `wikilink [[…]]`, `project +[[…]]`, `person @…/@[[…]]`,
  `tag #…`, `url http(s)://…` with its Theme tint/ink colors, and routes
  `on_token_activated` to its `filter-entity` / `open-url`.

### 4.3 Block-widget extensions
Host registers line matchers that attach an interactive widget:
```
BlockWidgetSpec { id, line_matcher: fn(&str)->Option<State>, draw, on_action }
```
- Noet registers the **todo** widget: matches `TODO|DOING|DONE(kind) …`, draws a
  status checkbox, and `on_block_action(line, "cycle")` lets Noet rewrite the
  line via its own `format_todo_line()` (sred never edits the todo syntax).

### 4.4 Theming & scale
- Editor accepts a host theme as Slint properties: `fg, bg, accent, selection,
  code, link` colors + a `scale: float` (Noet's `Z.f`) + `dark: bool`.
- The Rust renderer reads these instead of the hardcoded `layout::Theme`.

### 4.5 Accessibility / testing
- The editor surface exposes `accessible-role`/`accessible-label` so Noet's
  headless `i-slint-backend-testing` `ElementHandle` tests can find and drive it.

## 5. Division of responsibility

| Capability | Owner |
|---|---|
| Byte-lossless source buffer & `text()` | **sred** |
| Inline mark styling (bold/italic/code/strike/headings/lists/quote/code) | **sred** |
| Caret/selection/hit-test/scrolling | **sred** |
| Inline-token *mechanism* (match→style→click) | **sred** |
| Block-widget *mechanism* | **sred** |
| Theming/scale *inputs* | **sred** (host supplies values) |
| Embeddable component + Slint-version compat | **sred** |
| Headless-test hooks (a11y) | **sred** |
| Registering `[[ ]]`/`+[[ ]]`/`@`/`#`/url tokens + colors + click→filter | **Noet** |
| Todo line semantics + `format_todo_line()` serialization | **Noet** |
| →Todo / →Note extraction (uses `selected_text`) | **Noet** |
| Entity pickers (use `insert_text`) | **Noet** |
| Autosave debounce + `save_note(id,title,body)` + frontmatter | **Noet** |
| Entity chip strip (recompute from `text()` on `changed`) | **Noet** |
| Typst notes: keep raw editor + compiled-image preview | **Noet** |
| Live entity index / SQLite / search | **Noet** |

## 6. Compatibility notes

- **Slint version.** Noet is on **1.13**; sred is on **1.16** and the demo uses
  `MenuBar` (1.16-only). The *reusable* component must build on Noet's version,
  so 0.2 pins the sred workspace to **Slint 1.13** and moves `MenuBar` out of the
  reusable component (the demo uses an in-window panel, like Noet does). Keep the
  cosmic-text `set_rich_text` 5-arg call (that's a cosmic-text API, version-
  independent of Slint).
- **Typst.** sred cannot WYSIWYG arbitrary Typst. For `kind: typst` notes Noet
  keeps its raw editor + compiled-image preview; sred treats any Typst content it
  is given as opaque passthrough.
- **Color marks** (current sred view-only feature) have no markdown
  representation; under the source-anchored model they become ephemeral (not
  saved) or are dropped. Not required by Noet.

## 7. Beyond 0.2 (explicitly out of scope for the release gate)

- Hidden-marker "true" Live Preview (reveal `**`/`#` only when the caret is on
  the construct).
- Tables as an interactive grid; images as inline objects.
- Dirty-region incremental raster beyond the viewport optimization in M2.

See `ROADMAP.md` for milestones and `IMPLEMENTATION_PLAN.md` for the task
breakdown.
