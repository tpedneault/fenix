# Building Fenix

Requires a recent stable Rust toolchain (edition 2021).

```bash
cargo build --release
```

The binary is `target/release/fenix`. To run without building a
release binary first:

```bash
cargo run -p fenix-gui              # opens Home
cargo run -p fenix-gui -- path/to/file
```

## Optional external tools

Some features shell out to standard tools if they're present on `PATH`,
and degrade gracefully (never a hard error) if they're not:

- [`ripgrep`](https://github.com/BurntSushi/ripgrep) (`rg`) — project-wide
  search (`SPC p s`).
- [`git`](https://git-scm.com/) — git-status badges in the file explorer.
- [`Universal Ctags`](https://ctags.io/) (`ctags`) — project-definition
  completion for Tcl (`SPC m s`, `SPC m T` in a Tcl file). If it's missing, exits
  non-zero, or produces output this parser doesn't recognize, the
  reason is logged to stderr rather than just silently yielding no
  definitions — start Fenix with `--console` to read it (see
  [Reading what Fenix prints](#reading-what-fenix-prints)).
- [`docker`](https://docs.docker.com/engine/) or [`podman`](https://podman.io/)
  — the Docker panel (`SPC d d`). Fenix probes `docker` first and falls
  back to `podman` if `docker` isn't runnable (auto-detected once per
  run) — so a plain Podman install works with no configuration, and a
  `podman-docker` compatibility shim (where `docker` itself resolves to
  Podman) works too, indistinguishably. With neither on `PATH` (or an
  unreachable daemon) the panel just shows an empty listing instead of
  failing.

- [`arduino-cli`](https://arduino.github.io/arduino-cli/) — Arduino
  sketches (`SPC m ...` in a sketch). Found on `PATH`, in its installer's
  default location, or at `[embedded] arduino_cli`. Each board also
  needs its board package installed once (`SPC m c`, or `arduino-cli
  core install arduino:avr` for the Uno/Nano/Mega).
- [`arduino-language-server`](https://github.com/arduino/arduino-language-server)
  and Arduino's build of [`clangd`](https://github.com/arduino/clang-static-binaries)
  — completion and live errors in sketches. Looked for in Fenix's tools
  folder (`%LocalAppData%\fenix\tools`, each in its own subfolder as its
  release archive unpacks), on `PATH`, and inside an Arduino IDE 2
  install; `[embedded]` can point at them instead. Without them, sketches
  still build, upload and monitor; `SPC m i` says what's missing.

The PDF viewer (`SPC r ...`) needs a native library rather than a
`PATH` executable, so it's set up once by hand rather than
auto-detected:

- [`pdfium`](https://github.com/bblanchon/pdfium-binaries) — download
  the prebuilt release for your platform (`pdfium-win-x64.tgz` on
  Windows) and place `pdfium.dll` (or `libpdfium.so`/`libpdfium.dylib`
  elsewhere) next to `fenix.exe`, i.e. in whichever `target/debug/` or
  `target/release/` directory you actually run the built binary from.
  `FENIX_PDFIUM_PATH` can point at a different directory instead (handy
  for switching between `debug`/`release` builds without copying it
  twice), and a system-wide install is tried as a last resort. Without
  it, opening a `.pdf` shows a status-line error naming where it looked
  rather than a blank pane or a crash.

## Running the tests

```bash
cargo test --workspace
```

## Reading what Fenix prints

Fenix writes problems it can't show on screen, such as a language
server that won't start or a ctags failure, to stderr. On Windows it
starts without a console window, so run it with `--console` to see
them:

```powershell
fenix --console              # from a terminal: prints there
fenix --console 2> fenix.log # or keep it in a file
```

Started from a terminal, Fenix prints into that terminal. Note that
PowerShell and `cmd` don't wait for a windowed program, so their prompt
comes back straight away and the output arrives underneath it. Started
from a shortcut or Explorer, Fenix opens a console window of its own;
closing that window closes Fenix too. On Linux, starting Fenix from a
terminal is enough, and the flag does nothing.
