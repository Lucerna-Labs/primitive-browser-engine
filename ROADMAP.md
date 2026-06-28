# Roadmap

## Done (2026-06-28) — Skeleton thread

- [x] Orchestrator workspace composing the existing `cap-*` kit (F:) over the
      Spiderweb bus (D:) — no modifications to either.
- [x] `pbe-protocol`: typed bus contracts; `Arc<T>` payloads for cheap fan-out.
- [x] `pbe-stages`: `build-styled` (parse + cascade) and `paint` strands.
- [x] `pbe-orchestrator`: registers types + stages + spider; dispatches a render.
- [x] **Verified end-to-end:** `RenderRequest → StyledReady → PaintReady` flows
      as an emergent thread; `paint` produces a primitive list. Clean build
      (only pre-existing `cap-html-parse` warnings), `RUN_EXIT=0`.

## Done (2026-06-28) — Render to pixels

- [x] `pbe-render`: paint primitive list → deterministic display list + software
      raster (RGBA framebuffer → PPM). Zero GPU dep. 4 unit tests.
- [x] `render` strand wired as the 4th stage; orchestrator writes
      `out/demo.display-list.txt` + `out/demo.ppm`.
- [x] **Pixel-verified:** the `#1e2430` 640×200 box renders as `(30,36,48)`
      pixels at the top-left; outside is white. PNG preview confirmed visually.
- [x] `tools/ppm_to_png.py` — stdlib-only PPM→PNG for viewing.

## Next

### 1. GPU backend via `ordo-ux-vello` (swap the render stage)
Replace/augment the software rasterizer with `ordo-ux-vello` → `vello::Scene` →
GPU. **Seam:** the kit has *two* primitive vocabularies — `cap_paint` emits
`cap_primitives::Primitive`, but `ordo-ux-vello` consumes
`ordo-ux-primitives::Primitive`. Needs a converter. Pure composition; no engine
changes. (The software path stays as the headless/test backend.)

### 2. Split parse and cascade into separate strands
Today they fuse in `build-styled` because `StyledDom::new` **consumes** the
`DomTree` by value (a real ownership wall in the current kit). To make `parse`
its own strand publishing `Arc<DomTree>`, `cap-style-cascade` needs a
cascade-from-`&DomTree` (or `Arc<DomTree>`) entry point. **Additive change to a
kit crate — get operator approval before touching F: crates.**

### 3. Real HTML/CSS coverage
`cap-html-parse` currently ships an MVP recursive-descent `parse_html`; html5ever
is a dep but the spec-compliant `tree_sink` path is unconstructed (hence the two
warnings). Upgrading is kit work, tracked upstream in `F:\browser primitves`.

### 4. Layout stage
Insert a `layout` strand (taffy via `cap-layout`) between cascade and paint so
paint uses real box geometry instead of `estimate_bounds`'s MVP placeholders.

### 5. Use the highway
Once multiple pages/tiles render, promote `paint` onto a `highway` parallel lane
pool (on-ramp/off-ramp) and let backpressure vibrations throttle dispatch.

### 6. Fold into the agentic browser
Long-term, this becomes the `aegis-render` in-process backend behind the
`BrowserBackend` trait in `C:\Projects\aegis-browser` — replacing the Servo
WebDriver backend with direct Rust calls. (See that project's DESIGN.md §2/§6.5.)

## Known seams / walls

- **Two primitive vocabularies** (`cap-primitives` vs `ordo-ux-primitives`) —
  bridge needed for rendering (item 1).
- **`StyledDom::new` consumes `DomTree`** — blocks a clean parse/cascade strand
  split (item 2).
- **Cross-drive path deps** (C: orchestrator → D: bus → F: kit + F: store) — work
  today on this machine; not portable. A real move would vendor or git-submodule
  the kits.
