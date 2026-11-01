# Changelog

All notable changes to Fenix are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Fenix
uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Linux packages with each release, a `.deb` and an `.rpm` beside the
  archive: Fenix in your applications menu and `fenix` on `PATH`.
- A [Scoop](https://scoop.sh) bucket, `tpedneault/scoop-fenix`:
  `scoop bucket add fenix https://github.com/tpedneault/scoop-fenix`, then
  `scoop install fenix`.
- The project doctor (`SPC l m`) checks that rust-analyzer can read the
  standard library: a `rustc` on `PATH` without its source (a standalone
  install ahead of rustup's) made it report errors that aren't there.

## [1.0.0] - 2026-10-31

The first release: a keyboard-first, GPU-rendered editor with Vim's
editing grammar and a `SPC` leader for everything else.

### Editing

- **Vim, for real**: motions, operators, text objects, counts,
  registers, macros, marks, the jumplist, `:s` with backreferences, and
  Visual char, line and block modes.
- **A `SPC` leader with which-key**: every command a short mnemonic
  sequence, with a popup of what comes next.
- Tree-sitter highlighting and folding; native snippets with fields,
  mirrors and transformations; structural reindentation.
- Staged, atomic saves; crash recovery for unsaved buffers; the session
  (windows, splits, cursors, scroll positions) restored at the next
  start.

### Projects and files

- A project hub, a new-project wizard with templates, and per-project
  settings, tasks, language servers and debug launches
  (`.fenix/settings.toml`, `.fenix/tools.json`).
- Fuzzy pickers for files, symbols and buffers; a Search page that
  searches and replaces across the project.
- A file explorer and file manager: marks, copy and move, archives, the
  Recycle Bin, hidden files on demand, bookmarks.
- Terminals in panes and a terminal panel, started in the project.

### Code intelligence and debugging

- LSP: completion, hover, diagnostics, code actions, rename with a
  multi-file preview, several servers on one file with their answers
  merged.
- A debugger in the code, over the Debug Adapter Protocol: breakpoints
  and the current line in the gutter, values written inline, a DEBUG
  mode with the usual F-keys, and a dock for the stack, variables,
  watches and the console.
- A task runner that reads the project's own tasks, with output you can
  jump from.

### Git

- A status page with hunk staging, commits and amends, blame, a commit
  graph, side-by-side compare, interactive rebase, conflict resolution,
  worktrees, and undo for Git operations.

### Coding agents

- Claude Code, Codex, Devin, Gemini CLI or any agent that speaks the
  Agent Client Protocol, in a dock beside the code: threads that come
  back with the session, `@` mentions, the commands an agent runs shown
  as it runs them, and every change it makes reviewable hunk by hunk.
- `fenix --mcp`: Fenix as an MCP server for agents, with the language
  servers, diagnostics and the open buffers.

### Extensions

- Sandboxed WebAssembly extensions on extension API 1.0
  (`fenix-extension-api`): pages, canvases, surfaces, decorations,
  language servers and debug adapters, sockets, HTTP, tasks, coding
  agents, each held to the permissions its manifest asks for.
- A signed registry and a store inside Fenix (`SPC h x`), with packs
  offered at the first start; data-only extensions for themes, snippets,
  templates, keymaps and languages.
- Code review for GitHub and GitLab, containers, an agenda that syncs
  with Jira, a notebook with Mermaid diagrams, a PDF reader, VNC
  consoles, language packs, and tools for Arduino, Tcl, SCOS-2000 MIB
  and CCSDS -- all in the store.

### And

- An interactive tutorial (`SPC h T`): editing, Fenix's pages, Git, and
  the extensions you install.
- Every setting on one page (`SPC ,`), in `settings.toml` underneath.
- The control channel: `fenix file:line`, and a JSON-RPC protocol other
  tools drive Fenix with.
- A Windows installer, for you alone or for everyone on the machine,
  and archives for Windows and Linux.

[Unreleased]: https://github.com/tpedneault/fenix/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/tpedneault/fenix/releases/tag/v1.0.0
