# Primitive Browser Engine

A browser **rendering engine built as a pure primitive kit + orchestrator, on the
Spiderweb bus.** This is the [composition doctrine](#doctrine) applied to
rendering: don't build a monolithic engine — *compose* one out of dumb math/render
primitives, drive a sealed rasterizer from outside, and let an orchestrator own
all the policy.

> **Status (2026-06-28):** renders to pixels, end to end. A render request flows
> `parse → cascade → paint → render` across the bus as an emergent thread and
> produces two artifacts: a deterministic **display list** and an actual
> **rasterized image** (PPM). Verified at the pixel level — a
> `background-color:#1e2430` box renders as `(30,36,48)` pixels on a white page;
> outside the box is white. `cargo test` → 4 passed; `cargo run --bin pbe` →
> writes `out/demo.display-list.txt` + `out/demo.ppm` and exits 0.

## What this is (and is not)

This workspace is the **orchestrator** — all policy, no mechanism. It does *not*
contain a parser, a CSS engine, or a rasterizer. Those already exist as dumb,
reusable primitive crates, and we drive them from outside without modifying them:

| Pillar | Where | Role |
|---|---|---|
| **Render primitive kit** | `F:\browser primitves` (+ `cap-*` store crates) | All render mechanism: `parse_html`, `Stylesheet::parse_author`, `StyledDom::new`, `paint`, → `ordo-ux-vello` → `vello::Scene`. Never modified. |
| **Spiderweb bus** | `D:\Spiderweb-Bus-Next` | The fabric stages talk over: typed pub/sub, emergent threads, a supervising spider. std-only, zero-dep. Never modified. |
| **This orchestrator** | here | Registers stages + the spider, dispatches renders, reacts to the fabric. All policy. |

## Architecture

```text
 RenderRequest ─▶ [build-styled] ─▶ StyledReady ─▶ [paint] ─▶ PaintReady ─▶ [render] ─▶ FrameReady
   (on-ramp)       parse+cascade                    paint                    rasterize    (off-ramp)
```

Each box is a **strand** (a bus worker) wrapping a dumb primitive function. Each
arrow is a **typed socket**. No stage names another stage — the bus fans out by
type, so the render pipeline *emerges* as a thread through the web. Add the
`spider` and a crashed stage is restarted; add a `highway` (later) and paint can
fan across parallel lanes.

### Crates

- **`pbe-protocol`** — the message vocabulary (`RenderRequest`, `StyledReady`,
  `PaintReady`) and socket names. Pure contracts. Heavy payloads ride as
  `Arc<T>` so bus fan-out is a refcount bump, never a deep DOM copy (explicit,
  near-zero cost — the doctrine forbids invisible cost).
- **`pbe-stages`** — the render stages as strands (`build-styled`, `paint`,
  `render`). The only new code in the render path; each just adapts a `cap-*` or
  `pbe-render` call to the bus. No policy.
- **`pbe-render`** — the render off-ramp: turns a paint primitive list into a
  deterministic display list (golden-testable) and a software-rasterized RGBA
  framebuffer serialized as PPM. Zero GPU dependency — a GPU backend
  (`ordo-ux-vello`) is a *swap* of this stage later, not a prerequisite.
- **`pbe-orchestrator`** (`pbe` binary) — registers types, stages, and the
  spider; dispatches a demo render; writes artifacts to `out/`. All policy.

## Viewing the raster

The engine writes a binary PPM. To view it as PNG:

```sh
cargo run --bin pbe                       # writes out/demo.ppm + out/demo.display-list.txt
python tools/ppm_to_png.py out/demo.ppm   # -> out/demo.png
```

## Run

```sh
cargo run -p pbe-orchestrator      # or: cargo run --bin pbe
```

## <a name="doctrine"></a>Doctrine

- **Compose from outside; never crack the engine.** The `cap-*` kit and the bus
  are sealed executors we drive through their exposed surfaces. We add adapters,
  never patches or shims.
- **Dumb primitives, smart orchestrator.** Mechanism lives in the kit; *all*
  policy (what runs, retries, placement, fan-out) lives here.
- **Cost stays explicit.** Large non-`Clone` payloads (`DomTree`, `StyledDom`)
  move as `Arc` handles, not clones.

See `ROADMAP.md` for what's next and the known seams.
