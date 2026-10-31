# ADR 0001: The WASM runtime for extensions

- **Status:** accepted 2026-10-24, for Phase 3 of the extensions plan
- **Date:** 2026-10-24
- **Spike:** `crates/fenix-wasm` (`src/lib.rs`, `examples/spike.rs`)

## Decision

Extensions that run code run in **wasmtime 50**, as **components**
(the component model, with WIT interfaces). The code is compiled with
**Cranelift, on this machine**, once per extension version. The result is
kept beside the extension and loaded from there afterwards.

- **One engine per Fenix, shared by every extension.** Each extension
  gets its own `Store`, on its own worker thread. The UI thread never
  calls into an extension.
- **Every call has a deadline**, enforced by epoch interruption. A thread
  ticks the epoch every 10 ms (`fenix_wasm::Ticker`), and a call that
  passes its deadline traps.
- **The wasm stack is bounded** at 512 KiB, so runaway recursion traps
  instead of overflowing Fenix's own stack.
- **Each extension's memory is capped** with `StoreLimits` (`fenix_wasm::limits`).
- **A trap ends that call and nothing else.** The host marks the
  extension failed and shows why on its output page.
- **No precompiled native code from the registry or a package.** Fenix
  only loads code it compiled itself from the `.wasm` the signed index
  vouches for.

## What was measured

All numbers are from an Intel i5-8400 (6 cores) on Windows 11.

- **Sizes** are the growth of a release binary built with Fenix's own
  profile (LTO, one codegen unit), over an empty program.
- **Times** are from `cargo run --release -p fenix-wasm --example spike`.
  The ranges cover two runs, one on a busier machine.
- **The test module** is a generated 282 KiB module with 4,000 functions,
  standing in for a mid-sized extension.

### Binary size

| wasmtime with | Adds |
|---|---|
| runtime only (loads precompiled code) | 0.8 MiB |
| Cranelift (compiles here) | 11.8 MiB |
| Cranelift + Pulley | 11.9 MiB |
| Winch (baseline compiler) | 11.2 MiB |

Fenix's release binary is about 41 MiB today, so Cranelift adds a little
over a quarter. Phase 4 takes out more than that: PDFium bindings,
tokio (with VNC), mermaid/resvg, and the feature crates themselves.

### Time and memory

| | Cranelift | Winch | Pulley |
|---|---|---|---|
| create the engine | 0.2–1.1 ms | 0.07 ms | 0.04 ms |
| compile 282 KiB (in parallel) | 0.34–0.90 s | 0.20 s | 0.82 s |
| compiled artifact | 3.9 MiB | 4.0 MiB | 1.9 MiB |
| load the compiled artifact | 2–5 ms | 2 ms | 2–6 ms |
| instantiate | < 0.3 ms | 0.1 ms | < 0.1 ms |
| host → guest call | 30–47 ns | 33 ns | 266 ns |
| guest → host call | 5–6 ns | 4 ns | 149 ns |
| component call, guest calling host inside | 149–256 ns | 143 ns | 593 ns |
| `fib(30)` (compute) | 8–14 ms | 12 ms | 289 ms |
| paint a 1080p RGBA frame, a pixel at a time | 2.9–4.2 ms | 4.5 ms | 72 ms |
| copy that frame out of guest memory | 0.9–1.8 ms | 0.8 ms | 1.4 ms |
| 20 idle small instances | +4–9 MiB private, < 1 MiB working set | +5.5 / 0.4 MiB | +17 / 0.5 MiB |

Without wasmtime's `parallel-compilation` feature, compiling the same
module took 2.7 s. Fenix turns the feature on.

### Windows behaviour

The tests in `crates/fenix-wasm/src/lib.rs` pass on Windows:

- **Runaway loop:** a guest stuck in a loop traps with `Trap::Interrupt`
  at a 50 ms deadline.
- **Runaway recursion:** traps with `Trap::StackOverflow`. The host
  process carries on.
- **Memory past the cap:** growing it traps.
- **Pulley:** runs the same code.

Wasmtime turns guest faults into traps with a vectored exception handler.
Fenix installs no exception handler or panic hook of its own, so nothing
conflicts with it.

## Why Cranelift, compiled here

