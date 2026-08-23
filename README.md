# Primitive Browser Engine

The public workspace is self-contained: the small `cap-http`, `cap-geometry`,
and `cap-text-shape` contracts used by the browser are vendored under `crates/`
with their original package metadata. A fresh clone does not depend on Jesse's
machine-local crate store.

> **Status (2026-07-15):** the network on-ramp is now a **modular protocol
> layer** — one crate per modern fetch protocol, each independently
> swappable, debuggable, and upgradable. A new `pbe-proto` dispatch crate
> routes a URL to the matching protocol crate:
>
> - **`pbe-proto-http`** — `http`/`https`, driving the sealed system HTTP
>   client (zero linked HTTP/TLS deps; the same security posture as before).
> - **`pbe-proto-ws`** — `ws`/`wss` (WebSocket, RFC 6455): handshake via
>   the sealed HTTP client + a pure-Rust frame codec. No linked TLS/HTTP.
> - **`pbe-proto-data`** — `data:` URIs (RFC 2397): pure byte work, zero
>   I/O, zero deps.
>
> `pbe-net` is now a thin facade over `pbe-proto`, preserving the original
> `fetch` / `fetch_bytes` / `FetchedPage` / `FetchedBytes` API so existing
> callers (`pbe-shell`, `pbe-orchestrator`) keep working unchanged while
> transparently gaining `ws`/`wss`/`data:` routing. Legacy schemes
> (`file://`, `ftp://`, `scp://`, …) are rejected as `UnsupportedScheme`
> rather than silently mis-handled — modern protocols only, no backward
> compatibility with old protocols, by design.
>
> Verified: 66 new tests across the protocol crates (6 + 8 + 25 + 13 + 9 in
> `pbe-proto`/`-http`/`-ws`/`-data`/`pbe-net`), plus 7 `pbe-shell`
> scheme-classification + WebSocket-integration tests;
> `cargo clippy --all-targets -- -D warnings`
> clean and `cargo fmt --check` clean across every touched crate. See the
> updated Crates section below and `ROADMAP.md`.

