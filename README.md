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
  (`3dw`, `2dd`, ...), yank/paste mirrored onto the OS clipboard (the
  unnamed register only -- `y`/`d`/`c` push to it, `p`/`P` pull from it
  first, so copying in Fenix and pasting elsewhere -- or vice versa --
  just works), named registers (`"a`-`"z`/`"A`-`"Z` to select one for the
  next `y`/`d`/`c`/`x`/`s`/`p`/`P`, uppercase appends), undo/redo, search
  (`/`, `?`, `n`, `N`, `*`, `#`) with a live incsearch preview and
  persistent match highlighting while it's active, `:s` substitute with
  backreferences, indentation (`>>`/`<<`, auto-indent, `:set
  shiftwidth=N`).
- **Macros** (`q{a-zA-Z}` to record, a second bare `q` to stop, `@
  {register}` to replay, `@@` to repeat whichever was last played,
  `3@a`-style count prefix): real Vim's own model, not a separate
  storage -- a macro is just a named register's text (Vim's own keycode
  notation, `<Esc>`, `<C-r>`, ...), so `"ap` pastes a recorded macro's
  literal keystrokes as text and yanking text into a register makes it
  `@`-executable. Recording captures every keystroke, including
  `SPC`-leader sequences and prompts, not just what reaches Vim's own
  motion/operator dispatch. A self-referential macro is bailed out by a
  depth/key-count guard rather than actually hanging, unlike real Vim.
- **Jumplist** (`Ctrl-O`/`Ctrl-I`): back/forward through recent cursor
  positions, recorded on `gg`/`G`/`%`/a confirmed search/`*`/`#` and on
  jumping to a symbol definition, a grep match, a quickfix entry, or a
  mark -- so jumping to a definition (`SPC c s`), a search hit
  (`SPC p s`), or a mark (`` `a ``) and hitting `Ctrl-O` takes you right
  back, even across files. A single global back/forward pair of stacks
  (like a browser's history), not real Vim's per-window circular list --
  a disclosed simplification.
- **Marks** (`m{a-zA-Z}` to set, `` `{mark} ``/`'{mark}` to jump --
  exact position vs. the mark's line's first non-blank, real Vim's own
  split): named position bookmarks that work across files, unlike real
  Vim's lowercase-is-buffer-local/uppercase-is-global distinction, which
  Fenix doesn't replicate -- every mark can jump to wherever it was set,
  any file. Not composable with operators (no `` d`a `` / `d'a`) --
  jump-only.
- **`SPC`-leader menu** with a live which-key popup showing available
  continuations as you type a sequence -- reachable from Visual mode as
  well as Normal, so e.g. `SPC c f` (format the selection) can act on an
  active selection without leaving it first.
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
  `SPC f e` starts that same fuller explorer at your home directory
  instead of the current file's -- for a file that isn't in any project
  and isn't worth typing an absolute path for (something in
  `~/Downloads`, say): navigate down with the usual `j k l h`/`Enter`
  and open it directly, or press `S` once you're close to fuzzy-search
  every file under wherever you've navigated to, recursively (the same
  candidate list `SPC p f` builds, just rooted there instead of the
  project).
- **Files menu** (`SPC f ...`): `SPC f f` opens a file by typing its
  path (`~` expands to home, a relative path resolves against the
  project root) -- unlike every fuzzy finder here, it doesn't enumerate
  anything, so a `.gitignore`'d file (`.env`, say) opens exactly like
  any other, and a path that doesn't exist yet opens an empty buffer to
  save later. `SPC f a` is `SPC p f`'s fuzzy-find sibling but *including*
  gitignored files, for when you want to search by name rather than type
  the exact path. `SPC f r` fuzzy-finds a recently-opened file. `SPC f
  R`/`SPC f D`/`SPC f y` rename/delete (with confirmation)/copy-the-path
  of the file currently open.
- **Project tooling**: fuzzy find-file (`SPC p f`), project-wide search via
  ripgrep (`SPC p s`) with the result list kept around afterward as a
  quickfix list -- `SPC p n`/`SPC p N` step to the next/previous match
  directly in the editor without reopening the picker or re-running the
  search, clamping (not wrapping) at either end -- switch between known
  projects (`SPC p p`).
- **Search and replace** (`SPC s ...`): `SPC s s` fuzzy-finds any line in
  the current buffer (live-filtered as you type). `SPC s r` prompts for a
  pattern then a replacement and shows the match count before applying --
  a real UI over the same regex engine `:s` already uses, scoped to the
  current Visual selection's lines if invoked from Visual mode (mirroring
  real Vim's own `:'<,'>s`), the whole buffer otherwise. `SPC s p` does
  the same across the whole project: searches with ripgrep (respecting
  `.gitignore` by default, so build/generated files never show up),
  groups matches by file, and opens a real, navigable review buffer --
  toggle files out with `Space`, apply with `a`/`Enter` behind a y/n
  confirmation. An already-open file is edited in memory (left dirty, for
  you to review/save); anything else is edited on disk directly. One
  fresh regex pass per included file, not a snapshot from search time --
  a file that changed since the search is safely skipped rather than
  misapplied.