- **Runtime only, with the registry shipping precompiled code: rejected,**
  though it's the smallest.
  - A precompiled artifact fits one exact wasmtime build and set of
    options. Every Fenix release that moves wasmtime would need the
    registry to rebuild every extension for it.
  - A `.fenixext` installed from a file, the offline and mission case
    from §11, would have to carry artifacts for the exact Fenix it lands
    on.
  - Loading precompiled code is `unsafe` in wasmtime's API: it trusts the
    bytes to be native code wasmtime produced. That widens what the
    package signature has to vouch for, from "this is the wasm" to "this
    is safe machine code".
- **Winch: rejected.**
  - It's nearly the same size, since it still links most of Cranelift.
  - It compiles faster (0.2 s against 0.3–0.9 s), but Fenix compiles once
    per install, so that hardly matters.
  - Its code is 40–55% slower.
- **Pulley: not by default.**
  - Compute is 20–35× slower, and every call costs 5–30× more.
  - It's not smaller in this setup.
  - It stays an optional cargo feature (`pulley`), for a platform without
    a Cranelift backend.
- **Cranelift** costs about 12 MiB. In exchange, compiling happens on the
  machine: offline installs work, a wasmtime upgrade only means compiling
  again, and Fenix never loads native code it didn't produce.
  - Compiling once per extension version takes under a second for a
    mid-sized extension.
  - After that, starting one costs a few milliseconds.

## What it means for the host (Phase 3, step 2)

- **Compiling.** An extension is compiled when it's installed, or the
  first time it loads after a Fenix upgrade.
  - The artifact goes in `<extension>/.compiled/<hash>.cwasm`, where the
    hash is `Engine::precompile_compatibility_hash`.
  - An artifact that's missing or doesn't match is compiled again on the
    extension's worker thread, never the UI thread.
- **Deadlines** are counted in 10 ms ticks: short for a command or
  keystroke hook, longer for activation and background work. A call that
  traps on its deadline is reported on the output page, as any other
  trap.
- **The memory cap** defaults to 128 MiB an extension, more by setting.
- **The surface API**, which VNC and the PDF viewer need (see below): a
  1080p frame copies out of guest memory in about 1 ms and a guest can
  paint one in about 3 ms, so 60 fps leaves room. It should still share
  a buffer rather than copy, once there's a client that needs it.
- **Guests target `wasm32-wasip2`.** The host gives them WASI with
  nothing granted: no files, no network, no environment. Their
  stdout/stderr go to the output page. Everything else comes through
  Fenix's own WIT, under the extension's permissions. The cost of
  `wasmtime-wasi` is measured in the host step.