> **Status (2026-07-16):** the engine gained the four capabilities that
> made it a usable modern browser — each in its own swappable crate or
> behind the existing kit boundary, per the doctrine:
>
> - **JavaScript** (new `pbe-js` crate): wraps boa (pure-Rust ECMAScript,
>   no C build chain). `<script>` runs during page load; `console.log`,
>   `document.getTitle/setTitle`, and `fetch()` (routed through the
>   modular protocol layer) are wired into the browser. Script errors are
>   non-fatal. Why boa not V8: V8 links C++ and drags a foreign engine
>   into our address space; boa is pure-Rust, auditable — the same
>   trade `ring`-over-`aws-lc-rs` makes for TLS.
> - **Image codecs** (new `pbe-img-codecs` crate): JPEG + WebP + GIF via
>   the `image` crate behind a swappable boundary; the in-kit BMP/PNG
>   decoders stay zero-dep. The browser dispatches by magic bytes.
> - **Form controls**: `<input>`/`<button>`/`<textarea>`/`<select>` now
>   render as interactive widgets (the kit's `Style::input/.button`),
>   with stable per-page widget ids (base 1000+, never colliding with
>   the chrome's 1–99). Previously dropped by the reducer.
> - **CSS child combinator + attribute selectors**: `div > p` matches a
>   direct child (not a deeper descendant); `[attr]`, `[attr=val]`,
>   `[attr^=val]`, `[attr$=val]`, `[attr*=val]` parse and match against
>   id/class. Sibling combinators (`+`/`~`), `*`, and pseudo-classes
>   (`:hover`) still fail closed (need sibling-stack / interaction
>   state the reducer doesn't thread yet).
>
> Verified: 215 workspace tests pass (was 184); `cargo clippy --workspace
> --all-targets -- -D warnings` clean; `cargo fmt --all --check` clean.

The renderer still presents one `pmre-kit` API, but its implementation is split
across focused core, raster, text, layout, HTML, and showcase crates. No Rust
crate in the published workspace exceeds the 4,000-source-line ceiling.

A browser engine built by driving a **complete, self-contained rendering kit**
directly. Math primitives (the eight root atoms — `scan · hash · fold ·
project · scale · compare · combine · order`) compose up into function
primitives inside `pmre-kit`/`pmre-orchestrator`: the box model, flex layout,
SDF/scanline paint, real TrueType text. That composition is already finished —
`render_html(doc, w, h, clear) -> Framebuffer` does parse + layout + paint +
raster in one call. This workspace is a thin, direct caller around it, not an
orchestration layer wrapped around it.

> **Status (2026-07-04, later):** `<img>` support landed as a kit expansion
> plus a small browser-side prefetch composition:
>
> - **Kit change (upstream in `primitive-math-rendering-engine`):** new
>   `raster::Image` struct + zero-dep `decode_bmp` (24/32-bit BI_RGB) and
>   `decode_png` (color types 2 + 6, 8bpc, full DEFLATE inflater with fixed
>   and dynamic Huffman + LZ77 sliding window, all 5 PNG filter types); new
>   `raster::blit_image<Surface>` (nearest-neighbour, Porter-Duff over,
>   clip-and-band-aware so the parallel render path handles images too);
>   new `UxNode::Image { style, image: Arc<Image> }` and `Painted::Image`
>   variants threaded through layout + orchestrator paint; new HTML
>   `parse_with_images(src, &HashMap<String, Arc<Image>>)` entry point.
> - **Browser change (`pbe-shell`, pure composition):** scan HTML for
>   `<img src="…">`, fetch each via `pbe_net::fetch` (URLs) or `std::fs`
>   (local files), dispatch to the right decoder by magic bytes (BM… → BMP,
>   the 8-byte PNG signature → PNG), fold into an `Arc<Image>` map keyed by
>   src, hand to `parse_with_images`. Zero fetches inside the kit; per-image
>   failures are non-fatal (broken image → missing).
>
> Verified: 11 new kit tests (BMP round-trip against `Framebuffer::to_bmp`,
> nearest-neighbour blit + clip correctness, hand-crafted 2×2 PNG fixture,
> BMP compression / PNG palette / bogus bytes all reject without panicking,
> plus 3 `parse_with_images` tests), 8 new browser-side `image_scan_tests`
> (double / single / unquoted, attribute order, source-order preservation,
> unclosed-tag safety, decode-dispatch magic-byte routing), and a fourth
> end-to-end `browser_probe` composition assertion: write a 32×32 solid
> `#00e6ff` BMP, reference it from an HTML `<img src>`, load through
> Browser, count cyan pixels in the rendered frame — expected ~1024, got
> **exactly 1024**. Full sweep across both repos: `cargo test`,
> `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` all clean.
>
> **Still explicitly out of scope, by doctrine:** descendant combinators,
> pseudo-classes, positioned layout, `@media`, `<form>`/`<input>`,
> JavaScript. Adding those means kit changes upstream in
> `primitive-math-rendering-engine` (the same way `<img>` was added), not
> browser-layer shims.

> **Status (2026-07-04):** two composition-only changes landed on top of the
> 2026-07-03 work — no kit changes, no new mechanism, just calling capabilities
> the kit already exposes:
>
> - **Bloom `Quality` tier reaches the browser.** `Browser::render_with_quality(q)`
>   is an alternate composition alongside the byte-identical `render()`, calling
>   the kit's existing `render_ui_quality`. The `--quality` flag on `pbe` and
>   `pbe-window` opts into it. Default (`Quality::Fast`) is byte-identical to
>   today; `--quality tiled-full` applies the cache-tiled CPU bloom that a
>   full `pmre-orchestrator::examples::sweep` sweep on this exact hardware
>   (i7-13700K / RTX 5070 Ti) measured at **22–27 ms/frame at 1920×1080 vs
>   34–39 ms/frame for the wgpu GPU path** — a 1.27x–1.73x CPU win, holding
>   across 860×380, 800×600, and 1920×1080. The sweep example was extended
>   to accept optional `<width> <height>` args so the resolution scan could
>   be reproduced.
> - **External `<link rel="stylesheet">` support via composition.** The kit
>   already reads `<style>` blocks; the browser now scans the HTML for
>   `<link rel="stylesheet" href="…">` before parse (the atom `scan`
>   specialised to HTML), fetches each via `pbe_net::fetch` (URLs) or `std::fs`
>   (local paths), folds the results into one synthetic `<style>` block
>   prepended to the source, and hands the augmented document to the kit's
>   existing `html::parse`. **Zero kit change.** Every step is an atom
>   already there.
>
> Both landed with dedicated test coverage: 9 new `stylesheet_scan_tests` in
> `pbe-shell/src/lib.rs` (double/single/unquoted attrs, attribute order,
> non-stylesheet rel skipping, source order preservation, unclosed-tag
> safety, substring-attribute rejection) plus two full end-to-end
> composition assertions in `examples/browser_probe.rs`: the external CSS
> rule visibly colours the heading (1287 pixels at #d35400 in the rendered
> frame), and `Quality::TiledFull` visibly changes the frame (32599 pixels
> different from `Fast`, which itself is byte-identical to `render()`).
>
> Verified: `cargo build/test/clippy` scoped per crate (`-p pbe-shell -p
> pbe-orchestrator`), all green; `cargo run -p pbe-shell --example
> browser_probe` prints all three composition assertions passing.
>
> **Explicitly out of scope, by doctrine.** Descendant combinators (`div p`),
> pseudo-classes (`:hover`), attribute selectors, `@media`, images, and
> JavaScript are *kit-level* features. The kit is complete for what it
> renders; the browser is 100% for what the kit renders. Composition from
> outside doesn't add mechanism the kit doesn't already have — that would
> be the same category of mistake as the bus wrapper corrected on 2026-07-03.
> If any of these ever land, they land as kit changes upstream in
> `primitive-math-rendering-engine`, not as shims here.

> **Status (2026-07-03):** five changes landed the same day, each documented
> in full (including bugs caught and fixed along the way) in `ROADMAP.md`:
>
> 1. Vendored `pmre-kit` + `pmre-orchestrator` in from the sibling
>    [`primitive-math-rendering-engine`](../primitive-math-rendering-engine)
>    project, replacing the old F:\ `cap-*` rendering kit entirely.
> 2. **Corrected a mistake made in step 1:** kept the old Spiderweb-bus
>    strand/message-type architecture wrapped around the new kit at first —
>    ceremony that made sense for the *old*, genuinely disassembled cap-*
>    kit, but was backwards once wrapped around something already complete
>    and self-contained. `pbe-protocol`/`pbe-stages` were deleted;
>    `pbe-orchestrator`/`pbe-shell` call `pbe_net::fetch` and
>    `pmre_orchestrator::render_html`/`render_uxi` directly, no bus.
> 3. `pbe-window` became an actual browser: `pbe_shell::Browser` composes an
>    address bar + Back/Forward/Reload buttons + the loaded page from
>    `pmre-orchestrator`'s own interactive-UI system (`UiState`/
>    `handle_event`/`render_ui`), with the page in a native `Style::scroll`
>    region — no hand-rolled scroll math, correcting the same kind of
>    "rebuilt something already composed" mistake as #2.
> 4. Rendered `<a href>` links are now clickable and navigate — a kit change
>    (`Span`/`RichPiece` gained `href`, a new `layout::hit_test_link`) plus a
>    small, deliberately non-RFC-3986-complete href resolver in
>    `pbe_shell::Browser`.
> 5. **`<style>` blocks and class/id/type CSS selectors** — the single
>    biggest gap left after #1–4: `pmre-kit` read only inline `style="..."`
>    before this. A new bounded `pmre-kit::css` module (type/`.class`/`#id`
>    compound selectors, comma-separated lists, specificity-ordered cascade,
>    reusing the existing inline-style parser verbatim for rule bodies) —
>    deliberately **not** a full CSS engine: no combinators, pseudo-classes,
>    attribute selectors, or `@media`, and unsupported selector syntax is
>    dropped entirely rather than mis-matched. Payoff: three example pages
>    that predate this whole migration and use exactly this — `<style>`
>    blocks with class/id/type selectors — now render correctly,
>    **unmodified**, because the kit under them grew the capability they
>    always assumed.
>
> Verified: `cargo build/test/clippy --workspace` green in **both** this repo
> and the sibling `primitive-math-rendering-engine` (33 pmre-kit tests, up
> from 13 at the start of the day). `cargo run --bin pbe` on `examples/
> wrap-demo.html`/`inline-wrap.html`/`fuzzy-css-demo.html` — confirmed
> visually (blue type-selector headings, class-selector widths producing
> different real text-wrapping, an inline `<strong>` styled by a bare type
> selector inside a running paragraph). `cargo run -p pbe-shell --example
> browser_probe` — self-verifying, mirrors `pmre-orchestrator`'s own `todo`
> example: types a URL, presses Enter, walks Back/Forward, clicks a rendered
> link, all via real `dispatch()`/`UiEvent` calls. Live check (Windows-MCP
> screenshot): the chrome and an inline link's blue-underlined style both
> confirmed in a real window.
>
> **Known gap:** live *mouse-click* verification wasn't possible with the
> tools available this session (Windows-MCP's `Click`/`Move`/`Scroll` all hit
> an array-parameter serialization bug; computer-use declined access to this
> ad hoc binary earlier). The exact code path a real click hits is exercised
> by `browser_probe.rs`'s real event dispatch, just with a computed rather
> than OS-reported coordinate.
>
> **Known limitations, accepted:** no CSS combinators/pseudo-classes/
> attribute selectors/`@media`, no forms, no JavaScript. External
> stylesheets landed 2026-07-04 via the fetch-and-inject composition;
> `<img>` support (BMP + PNG) landed the same day as a kit expansion +
> browser prefetch. See `ROADMAP.md`'s 2026-07-03 and 2026-07-04 entries
> for the full history and rationale.

## What this is (and is not)

This workspace does *not* contain a parser, a layout solver, or a rasterizer —
those are `pmre-kit`'s job, and it already does them completely. There is no
pipeline here to orchestrate:

| Pillar | Where | Role |
|---|---|---|
| **The renderer** | `crates/pmre-kit` + `crates/pmre-orchestrator` (vendored from `primitive-math-rendering-engine`) | Complete and self-contained: `html::parse` reduces HTML to a box tree, `layout::solve` runs a real flex/box solver, `render_html`/`render_uxi` paint + rasterize to a `Framebuffer` — SDF shapes, scanline path fill/stroke, real TrueType text. Zero dependencies. Changes land upstream in the sibling project and get re-copied — never edited in place here. |
| **The network on-ramp** | `crates/pbe-net` | A complete, direct `url -> Result<FetchedPage>` call driving the sealed system `curl` binary. Not a pipeline stage. |
| **This workspace** | here | A thin caller: load a page, hand it to the renderer, present or persist the result. |

## Architecture

```text
pbe (CLI):     load HTML (file / demo / pbe_net::fetch) ──▶ pmre_orchestrator::render_html ──▶ Framebuffer::to_bmp ──▶ out/<label>.bmp

pbe-window:    pbe_shell::Browser — one UiState drives a composed tree:
                 chrome bar (Back/Forward/Reload buttons, address-bar input)
                 + loaded page (html::parse) embedded in a Style::scroll region
               winit events ──▶ UiEvent ──▶ Browser::dispatch ──▶ render_ui ──▶ Framebuffer::to_u32 ──▶ softbuffer surface
```

No bus, no message types, no stages — both binaries are direct, synchronous
callers. `pbe-window` is a real browser: typing a URL into the address bar
and pressing Enter navigates, Back/Forward walk a real history stack, and the
page scrolls via `pmre-kit`'s own `Style::scroll` region (wheel scroll *and*
a draggable scrollbar) — no hand-rolled scroll math in this workspace at all.

### Security posture — networking by composition, not by linking

The engine **links no HTTP or TLS code**. The `pbe-net` crate drives the
**sealed system `curl` binary** from outside via `std::process` — the same
"drive a sealed executor from outside, never link its internals" doctrine
applied to networking as to rendering. Consequences:

- **Zero linked network/crypto dependencies** — `cargo tree -p pbe-net` is just
  the `cap-http` *type contract* (+ smallvec/thiserror). Nothing to audit, no
  TLS/parser CVE in our address space.
- **Process isolation** — the network touches a separate sealed process; the
  engine only ever receives bytes over a pipe. The fetch boundary is an OS
  process boundary, not a call into mutable foreign code.
- **Scheme allow-list** — only `http`/`https` reach curl (`--proto =http,https`
  plus a pre-spawn check); no `file://`, `scp://`, etc.
- **Immutable values** — a fetched page is a plain owned struct; the fetch
  primitive is a pure `url -> Result<FetchedPage>` with no shared state.

### Crates

- **`pbe-net`** — network on-ramp **facade**; delegates to the modular
  `pbe-proto` protocol layer while preserving the original `fetch` /
  `fetch_bytes` API for existing callers. Called directly, not through a bus.
- **`pbe-proto`** — the **protocol dispatch** layer: the single composition
  point that routes a URL to its per-protocol crate. Owns the shared
  `Resource` type and `FetchError` enum; each modern protocol lives in its
  own swappable crate behind it:
  - **`pbe-proto-http`** — `http`/`https`; drives the sealed system HTTP
    client binary via `std::process` (zero linked HTTP/TLS deps).
  - **`pbe-proto-ws`** — `ws`/`wss` (WebSocket, RFC 6455); persistent
    connections via `WsConnection::connect` (TCP + rustls TLS for `wss`,
    client handshake over the socket, then a send/recv frame loop) plus a
    pure-Rust frame codec. `wss` links `rustls` (pure-Rust `ring` crypto) —
    a deliberate exception to the "link no crypto" posture, because
    WebSocket is persistent and the sealed-binary approach cannot carry the
    bidirectional frame stream. `ws`/`ws(s)` URLs are wired into
    `pbe-shell` via `Browser::open_websocket`/`poll_websocket`/`send_websocket`/`close_websocket`.
  - **`pbe-proto-data`** — `data:` URIs (RFC 2397); pure byte work, zero
    I/O, zero deps.
  Legacy schemes (`file://`, `ftp://`, `scp://`, …) are rejected as
  `UnsupportedScheme` — only modern fetch protocols are routed. Each crate
  can be upgraded, debugged, or swapped in isolation.
- **`pbe-js`** — JavaScript engine: wraps boa (pure-Rust ECMAScript) with a
  minimal DOM surface (`console`, `document.title`, `fetch`) routed through
  caller-supplied hooks. The browser runs `<script>` blocks through this.
- **`pbe-img-codecs`** — JPEG/WebP/GIF decoders (via the `image` crate)
  that decode to the kit's `Image` type, behind a swappable boundary. The
  in-kit BMP/PNG decoders stay zero-dep; the browser dispatches by magic bytes.
- **`pbe-text`** — single composition point for real, shaper-based text
  measurement + wrapping over `cap-text-shape` (cosmic-text). Kept for
  possible reuse; not currently wired into the render path (`pmre-kit` has
  its own independent TrueType text engine).
- **`pmre-kit`** / **`pmre-orchestrator`** — the renderer, vendored from
  `primitive-math-rendering-engine`. Zero dependencies (`pmre-orchestrator`
  optionally pulls `wgpu`+`pollster` for an opt-in GPU bloom post-process,
  unused by this workspace).
- **`pbe-orchestrator`** (`pbe` binary) — resolves a page source from CLI args,
  fetches if needed, calls `render_html`, writes a BMP to `out/`. A few dozen
  lines; nothing to orchestrate.
- **`pbe-shell`** (`pbe-window` binary, `src/lib.rs` + `src/main.rs`) — the
  browser itself. `src/lib.rs` exposes `Browser`: navigation history, the
  loaded page, and the composed chrome+scroll tree, built entirely from
  `pmre-orchestrator`'s `UiState`/`handle_event`/`render_ui`. `src/main.rs` is
  just a winit↔`UiEvent` translator + softbuffer presenter — the browser
  itself owns all chrome/scroll/navigation behavior.
  `examples/browser_probe.rs` self-verifies navigation (type an address,
  Enter, Back, Forward, click a rendered `<a href>` link) through real
  `dispatch()` calls before rendering a BMP, mirroring `pmre-orchestrator`'s
  own `todo` example.

## Viewing the raster

The engine writes a 24-bit BMP — directly viewable in any standard image
viewer, no conversion needed:

```sh
cargo run --bin pbe   # writes out/demo.bmp
```

(`tools/ppm_to_png.py` is left over from an earlier PPM-output era and no
longer applies — nothing in this workspace emits PPM anymore.)

## Run

```sh
cargo run --bin pbe                              # the built-in demo page
cargo run --bin pbe -- page.html                 # render a local HTML file
cargo run --bin pbe -- --url https://example.com # fetch + render a LIVE page
cargo run --bin pbe-window -- page.html          # open the browser on a local file
cargo run --bin pbe-window -- --url https://example.com  # ...or a live URL

# Opt into the bloom Quality tier (default is Fast = no post; byte-identical to today).
# tiled-full is the CPU-wins-GPU cache-tiled path benchmarked at 1.27x–1.73x
# faster than the wgpu compute-shader bloom on this hardware.
cargo run --bin pbe -- page.html --quality tiled-full
cargo run --bin pbe-window -- page.html --quality tiled-full
```

`pbe-window` is a real browser: click the address bar, type a new local path
or `http(s)://` URL, press Enter to navigate. Back/Forward walk history,
Reload refetches the current page, the page area scrolls natively (wheel or
drag the scrollbar), and clicking a rendered `<a href>` link navigates to it.

`pmre-kit`'s HTML reducer reads inline `style="..."` attributes and
`<style>` blocks with type/`.class`/`#id`/compound selectors (see
`crates/pmre-kit/src/css.rs`) — no combinators (`div p`, `ul > li`), no
pseudo-classes (`:hover`), no attribute selectors. External stylesheets
(`<link rel="stylesheet" href="…">`) are handled at the browser layer:
`pbe-shell` fetches each referenced sheet before parse and injects the
combined text as a synthetic `<style>` block, so the same kit CSS engine
applies them — no external stylesheets are dropped, but no additional
selector syntax is unlocked either. Selectors using unsupported syntax
simply match nothing rather than mis-matching a broader or narrower set
of elements.

Artifacts land in `out/<label>.bmp`, where `<label>` is the HTML file stem
(`demo` for the built-in page).

## <a name="doctrine"></a>Doctrine

- **Compose from outside; never crack the engine.** The renderer is a sealed,
  already-complete executor we drive through its exposed surface
  (`render_html`/`render_uxi`). We add callers, never patches, shims, or
  orchestration ceremony it doesn't need — `pmre-kit` changes land upstream in
  `primitive-math-rendering-engine` and get re-copied, never edited in place.
- **Don't orchestrate what's already composed.** A bus/strand layer is for
  gluing together genuinely disassembled mechanism (the old cap-* kit was
  that). A complete, self-contained renderer doesn't need one — matching the
  shape of the tool to the shape of the problem matters more than reusing a
  familiar pattern.
- **Cost stays explicit.** The rasterized frame moves as an owned value, not
  a needless clone.
- **When the kit already has a widget for it, use the kit's widget.**
  `pbe-window`'s browser chrome and scrolling are built entirely from
  `pmre-orchestrator`'s own `UiState`/`Style::input`/`Style::button`/
  `Style::scroll` — the same primitives its calculator/todo demos use — not
  hand-rolled winit input handling or custom scroll-offset math. Composing
  from what's already there beats re-deriving it, even inside your own
  workspace.

See `ROADMAP.md` for what's next and the known seams.
