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
| Select a tool or width | Click its bottom-bar icon or sample |
| Draw with primary/secondary color | Left/right mouse action |
| Pencil, brush, airbrush | `p`, `b`, `a` |
| Eraser, fill, color picker | `e`, `f`, `i` |
| Line, rectangle, ellipse, rounded rectangle | `l`, `r`, `o`, `u` |
| Text, highlighter | `t`, `h` |
| Previous / next applicable width | `[` / `]` |
| Enter primary/secondary color | `c` / `v` |
| Undo / redo | `z` / `y` |
| Clear annotation layer | `C` |
| Save and quit | `q`, `Esc`, or `Ctrl-C` |

The two-row bottom bar places classic-style tool icons, contextual width samples, and a 28-color
palette side by side. Palette clicks use the corresponding mouse button. The Pencil is always one
pixel wide; tools without a width option leave that region blank. The eraser always paints with the
secondary color. Any SVG containing flood fill is exported as a raster-backed SVG because fill is
pixel-derived.

## Intentional scope

The application omits selection/clipboard operations, magnification, curve and polygon tools, zoom
and scroll commands, canvas resize commands, filled shape modes, rich text styling, transparency
editing, and menus. Opening and exporting are CLI operations. Like Kitdraw, imported content is an
immutable base and `C` clears only annotations; vvpaint extends that behavior by making clear
undoable.

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