- **Building extensions in Rust needs the `wasm32-wasip2` target.** On
  the development machine, rustup (installed 2026-10-24 alongside the
  MSI's Rust 1.98, which stays first on PATH) has toolchain 1.98.0 with
  it, used as `~/.cargo/bin/cargo +1.98.0 build --target wasm32-wasip2`.
  A hello-world builds to an 82 KiB component.
- **Async.** WIT v0 doesn't use wasmtime's async support or the
  component-model async proposals. A host function that waits (a
  process, an HTTP request) blocks only its extension's worker thread.

## VNC and the PDF viewer

The plan wanted this spike to prototype VNC at full frame rate and the
PDF renderer options. When the spike ran, both were staying built into
the core (tpedneault/fenix#127), so neither was prototyped.

That changed on 2026-10-24: once the runtime is in, both move into WASM
extensions. That makes the surface API a requirement rather than a
maybe, and puts two prototypes ahead of their ports:

- **VNC.** A guest needs TCP sockets, granted by permission. It decodes
  the protocol and writes frames to a surface. The numbers above (paint
  in about 3 ms, copy in about 1 ms at 1080p) say a full frame rate is
  within reach. Decoding rate and input latency still need measuring.
- **PDF.** PDFium has to run inside the guest, built for wasm, with
  rendering faithful to the native build. The plan's fallback (§5.4,
  option 3) is a companion process, still installed and switched off
  like any other extension.

## The surface API and PDF rendering (2026-10-28)

**Surfaces.** A surface is one of an extension's pages whose content is
pixels it streams (`surface-open`). The worker writes BGRA rectangles,
fills and copies (`surface-write`, `-fill`, `-copy`) into a back buffer
shared with the UI, and `surface-present` copies what changed to the
front buffer. On its next frame the UI uploads only the rectangle that
changed. That's the VNC host's own design, kept: no pixels travel
through the UI's event queue, and a frame half written is never shown.
Keys, the pointer and the wheel come back to the extension through the
`surface-input` export. A surface that captures the keyboard (VNC)
hears every key, pressed and let go, until Ctrl+\ gives it back; one
that doesn't (PDF) has a page's keys. Pointer and cursor positions are
in the surface's own pixels, whichever way it's scaled (`stretch`,
`fit`, `none`). The pane's size and the display's scale come as
`surface-resized`. A remote desktop asks to be resized to it, and the
PDF viewer renders at it.

**PDF: a companion process, not PDFium compiled to wasm. Decided.**

- **PDFium for wasm doesn't exist in a form Fenix can run.** The
  WebAssembly builds there are (pdfium-binaries' `pdfium-wasm`, what
  pdfium-render uses in a browser) are Emscripten modules. They need
  Emscripten's JavaScript runtime, not WASI, and they aren't
  components.
  - Building PDFium for `wasm32-wasip2` means porting Chromium's build of
    it, with FreeType, libjpeg, OpenJPEG, lcms2, zlib, Abseil and
    PartitionAlloc, to a target with no threads and no `mmap`. It would
    also mean carrying that port across every PDFium release.
  - It would also be the biggest guest by far: about 5 MiB of wasm, a
    compile of several seconds per install, and memory beyond the
    128 MiB cap for a long document.
  - Its rendering would be 1.5 to 3 times slower than native.
- **A pure-Rust renderer** (hayro, pdf-rs) runs in the guest today, but
  it doesn't render as faithfully as PDFium: fonts, shading, JBIG2,
  transparency groups. Faithful rendering is what the reader is for.
- **So the PDF extension runs `fenix-pdfium`, a small companion
  program.**
  - It wraps the same `fenix-pdf` crate Fenix uses today, so pages
    render exactly as they do now.
  - It's built and signed with Fenix's own release, beside `fenix.exe`.
    It isn't shipped in the extension's package, so the rule above holds:
    no native code from the registry or a package.
  - It loads the same `pdfium.dll` (or `.so`, `.dylib`) the user puts
    there now (BUILDING.md).
  - The extension finds it with `find-program("fenix-pdfium")` and starts
    it with `spawn-raw`, allowed by `permissions.process`.
  - It sends requests on stdin and reads replies on stdout as bytes
    (`process-bytes`), framed by length: pages' sizes, rendered pages as
    BGRA, a page's characters and their boxes, search matches, links,
    the outline.
  - It writes rendered pages to its surface. Text selection and search
    are boxes over it (`surface-marks`), worked out from the characters'
    boxes in the guest. A selection is copied with `clipboard-set`.
- **Cost.** A 1080p page is 8 MiB of BGRA. It crosses a pipe, guest
  memory, then the shared buffer. At the rates measured above (copy in
  about 1 ms, pipes at well over 1 GiB/s), that's a few milliseconds on
  top of PDFium's own render. Rendering happens on a zoom, a resize or a
  new page, not every frame. Scrolling moves what's drawn
  (`surface-copy`) and renders only the strip that came into view.
- **Fenix's binary loses its PDFium bindings, and the reader becomes an
  extension like any other.** Without `fenix-pdfium` or PDFium, the
  extension says so and points to BUILDING.md, as the built-in reader
  did.
- **Done:** `crates/fenix-pdfium` is the program, its protocol in
  `src/protocol.rs` (the extension, `fenix.pdf`, has a copy). A render is
  of a part of a page, the page laid out at a size, so the reader asks
  only for what's in sight; the program also says when an open file is
  written again. The release builds it with `fenix` and puts it beside
  `fenix.exe` in the archive.

**Sockets** (`socket-connect`, `udp-bind`, `udp-send`) run on threads
of their own, as processes do. What comes in is delivered as events.
A reader that gets 8 MiB ahead of its extension waits, so a busy VNC
server can't fill Fenix's memory. Every address is held to
`permissions.sockets`: `host:port`, port ranges, `*.domain`, `tcp://`
or `udp://` only, `:port` for a UDP port of this machine's, and
`${setting:key}` for the addresses a setting holds (`vnc.hosts`' rows,
`ccsds.sources`' URLs).
