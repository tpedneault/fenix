# Architecture

Fenix is a Cargo workspace of small crates. Almost all of them are
host-agnostic: they know nothing about windows, the GPU or the event
loop, and each is unit-tested on its own (`cargo test --workspace`).
`fenix-gui` is the one place that wires them together: `wgpu`
rendering, `winit` input, and `App`.

Crates that talk to an external program (`git`, language servers, debug
adapters, coding agents) shell out to it or speak its protocol directly,
and are tested against the real thing, or a fake of it built for the
tests, where that's practical.

Much of what Fenix does comes from extensions: sandboxed
WebAssembly components, developed in
[fenix-extensions](https://github.com/tpedneault/fenix-extensions) and
installed from its registry. Code review, containers, the agenda and
Jira, the notebook, VNC, the PDF reader, Arduino, Tcl, the MIB and CCSDS
tools and the language packs all live there, with the crates only they
use.

## Text and editing

| Crate | Role |
|---|---|
| `fenix-core` | The rope-backed `Buffer`/`Cursor`, undo/redo |
| `fenix-vim` | Modal editing: motions, operators, text objects, search/substitute, indentation |
| `fenix-keymap` | Generic key-sequence trie (`KeyPress`, `KeyTrie`, `Matcher`) — shared by Vim's normal/visual keymaps and the leader menu |
| `fenix-syntax` | tree-sitter-backed incremental parsing and highlight-span extraction; extensions' grammars as WebAssembly (tree-sitter's `wasm` store, its own wasmtime) |
| `fenix-format` | Structural, language-independent reindentation (bracket-nesting depth) — `SPC c f`/`SPC c F` |
| `fenix-completion` | Completion sources: the external symbols file; Tcl's built-in names, for highlighting its calls |
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
| `fenix-rpc` | `Content-Length`-framed message transport over a child process's stdio, shared by LSP and DAP, with a bounded reader for peers Fenix didn't spawn; and newline-delimited messages (`read_line_message_limited`, `write_line_message`) for ACP and MCP |
| `fenix-control` | The control channel: a per-user named pipe or Unix socket, and its JSON-RPC protocol (open files and projects, state, commands, notebook capture, agenda tasks and clock) on both ends -- see [CONTROL.md](CONTROL.md) |
| `fenix-lsp` | A Language Server Protocol client: handshake, document sync, requests, and transactional workspace edits; `fanout`, the pure routing and merging for several servers on one document (which servers a request goes to, merged completion/hover/locations, per-server diagnostics), tested against two real processes of its `fake-language-server` test binary |
| `fenix-dap` | A Debug Adapter Protocol client on the same transport |
| `fenix-acp` | The Agent Client Protocol (ACP v1) client for coding agents -- see [AGENTS.md](AGENTS.md) and [ADR 0002](adr/0002-agents-acp.md). Lenient protocol types; `AcpClient`, a threaded JSON-RPC connection over a process tree's stdio; `Thread`, the reducer that turns `session/update`s into a conversation; `Hub`, the sans-I/O sessions hub (connections per agent and project, new/resume/load/read-only, queued prompts, permissions, crash backoff, idle stop), which takes inputs and gives back `Effect`s; the thread store; the catalog of agents (built-in, yours, the registry's, extensions'); the ACP registry's installs; and a mock agent (`fenix:mock` in process, `fake-agent` as a binary) that plays Claude Code's adapter, against which the conformance suite runs |
| `fenix-tasks` | Project task discovery, output parsing, `path:line:col` reading (shared with the command line), and process-tree supervision (`ProcessTree`: a child and everything it starts, killed together) |
| `fenix-terminal` | PTY spawn/read/write/resize (`portable-pty`) plus ANSI screen-grid state (`vt100`) and terminal-query replies for both terminal surfaces — no thread/event-loop knowledge of its own |

## Git

| Crate | Role |
|---|---|
| `fenix-diff` | Unified-diff parsing (files/hunks/lines, both sides' line numbers) and single-hunk patch synthesis — pure, no I/O; what hunk staging and diff rendering are both built on |
| `fenix-git` | Shells out to `git`: status/files/branches/remotes/tags, commit graph topology and lane assignment, diffs (working tree, commit, ref-to-ref), fetch, and applying a patch to stage/unstage/discard one hunk |

## Extensions

| Crate | Role |
|---|---|
| `fenix-extensions` | What an extension says it is (`extension.toml`), reading one from a `.fenixext` package or a folder, and the extensions installed on this machine (`extensions.toml`), bundles included |
| `fenix-wasm` | The extension host's runtime: a wasmtime engine where every call has a deadline, the stack is bounded and memory is capped -- see [ADR 0001](adr/0001-wasm-runtime.md) |
| `fenix-extension-api` | What an extension written in Rust builds against: the WIT world (`wit/world.wit`) and the types and macros that make it easy to use -- see [Writing extensions](WRITING_EXTENSIONS.md) |
| `fenix-model` | Extensions' 3D models: a binary glTF read (meshes, skeleton, clips, textures), posed by blended clips and skinned on the CPU, optional parts; and models built in code and written as `.glb` |
| `fenix-pdf` | Opens PDFs and rasterizes pages, or parts of them, to pixel buffers through PDFium |
| `fenix-pdfium` | The PDF reader extension's companion program: `fenix-pdf` on the other end of a pipe, its protocol in `src/protocol.rs`. Built and shipped beside `fenix.exe` |

The PDF reader (`fenix.pdf`) draws on a surface what `fenix-pdfium`
renders; the `fenix` binary has no PDFium bindings of its own. The VNC
viewer (`fenix.vnc`) is its own RFB client, on the extension API's
sockets and surfaces. The SCOS-2000 MIB (`fenix.mib`) and CCSDS & PUS
(`fenix.ccsds`) share the `fenix-mib` and `fenix-ccsds` crates in
fenix-extensions' `crates/`.

## The application

| Crate | Role |
|---|---|
| `fenix-brand` | The Fenix mark, wordmark and icon, drawn from one vector definition |
| `fenix-gui` | Everything GPU/window-facing: `wgpu` rendering, `winit` input, and `App`, which wires all of the above together |
