# sred — Roadmap to 0.2.0 ("primary editor for Noet")

**Release target — sred 0.2.0:** an embeddable, byte-lossless, themeable inline
markdown editor that Noet can use as its **primary** editing surface (replacing
the raw-markdown `TextEdit`), with raw mode kept as a fallback.

Current: **0.1.0** (standalone WYSIWYG demo; structured model with reconstructive
save; 20 tests). The path to 0.2.0 is eight internal milestones (M1–M8); only the
final integration gate (M8) ships as the **0.2.0** release. The keystone (M1) is
an architecture pivot to a *source-anchored* buffer — see `DESIGN.md` §2.

Legend: M1–M7 are development milestones (no public version); each has a hard
**acceptance gate**. M8 is the 0.2.0 release gate. Intermediate builds may
optionally be cut as `0.2.0-alpha.N`.

---

## M1 — Source-anchored core · keystone
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

## M2 — Adopt cosmic-text's `Editor` substrate (brings scrolling for free)
Reshaped after the cosmic-text investigation (see `DESIGN.md` §3.1): instead of
hand-rolling a viewport/scroll, **replace the editing engine with cosmic-text's
`Editor`** (cursor, selection, motions via `Action`, scroll/scroll-to-cursor,
clipboard, IME, `draw`-with-selection). Reconstruct byte-exact text from
`lines + endings` (proven lossless by `tests/cosmic_fidelity_spike.rs`, empty-
buffer guard). Keep undo via plain `Editor` + a `Change` stack (no `ViEditor`
vim modes). sred's `view.rs` re-applies markdown styling as per-line attrs after
each edit. This *subsumes* the old "scrolling & viewport" milestone and deletes
the hand-rolled cursor/selection/motion/scroll code.

**Acceptance**
- `tests/fidelity.rs` stays green through the substrate swap (byte-lossless).
- Scrolling (wheel + caret-follow) works on a 5,000-line note; latency bounded.
- Drag-selection + word/line motions come from `Editor` and still pass the GUI
  test.

---

## M3 — Theming & scale hooks
The editor takes its palette + font scale from the host instead of the hardcoded
`layout::Theme`: `fg/bg/accent/selection/code/link` colors, `scale: float`,
`dark: bool`, exposed as Slint properties read by the Rust renderer.

