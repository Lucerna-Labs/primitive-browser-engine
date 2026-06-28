# Primitive Browser Engine

A browser **rendering engine built as a pure primitive kit + orchestrator, on the
Spiderweb bus.** This is the [composition doctrine](#doctrine) applied to
rendering: don't build a monolithic engine — *compose* one out of dumb math/render
primitives, drive a sealed rasterizer from outside, and let an orchestrator own
all the policy.

> **Status (2026-06-28):** end-to-end skeleton runs. A render request flows
> `parse → cascade → paint` across the bus as an emergent thread and returns a
> primitive list. Verified: `cargo run -p pbe-orchestrator` →
> *"'demo' produced 1 render primitive(s) via the bus."*

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
 RenderRequest ──▶ [build-styled] ──▶ StyledReady ──▶ [paint] ──▶ PaintReady
   (on-ramp)        parse+cascade                       paint        (off-ramp)
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
- **`pbe-stages`** — the render stages as strands. The only new code in the
  render path; each just adapts a `cap-*` call to the bus. No policy.
- **`pbe-orchestrator`** (`pbe` binary) — registers types, stages, and the
  spider; dispatches a demo render; reports the result. All policy.

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
