# Embedding sred in a host app (Noet integration)

This is the concrete "how to embed" guide. The reusable surface is **`sred-core`**
(UI-free) plus the **`RichTextEditor.slint`** component. The host owns the input
and the image; sred owns the editing + rendering.

## 1. Dependencies

```toml
# host Cargo.toml
[dependencies]
sred-core = { path = "../sred/sred-core", features = ["syntax-highlight"] }
```

`sred-core` pulls cosmic-text + ropey (+ optional syntect). It has **no Slint
dependency**, so it is **Slint-version-independent** — it works with Noet's Slint
1.13 unchanged. Only the optional `RichTextEditor.slint` component is compiled by
the host's own `slint-build`, so version alignment is a non-issue for the engine.

## 2. The one type you need: `sred_core::Editor`

The facade bundles the whole per-keystroke pipeline. The host loop is:

```rust
use sred_core::{Editor, Command, Format, layout::Theme};

let mut ed = Editor::from_source(&note_body, Format::Markdown);
ed.set_theme(noet_theme());          // your palette + font scale (§4)
ed.set_viewport(width_px, height_px); // call again on resize

// --- on input events, drive the editor, then render ---
// key char:     ed.apply(Command::Insert("x".into()));
// standard UI:  ed.command("selectall", None);
// clipboard:    ed.command("copy", None) / ed.command("cut", None)
// paste:        ed.command("paste", Some(&clipboard_text))
// arrows:       ed.command("left", None), ed.command("select-down", None), ...
// toolbar bold: ed.command("bold", None);   // TOGGLES: wraps the selection, or
//   unwraps it if already bold. Route format buttons (bold/italic/code/strike,
//   headings, lists) through command names — NOT by inserting literal "**…**"
//   snippets, which can't un-toggle or wrap a selection.
// undo/redo UI: ed.can_undo() / ed.can_redo() → enable/disable the buttons.
// click:        ed.click(x, y);   drag: ed.drag(x, y);   dbl: ed.double_click(x, y)
//   ^ x,y are in DOCUMENT space (the full-frame coords a TouchArea inside a
//     Flickable reports — its mouse-y already includes the scroll). Don't add scroll.
// wheel:        ed.scroll_by(-delta_y);

let out = ed.render(/* follow caret = */ true);
// out.frame  : RGBA Vec — wrap in your image type and show it
// out.caret  : caret rect (document coords) — draw a 2px Rectangle
// out.scroll_y / out.doc_height : drive your scroll container

// persist — byte-lossless, exactly what the user typed:
backend.save_note(id, title, ed.text());
```

`Editor::command(name, clipboard_text)` is the preferred host adapter for normal
editor controls. It centralizes the behavior every embedding app otherwise
reimplements:

| UI action | command |
|---|---|
| Ctrl/Cmd+A | `selectall` |
| Ctrl/Cmd+C / X | `copy` / `cut` returning `ClipboardOp::SetText(text)` |
| Ctrl/Cmd+V | `paste` with `Some(clipboard_text)`, or `None` to request a paste |
| arrows / Shift+arrows | `left`, `right`, `up`, `down`, `select-left`, `select-right`, `select-up`, `select-down` |
| Home/End variants | `home`, `end`, `doc-start`, `doc-end`, and `select-*` variants |
| word movement/delete | `word-left`, `word-right`, `delete-word-backward`, `delete-word-forward` |
| formatting | `bold`, `italic`, `code`, `strike`, `h1`, `h2`, `h3`, `bullet`, `ordered`, `quote`, `codeblock` |

The host still decides how raw toolkit key events map to these command names and
how to read/write the OS clipboard. `sred-core` owns the editor semantics once a
command name reaches it, including Shift+Up/Shift+Down vertical selection.

### Slint specifics
- Convert `out.frame` to an `Image`:
  ```rust
  let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(out.frame.width, out.frame.height);
  buf.make_mut_bytes().copy_from_slice(&out.frame.rgba);
  ui.set_editor_image(Image::from_rgba8(buf));
  ```
- Show it in a `Flickable` (viewport-y = `-out.scroll_y`), draw the caret as a
  `Rectangle`, and put a `FocusScope` + `TouchArea` over it to forward keys and
  pointer events into `ed`. `sred-slint/ui/sred.slint` + `sred-slint/src/lib.rs`
  is a complete reference wiring you can copy.

