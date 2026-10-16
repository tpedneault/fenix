# Fenix 2.0 smoke checklist

A short manual pass for every feature that moves to an extension in 2.0
(see `docs/EXTENSIONS_PLAN.md`), plus the core. Phase 0 must not change
behaviour, so for each change run the sections it touches plus **Core**,
and compare with the baseline at the end of this file.

## Setup

Always use an isolated profile, so real settings and a running Fenix are
never touched (PowerShell):

```powershell
$env:FENIX_HOME = "$env:TEMP\fenix-test\home"
$env:FENIX_STANDALONE = "1"
$env:FENIX_PDFIUM_PATH = "<folder with pdfium.dll>"
cargo run -p fenix-gui --bin fenix -- --console
```

Delete `$env:TEMP\fenix-test\home` for a clean profile.

Run it from a scratch folder, not this repository, so edits and commits
can't land here. The baseline used `$env:TEMP\fenix-test\work`:

- a Git repo made with `cargo init --name smoke`, with `src/main.rs`
  holding a local `fn helper()` called from `main`;
- `demo.tcl` (a `proc greet_user` and a call to it);
- `hex.txt` with one CCSDS space packet as hex
  (`08 01 C0 00 00 03 01 02 03 04`);
- `smoke.pdf`, any small two-page PDF.

Mark each line **pass**, **fail** (with what happened) or **skip** (with
why). "Skippable" items need an outside service; skip them when it isn't
there.

## Core

| # | Check | Expect |
|---|---|---|
| C1 | Launch with a clean profile | Home appears; no error |
| C2 | `SPC p f`, open `src/main.rs` | The file opens, highlighted |
| C3 | `o`, type, `Esc`, `SPC f s` | Modeline says saved; the file on disk changed |
| C4 | `u`, then `Ctrl-r` | The insert is undone in one step, then redone |
| C5 | `SPC w v`, `SPC w s`, `SPC w q` | Splits open and close |
| C6 | `SPC p p` | The project hub opens (empty in a fresh profile; `a` adds a folder) |
| C7 | `SPC p f`, type a file name | Fuzzy matches, best first |
| C8 | `SPC p s`, search `helper`, Enter on a match | ripgrep results; Enter jumps to the line |
| C9 | On the call to `helper`: `K`, then `gd` | rust-analyzer hover, then the cursor jumps to `fn helper` |
| C10 | `SPC g g`, `s` on a modified file | Git status page; the file moves to Staged |
| C11 | `c`, `c`, `i`, type a message, `Esc`, Enter | Commit made; Head shows the new message |
| C12 | `SPC o t`, `echo hi` | Terminal panel runs it. Click a pane to leave it |
| C13 | A split with two files, `SPC q q`, relaunch | Same workspaces, splits, files and cursors; window state kept |
| C14 | Leave an unsaved edit, `Stop-Process -Name fenix -Force`, relaunch, `SPC f v` | Session restored; the unsaved file is listed and recovers |

## Features moving to extensions