**Acceptance**
- Editor visually matches a host-supplied theme; flipping `dark` reflows colors.
- Changing `scale` (Noet's `Z.f`) rescales text and caret without layout drift.

---

## M4 — Embeddable component + Slint-version alignment
Extract a reusable `sred::Editor` binding that wires a `RichTextEditor` (no
`MenuBar`) to an `EditorCore` and exposes the §4 API. Pin the sred workspace to
**Slint 1.13** (Noet's version); move `MenuBar` to demo-only (in-window panel).
Publish a Slint library path so a host can `import { RichTextEditor } from "@sred"`.

**Acceptance**
- A minimal external Slint **1.13** app embeds `RichTextEditor`, sets/gets text,
  and receives `on_changed` — in < 30 lines of glue.
- `cargo build` of the whole workspace on Slint 1.13 is clean.

---

## M5 — Inline-token extension API
Host-registered `TokenSpec { id, matcher, style, clickable }`. Built-in markdown
emphasis/links route through the same decoration pipeline. Finalize
`insert_text` / `selected_text` / `selection`.

**Acceptance**
- Registering a `[[wikilink]]` token renders matches as colored chips; clicking
  emits `on_token_activated("wikilink", value, range)`.
- Built-in `**bold**` styling still works via the same pipeline.
- `selected_text()` returns the exact selected source substring.

---

## M6 — Block-widget hooks + todo affordance
Host-registered `BlockWidgetSpec` attaches an interactive widget to matching
lines; sred reserves the slot, draws/hit-tests it, and emits `on_block_action`.
Reference use: a checkbox on `TODO|DOING|DONE(kind) …` lines.

**Acceptance**
- A todo line shows a status checkbox; clicking emits `on_block_action(line,
  "cycle")`; the underlying source line is otherwise byte-unchanged.
- Non-todo lines are unaffected.

---

## M7 — Accessibility & headless-test parity
Add `accessible-role`/`accessible-label` to the editor surface; drive it via
`i-slint-backend-testing` **1.13** `ElementHandle`/`mock_single_click`/
`mock_elapsed_time`; port sred's GUI tests to that API and ship reusable test
helpers Noet can call.

**Acceptance**
- A headless test finds the editor via `ElementHandle`, types text, and asserts
  `text()` — under backend-testing 1.13, one-test-per-process model.

---

## M8 — Integration hardening in Noet (release gate) · 0.2.0
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

---

## Post-0.2.0 — full format support (parser-driven)

> **Phase 2 (block-level) is DONE** (0.3.0-alpha.2). The dedicated plan it was
> built from is [`docs/MF_PHASE2.md`](MF_PHASE2.md) — architecture (doc-level
> analysis + caret-aware projection), invariants, and the step-by-step plan.


0.2.0 ships a **pragmatic, hand-rolled** recognizer: common Markdown constructs
and (as of v0.2.0-alpha.6) **Level-1 Typst markup** live-preview (headings,
strong/emph, raw, math, lists). The path to *spec-complete* support is to stop
hand-maintaining the grammars and instead **drive styling from real parsers**,
which already expose **source spans** — exactly what the source-anchored
live-preview needs (map spans → inline styling + marker hiding). This matches the
project rule of leveraging existing libraries rather than duplicating them.

### MF1 — Full CommonMark via `pulldown-cmark`
- **Inline: DONE** (0.3.0-alpha). `line_marks_md` now parses each line with
  `pulldown-cmark` (+ GFM strikethrough) and maps Strong/Emphasis/Code/
  Strikethrough/Link spans — spec-correct nesting, delimiter matching, code-span
  protection, links. Per-line projection/delta/marker-hiding machinery unchanged.
- **Block: DONE** (0.3.0-alpha.2). A single whole-document `pulldown-cmark`
  `into_offset_iter()` pass (`scan_md`) drives setext headings, indented code
  (lazy-continuation-aware), task lists, GFM tables, reference links, and nested
  lists/quotes. Marker hiding + exact deltas via `Marker { start, len, repl }`;
  styling split into caret-independent `analyze()` + caret-dependent `project()`,
  with `ANALYSIS_CACHE` (by text) + `PROJECT_CACHE` (by per-line digest). See
  `tests/commonmark.rs`. (HTML blocks remain a future nicety.)

### MF2 — Full Typst via `typst-syntax`
- **Inline: DONE** (0.3.0-alpha). `line_marks_typst` walks `typst-syntax`'s
  `LinkedNode` tree using the crate's own `highlight()`/`Tag` categorizer —
  strong/emph/raw, math, refs/labels, and `#`-code-mode tokens. Supersedes the
  Level-1 hand-rolled recognizer.
- **Block + color: DONE** (0.3.0-alpha.2). `scan_typst` reads heading depth and
  list/enum/term markers from the `typst-syntax` tree (marker byte ranges → exact
  deltas, indentation preserved). Per-token *colors* (keyword/function/number/
  string/comment/operator) flow through a `SynCat` channel resolved at projection
  time via a host `SynPalette` (`Theme` syntax palette), alongside `MarkSet`. See
  `tests/typst_blocks.rs`.

### MF3 — Rendered fragments (math / figures)
- The hard part text-styling can't do: typeset `$…$` math and `#figure` output.
- Realistic approach: compile fragments with the Typst engine and inline them as
  images (or keep a compiled side-preview). True inline interleaving of rendered
  fragments with cosmic-text layout is a large, separate effort.

---

## Future file formats

**World 1 — plaintext markup (natural fit).** Anything that's "UTF-8 + inline
markup" drops into the source-anchored model as a new parser-driven recognizer
(the MF1/MF2 pattern), byte-lossless and fast: **reStructuredText**, **AsciiDoc**,
**Org-mode**, **MediaWiki/Wikitext**, **Textile**, **LaTeX**. Prioritize by demand.

**World 2 — binary rich documents (DOCX, ODT/ODP) — a separate track.** These are
ZIP+XML containers, *not* plaintext, so they break the source-anchored model: they
need a structured document model with import/export and **cannot be byte-lossless**
(round-tripping arbitrary DOCX losslessly is the hard part of Word/LibreOffice
interop). Recommended path if pursued:
1. **Import/export conversion first** (DOCX ↔ Markdown, pandoc-style) — lowest cost,
   reuses the existing editor; lossy but useful.
2. **Native structured editing** only if there's real demand — a second
   architecture alongside the source-anchored core.
3. **Comments before redlining.** Anchored comments (`word/comments.xml` ranges)
   map onto the existing decoration/annotation system. Full tracked-changes
   (`<w:ins>`/`<w:del>` accept/reject with author/date) is a much larger model
   commitment — defer unless collaboration is a headline requirement.
Rust building blocks: `zip` + `quick-xml`; `docx-rs` (partial DOCX r/w); ODF
support in Rust is currently sparse.
