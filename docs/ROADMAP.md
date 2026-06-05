# sred — Roadmap to 0.3.0 ("primary editor for Noet")

**Release target — sred 0.3.0:** an embeddable, byte-lossless, themeable inline
markdown editor that Noet can use as its **primary** editing surface (replacing
the raw-markdown `TextEdit`), with raw mode kept as a fallback.

Current: **0.1.0** (standalone WYSIWYG demo; structured model with reconstructive
save; 20 tests). The path to 0.3.0 is eight milestones. The keystone (M1) is an
architecture pivot to a *source-anchored* buffer — see `DESIGN.md` §2.

Legend: each milestone bumps the version and has a hard **acceptance gate**.

---

## M1 — Source-anchored core · `0.2.0` · keystone
Replace the structured `EditorCore` with a raw-markdown buffer (rope) as the
source of truth; the rich view becomes a parsed projection. Editing (typing,
toolbar, autoformat) splices the raw text — markers are real (`**…**`, `# `,
`- `). Inline styling + block styles are *derived* from source each edit.

**Acceptance**
- `set_text(x); text() == x` byte-for-byte for a corpus of hand-written markdown
  (headings, lists, nested lists, code fences, quotes, tables, raw HTML, mixed
  emphasis markers, CRLF, trailing whitespace, blank-line runs).
- A single edit changes only the touched bytes (diff-minimality test).
- Toolbar "Bold" on a selection yields `**…**` in `text()`; "H2" yields `## `.
- Existing visual features (caret, selection, strike/underline, undo/redo,
  autoformat, list-exit) still pass.

---

## M2 — Scrolling & viewport · `0.2.1`
Re-enable scrolling without breaking drag-select: caret-follow auto-scroll,
mouse-wheel, and viewport-bounded shaping/raster (only render the visible range
of long notes).

**Acceptance**
- A 5,000-line note scrolls by wheel and keeps the caret on-screen while typing.
- Drag-selection still works (no Flickable/selection conflict).
- Per-keystroke latency stays flat as document length grows (viewport-bounded).

---

## M3 — Theming & scale hooks · `0.2.2`
The editor takes its palette + font scale from the host instead of the hardcoded
`layout::Theme`: `fg/bg/accent/selection/code/link` colors, `scale: float`,
`dark: bool`, exposed as Slint properties read by the Rust renderer.

**Acceptance**
- Editor visually matches a host-supplied theme; flipping `dark` reflows colors.
- Changing `scale` (Noet's `Z.f`) rescales text and caret without layout drift.

---

## M4 — Embeddable component + Slint-version alignment · `0.2.3`
Extract a reusable `sred::Editor` binding that wires a `RichTextEditor` (no
`MenuBar`) to an `EditorCore` and exposes the §4 API. Pin the sred workspace to
**Slint 1.13** (Noet's version); move `MenuBar` to demo-only (in-window panel).
Publish a Slint library path so a host can `import { RichTextEditor } from "@sred"`.

**Acceptance**
- A minimal external Slint **1.13** app embeds `RichTextEditor`, sets/gets text,
  and receives `on_changed` — in < 30 lines of glue.
- `cargo build` of the whole workspace on Slint 1.13 is clean.

---

## M5 — Inline-token extension API · `0.2.4`
Host-registered `TokenSpec { id, matcher, style, clickable }`. Built-in markdown
emphasis/links route through the same decoration pipeline. Finalize
`insert_text` / `selected_text` / `selection`.

**Acceptance**
- Registering a `[[wikilink]]` token renders matches as colored chips; clicking
  emits `on_token_activated("wikilink", value, range)`.
- Built-in `**bold**` styling still works via the same pipeline.
- `selected_text()` returns the exact selected source substring.

---

## M6 — Block-widget hooks + todo affordance · `0.2.5`
Host-registered `BlockWidgetSpec` attaches an interactive widget to matching
lines; sred reserves the slot, draws/hit-tests it, and emits `on_block_action`.
Reference use: a checkbox on `TODO|DOING|DONE(kind) …` lines.

**Acceptance**
- A todo line shows a status checkbox; clicking emits `on_block_action(line,
  "cycle")`; the underlying source line is otherwise byte-unchanged.
- Non-todo lines are unaffected.

---

## M7 — Accessibility & headless-test parity · `0.2.6`
Add `accessible-role`/`accessible-label` to the editor surface; drive it via
`i-slint-backend-testing` **1.13** `ElementHandle`/`mock_single_click`/
`mock_elapsed_time`; port sred's GUI tests to that API and ship reusable test
helpers Noet can call.

**Acceptance**
- A headless test finds the editor via `ElementHandle`, types text, and asserts
  `text()` — under backend-testing 1.13, one-test-per-process model.

---

## M8 — Integration hardening in Noet (release gate) · `0.3.0`
Drop sred into Noet behind a `WYSIWYG (beta)` toggle next to `Preview`. Register
Noet's tokens (`[[ ]]`,`+[[ ]]`,`@`,`#`,url, todo) with Theme colors and route
clicks to `filter-entity`/`open-url`. Wire selection→Todo/Note, entity pickers
(`insert_text`), autosave (`changed`→`save_note`), live entity chips (recompute
from `text()`). Keep raw editor + image preview for `kind: typst`. Run the
fidelity suite against the **real notes corpus**; do a perf pass.

**Release criteria (all must hold)**
- Editing a sample of real notes round-trips **byte-for-byte** on save.
- All Noet domain tokens render and are clickable; todo checkboxes toggle.
- →Todo/→Note, entity pickers, autosave, and entity chips work through sred.
- No perceptible typing/scroll latency regression vs the raw `TextEdit`.
- Noet's headless GUI suite passes with sred embedded.
- sred is selectable as the **default** editor; raw mode remains a fallback.

---

## Sequencing & risk

- **M1 is the long pole** and unblocks everything else; do it first and in
  isolation (no UI churn) with the fidelity suite as its gate.
- M2–M3 are independent and can overlap once M1 lands.
- M4 must precede M5–M7 (they extend the embeddable surface).
- M8 is integration in the *Noet* repo and gates the release; expect a feedback
  loop back into M1 (fidelity edge cases) and M5 (token rendering).
- **Biggest risk:** fidelity edge cases on real notes (todo lines, nested lists,
  HTML, Typst regions). Mitigated by M1's corpus test + M8 keeping raw mode as a
  fallback so the release is never all-or-nothing.
