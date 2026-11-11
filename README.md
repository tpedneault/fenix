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

## Who it's for

Developers who want to keep their hands on the keyboard all day, in any
language, without assembling an editor from plugins first. If you
already think in `ciw` and `dap`, Fenix speaks that from the first
keystroke; if you don't, standard keys (Ctrl+C/V, Shift+arrows) work
too, and `SPC` (or `Ctrl+K`) always shows you what comes next. Git, a
debugger, language servers, terminals and coding agents are part of the
editor, set up when you install it.

## Highlights

- **Real Vim editing**: motions, operators, text objects, counts,
  registers, macros, marks, the jumplist, `:s` with backreferences, and
  Visual char/line/block modes.
- **`SPC` leader with which-key**: every command is a short mnemonic
  sequence, and a popup shows what comes next as you type.
- **Or standard keys**: a traditional IDE's -- typing at once, Shift+arrows
  to select, Ctrl+C/X/V, Alt+click for more cursors, F12 and F2 -- with the
  same leader menu on `Ctrl+K` (`editor.keys = "standard"`, `SPC t k`).
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
- **Git as a daily driver**: a status page, hunk staging, blame, a
  commit graph, side-by-side compare, interactive rebase, conflict
  resolution, worktrees, and undo for Git operations.
- **Coding agents**: Claude Code, Codex, Devin, Gemini CLI or any agent
  that speaks the Agent Client Protocol, in a dock beside your code,
  every change they make reviewable hunk by hunk.
- **Built not to lose work**: staged, atomic saves, crash recovery for
  unsaved buffers, and session restore of windows, splits and cursors.
- **Learn it inside it**: an interactive tutorial (`SPC h T`) with
  lessons for editing, Fenix's pages, Git, and the extensions you install.
- **Extensions**: sandboxed WebAssembly extensions, installed from the
  built-in store (`SPC h x`), each allowed only what its manifest asks
  for. Language packs, themes, code review for GitHub and GitLab,
  containers, an agenda that syncs with Jira, a notebook with Mermaid
  diagrams, VNC consoles and a PDF reader all ship this way.

See [Features](docs/FEATURES.md) for the full tour.

### What extensions can do

Extensions get their own pages, keys, settings and tasks, so a
specialist workflow can be as at home as Git is. The store has a few
that show how far that goes: Arduino boards built, uploaded and
monitored from the editor, Tcl, and tools for spacecraft operations --
SCOS-2000 MIB databases and CCSDS packets. They're there for the people
who need them, and as examples of what you can build:
[Writing extensions](docs/WRITING_EXTENSIONS.md) shows how.

## How Fenix compares

Honest differences, so you can tell whether it's for you:

- **Neovim**: the same editing grammar, but Fenix is a graphical
  application with Git, a debugger, language servers, a project search
  page and agents built in and configured from one settings page
  (`SPC ,`), rather than assembled from plugins and Lua. Extensions run
  in a WebAssembly sandbox with declared permissions, not with the
  editor's full access. Neovim runs in a terminal and over plain SSH,
  has a vast plugin ecosystem, and implements more of Vim: Fenix has
  [gaps](https://github.com/tpedneault/fenix/issues?q=is%3Aissue+is%3Aopen+label%3Avim)
  it's closing.
- **Helix**: both are keyboard-first and work out of the box. Helix uses
  a selection-first grammar (select, then act); Fenix keeps Vim's
  verb-then-object grammar, so Vim muscle memory carries over. Fenix
  adds pages for Git, debugging, agents and an extension store; Helix
  runs in a terminal and keeps a smaller core.
- **Zed**: both are native, GPU-rendered and written in Rust, and both
  run agents over the Agent Client Protocol (Zed started it). In Zed,
  Vim mode is one keymap among others; Fenix is built around
  modal editing and the `SPC` leader, and every page -- Git, the
  debugger, settings, the store -- is driven from the keyboard. Zed has
  real-time collaboration and runs on macOS; Fenix doesn't yet.
- **VS Code**: Fenix has much of what you'd install extensions for in VS
  Code -- Git, a debugger over DAP, language servers, terminals, agents
  -- in a native window rather than Electron. VS Code's extension
  ecosystem and remote development are far larger and more mature; if
  you depend on a specific extension, check the
  [store](https://github.com/tpedneault/fenix-extensions) first.

## Is Fenix ready for daily use?

For many people, yes -- with eyes open. Fenix is young: 1.0 shipped on
31 October 2026.

- **Platforms**: Windows 10 and 11 are the primary platform; Linux
  (x86_64, glibc 2.35 or newer) is supported. CI builds and tests on
  both. **macOS isn't built or tested yet**, and there's no desktop
  build for ARM Linux.
- **Solid**: Vim editing, files and projects, saves and crash recovery,
  Git, language servers, the debugger, terminals, coding agents and the
  extension store. Each is covered by the test suite CI runs on every
  change.
- **Still in progress**:
  - Remote projects over SSH are a preview: files, the explorer and
    finding files work; terminals, Git, search, language servers, tasks,
    the debugger and agents don't run on a remote project yet
    ([plan](docs/plans/remote-ssh.md)).
  - Some Vim motions, operators and Insert-mode keys are missing
    ([#22](https://github.com/tpedneault/fenix/issues/22),
    [#23](https://github.com/tpedneault/fenix/issues/23),
    [#25](https://github.com/tpedneault/fenix/issues/25)).
  - Accessibility: no screen-reader support yet, right-to-left text
    doesn't lay out ([#17](https://github.com/tpedneault/fenix/issues/17)),
    and Unicode text sent by some input tools doesn't arrive
    ([#37](https://github.com/tpedneault/fenix/issues/37)).
  - Performance: Fenix redraws while idle
    ([#74](https://github.com/tpedneault/fenix/issues/74)), and startup
    slows with many extensions
    ([#33](https://github.com/tpedneault/fenix/issues/33)).
- **Your settings and files are safe across upgrades**: within 1.x, an
  upgrade keeps existing settings, stored data and keys working
  ([how versions work](docs/RELEASING.md)).

[ROADMAP.md](ROADMAP.md) says what's being worked on next.

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

**The nightly**, to try what's next: `master` built each night it
changed, on the
[`nightly` release](https://github.com/tpedneault/fenix/releases/tag/nightly)
-- the same installer, zip, archive, `.deb` and `.rpm`, under names
that don't change (`fenix-nightly-x86_64-pc-windows-msvc-setup.exe`,
`fenix-nightly_amd64.deb`, ...). It installs over a release, and the
next release installs over it. `fenix --version` says which nightly it
is and its commit. [Releasing](docs/RELEASING.md#for-users-running-the-nightly)
has the rest.

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
supported, and CI tests on both. macOS isn't built yet (see
[Is Fenix ready for daily use?](#is-fenix-ready-for-daily-use)).

**Updates:** Fenix can tell you on Home when a newer release is out
(`updates.check`, off until you turn it on), and with
`updates.channel = "nightly"` when a newer nightly is out too. It never
downloads anything itself: see [Releasing](docs/RELEASING.md) for what
each kind of release carries.

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
- [Differences from Vim](docs/VIM_DIFFERENCES.md): what Fenix's Vim keys
  do otherwise, or don't do
- [Configuration](docs/CONFIGURATION.md): `settings.toml`, where files
  live, and every setting
- [Coming from another editor](docs/COMING_FROM.md): Fenix for Neovim,
  Helix and VS Code users, and bringing your settings over
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
- [Roadmap](ROADMAP.md) and [how releases work](docs/RELEASING.md)

## Contributing

Bug reports and ideas are welcome in
[Issues](https://github.com/tpedneault/fenix/issues). Before opening a
pull request, please read [CONTRIBUTING.md](CONTRIBUTING.md).

Found something that could put Fenix's users at risk? Please report it
privately: [SECURITY.md](SECURITY.md) says how.

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