| # | Feature | Check | Expect |
|---|---|---|---|
| N1 | Notebook | `SPC n n` | Notebook page, under `FENIX_HOME` |
| N3 | Notebook | `SPC n j` | Today's journal opens |
| A1 | Agenda | `SPC a a` | Agenda page opens |
| A2 | Agenda | `n` on the agenda, type a name, Enter | "added" message; the task is listed |
| A3 | Agenda | `SPC a t`, Enter on the task | "Clock on"; the modeline shows it. (While clocked, `SPC a t` opens a stop/switch picker) |
| J1 | Jira (skippable: needs a server) | `SPC j j` | Issues, or "Jira isn't set up yet" without a server |
| P1 | PDF | Open `smoke.pdf` | Page renders (needs `FENIX_PDFIUM_PATH`) |
| P2 | PDF | `SPC r n`, `SPC r p`, `SPC r =`, `SPC r o` | Page 2/2, back, zoom, outline sidebar |
| P3 | PDF | `SPC r f` | Document picker lists it |
| V1 | VNC (skippable: needs a server) | `SPC v v` | Session picker; connecting needs a configured server |
| G1 | Diagrams | `SPC n d`, flowchart, Example, name it | `.mmd` opens with the drawing beside it |
| G2 | Diagrams | Break line 1 (`x` on `flowchart`) | Diagnostic on line 1; the last good drawing stays |
| D1 | Docker (needs Docker or Podman) | `SPC d d` | New workspace, six titled panes: Containers, Images, Volumes, Networks, Status, Logs; Status shows CPU/MEM |
| D2 | Docker | `j`/`k` in Containers | Status follows the row under the cursor |
| D3 | Docker | `x` in Containers | Popup lists start/stop/restart/logs/remove/refresh; the next key closes it |
| D4 | Docker | `2`, `4`, `1` | Focus jumps to the numbered pane |
| D5 | Docker | `l` on a running container | Logs pane follows its output |
| D6 | Docker | `d`, then `n` | "Remove this container? (y/n)" above the modeline; `n` cancels |
| D7 | Docker | `u` | Lists refresh |
| D8 | Docker | `SPC TAB [` away, `SPC d d` again | Refocuses the same session; no new workspace |
| D9 | Docker | `SPC d q`; reopen, then `SPC b k` on a pane | Either way the whole session and its workspace close |
| D10 | Docker | Session open, `SPC q q`, relaunch | The Docker workspace isn't restored; the others are |
| R1 | Arduino (needs `arduino-cli`) | `SPC f n`, name it | Sketch created and opened; `SPC m` shows the Arduino menu |
| W1 | GitHub/GitLab review (skippable: needs a token and a remote) | `SPC g M` | Reviews, or a clear message (no remote / no token) |
| T1 | Tcl | Open `demo.tcl` | `proc`, `puts` and the proc name highlighted |
| T2 | Tcl | `o`, type `gre` | Completion offers `greet_user` (symbol) |
| M1 | MIB | `SPC k k` | MIB page ("No MIB for this project" without roots) |
| M2 | MIB | `SPC k ,` | The project's MIB settings |
| S1 | CCSDS | `SPC k T` | Time converter |
| S2 | CCSDS | On the hex line in `hex.txt`, `SPC k d` | Decoded TM packet with its fields |
| S3 | CCSDS | `SPC f x` on a file | Hex view |

## Upgrade check (settings, session or stored data changes)

Copy a 1.x profile into a fresh `FENIX_HOME` (never point at the real
one), launch, and check that settings, projects, recent files, the
session, the notebook, the agenda and MIB roots are all still there.

## Baseline: `release/2.0` at `bcdd773`

2026-10-16, Windows 11 Pro 26200, 4K display at 200%, debug build,
Docker Desktop 29.7.2, arduino-cli, rust-analyzer 1.98 (standalone MSVC
install, no rust-src). Driven through synthetic input: `Esc` was sent as
`Ctrl+Alt+Q`.

Everything passed: C1–C14, N1, N3, A1–A3, J1, P1–P3, G1, G2, D1–D10,
R1, W1, T1, T2, M1, M2, S1–S3.

Skipped or partial:

- **J1, W1**: no Jira server, no forge remote or token. Both give a
  clear message.
- **V1**: no VNC server configured, so the picker was empty and nothing
  connected.
- **C6**: no project added, so only the empty hub was checked.
- **C9**: hover and `gd` on `println!` do nothing, because this Rust
  install has no `rust-src`, so std isn't indexed. Local symbols work.

Already the case on `release/2.0`, not regressions:

- The terminal panel (`SPC o t`) starts in the user's home directory,
  not the project.
- The Windows gate fails on
  `app::tests::project_todo_picker_reads_comments_only_and_fills_the_quickfix_list`
  once ripgrep is on `PATH` (it skips itself without `rg`). A TODO
  message kept the rest of the line after a block comment closed
  (`in xml --></a>`). Fixed on `feature/ext-todo-comment-end`.