- **Windows, buffers, workspaces**: splits (`SPC w v`/`SPC w s`) with each
  pane keeping its own independent cursor and scroll position, directional
  navigation, a buffer switcher (`SPC b b`), and Doom-Emacs-style
  workspaces (`SPC TAB`). Every pane shows a small title bar naming its
  buffer (the filename, or a placeholder like `*dashboard*`/`*docker*`/
  a dired buffer's own directory for one with no path) -- with a split
  open, two different files are labeled at a glance, not just whichever
  one happens to be focused (the modeline only ever names that one).
  The focused pane's title is colored with an accent so it's obvious at
  a glance which one has focus, whether you're editing, or inside the
  Docker or Git panel. On the Docker and Git panels specifically, every
  title is also prefixed with a number (`1. Containers`, `2. Images`,
  ...) -- pressing that digit jumps focus straight to the matching pane.
- **Modeline**: mode badge, filename, and cursor position on the left;
  a live local date/time clock flush against the right edge, ticking in
  place as you work (omitted rather than overlapping anything if the
  window's too narrow to fit it).
- **Startup dashboard**: a real, Vim-navigable buffer listing known
  projects and recent files, shown when Fenix is launched with no file
  argument (`SPC o d` to reopen it later).
- **Docker panel** (Lazydocker-style): `SPC d d` opens a real, six-pane
  workspace -- Containers/Images/Volumes/Networks on the left (each its
  own real, Vim-navigable buffer with a title bar), Status and Logs
  stacked on the right. Status live-updates to whatever's under the
  cursor in the focused left pane, including a selected container's
  CPU/MEM (which also ticks on its own every ~2s without a keypress);
  Logs is a dedicated pane for streamed log output. Each Containers row
  is just `[X] name`, prefixed with a one-letter, color-coded status
  badge (`R` running, `P` paused, `X` exited, etc.) instead of inline
  text that used to clip at small font sizes. `s`/`S`/`R` start/stop/
  restart the container under the cursor, `r` runs a new container from
  the image under the cursor, `d` removes the container/image/network
  under the cursor (with a `y`/`n` confirmation), `u` refreshes. `l`
  switches Logs into a live tail of that container's logs (`docker logs
  -f`), streaming new lines in and auto-scrolling to the bottom while
  you're already there -- scroll up to read earlier output and it leaves
  you alone until you navigate back to the end. Per-pane keybinding
  hints no longer clip inline either -- press `x` on a Containers/Images/
  Volumes/Networks pane for a Lazydocker-style contextual popup listing
  that pane's available keys; it's purely informational and dismisses on
  the very next keypress, which still does whatever it would normally
  do. `SPC d b` builds an image from the current project root's
  `Dockerfile`; `SPC d q` closes the whole session.
- **Git panel** (Lazygit-style): `SPC g g` opens a real, seven-pane
  workspace -- Status/Staged/Unstaged/Branches/Commits/Stash stacked on
  the left (each its own real, Vim-navigable buffer with a title bar),
  Main on the right showing a diff of whatever's under the cursor in
  Staged/Unstaged/Commits/Stash. Staged and Unstaged are two independent
  views of the same file list (a file that's both staged *and* further
  modified appears in both, since git tracks the two halves separately)
  -- `s`/`S` stage/unstage the file under the cursor from either pane,
  `a`/`A` stage/unstage everything, `c` commits (prompts for a message),
  `d` on Unstaged discards the file under the cursor (`y`/`n` confirm,
  handling untracked files correctly via `git clean` rather than `git
  checkout`), `z` stashes every change, `P`/`p` push/pull. Status is a
  fixed repo-overview summary (branch, upstream, ahead/behind, staged/
  unstaged/untracked counts) that live-updates on its own every ~2s via
  a background poller, independent of cursor movement -- the inverse of
  the Docker panel's Status/Logs split, same pattern, roles swapped to
  match what's actually true of each domain. On Branches: `c` checks out
  the branch under the cursor, `n` creates a new one (prompts for a
  name), `d` deletes it (confirm). On Stash: `a` applies the entry under
  the cursor, `g` pops it, `d` drops it (confirm). `u` refreshes the
  whole session from any pane; `x` on Staged/Unstaged/Branches/Commits/
  Stash shows a contextual popup of that pane's keys, same dismiss-on-
  next-keypress convention as Docker's. Real lazygit's own `<space>`
  stage-toggle isn't used here -- `SPC` is already Fenix's global
  leader-key trigger -- so Staged/Unstaged use separate `s`/`S` keys
  instead, matching the Docker panel's own `s`/`S`/`R` precedent.
  Main's diff fetch runs off the input thread (a background thread posts
  the result back when it lands, discarding a slow one a faster later
  selection already superseded) so scrolling through many files never
  blocks the UI waiting on a `git` subprocess. `SPC g q` closes the
  whole session.
- **JIRA dashboard** (`SPC j ...`): track projects and users by hand
  (`SPC j p a`/`SPC j u a` to add, `SPC j p d`/`SPC j u d` to remove),
  then `SPC j j` opens a four-pane workspace -- Projects | Users |
  Issues | Detail, every pane read-only (like the Docker/Git panels --
  it's a generated listing, not something you edit; `x` on any pane
  shows its own key list, the same which-key-style popup Docker/Git
  already have). Moving the cursor onto a tracked user runs a JQL
  search for every issue assigned to them, scoped to every currently-
  tracked project, and lists the results in Issues; moving onto an
  issue shows its full detail (description, status, assignee, reporter,
  dates, comments) in Detail. `SPC j r` refreshes, `SPC j q` closes the
  session, `SPC j g` jumps straight to any issue by key (even one not
  present in the current query). Talks to a self-hosted Jira Server/
  Data Center instance's REST API via a personal access token (see
  [Configuration](#configuration)).
  `SPC j i a` creates a new issue in a tracked project (pick the
  project, type an issue type and a summary). On Issues or Detail: `t`
  transitions the issue's status (fetches the real available
  transitions for its current workflow state and offers a picker --
  never a fixed list), `T` edits the title, `A` reassigns to one of
  your tracked users, `P` changes priority (fetched live from the
  instance's real configured scheme, not a guessed list), `l` logs
  time (Jira's own duration syntax, e.g. `2h 30m`), `y` copies the
  issue's browse URL to the clipboard, and `c`/`e` open a real, Vim-
  navigable scratch buffer -- empty for a new comment, pre-filled with
  the current text for a description edit -- so you get full editing
  power for anything longer than one line; `SPC j s` submits it,
  `SPC j x` discards it. On Issues, `f` opens a multi-select picker
  over every status seen so far this session (`Tab` toggles a status
  on/off without closing the picker, `Enter` applies -- an empty
  selection means "show everything") to hide statuses you don't want
  cluttering the list (e.g. Done/Closed) -- folded into the JQL query
  itself, so it stays applied across refreshes; resets when you close
  and reopen the panel.
- **VNC console panes** (`SPC v ...`): configure VM hosts by hand under
  `[vnc]`, then `SPC v v` fuzzy-picks one by name to open (or switch
  back to) a live VNC connection as an ordinary, splittable pane. Each
  session connects once and stays live in the background indefinitely
  (instant switching thereafter), auto-reconnects with backoff if it
  drops, and throttles its poll rate while unfocused. Mouse and keyboard
  forward straight to the VM while the pane is focused (`Ctrl-\` to
  release, same convention as the terminal panel); clipboard is
  mirrored both ways. `SPC v s` saves the current frame as a PNG.
  Client-side scaling only, and no encryption/authentication at all --
  trusted-network hosts only.
- **PDF viewer** (`SPC r ...`): open a `.pdf` file the same way you'd
  open any other file (typed path, the explorer, a CLI argument) and it
  renders as a scaled-to-fit page in an ordinary, splittable pane instead
  of loading as text. The mouse wheel, `j`/`k` and the arrow keys scroll
  the document continuously -- straight through page boundaries, so a
  scroll never dead-ends at the bottom of a page -- and `PageDown`/
  `PageUp`, `n`/`p`, `Home`/`End` turn/jump pages outright, all as bare
  single keystrokes while a PDF pane is focused. `SPC r g` jumps
  straight to a typed page number; `+`/`-`/`0`/`w` (or `SPC r =`/
  `SPC r -`/`SPC r 0`/`SPC r w`) zoom in/out and fit the page/width, with
  `h`/`l` panning sideways across whatever doesn't fit in the pane at the
  current zoom. The status line shows `Page N/M` and the current zoom in
  place of the line/column an ordinary buffer shows. The render
  re-fits automatically on window resize (except at a fixed percentage
  zoom, which stays put across a resize on purpose). `SPC r o` toggles a
  split-pane outline/bookmarks panel -- a real, Vim-navigable listing
  where `Enter` on an entry jumps the PDF straight to its page. `SPC r /`
  searches the whole document for a word or phrase and lists every match
  (page number plus surrounding context) in its own split pane, `Enter`
  jumping straight to that match's page the same way the outline does.
  Requires `pdfium.dll` (see
  [Optional external tools](#optional-external-tools)) -- without it,
  opening a PDF shows an error instead of a blank pane.
- **Autocompletion**: a popup that's always available, sourced from
  whatever's already been typed in the current buffer (`<C-n>`/`<C-p>`-
  style buffer-word completion, any language) -- layered, for Tcl
  specifically, with a built-in keyword list,
  [Universal Ctags](https://ctags.io/)-scanned project definitions, and
  an optional external symbols file (see
  [Configuration](#configuration)). Namespaced procs show their fully-
  qualified path (`myns::subns::proc`, no leading `::`), not just the
  bare proc name.
- **Symbol picker**: `SPC c s` opens a fuzzy-find popup listing every
  known Tcl definition (`proc`/`namespace`) by its fully-qualified name,
  sourced from the same [Universal Ctags](https://ctags.io/) scan
  autocompletion draws on -- confirming a selection opens the file it's
  defined in (if not already open) and jumps straight to that line.
- **Formatting**: `SPC c f` formats the active Visual selection, `SPC c
  F` the whole buffer, by shelling out to an external formatter for the
  buffer's language -- currently just Tcl, via
  [`tclfmt`](https://github.com/nmoroze/tclint) (see
  [Optional external tools](#optional-external-tools)). `SPC` reaches
  the leader menu from Visual mode as well as Normal for this reason, so
  `SPC c f` can act on a selection without leaving it first.
- **SCOS-2000 MIB** (`SPC m ...`): fuzzy-find and inspect telecommands
  (`SPC m t`), TM packets (`SPC m k`), TM parameters (`SPC m p`), and
  calibration definitions (`SPC m c`, numeric curves/status
  enumerations/range checks) from one or more configured MIB directories
  (see [Configuration](#configuration)) -- each opens a real, Vim-
  navigable buffer with the definition's summary, related rows (a
  telecommand's parameters with their calibration references, a TM
  packet's parameters, a TM parameter's packet occurrences), and raw
  fields. `SPC m i` builds and inserts a telecommand: pick one, build or
  skip its variable arguments (an argument with known engineering
  aliases offers a picker of them; one with a known numeric range warns,
  without blocking, if the typed value falls outside it), review the
  rendered command, confirm to insert at wherever the wizard started.
  `SPC m r` reparses the configured MIB directories from disk. `SPC m
  a` registers a new MIB directory without leaving the editor: browse
  to it in the file explorer, `S` to select it, then type a label --
  persisted to `config.ini` immediately, same as everything else here.
  `SPC m d` fuzzy-finds a configured directory to remove the same way.
  Ported from an ICD 7.2 SCOS-2000 MIB workflow in the author's previous
  (Emacs) config -- see that config's own
  [MIB module](https://github.com/tpedneault/orbit-emacs/blob/master/modules/mod-mib.el)
  for the original.
- **Themes**: `Orbit Dark`, `TempleOS`, `Gruvbox Dark`, `Nord`, `Dracula`,
  `Solarized Dark`, and `One Dark`, jumped to directly by name with a
  fuzzy picker (`SPC t p`), persisted.
- **Terminal panel**: `SPC o t` toggles a real, interactive terminal --
  `powershell.exe` on Windows, `$SHELL` (falling back to `/bin/sh`)
  elsewhere -- as a full-width strip along the bottom of the window,
  under every existing split, with full ANSI color support (16-color,
  256-color, and RGB foreground/background). Toggling it off only hides
  it: the shell process, and anything running in it, keeps running in
  the background, and reopening shows it caught up to wherever it got
  to. `Ctrl-\` unfocuses the terminal without hiding it (mirrors
  Neovim's own `:terminal` convention), so normal Vim window navigation
  (`SPC w ...`) can move focus elsewhere while it stays visible. v1
  limitations: no mouse reporting, no bracketed paste, no F-keys, no
  application-cursor-mode variants -- covers ordinary shell/REPL/pager
  use, not a full terminfo-correct implementation.
- **Table/spreadsheet view**: `SPC f t` toggles the focused buffer
  between plain text and an elastic-column table view of its own,
  genuinely tab-separated content -- real elastic tabstops, not a
  padding trick: the renderer expands each real `\t` to the visual
  column its column needs (computed from the widest value currently in
  it, re-measured after every edit), so the file on disk stays exactly
  what you see, always genuinely tab-separated, and ordinary Vim editing
  (`i`, `cw`, ...) between two tabs just works. `]`/`[` jump to the
  next/previous column, `c` fuzzy-finds one by name, and `j`/`k` are
  reinterpreted to move a row while staying in the same visual column --
  plain char-based motion doesn't track "same column" once rows have
  different raw lengths up to it. Built for browsing MIB `.dat` files
  and any other TSV data, but general-purpose.

## Building

Requires a recent stable Rust toolchain (edition 2021).

```bash
cargo build --release
```

The binary is `target/release/fenix`. To run without building a
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
  completion for Tcl (`SPC c s`, `SPC c r`). If it's missing, exits
  non-zero, or produces output this parser doesn't recognize, the
  reason is logged to stderr rather than just silently yielding no
  definitions — check the terminal Fenix was launched from.
- [`tclfmt`](https://github.com/nmoroze/tclint) (part of the `tclint`
  project — `pip install tclint`) — formatting Tcl buffers/selections
  (`SPC c f`/`SPC c F`). Without it on `PATH`, those keys are a no-op
  (logged to stderr) instead of a hard error.
- [`docker`](https://docs.docker.com/engine/) or [`podman`](https://podman.io/)
  — the Docker panel (`SPC d d`). Fenix probes `docker` first and falls
  back to `podman` if `docker` isn't runnable (auto-detected once per
  run) — so a plain Podman install works with no configuration, and a
  `podman-docker` compatibility shim (where `docker` itself resolves to
  Podman) works too, indistinguishably. With neither on `PATH` (or an
  unreachable daemon) the panel just shows an empty listing instead of
  failing.

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
| `SPC SPC` | Find file in project (same as `SPC p f`) |
| `SPC f s` | Save |
| `SPC f j` | Open the file explorer at the current file's directory |
| `SPC f e` | Open the file explorer at your home directory; open a file directly, or `S` fuzzy-searches recursively from wherever you navigate to |
| `SPC f t` | Toggle the focused buffer between plain text and table view |
| `SPC f f` | Open a file by typing its path (bypasses `.gitignore`) |
| `SPC f a` | Fuzzy-find a file in the project, including gitignored ones |
| `SPC f r` | Fuzzy-find a recently-opened file |
| `SPC f R` | Rename the current file on disk |
| `SPC f D` | Delete the current file (with confirmation) |
| `SPC f y` | Copy the current file's path to the clipboard |
| `SPC q q` | Quit |
| `SPC t n` | Cycle line numbers (off / absolute / relative) |
| `SPC t p` | Pick a theme by name (fuzzy picker) |
| `SPC t =` / `SPC t -` / `SPC t 0` | Font size: increase / decrease / reset |
| `SPC t f` | Toggle fullscreen |
| `SPC t a` | Toggle caret-fade/scroll-ease/yank-pulse animations on/off |
| `SPC e t` | Toggle the file explorer sidebar |
| `SPC p f` | Find file in project |
| `SPC p s` | Search project (ripgrep) |
| `SPC p n` / `SPC p N` | Next / previous match in the last project search (quickfix) |
| `SPC p p` | Switch project |
| `SPC p a` / `SPC p d` | Add / remove a project from the known-projects list |
| `SPC s s` | Fuzzy-find a line in the current buffer |
| `SPC s r` | Search and replace in the current buffer (Visual-scoped if invoked from Visual mode) |
| `SPC s p` | Search and replace across the project |
| `SPC o d` | Open the startup dashboard |
| `SPC o t` | Toggle the terminal panel |
| `SPC d d` | Open (or refocus/refresh) the Docker panel |
| `SPC d b` | Build an image from the current project's `Dockerfile` |
| `SPC d q` | Close the Docker panel session |
| `SPC g g` | Open (or refocus/refresh) the Git panel |
| `SPC g q` | Close the Git panel session |
| `SPC j j` | Open (or refocus) the JIRA dashboard |
| `SPC j p a` / `SPC j p d` | Add / remove a tracked JIRA project |
| `SPC j u a` / `SPC j u d` | Add / remove a tracked JIRA user |
| `SPC j i a` | Create a new issue in a tracked project |
| `SPC j g` | Jump straight to any issue by key |
| `SPC j r` | Refresh the JIRA dashboard's current issues/detail |
| `SPC j q` | Close the JIRA dashboard session |
| `SPC j s` / `SPC j x` | Submit / cancel a pending comment or description edit |
| `SPC v v` | Open (or switch to) a configured VNC session by name |
| `SPC v q` | Close the focused VNC session |
| `SPC v s` | Save the focused VNC session's current frame as a PNG |
| `SPC r n` | Turn the focused PDF session to the next page |
| `SPC r p` | Turn the focused PDF session to the previous page |
| `SPC r g` | Prompt for a page number and jump to it |
| `SPC r [` / `SPC r ]` | Jump to the first / last page |
| `SPC r =` / `SPC r -` | Zoom the focused PDF session in / out |
| `SPC r f` | Open a document from the `config.ini` `[documents]` index |
| `SPC r 0` | Fit the page to the pane |
| `SPC r w` | Fit the page's width to the pane |
| `SPC r o` | Toggle the focused PDF session's outline/bookmarks panel |
| `SPC r /` | Search the focused PDF session's text for a word or phrase |
| wheel, `j` / `k`, `Down` / `Up` | Scroll the document, continuing onto the next/previous page at an edge (PDF panes only) |
| `PageDown` / `PageUp`, `n` / `p` | Next / previous page (PDF panes only) |
| `Home` / `End`, `g` / `G` | First / last page (PDF panes only) |
| `h` / `l`, `Left` / `Right` | Pan sideways while the page is wider than the pane (PDF panes only) |
| `+` / `-` / `0` / `w` / `/` | Zoom in / out, fit page, fit width, search (PDF panes only) |
| `SPC c r` | Refresh completion tags (re-scans with ctags, re-reads the symbols file) |
| `SPC c f` | Format the active Visual selection |
| `SPC c F` | Format the whole focused buffer |
| `SPC c s` | Fuzzy-find a Tcl symbol by its fully-qualified name and jump to its definition |
| `SPC m i` | Build and insert a telecommand from the MIB |
| `SPC m t` | Fuzzy-find a MIB telecommand and view its details |
| `SPC m k` | Fuzzy-find a MIB TM packet and view its details |
| `SPC m p` | Fuzzy-find a MIB TM parameter and view its details |
| `SPC m c` | Fuzzy-find a MIB calibration definition and view its details |
| `SPC m r` | Reparse the configured MIB directories from disk |
| `SPC m a` | Browse to and register a new MIB root directory |
| `SPC m d` | Fuzzy-find and remove a configured MIB root |
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

### Table view (`SPC f t`)

Toggles the focused buffer in place -- same file, same undo history,
just rendered with elastic-column alignment instead of plain text.
Every ordinary Vim motion and edit works (`j`/`k` and `c` are
reinterpreted, everything else is unchanged):

| Keys | Action |
|---|---|
| `]` / `[` | Jump to the start of the next / previous column |
| `j` / `k` | Move a row, staying in the same visual column |
| `c` | Fuzzy-find a column by name and jump to it |

### Search & replace review buffer (`SPC s p`)

A real, Vim-navigable buffer listing every file a pending project-wide
replace would touch, one row each (`j k gg G / n N ...` all work):

| Keys | Action |
|---|---|
| `Space` / `t` | Toggle the file under the cursor in/out of the replace |
| `a` / `Enter` | Arm the apply confirmation (`y`/`n`), or apply if already armed |
| `q` / `Esc` | Cancel -- closes the buffer, writes nothing |

### Docker panel (`SPC d d`)

Opens its own workspace with six real, titled panes -- Containers,
Images, Volumes, and Networks stacked on the left, Status and Logs
stacked on the right. Each is an ordinary Vim-navigable buffer (`j k gg
G / n N ...` all work); moving the cursor in a left pane live-updates
Status with that row's info. A Containers row is just a color-coded
status badge plus the container's name (`[R]` green for running, `[P]`
yellow for paused, `[X]` red for exited, etc.) -- press `x` on a pane for
a which-key-style popup of its available keys instead. Each title bar is
numbered (`1. Containers`, `2. Images`, ...) and the focused one is
shown in an accent color -- pressing that digit jumps straight to it.
Only these are special, and only on the pane named:

| Keys | Pane | Action |
|---|---|---|
| `1`-`6` | any | Jump to the pane numbered that in its title bar |
| `s` | Containers | Start the container under the cursor |
| `S` | Containers | Stop the container under the cursor |
| `R` | Containers | Restart the container under the cursor |
| `l` | Containers | Stream that container's logs live into the Logs pane (`docker logs -f`) |
| `r` | Images | Run a new detached container from the image under the cursor |
| `d` | Containers, Images, Networks | Remove the entry under the cursor (`y`/`n` to confirm) |
| `u` | any | Refresh the whole session |
| `x` | Containers, Images, Volumes, Networks | Show this pane's available keys |

### Git panel (`SPC g g`)

Opens its own workspace with six real, titled panes -- Status, Files,
Branches, Commits, and Stash stacked on the left, Main on the right.
Each is an ordinary Vim-navigable buffer (`j k gg G / n N ...` all
work). Moving the cursor in Files, Commits, or Stash re-syncs Main to
that row's diff; Status doesn't follow the cursor -- it's a fixed
repo-overview summary that live-updates on its own every ~2s
regardless of where the cursor is. Each title bar is numbered (`1.
Status`, `2. Files`, ...) and the focused one is shown in an accent
color -- pressing that digit jumps straight to it. Only these are
special, and only on the pane named:

Files is a collapsible directory tree, not a flat list -- changed
paths are grouped by directory (`> src/` collapsed, `v src/` expanded),
so `Tab` on a directory reveals or hides its files, and every action
key (`s`/`S`/`d`) works on a directory the same way it works on a
single file: stage, unstage, or discard *everything underneath it* in
one keypress. Discarding a directory runs both `git checkout --` (for
tracked changes) and `git clean -fd --` (for untracked files) under it,
since a real directory routinely holds a mix of both at once. Every
directory starts collapsed; expansion state persists across `u`
refreshes within the session.

| Keys | Pane | Action |
|---|---|---|
| `1`-`6` | any | Jump to the pane numbered that in its title bar |
| `Tab` | Files | Expand/collapse the directory under the cursor |
| `s` | Files | Stage the file (or every file under the directory) under the cursor |
| `S` | Files | Unstage the file (or directory) under the cursor |
| `a` | Files | Stage every changed file |
| `A` | Files | Unstage every staged file |
| `c` | Files | Commit (prompts for a message) |
| `d` | Files | Discard the file (or directory) under the cursor (`y`/`n` to confirm) |
| `z` | Files | Stash every change |
| `P` / `p` | Files | Push / pull |
| `c` | Branches | Checkout the branch under the cursor |
| `n` | Branches | New branch (prompts for a name) |
| `d` | Branches | Delete the branch under the cursor (`y`/`n` to confirm) |
| `a` | Stash | Apply the entry under the cursor |
| `g` | Stash | Pop the entry under the cursor |
| `d` | Stash | Drop the entry under the cursor (`y`/`n` to confirm) |
| `u` | any | Refresh the whole session |
| `x` | Files, Branches, Commits, Stash | Show this pane's available keys |

Real lazygit's own `<space>` stage-toggle isn't used here, since `SPC`
is already Fenix's global leader-key trigger -- Files uses separate
`s`/`S` keys instead, the same distinct-keys-per-action convention the
Docker panel's own `s`/`S`/`R` already established.

### JIRA dashboard (`SPC j j`)

Opens its own workspace with four real, titled panes -- Projects and
Users stacked on the left, Issues (the main pane) and Detail on the
right. Each is an ordinary Vim-navigable buffer (`j k gg G / n N ...`
all work) but genuinely read-only, like the Docker/Git panels -- it's
a generated listing, not something you edit; any edit that slips
through is silently reverted. Moving the cursor onto a tracked user
re-runs the query behind Issues; moving onto an issue re-fetches
Detail. Only these are special, and only on the pane named:

| Keys | Pane | Action |
|---|---|---|
| `1`-`4` | any | Jump to the pane numbered that in its title bar |
| `t` | Issues, Detail | Transition the issue's status (fetches the real available transitions, offers a picker) |
| `T` | Issues, Detail | Edit the title |
| `A` | Issues, Detail | Reassign to one of your tracked users (picker) |
| `P` | Issues, Detail | Change priority (picker, fetched live from the instance's real configured scheme) |
| `c` | Issues, Detail | Add a comment (opens a real scratch buffer) |
| `e` | Issues, Detail | Edit the description (opens a real scratch buffer, pre-filled) |
| `l` | Issues, Detail | Log time (Jira's own duration syntax, e.g. `2h 30m`) |
| `y` | Issues, Detail | Copy the issue's browse URL to the clipboard |
| `f` | Issues | Open the multi-select status filter |
| `x` | Issues, Detail | Show this pane's available keys |

`c`/`e` hand you a genuine, full-featured buffer -- write as much as
you want, over as many lines as you want, with every ordinary Vim
motion and edit available. `SPC j s` submits it (posts the comment, or
saves the new description) and restores Detail to its normal view;
`SPC j x` discards it the same way, without submitting anything. These
are leader bindings rather than pane-scoped bare keys on purpose: real
prose routinely contains the letters `c`/`e`/`t`/`T`/`l`, and a bare-key
trigger would hijack ordinary typing the moment it did.

`f` opens a multi-select picker over every status Issues has shown at
least once this session -- `Tab` toggles the entry under the cursor
on/off without closing the picker (any already-excluded statuses show
up pre-checked), `Enter` applies whatever's checked (nothing checked
means "show everything," the normal way to clear the filter), `Esc`
cancels without changing anything. The filter is folded directly into
the JQL query (`AND status NOT IN (...)`), so it stays applied across
`SPC j r` refreshes; it resets when the panel is closed and reopened,
and isn't saved to `config.ini`.

### VNC console panes (`SPC v v`)

Embeds a live VNC (RFB) connection to a VM as an ordinary, splittable
pane -- configure hosts once under `[vnc]` (see Configuration below),
then `SPC v v` fuzzy-picks one by name to open or switch straight to
it. Each session connects the first time you pick it and then stays
live in the background indefinitely, so switching back later is
instant, not a fresh handshake; an unfocused/hidden session polls at a
much slower rate to stay cheap while you're not looking at it, and a
dropped connection retries automatically with backoff before giving up
and leaving the pane on its last frame.

| Keys | Action |
|---|---|
| `SPC v v` | Open or switch to a configured VNC session (picker by name) |
| `SPC v q` | Close the focused VNC session |
| `SPC v s` | Save the focused session's current frame as a timestamped PNG |
| `Ctrl-\` | Release keyboard capture back to the editor (same chord as the terminal panel) |

Clicking into a VNC pane both focuses it and starts sending your mouse
there; every other key while it's focused is forwarded to the VM
instead of Vim, exactly like the terminal panel. Clipboard content is
mirrored in both directions: the VM's clipboard always flows to yours
as it changes, and yours flows to the VM whenever you focus a session.

Resizing is client-side only (the video scales to fit the pane; the
VM's own resolution is never changed), and the connection is always
made in the clear -- there's no encryption or authentication support at
all, matching the assumption that every configured host is on a
trusted local network. Don't point this at anything reachable over an
untrusted network without your own tunnel (SSH port-forwarding, a VPN)
in front of it.

### PDF viewer (`SPC r ...`)

Opening a `.pdf` -- by typed path (`SPC f f`), the explorer, a recent
file, or a CLI argument -- renders it as a scaled-to-fit page in an
ordinary, splittable pane instead of loading its raw bytes as text.
Rendering happens on one shared background worker (every open PDF
shares it), so opening a document never blocks the editor and several
can be open at once.

Reading is done with bare single keystrokes while the PDF pane is
focused -- a three-key leader chord per page is not a page-turn gesture
anyone would use to read a 50-page document. The `SPC r ...` bindings all
still work (and are what the which-key menu discovers); they're the same
commands, just reachable in one keystroke here.

| Keys | Action |
|---|---|
| mouse wheel, `j` / `k`, `Down` / `Up` | Scroll the page; at the bottom/top edge, continue onto the next/previous page |
| `PageDown` / `PageUp`, `n` / `p`, `SPC r n` / `SPC r p` | Next / previous page |
| `Home` / `End`, `g` / `G`, `SPC r [` / `SPC r ]` | First / last page |
| `SPC r g` | Prompt for a page number and jump to it |
| `+` / `-`, `SPC r =` / `SPC r -` | Zoom in / out, in coarse 10% steps |
| `0`, `SPC r 0` | Fit the whole page to the pane (the default) |
| `w`, `SPC r w` | Fit the page's width to the pane -- a tall page then scrolls vertically instead of shrinking further |
| `h` / `l`, `Left` / `Right` | Pan sideways once the page is wider than the pane |
| `SPC r o` | Toggle the outline/bookmarks panel |
| `/`, `SPC r /` | Search the document's text |

`SPC r f` opens a fuzzy picker over a **document index** you define by
hand in `config.ini`:

```ini
[documents]
doc1 = Space Packet Protocol|C:\refs\133x0b2e2.pdf
doc2 = Time Code Formats|C:\refs\301x0b4.pdf
```

Each entry is a display name and a path. The picker lists and
fuzzy-matches the *names*, so a reference you open constantly is two
keystrokes and a few characters away rather than a path to go hunting
for. Confirming opens that document **in the focused pane**, replacing
whatever it was showing -- unlike every other way of opening a PDF
(`SPC f f`, the explorer, a CLI argument), which gives the document its
own workspace. Picking a reference off a shelf means "show it to me
here", and if the pane already held a different PDF, that one (and its
outline/search companion panes) is retired first. An entry can point at
any file Fenix opens, not just a PDF -- a Markdown or plain-text
reference opens as ordinary editable text. A path that has since moved
is reported by name instead of opening an empty buffer, and an empty or
missing `[documents]` section says so rather than opening a picker over
nothing.

Scrolling is continuous across page boundaries in both directions:
scrolling past the bottom of a page turns to the next one at its top,
and scrolling back up past the top turns to the previous one at its
*bottom*, so scrolling back retraces exactly what scrolling forward
covered. Under the default fit-page zoom there is never anything to
scroll within a page, so every scroll gesture simply turns the page.

The status line shows `Page N/M` and the current zoom (`Fit page`,
`Fit width`, or a percentage) where an ordinary buffer shows `Ln`/`Col`
-- a PDF pane has no text and no cursor, so a line/column there would be
meaningless.

The outline panel (`SPC r o`) opens as a split next to the PDF pane,
listing the document's bookmark tree flattened into indented lines (a
nested bookmark just gets deeper indentation -- there's no tree widget,
so this is the whole tree in one flat, ordinary buffer). It's real,
Vim-navigable text: move around it with `j`/`k`/`gg`/`G`/`/` like
anything else, and press `Enter` on an entry to jump the PDF straight to
its page. `SPC r o` again -- from either the outline pane or the PDF
pane -- closes it. The outline is fetched once per document (a PDF's
bookmarks can't change while it's open) and cached, so reopening it is
instant after the first time; a PDF with no bookmarks at all shows a
single explanatory line instead of an empty pane.

`SPC r /` prompts for a search query and, once it comes back, opens (or
reuses, if one's already showing for this document) a results pane
listing every match in page order as `p.NNN  <context>` -- one line per
occurrence, anywhere in the document, not just the current page. It's
the same kind of real, Vim-navigable buffer the outline panel is;
`Enter` on a result jumps the PDF pane straight to that match's page. A
query with no hits shows an explanatory placeholder line rather than an
empty pane, same as the outline's no-bookmarks case. Search runs fresh
against the document each time rather than keeping the whole document's
text extracted in memory between searches -- there's no results cache to
go stale, just a brief "searching..." status message while it works.

The page re-renders to fit whenever its pane is resized, *except* at a
fixed zoom percentage (`SPC r =`/`SPC r -`), which stays exactly where
you left it across a resize instead of silently re-fitting -- panning
with `hjkl` then just shows a different part of the same render, no
fresh page turn needed. Fit-page/fit-width do still re-render on
resize, since what "fits" depends on the pane's own size by definition.

Pages are rasterized straight to BGRA and uploaded to a GPU texture. The
crop that's uploaded is only recomputed when the visible window actually
changes (a page turn, a resize, a zoom, a pan), the texture behind it is
only recreated when that crop's *size* changes, and a render that
already fits the pane exactly -- the fit-page default -- is uploaded
without being copied through a crop buffer at all.

A PDF pane's buffer is always empty and pathless -- the rendered page
lives in a GPU texture, not the buffer's own text -- so `:w`/`SPC f s`
on one is a no-op, same as every other generated panel in Fenix;
there's no risk of a stray save overwriting the real PDF file on disk.
Needs `pdfium.dll` (see [Optional external tools](#optional-external-tools))
-- without it, opening a PDF reports the error in the status line
rather than rendering.

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
runtime (picking a theme, font size, `:set shiftwidth=N`); you can also
hand-edit it directly. Every key is optional — a missing or unparsable
value just falls back to the built-in default instead of failing to
load. A value's surrounding whitespace is always trimmed; wrap it in
double quotes (`key = " "`) to keep whitespace that actually matters.

```ini
[editor]
theme = TempleOS
font_size = 16
font_family = Fira Code
indent_width = 4
tab_width = 8
animations = true

[completion]
symbols_file = /home/you/tcl-symbols.txt

[mib]
root1 = MIB-A|C:\data\mib-a
root2 = MIB-B|C:\data\mib-b
telecommand_template = telecommand_send PUS_T={type} PUS_ST={stype} APID={apid} MNEMO={mnemo} ARGUMENTS=[{arguments}]
telecommand_argument_template = {name}={value}
telecommand_argument_separator = ", "

[jira]
base_url = https://jira.example.com
token = your-personal-access-token
project1 = PROJ|My Project
user1 = jo1111111|John Doe

[vnc]
host1 = build-vm|10.0.0.5|5900
host2 = test-vm|10.0.0.6|5900

[documents]
doc1 = Space Packet Protocol|C:\refs\133x0b2e2.pdf
doc2 = Time Code Formats|C:\refs\301x0b4.pdf
doc3 = Team Onboarding Notes|C:\refs\onboarding.md
```

| Section | Key | Meaning |
|---|---|---|
| `editor` | `theme` | `Orbit Dark`, `TempleOS`, `Gruvbox Dark`, `Nord`, `Dracula`, `Solarized Dark`, or `One Dark` (case-insensitive) |
| `editor` | `font_size` | Body text size in points |
| `editor` | `font_family` | Body text font family, by name, as installed on your system. Overrides whatever the active theme names; unset falls back to the theme's own choice (and from there to your system's default monospace font) |
| `editor` | `indent_width` | Spaces per indent level (`>>`/`<<`, Tab, auto-indent) |
| `editor` | `tab_width` | Visual columns a literal tab character expands to when rendered (real Vim's own `:set tabstop`) -- distinct from `indent_width`, which governs what Tab/`>>`/`<<` actually insert (always spaces) |
| `editor` | `animations` | `true`/`false` -- whether caret-fade, scroll-ease, and yank/paste-pulse animations play at all; unset defaults to `true`. `SPC t a` toggles and persists this live |
| `completion` | `symbols_file` | Path to a plain-text symbols list, one identifier per line (blank lines and `#`-comments ignored), merged into the Tcl completion popup |
| `mib` | `root1`, `root2`, ... | A configured SCOS-2000 MIB directory, as `LABEL\|PATH` (numbered since a plain INI key can't repeat) — see the SCOS-2000 MIB feature above |
| `mib` | `telecommand_template` | Template used when `SPC m i` inserts a telecommand -- `{type}`, `{stype}`, `{apid}`, `{mnemo}`, `{description}`, `{mib}`, `{arguments}` |
| `mib` | `telecommand_argument_template` | Template for one variable telecommand argument within `{arguments}` -- `{name}`, `{value}` |
| `mib` | `telecommand_argument_separator` | Separator joining rendered arguments together. Every INI value here has its surrounding whitespace stripped, so a separator that depends on it (a trailing space, or one that's pure whitespace) needs to be wrapped in double quotes -- `", "` or `" "` -- to survive; an unquoted `,` works exactly as before |
| `documents` | `doc1`, `doc2`, ... | One entry in the `SPC r f` document index, as `NAME\|PATH` (numbered, same reason as `mib`'s roots). `NAME` is what the picker lists and fuzzy-matches; `PATH` can be any file Fenix opens, PDF or not |
| `jira` | `base_url` | The self-hosted Jira Server/Data Center instance's REST API root (e.g. `https://jira.example.com`) — see the JIRA dashboard feature above |
| `jira` | `token` | A personal access token for `base_url`, sent as a `Bearer` token — plaintext, same as every other setting in this file |
| `jira` | `project1`, `project2`, ... | A tracked project, as `KEY\|Display Name` (numbered, same convention as `mib`'s `root1`/`root2`) — added/removed via `SPC j p a`/`SPC j p d` rather than hand-edited, though either works |
| `jira` | `user1`, `user2`, ... | A tracked user, as `id\|Display Name` — added/removed via `SPC j u a`/`SPC j u d` |
| `vnc` | `host1`, `host2`, ... | A configured VNC target, as `NAME\|HOST\|PORT` (numbered, same convention as `mib`'s `root1`/`root2`) — see the VNC console panes feature above. No authentication support — every host is assumed to be unauthenticated and reachable only over a trusted network |

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
| `fenix-format` | External formatter shelling (`tclfmt` for Tcl today) — `SPC c f`/`SPC c F` |
| `fenix-mib` | SCOS-2000 MIB parsing (ICD 7.2) and telecommand/TM-packet/TM-parameter/calibration queries — `SPC m ...` |
| `fenix-table` | Pure layout math for a delimited table (row parsing, per-column widths, tab-stop positions) — feeds `fenix-gui`'s elastic-column table view, `SPC f t` |
| `fenix-docker` | Docker/Podman CLI shelling (auto-detected): container/image listing, start/stop/restart/remove/run/build |
| `fenix-jira` | A Jira Server/Data Center REST API client (`ureq`, PAT auth) — issue search and single-issue fetch, no thread/event-loop knowledge of its own |
| `fenix-config` | The unified `config.ini` reader/writer |
| `fenix-terminal` | PTY spawn/read/write/resize (`portable-pty`) plus ANSI screen-grid state (`vt100`) for the terminal panel — no thread/event-loop knowledge of its own |
| `fenix-gui` | Everything GPU/window-facing: `wgpu` rendering, `winit` input, and `App`, which wires all of the above together |

## License

No license has been chosen yet — treat this as source-available for
reference until one is added.
