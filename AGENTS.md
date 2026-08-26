# vvpaint

Vivid 1.5 producer: a paint and annotation program whose canvas rides a raster track. Read the root
`AGENTS.md` first; this file adds what is specific to vvpaint.

## What it is

A single-window drawing surface over an optional base image. The interaction model, drawing
algorithms, and export behaviour are derived from Kitdraw 0.2.1 (MIT; see `LICENSE`), so when a
question is not answered here, Kitdraw is the reference.

Three rectangles are easy to confuse, and most bugs in this crate have been about confusing them:

| | What it is | Where |
| --- | --- | --- |
| **Viewport** | the pane's whole client area in physical pixels | `Layout::viewport_*` |
| **Canvas / backing** | the drawing surface, `canvas_rows` of cells wide | `Layout::backing_*`, `DrawingCanvas` |
| **Fit rect** | where the base image is letterboxed inside the canvas | `DrawingCanvas::fit()` |

Every stored element uses **normalized 0..1 canvas coordinates**, so a resize replays history
rather than resampling pixels (`DrawingCanvas::resize` → `rebuild`).

## Invariants

- **The terminal grid starts at the client-area origin.** Vivido's `dynamic_padding` defaults to
  false, so sub-cell slack collects at the right and bottom rather than being split, and the base
  padding defaults to zero. The Vivid target descriptor does not publish padding, so there is
  nothing to derive it from — do not reintroduce a centring guess in `vivid::layout_for`. It was
  one, and it shifted every stroke by half the vertical remainder.
- **Descriptor cell metrics are rounded; the renderer places nodes with the unrounded value.** Do
  not build a pixel rectangle by multiplying rows by the published cell height and expect it to
  match the renderer exactly.
- **Media bytes never touch the PTY.** Only the toolbar rows and the status line are terminal text.
  Anything a caller needs to read back — geometry, pointer position, dirty state — has to be on the
  status line, because that is the only channel it can query.
- **Pointer motion must not republish a raster frame.** `handle_mouse` returns a `MouseAction` that
  separates canvas redraw from UI redraw; keep them separate or a mouse move pushes a full frame.
- **Every control keeps a key binding.** The toolbar is a convenience, not the interface: a caller
  driving vvpaint from outside must never have to compute a toolbar coordinate. If you add a
  control, bind it and add it to the README table and the `vvpaint` skill.

## Testing

Unit tests live in `#[cfg(test)]` modules beside the code. Two things are worth knowing:

- `tests/headless.rs` is an `#[ignore]`d smoke test that launches a real headless Vivido and drives
  it over IPC. It needs a wgpu adapter.
- Geometry regressions hide easily because it is tempting to write fixtures with a zero grid origin
  and a canvas the same size as its image. `vivid::tests` and `terminal::tests` deliberately use a
  real pane's numbers (1938x1138 viewport, 129x33 cells of 15x34) and a letterboxed image.

## Verification

From `vvpaint/`:

```sh
cargo fmt --all --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

With a wgpu adapter available, also:

```sh
cargo test --test headless -- --ignored --test-threads=1
```
