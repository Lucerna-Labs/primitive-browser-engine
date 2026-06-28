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

## Done (2026-06-28) — Internet capability (fetch the live web)

- [x] `pbe-net`: fetch a URL by driving the **sealed system `curl`** via
      `std::process` — zero linked HTTP/TLS deps (composition, not a fat crate).
      Scheme allow-list (http/https only), process isolation, immutable output.
      `cargo tree -p pbe-net` = just the cap-http contract + smallvec/thiserror.
- [x] `fetch` stage (network on-ramp): `FetchRequest` → fetch → `RenderRequest`,
      so the same pipeline renders local files AND the live web.
- [x] CLI `--url <URL> [<css>]`; URL labels sanitized for artifact filenames.
- [x] `<style>` extraction in build-styled: embedded page CSS is recovered and
      cascaded (a page now styles itself; was previously ignored).
- [x] Honest exit semantics: a valid frame with 0 paint primitives is success
      with a NOTE (cap-paint MVP limitation), not a failure.
- [x] **Verified live:** fetched a local HTTP page over the network → extracted
      its `<style>` (#c81e3a 500x260 box) → painted 1 primitive → rasterized;
      PNG confirms a crimson box. example.com/rust-lang.org fetch+parse also
      verified (242 elements parsed from rust-lang.org).
- [x] 13 tests pass (2 pbe-net + 4 pbe-render + 5 pbe-stages + 2 integration).

### Known limit (kit depth, not engine wiring)
Text and real box layout are not implemented in `cap-paint`/`cap-style-cascade`
(MVP): paint only emits primitives for elements with a background/border, and
bounds are origin-anchored placeholders. So text-heavy live pages (example.com)
parse fully but paint little. Closing this is kit work (needs approval to edit
F: crates) or a richer paint stage in `pbe-render` — tracked under "Next".

## Build hygiene

Zero-warning build, verified 2026-06-28 (after internet capability landed):
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `cargo fmt --check` (pbe-* crates) → exit 0
- `cargo test --workspace` → 13 passed
- `cargo build --release` + live `--url` run → exit 0, no warnings

The lone prior warnings were dead-code on the `HtmlTreeSink` placeholder in
`cap-html-parse` (the F: kit). Fixed at the source with `#[allow(dead_code)]`
(approved kit edit; additive, reversible) since that stub is intentionally
unused until spec-compliant parsing is wired. NOTE: `F:\browser primitves` is
not a git repo, so that fix lives only in the working tree there — re-apply if
the kit is ever reset/re-cloned.

## Known seams / walls

- **Two primitive vocabularies** (`cap-primitives` vs `ordo-ux-primitives`) —
  bridge needed for rendering (item 1).
- **`StyledDom::new` consumes `DomTree`** — blocks a clean parse/cascade strand
  split (item 2).
- **Cross-drive path deps** (C: orchestrator → D: bus → F: kit + F: store) — work
  today on this machine; not portable. A real move would vendor or git-submodule
  the kits.
