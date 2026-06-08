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
