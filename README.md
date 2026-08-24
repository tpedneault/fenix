# Fenix

A from-scratch, keyboard-first text editor written in Rust, built on
[`wgpu`](https://github.com/gfx-rs/wgpu) and [`winit`](https://github.com/rust-windowing/winit).
Fenix is modal (Vim-grammar editing) with a `SPC`-leader mnemonic layer
(Doom Emacs/Spacemacs-style) on top, a real file explorer, project-aware
fuzzy pickers, tree-sitter syntax highlighting, and a startup dashboard.

This is an early, personal project — expect rough edges. It's shared as-is
for anyone curious to poke around or build on it.

## Features

- **Modal editing**: Normal/Insert/Visual (char/line/block)/Replace/Command
  modes, the standard motion set (`h j k l w b e 0 ^ $ gg G f F t T ; , % { }`),
  operators (`d c y` composing with motions and text objects, plus the
  doubled `dd`/`cc`/`yy` forms), `iw`/`aw` text objects, numeric counts
  (`3dw`, `2dd`, ...), registers-free yank/paste, undo/redo, search
  (`/`, `?`, `n`, `N`, `*`, `#`) with a live incsearch preview and
  persistent match highlighting while it's active, `:s` substitute with
  backreferences, indentation (`>>`/`<<`, auto-indent, `:set
  shiftwidth=N`).
- **`SPC`-leader menu** with a live which-key popup showing available
  continuations as you type a sequence.
- **Syntax highlighting** via tree-sitter for Rust, TOML, Markdown, JSON,
  YAML, Python, JavaScript/TypeScript/TSX, C, Bash, Tcl, Dockerfile/
  Containerfile, and Batch (`.bat`/`.cmd`). Docker Compose files already
  get full highlighting for free via the existing YAML support -- no
  separate grammar needed. `Dockerfile`/`Containerfile` are detected by
  filename (they conventionally have no extension), including per-stage
  names like `Dockerfile.prod`. In Tcl, a bare word in command position
  is only colored as a command if it's actually known -- a built-in, a
  ctags-scanned project definition, or a symbols-file entry (the same
  three sources autocompletion draws from), matched against its
  fully-qualified path with an optional leading `::` -- not just any
  word that happens to be first on a line.
- **File explorer** (dired-style): `SPC f j` opens a real, Vim-navigable
  buffer (splittable, closable with `SPC b k`, listed in `SPC b b`) --
  `Enter` opens a file or navigates into a directory, `-` goes up, `R`
  refreshes, `.` toggles hidden files; ordinary motions (`j k gg G /`)
  work for free since it's real text. A persistent sidebar (`SPC e t`)
  is also available, with the fuller dired feature set (git-status
  badges, marking, batch create/rename/copy/move/delete, inline subtree
  expansion) -- those aren't yet wired up for the buffer-backed form.
- **Project tooling**: fuzzy find-file (`SPC p f`), project-wide search via
  ripgrep (`SPC p s`), switch between known projects (`SPC p p`).
- **Windows, buffers, workspaces**: splits (`SPC w v`/`SPC w s`) with each
  pane keeping its own independent cursor and scroll position, directional
  navigation, a buffer switcher (`SPC b b`), and Doom-Emacs-style
  workspaces (`SPC TAB`).
- **Startup dashboard**: a real, Vim-navigable buffer listing known
  projects and recent files, shown when Fenix is launched with no file
  argument (`SPC d d` to reopen it later).
- **Autocompletion** for Tcl: a popup sourced from a built-in keyword
  list, [Universal Ctags](https://ctags.io/)-scanned project definitions,
  and an optional external symbols file (see [Configuration](#configuration)).
  Namespaced procs show their fully-qualified path (`myns::subns::proc`,
  no leading `::`), not just the bare proc name.
- **Themes**: `Orbit Dark`, `TempleOS`, `Gruvbox Dark`, `Nord`, `Dracula`,
  `Solarized Dark`, and `One Dark`, cycled at runtime (`SPC t t`) or
  jumped to directly by name with a fuzzy picker (`SPC t p`), persisted
  either way.

## Building

Requires a recent stable Rust toolchain (edition 2021).

```bash
cargo build --release
```

The binary is `target/release/fenix-gui`. To run without building a
release binary first:

```bash
cargo run -p fenix-gui              # opens the startup dashboard
cargo run -p fenix-gui -- path/to/file
```

### Optional external tools

Some features shell out to standard tools if they're present on `PATH`,
and degrade gracefully (never a hard error) if they're not:

- [`ripgrep`](https://github.com/BurntSushi/ripgrep) (`rg`) — project-wide
  search (`SPC p s`).
- [`git`](https://git-scm.com/) — git-status badges in the file explorer.
- [`Universal Ctags`](https://ctags.io/) (`ctags`) — project-definition
  completion for Tcl.

### Running the tests

```bash
cargo test --workspace
```

## Keybindings

Fenix follows real Vim for editing and a Doom-Emacs-style `SPC` leader
for everything else. `SPC` starts a leader sequence from Normal mode; a
popup shows what keys continue it.

### Leader (`SPC ...`)

| Keys | Action |
|---|---|
| `SPC f s` | Save |
| `SPC f j` | Open the file explorer at the current file's directory |
| `SPC q q` | Quit |
| `SPC t n` | Cycle line numbers (off / absolute / relative) |
| `SPC t t` | Cycle theme |
| `SPC t p` | Pick a theme by name (fuzzy picker) |
| `SPC t =` / `SPC t -` / `SPC t 0` | Font size: increase / decrease / reset |
| `SPC e t` | Toggle the file explorer sidebar |
| `SPC p f` | Find file in project |
| `SPC p s` | Search project (ripgrep) |
| `SPC p p` | Switch project |
| `SPC p a` / `SPC p d` | Add / remove a project from the known-projects list |
| `SPC d d` | Open the startup dashboard |
| `SPC c r` | Refresh completion tags (re-scans with ctags, re-reads the symbols file) |
| `SPC w v` / `SPC w s` | Split window vertically / horizontally |
| `SPC w h/j/k/l` | Move focus between windows |
| `SPC w w` | Cycle to the next window |
| `SPC w q` / `SPC w o` / `SPC w =` | Close window / close all others / balance splits |
| `SPC b b` | Switch buffer |
| `SPC b n` / `SPC b p` | Next / previous buffer |
| `SPC b k` | Kill (close) the focused buffer |
| `SPC b X` | New scratch buffer |
| `SPC TAB n` | New workspace |
| `SPC TAB ]` / `SPC TAB [` | Next / previous workspace |
| `SPC TAB d` | Remove the active workspace |

### File explorer sidebar (`SPC e t`)

| Keys | Action |
|---|---|
| `j` / `k` | Move down / up |
| `l` / `Enter` | Open |
| `h` / `-` | Go to parent directory |
| `Tab` | Expand / collapse a directory |
| `m` / `u` / `U` / `t` | Mark / unmark / unmark all / toggle all marks |
| `D` | Delete (marked, or entry under cursor) |
| `R` | Rename |
| `c` / `+` | Create file / directory |
| `C` / `M` | Copy / move to... |
| `.` | Toggle hidden files |
| `g r` | Refresh |
| `S` | Select this directory (when picking a project root) |
| `q` / `Esc` | Quit |

### Dired buffer (`SPC f j`)

A real buffer, so every ordinary Vim motion works (`j k gg G / n N ...`).
Only these are special:

| Keys | Action |
|---|---|
| `Enter` | Open the file, or navigate into the directory, at point |
| `-` | Go to the parent directory |
| `R` | Refresh |
| `.` | Toggle hidden files |

### Autocompletion popup (Tcl, Insert mode)

| Keys | Action |
|---|---|
| `Ctrl-Space` | Force-open the popup (even with no prefix typed) |
| `Up`/`Down` or `Ctrl-P`/`Ctrl-N` | Move selection |
| `Tab` / `Enter` | Accept the selected candidate |
| `Esc` | Dismiss the popup (stays in Insert mode) |

## Configuration

Fenix reads a single INI-format settings file:

- **Windows**: `%AppData%\fenix\config.ini`
- **Linux/macOS**: `~/.config/fenix/config.ini` (or wherever
  `$XDG_CONFIG_HOME`/the platform's config directory points)

It's created automatically the first time you change a setting at
runtime (theme cycling, font size, `:set shiftwidth=N`); you can also
hand-edit it directly. Every key is optional — a missing or unparsable
value just falls back to the built-in default instead of failing to
load.

```ini
[editor]
theme = TempleOS
font_size = 16
font_family = Fira Code
indent_width = 4

[completion]
symbols_file = /home/you/tcl-symbols.txt
```

| Section | Key | Meaning |
|---|---|---|
| `editor` | `theme` | `Orbit Dark`, `TempleOS`, `Gruvbox Dark`, `Nord`, `Dracula`, `Solarized Dark`, or `One Dark` (case-insensitive) |
| `editor` | `font_size` | Body text size in points |
| `editor` | `font_family` | Body text font family, by name, as installed on your system. Overrides whatever the active theme names; unset falls back to the theme's own choice (and from there to your system's default monospace font) |
| `editor` | `indent_width` | Spaces per indent level (`>>`/`<<`, Tab, auto-indent) |
| `completion` | `symbols_file` | Path to a plain-text symbols list, one identifier per line (blank lines and `#`-comments ignored), merged into the Tcl completion popup |

Known projects (`SPC p a`/`SPC p d`) and recently-opened files (used by
the dashboard) are stored separately as plain newline-separated path
lists in the same directory (`projects.txt`, `recent_files.txt`) — they're
data, not settings, so they don't live in `config.ini`.

## Architecture

Fenix is a Cargo workspace split into small, mostly host-agnostic
crates, each independently unit-tested (`cargo test --workspace`):

| Crate | Role |
|---|---|
| `fenix-core` | The rope-backed `Buffer`/`Cursor`, undo/redo |
| `fenix-keymap` | Generic key-sequence trie (`KeyPress`, `KeyTrie`, `Matcher`) — shared by Vim's normal/visual keymaps and the leader menu |
| `fenix-vim` | Modal editing: motions, operators, text objects, search/substitute, indentation |
| `fenix-syntax` | tree-sitter-backed incremental parsing and highlight-span extraction |
| `fenix-buffers` | The open-buffer registry (`BufferId` → buffer/cursor/syntax state) |
| `fenix-window` | A generic split-window tree (layout, navigation, resize) — no knowledge of buffers |
| `fenix-explorer` | Directory listing, marking, file operations, git-status — no GPU/rendering |
| `fenix-picker` | Generic fuzzy matching + live-filtered candidate list, used by every fuzzy-finder |
| `fenix-project` | Project-root detection, ripgrep/fd shelling, known-projects/recent-files persistence |
| `fenix-completion` | Completion sources: Tcl keywords, ctags-scanned definitions, external symbols file |
| `fenix-config` | The unified `config.ini` reader/writer |
| `fenix-gui` | Everything GPU/window-facing: `wgpu` rendering, `winit` input, and `App`, which wires all of the above together |

## License

No license has been chosen yet — treat this as source-available for
reference until one is added.
