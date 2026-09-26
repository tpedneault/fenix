<p align="center">
  <img src="docs/brand/fenix-lockup-dark-bg.png" alt="Fenix" height="72">
</p>

<p align="center">
  A keyboard-first text editor, written from scratch in Rust.
</p>

<p align="center">
  <a href="https://github.com/tpedneault/fenix/actions/workflows/ci.yml"><img src="https://github.com/tpedneault/fenix/actions/workflows/ci.yml/badge.svg?branch=master" alt="CI"></a>
  <a href="https://github.com/tpedneault/fenix/releases/latest"><img src="https://img.shields.io/github/v/release/tpedneault/fenix" alt="Latest release"></a>
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" alt="License: MIT OR Apache-2.0"></a>
</p>

Fenix is a modal editor: Vim's editing grammar, with a Doom Emacs–style
`SPC` leader layer on top for everything else. It's GPU-rendered with
[`wgpu`](https://github.com/gfx-rs/wgpu) and
[`winit`](https://github.com/rust-windowing/winit), and brings the
tools of a working day into the editor as pages you move between with
the keyboard: projects, Git and code review, tasks, terminals,
containers, and more.

## Highlights

- **Real Vim editing**: motions, operators, text objects, counts,
  registers, macros, marks, the jumplist, `:s` with backreferences, and
  Visual char/line/block modes.
- **`SPC` leader with which-key**: every command is a short mnemonic
  sequence, and a popup shows what comes next as you type.
- **Projects**: a project hub, fuzzy file and symbol pickers, ripgrep
  search and replace, a new-project wizard with templates, and per-project
  settings, tasks, language servers and debug launches.
- **Code intelligence**: tree-sitter highlighting and folding, LSP
  (completion, hover, diagnostics, rename with a multi-file preview), a
  DAP debugger, a task runner, and native snippets.
- **Git as a daily driver**: a status page, hunk staging, blame, a
  commit graph, interactive rebase, conflict resolution, worktrees, and
  undo for Git operations.
- **Code review**: GitHub pull requests and GitLab merge requests, with
  review threads you can read, answer and resolve without leaving Fenix.
- **More in the editor**: a file explorer and file manager, terminals in
  panes, a Docker/Podman panel, a PDF viewer, VNC console panes, and a
  personal agenda with time tracking that syncs with Jira.
- **Built not to lose work**: staged, atomic saves, crash recovery for
  unsaved buffers, and session restore of windows, splits and cursors.
- **Specialist support**: Arduino sketches (build, upload, serial
  monitor), Tcl, and SCOS-2000 MIB tables.

See [Features](docs/FEATURES.md) for the full tour.

## Install

Download the latest build for Windows or Linux from
[Releases](https://github.com/tpedneault/fenix/releases/latest), unpack
it, and run `fenix` (`fenix.exe` on Windows).

Or build it yourself with a recent stable [Rust](https://rustup.rs)
toolchain:

```bash
git clone https://github.com/tpedneault/fenix
cd fenix
cargo build --release      # the binary is target/release/fenix
```

Fenix runs without anything else installed. Some features use a tool
when it's on your `PATH` (`git`, `rg`, `docker`/`podman`, `ctags`,
`arduino-cli`, language servers), and the PDF viewer needs the PDFium
library. [Building](docs/BUILDING.md) lists them all.

**Platforms:** Windows 10 and 11 are the primary platform. Linux is
supported, and CI tests on both.

## First steps

Run `fenix` to open Home, or `fenix path/to/file` to open a file. Press
`SPC` in Normal mode and the popup shows what you can do next. A few
places to start:

| Keys | What it does |
|---|---|
| `SPC SPC` | Find a file in the project |
| `SPC p p` | Project hub: open, create or check a project |
| `SPC e t` | File explorer sidebar |
| `SPC p s` | Search the project |
| `SPC g g` | Git status page |
| `SPC o T` | A shell in the focused pane |
| `SPC ,` | Every setting on one page |
| `SPC q q` | Quit |

## Documentation

- [Features](docs/FEATURES.md): everything Fenix does, by area
- [Keybindings](docs/KEYBINDINGS.md): the leader menu and every page's keys
- [Configuration](docs/CONFIGURATION.md): `settings.toml`, where files
  live, and every setting
- [Building](docs/BUILDING.md): building from source, and the optional tools
- [Snippets](docs/SNIPPETS.md), [project tools](docs/PROJECT_TOOLS.md)
  and [session restoration](docs/SESSION_RESTORATION.md)
- [Architecture](docs/ARCHITECTURE.md): how the workspace is split into crates
- [Continuous integration](docs/CI.md)
- [Changelog](CHANGELOG.md)

## Contributing

Bug reports and ideas are welcome in
[Issues](https://github.com/tpedneault/fenix/issues). Before opening a
pull request, please read [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Fenix is licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Bundled fonts and vendored code keep their own licenses;
see [THIRD-PARTY.md](THIRD-PARTY.md).

Unless you explicitly state otherwise, any contribution you
intentionally submit for inclusion in Fenix, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms
or conditions.
