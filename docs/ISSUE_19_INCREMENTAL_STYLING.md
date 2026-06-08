# Issue #19 — Incremental styling (cold-start, executable plan)

**Trigger:** start a fresh session and say **"start #19"** (or "do incremental
styling"). This file is written to be executed from scratch — read it first.

**Status:** deferred from the v0.7.x session (sred is at **v0.7.5**, clean/stable,
all other issues closed). This is the one remaining open issue and the one
*architectural*, correctness-critical change, so it gets its own clean session.

GitHub: <https://github.com/marctjones/sred/issues/19>.

---

## 1. The problem (measured, not assumed)

`view::styled_runs` rebuilds spans for the **entire document on every render** and
returns a **flat owned `Vec<Span>`**. Even when nothing changed, returning owned
spans forces O(n) string clones, and `layout::build_buffer_cached` re-hashes all of
them (`split_lines`) to diff. So per-render cost is O(document) regardless of how
little changed.

Measurements at 4000 lines (this machine, under load — numbers are relative):
- A content **edit**: `analyze` ~11 ms (after the v0.7.1 sparse-digest fix) + the
  O(n) span work; total keystroke ~16 ms.
- A **caret move / scroll** (analyze cache hit): a ~5–7 ms "floor" that is purely
  the whole-document span production + per-line project-cache clones + layout
  re-hash. **This floor scales with total line count** — the target of #19.

Reproduce the split:
```
cargo test -p sred-core --release --test perf_probe \
  edit_vs_caretmove_attribution -- --ignored --nocapture
SRED_PERF=1 cargo test -p sred-core --release --test perf_probe \
  stage_breakdown_4000 -- --ignored --nocapture   # prints analyze parse/build split
```
(Both probes already exist in `sred-core/tests/perf_probe.rs`; `SRED_PERF` gates an
`analyze` parse-vs-build breakdown in `view.rs::analyze`.)

Feel is **already fine** (edits at one frame@4000). #19 is **scaling headroom /
CPU-battery** for much larger docs — not a perceived-speed fix.

## 2. Why it was deferred (read before deciding to do it)

- It changes the **`styled_runs` ↔ `layout` contract** — the caret/hit/**byte-delta**
  mapping path, the invariant the whole editor rests on.
- It is **not unambiguously a code-quality win**: per-line `Rc` indirection is more
  complex than a flat `Vec<Span>`. (The maintainer's standing bar is "implement
  only if unambiguously better in *speed and code quality*." #19 passes speed,
  is debatable on quality. **Decide this explicitly before starting.**)
- A cheaper **doc-level memo** alternative only helps no-change repaints (scroll)
  and is marginal, because the owned-`Vec<Span>` return still clones O(n) strings.
  There is no flat win without the contract change.

## 3. The design (the real fix)

Make per-render styling cost O(changed lines), not O(document):

1. **Per-line span groups, reference-counted.** Represent a line's spans as
   `Rc<[Span]>` (or `Rc<LineSpans { spans: Vec<Span>, delta: i32 }>`). `PROJECT_CACHE`
   stores the `Rc`; a cache hit is an `Rc::clone` (O(1), no string allocation).
2. **`styled_runs` returns per-line groups**, e.g. `Vec<Rc<LineSpans>>` (+ the
   `deltas` are inside the groups or a parallel cheap `Vec<i32>`), instead of the
   flat `Vec<Span>`. This is a **public API change** — plan the migration:
   - Option A: change the return type and bump to **0.8.0** (semver-major-ish for a
     0.x). Update `api.rs` + the ~dozen test call sites.
   - Option B: keep `styled_runs` returning flat (for compatibility) and add
     `styled_lines() -> Vec<Rc<LineSpans>>`, routing the renderer through the new
     one. Less churn, two paths to maintain.
   Prefer A for cleanliness if doing it at all.
3. **Layout consumes per-line groups with identity-based diff.** In
   `layout::build_buffer_cached`, replace `split_lines`' span re-hashing with the
   `Rc` pointer (or the line digest already in `LineInfo`) as each line's change
   signature → O(changed lines) to detect + splice. `build_buffer` /
   `render` / `render_viewport` iterate groups instead of a flat slice.
4. Keep `analyze`/`project` exactly as-is (they're already incremental + cached);
   this is purely about not *rebuilding/cloning/​re-hashing* the assembled spans.

## 4. Files

- `sred-core/src/view.rs` — `LineSpans` type, `PROJECT_CACHE` value → `Rc`,
  `styled_runs`/`styled_runs_with` return type, `project_cached`.
- `sred-core/src/layout.rs` — `build_buffer`, `build_buffer_cached`, `split_lines`
  (the `LineParts`/`sig` machinery), and the span iteration in `render` /
  `render_viewport`. **The geometry (caret/hit/delta) must stay byte-identical.**
- `sred-core/src/api.rs` — `styled_with` and the render paths.
- Tests + the `*_matches_fresh` / `incremental_matches_full_render` guards.

## 5. Acceptance gates (all must stay GREEN and byte-identical)

- `incremental_matches_full_render` (layout) — the persistent buffer must equal a
  full rebuild. **This is the primary safety net for the contract change.**
- `styling_cache_matches_fresh_computation` (lib) — cached == fresh, byte-identical
  spans + deltas + decorations.
- `tests/fidelity.rs` — `text()` round-trips byte-for-byte.
- `tests/{commonmark,typst_blocks,multicursor,geometry,editing_v04,ime_a11y,power_v06,lists}.rs`
  — all unchanged.
- `cargo clippy --workspace --all-targets` zero warnings; `cargo fmt --all --check`.
- `perf_probe edit_vs_caretmove_attribution`: the **caret-move/scroll cost no longer
  scales with total line count** (the deliverable), with **no** edit-path regression.

## 6. Risk / rollback

Highest-risk change in the project (touches the delta/caret invariant). If any gate
can't be satisfied cleanly, **revert and report** rather than ship — exactly the
discipline that kept v0.7.x clean. Land it on a branch (`issue-19-incremental`),
verify all gates, then a single release (likely **0.8.0** if the API changed).

## 7. Process (matches the v0.7.x cadence)

Branch → implement → all gates green (debug + `--release` + `--features
syntax-highlight`) → fmt + clippy clean → bump version + CHANGELOG + README →
commit → tag → push `main` → `gh release create` → `gh issue close 19`.
Credentials/`gh` are already configured (see the `local-credentials` skill).

---

## 8. Spike result (2026-06-08) — measured, then reverted. **Recommendation: no-go.**

The design was prototyped end-to-end on a throwaway branch (`issue-19-spike`,
since deleted; `main` untouched): a full per-line `Rc<LineSpans>` path added
*alongside* the flat one (`styled_lines_with`, `render_view_lines`,
`build_buffer_cached_lines` with `Rc`-pointer-identity line diffing), plus a
byte-identity parity gate and a flat-vs-lines caret-move probe.

**1. Safety — provable.** A parity harness drove identical command streams
(caret moves, inserts, Enter, Backspace) through both paths and asserted
pixel-for-pixel + caret + `doc_height` equality across Markdown, Typst, empty/
tiny docs, and the trailing-hidden-marker edge. All green. The contract change
*can* be made byte-identical.

**2. Speed — real but only a constant factor; the headline deliverable is NOT met.**
Caret-move floor, `--release`, this machine:

| doc lines | flat (today) | #19 lines path | speedup |
|-----------|-------------:|---------------:|--------:|
| 500       |     1546 µs  |      1451 µs   |  1.07×  |
| 2000      |     5628 µs  |      2649 µs   |  2.12×  |
| 4000      |     9439 µs  |      5256 µs   |  1.80×  |
| 8000      |    31418 µs  |     11460 µs   |  2.74×  |

The lines path is ~2× faster at scale but **still scales linearly** (2000→2649,
4000→5256 ≈ 2× for 2× lines). §5's acceptance gate — *"caret-move/scroll cost no
longer scales with total line count"* — is **not achieved**, and cannot be by the
styling contract change alone. The floor has several independent O(doc) passes;
this change only attacks one (the per-line string clone out of `PROJECT_CACHE` +
the `split_lines` re-hash). Still O(doc) and untouched:

- `layout.rs::doc_height` folds over **every** layout run, every render.
- `layout.rs::caret_geom` scans runs from the top to the caret line.
- the per-line assembly loop + pointer vector in the cached builder.

`doc_height` alone keeps render O(doc) no matter how perfect styling becomes. True
flat scaling needs incremental height + caret geometry too — a much larger surgery
than this issue scopes.

**3. Quality — net-neutral-to-negative.** `set_rich_text` and `split_lines`
disagree on line count for docs ending in a hidden-marker line (heading/fence/
quote as the final line); the flat path papers over this with a defensive
full-rebuild. The lines path had to reverse-engineer and replicate that exactly
(`diff_line_slice` for diff signatures while the full build keeps all groups) — it
works, but it's a latent trap, and those same docs get **zero** speedup (both
paths full-rebuild every render). Against the maintainer's "unambiguously better
in speed *and* code quality" bar, this fails: a partial speed win that misses its
own gate, plus added `Rc` indirection, an API break to 0.8.0, and new fragility on
the riskiest invariant in the repo.

**Verdict:** reverted, no release. If sub-frame feel at 8k+ lines ever becomes a
real need, the correct fix is the broader one (incremental `doc_height` + caret
geometry + viewport-bounded styling together), not this styling contract change in
isolation. Until then #19 stays open but de-prioritized.
