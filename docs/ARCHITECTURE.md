# Architecture

Fenix is a Cargo workspace of small crates. Almost all of them are
host-agnostic: they know nothing about windows, the GPU or the event
loop, and each is unit-tested on its own (`cargo test --workspace`).
`fenix-gui` is the one place that wires them together: `wgpu`
rendering, `winit` input, and `App`.

Crates that talk to an external program or service (`git`, `docker`,
`arduino-cli`, language servers, Jira, GitLab, GitHub) shell out to it
or call its API directly, and are tested against the real thing where
that's practical.

## Text and editing

| Crate | Role |
|---|---|
| `fenix-core` | The rope-backed `Buffer`/`Cursor`, undo/redo |
| `fenix-vim` | Modal editing: motions, operators, text objects, search/substitute, indentation |
| `fenix-keymap` | Generic key-sequence trie (`KeyPress`, `KeyTrie`, `Matcher`) — shared by Vim's normal/visual keymaps and the leader menu |
| `fenix-syntax` | tree-sitter-backed incremental parsing and highlight-span extraction |
| `fenix-format` | Structural, language-independent reindentation (bracket-nesting depth) — `SPC c f`/`SPC c F` |
| `fenix-completion` | Completion sources: Tcl keywords, ctags-scanned definitions, external symbols file |
| `fenix-snippets` | Native, data-only snippets: parsing, fields, mirrors and transformations |
| `fenix-table` | Pure layout math for a delimited table (row parsing, per-column widths, tab-stop positions) — feeds `fenix-gui`'s elastic-column table view, `SPC f t` |

## Buffers, windows and storage

| Crate | Role |
|---|---|
| `fenix-buffers` | The open-buffer registry (`BufferId` → buffer/cursor/syntax state) |
| `fenix-window` | A generic split-window tree (layout, navigation, resize) — no knowledge of buffers |
| `fenix-storage` | Checked, staged writes shared by documents, settings and recovery: the destination is only replaced once a complete, synced copy exists |
| `fenix-recovery` | Unsaved work written somewhere it survives a crash, and restored at the next launch |
| `fenix-config` | `settings.toml`: every setting declared once (the schema), read, checked and saved in place; the project layer |

## Files and projects

| Crate | Role |
|---|---|
| `fenix-fs` | The platform facts `std::fs` does not give you: places, archives, the Recycle Bin, big transfers, shell integration |
| `fenix-explorer` | Directory listing, marking, file operations, git-status — no GPU/rendering |
| `fenix-picker` | Generic fuzzy matching + live-filtered candidate list, used by every fuzzy-finder |
| `fenix-project` | Project-root detection, ripgrep/fd shelling, known-projects/recent-files persistence |

## Language tooling and processes

| Crate | Role |
|---|---|
| `fenix-rpc` | `Content-Length`-framed message transport over a child process's stdio, shared by LSP and DAP |
| `fenix-lsp` | A Language Server Protocol client: handshake, document sync, requests, and transactional workspace edits |
| `fenix-dap` | A Debug Adapter Protocol client on the same transport |
| `fenix-tasks` | Project task discovery, output parsing, and process-tree supervision |
| `fenix-terminal` | PTY spawn/read/write/resize (`portable-pty`) plus ANSI screen-grid state (`vt100`) and terminal-query replies for both terminal surfaces — no thread/event-loop knowledge of its own |

## Git and forges

| Crate | Role |
|---|---|
| `fenix-diff` | Unified-diff parsing (files/hunks/lines, both sides' line numbers) and single-hunk patch synthesis — pure, no I/O; what hunk staging and diff rendering are both built on |
| `fenix-git` | Shells out to `git`: status/files/branches/remotes/tags, commit graph topology and lane assignment, diffs (working tree, commit, ref-to-ref), fetch, and applying a patch to stage/unstage/discard one hunk |
| `fenix-forge` | A neutral model of a code-review forge (pull/merge requests, reviews, threads), with no I/O of its own |
| `fenix-github` | `fenix-forge` over GitHub's REST and GraphQL APIs |
| `fenix-gitlab` | `fenix-forge` over GitLab's `/api/v4` |

## Integrations

| Crate | Role |
|---|---|
| `fenix-agenda` | The personal task and time-tracking model behind `SPC a`, and its sync with Jira |
| `fenix-jira` | A Jira Server/Data Center REST API client (`ureq`, PAT auth) — issue search and single-issue fetch, no thread/event-loop knowledge of its own |
| `fenix-docker` | Docker/Podman CLI shelling (auto-detected): container/image listing, start/stop/restart/remove/run/build |
| `fenix-embedded` | Microcontroller projects behind a `Platform` trait (Arduino, via `arduino-cli`, today): sketch detection, build/upload/monitor/language-server commands, board/port/library/package queries, tool discovery — no thread/event-loop knowledge of its own |
| `fenix-mib` | SCOS-2000 MIB parsing (ICD 7.2) and telecommand/TM-packet/TM-parameter/calibration queries — `SPC m ...` in Tcl files |
| `fenix-pdf` | Opens PDFs and rasterizes pages to pixel buffers through PDFium |
| `fenix-vnc` | A VNC (RFB) client: framebuffer decoding and keyboard/pointer/clipboard input |

## The application

| Crate | Role |
|---|---|
| `fenix-brand` | The Fenix mark, wordmark and icon, drawn from one vector definition |
| `fenix-gui` | Everything GPU/window-facing: `wgpu` rendering, `winit` input, and `App`, which wires all of the above together |

## Vendored code

`vendor/vnc-rs` is the published `vnc-rs` crate with one added event
(where a framebuffer update ends); see `vendor/vnc-rs/PATCH.md`.
