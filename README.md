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
the keyboard: projects, Git, coding agents, a debugger, terminals, and
whatever else you add from the extension store.

## Highlights

- **Real Vim editing**: motions, operators, text objects, counts,
  registers, macros, marks, the jumplist, `:s` with backreferences, and
  Visual char/line/block modes.
- **`SPC` leader with which-key**: every command is a short mnemonic
  sequence, and a popup shows what comes next as you type.
- **Projects**: a project hub, fuzzy file and symbol pickers, a search
  page with replace across the project, a file explorer and file manager,
  a new-project wizard with templates, and per-project settings, tasks,
  language servers and debug launches.
- **Code intelligence**: tree-sitter highlighting and folding, LSP
  (completion, hover, diagnostics, rename with a multi-file preview), a
  task runner, and native snippets.
- **A debugger in the code**: breakpoints and the current line in the
  gutter, values written inline, a DEBUG mode with the usual F-keys, and
  a dock for the stack, variables, watches and console (any DAP adapter).
- **Coding agents**: Claude Code, Codex, Devin, Gemini CLI or any agent
  that speaks the Agent Client Protocol, in a dock beside your code,
  every change they make reviewable hunk by hunk.
- **Git as a daily driver**: a status page, hunk staging, blame, a
  commit graph, side-by-side compare, interactive rebase, conflict
  resolution, worktrees, and undo for Git operations.
- **Extensions**: sandboxed WebAssembly extensions, installed from the
  built-in store (`SPC h x`). Code review for GitHub and GitLab, containers,
  an agenda that syncs with Jira, a notebook with Mermaid diagrams, VNC
  consoles, a PDF reader, language packs, themes, and specialist tools
  (Arduino, Tcl, SCOS-2000 MIB, CCSDS) all ship this way.
- **Built not to lose work**: staged, atomic saves, crash recovery for
  unsaved buffers, and session restore of windows, splits and cursors.
- **Learn it inside it**: an interactive tutorial (`SPC h T`) with
  lessons for editing, Fenix's pages, Git, and the extensions you install.

See [Features](docs/FEATURES.md) for the full tour.

## Install

**Windows:** download the installer from
[Releases](https://github.com/tpedneault/fenix/releases/latest)
(`fenix-<version>-x86_64-pc-windows-msvc-setup.exe`). It installs for you
alone without administrator rights, or for everyone on the machine; puts
Fenix in the Start menu and on `PATH`; can add "Open with Fenix" to
Explorer; and upgrades an older install in place. A portable zip is
published beside it. Or, with [Scoop](https://scoop.sh):

```powershell
scoop bucket add fenix https://github.com/tpedneault/scoop-fenix
scoop install fenix
```

**Linux:** download the `.deb` or `.rpm` from
[Releases](https://github.com/tpedneault/fenix/releases/latest) and install
it (`sudo apt install ./fenix_<version>_amd64.deb`, or
`sudo dnf install ./fenix-<version>-1.x86_64.rpm`): Fenix in your
applications menu, `fenix` on `PATH`. Or unpack the `.tar.gz` and run
`fenix`. Any distribution with glibc 2.35 or newer.

**From source**, with a recent stable [Rust](https://rustup.rs) toolchain:

```bash
git clone https://github.com/tpedneault/fenix
cd fenix
cargo build --release      # the binary is target/release/fenix
```

Fenix runs without anything else installed. Some features use a tool
when it's on your `PATH` (`git`, `rg`, language servers, debug adapters,
and what individual extensions need). [Building](docs/BUILDING.md) lists
them all.

**Platforms:** Windows 10 and 11 are the primary platform. Linux is
supported, and CI tests on both.

## First steps

Run `fenix` to open Home, or `fenix path/to/file` to open a file. Press
`SPC` in Normal mode and the popup shows what you can do next. The
tutorial (`SPC h T`) is the quickest way in. A few places to start:

| Keys | What it does |
|---|---|
| `SPC SPC` | Find a file in the project |
| `SPC p p` | Project hub: open, create or check a project |
| `SPC e t` | File explorer sidebar |
| `SPC /` | Search the project |
| `SPC g g` | Git status page |
| `SPC o T` | A shell in the focused pane |
| `SPC h x` | Browse and install extensions |
| `SPC ,` | Every setting on one page |
| `SPC q q` | Quit |

## Documentation

**Using Fenix**

- [Features](docs/FEATURES.md): everything Fenix does, by area
- [Keybindings](docs/KEYBINDINGS.md): the leader menu and every page's keys
- [Configuration](docs/CONFIGURATION.md): `settings.toml`, where files
  live, and every setting
- [Coding agents](docs/AGENTS.md): setting up Claude Code, Codex, Devin
  and others, threads, and reviewing what they change
- [Snippets](docs/SNIPPETS.md), [project tools](docs/PROJECT_TOOLS.md)
  and [session restoration](docs/SESSION_RESTORATION.md)
- [The control channel](docs/CONTROL.md): `fenix file:line`, and the
  protocol other tools use to drive Fenix

**Extending and building Fenix**

- [Writing extensions](docs/WRITING_EXTENSIONS.md): the extension API,
  packaging and publishing to the store
- [fenix-extensions](https://github.com/tpedneault/fenix-extensions):
  the extensions in the store, and the registry
- [Building](docs/BUILDING.md), [Architecture](docs/ARCHITECTURE.md) and
  [Continuous integration](docs/CI.md)
- [Design records](docs/adr/): why the extension runtime and the agent
  client are built the way they are
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
