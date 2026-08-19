# vvpaint

`vvpaint` is a lightweight paint and annotation producer for Vivid 1.5 terminal presenters. Run
it from a shell inside Vivido or another presenter that selects `terminal-surface-v1`:

```text
vvpaint
vvpaint screenshot.png -o notes.png
vvpaint screenshot.png --export-size original -o notes.svg
```

The canvas is carried on a Vivid raster track; only terminal UI and input escape sequences use the
PTY. The application never sends media bytes or Vivid credentials through terminal output.

## Controls

| Action | Control |
| --- | --- |
| Draw with primary/secondary color | Left/right mouse drag |
| Freehand, line, rectangle, ellipse | `f`, `l`, `r`, `e` |
| Arrow, text, highlighter, redaction | `a`, `t`, `h`, `x` |
| Eraser, fill, color picker | `d`, `g`, `p` |
| Change size | `[` / `]` |
| Enter primary/secondary color | `c` / `b` |
| Undo / redo | `z` / `y` |
| Clear annotation layer | `C` |
| Save and quit | `q`, `Esc`, or `Ctrl-C` |

Palette clicks use the corresponding mouse button. The eraser always paints with the secondary
color. Redaction is always opaque black, and any SVG containing redaction or flood fill is exported
as a raster-backed SVG to avoid leaking obscured or pixel-derived content.

## Intentional scope

The first version omits selection/clipboard operations, zoom and scroll commands, canvas resize
commands, filled shape modes, rich text styling, transparency editing, menus, and toolbars. Opening
and exporting are CLI operations. Like Kitdraw, imported content is an immutable base and `C`
clears only annotations; vvpaint extends that behavior by making clear undoable.

## Provenance

The interaction model, drawing algorithms, export behavior, and embedded Noto Sans asset are
derived from Kitdraw 0.2.1. Kitdraw is MIT licensed; see `LICENSE`. Noto Sans is licensed under the
SIL Open Font License 1.1; see `assets/NotoSans-OFL.txt`. The font bytes are included from the
unchanged repository reference asset at `../kitdraw/assets/NotoSans-Regular.ttf`.

## Verification

Run the normal gates from this directory:

```text
cargo fmt --all --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

After building the sibling Vivido debug binary, run the opt-in real presenter smoke test with:

```text
cargo test --test headless -- --ignored --test-threads=1
```