## 3. Mounting it behind a beta toggle in Noet

Replace the raw `ed := TextEdit` edit pane with the sred surface only when a
`WYSIWYG (beta)` switch is on; keep `TextEdit` as the fallback. Bind sred's
`text()` to the same `current-body`/autosave path. For `kind: typst` notes, keep
the raw editor + compiled-image preview — sred edits markdown, not typeset Typst.

## 4. Theming

`set_theme(Theme { font_size, line_height, margin_x, margin_y, fg, bg, link,
code, selection })`. Build it from Noet's `Theme` global and multiply sizes by
`Z.f`:

```rust
fn noet_theme(scale: f32, dark: bool) -> Theme {
    let mut t = Theme::default();
    t.font_size  *= scale;
    t.line_height *= scale;
    if dark { t.fg = [220,224,228,255]; t.bg = [22,26,31,255]; /* … */ }
    t
}
```

Re-`set_theme` + re-`render` when the palette or `Z.f` changes.

## 5. Domain tokens (`[[wikilink]]`, `+[[project]]`, `@mention`, `#tag`, url)

cosmic-text/sred know nothing about these — they're Noet's. The hook is a token
registry on the `Editor`:

```rust
ed.register_token(TokenSpec {
    id: "wikilink".into(),
    fg: [31, 122, 68, 255],            // Noet's project ink
    matcher: Box::new(|line| noet::parse_links(line)),  // -> Vec<TokenMatch{start,end,value}>
});
// click handling:
if let Some((id, value)) = ed.token_at(x, y) {
    match id.as_str() { "wikilink" => filter_project(&value), "url" => open(&value), _ => {} }
}
```

Available now via `ed.register_token(TokenSpec { id, fg, matcher })` and
`ed.token_at(x, y) -> Option<(id, value)>`. Alternatively, Noet can recompute its entity chips from `ed.text()` on
`changed` (exactly as it does today from `current-body`) and keep the existing
filter/click affordances in the chip strip. The in-editor coloring of tokens is
the only thing waiting on M5.

## 6. What sred owns vs what Noet owns

| sred (`sred-core`) | Noet |
|---|---|
| byte-lossless editing, undo/redo, motions, selection, clipboard ops | frontmatter, autosave (`save_note`), entity index |
| markdown Live Preview styling + syntect code highlighting | domain token semantics + click→filter (via §5) |
| caret/selection/scroll geometry, RGBA rendering | the Slint window, toolbar/menus, theme values |
| `text()` (what to persist) | Typst compile→image preview; todo `format_todo_line()` |

## 6b. Embedding the Slint component safely (relayout)

The reference `RichTextEditor` (`sred-slint/ui/sred.slint`) is now relayout-safe by
construction, but if you adapt it, keep these two rules (they avoid Slint's
`Recursion detected` panic — see #1):

- **Do not report the viewport size from `changed width/height` or `init`
  geometry handlers.** Those fire *during* the layout flush; invoking a callback
  (e.g. `resized(...)`) from them re-enters the property system and recurses.
  Report size from a post-layout `Timer` instead (the component does this and
  only calls `resized` when the size actually changes).
- **Pin the component's preferred size** (`preferred-width/height: 0px`) and let
  sizes flow top-down (`scope`/`flick` use `width/height: 100%`). Otherwise a
  host that negotiates preferred size with sibling stretch children can cycle
  `root.preferred ← child ← root.size`.

Embed it as a **layout child** (e.g. inside a `VerticalLayout`/`HorizontalLayout`),
not as a 100%-fill child of a non-layout parent.

**Accessibility:** the component mirrors the document into `doc-text` and exposes
`accessible-role: text` + `accessible-value`, so screen readers can read the
content even though the glyphs are a bitmap (#2). Drive it from your source text.

## 7. Status

- ✅ `Editor` facade (`sred-core/src/api.rs`) — drive + render + byte-lossless text.
- ✅ Host-providable `Theme`.
- ✅ Token extension API (§5) — register_token / token_at on the Editor facade.
- ◻ Reference: mount in Noet behind the beta toggle.

See `DESIGN.md` for the architecture and `ROADMAP.md` for the milestone order.
