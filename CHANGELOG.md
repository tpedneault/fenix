# Changelog

All notable changes to Fenix are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Fenix
uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Coding agents can ask more of Fenix through `fenix --mcp`: definitions,
  references, hover and symbols from the language servers for any file,
  an open file's text with its unsaved edits, and where the debugger is
  stopped. With `agents.mcp.fenix_write` they can evaluate in the
  debugger too.
- Agents Fenix starts are told to prefer those tools
  (`agents.mcp.fenix_brief`), and use the ones that only read without
  asking (`agents.mcp.fenix_auto_read`).
- Extensions can offer agents tools (`contributes.agent_tools`).
- `agents.mcp.servers` takes the address of a running MCP server, over
  HTTP or SSE, as well as a command.
- The control channel has what these use: `fenix/buffer.read`,
  `fenix/lsp`, `fenix/debug.state`, `fenix/debug.evaluate`, `fenix/tools`
  and `fenix/tools.call`.

### Changed

- What a coding agent says reads as Markdown rendered, in the agent
  panel: headings, emphasis, inline code, lists, quotes, tables, links
  and highlighted code blocks, laid out as the reading view lays out a
  note. Yanking a message (`y`) still copies the Markdown it said.

### Fixed

- A file is open once, however its path is spelled: on Windows a file
  reached as `\\?\C:\a.rs` (from Home, the recent files or a restored
  session) and as `C:\a.rs` (from the finder or a language server) opened
  in two tabs that disagreed. (#56)
- A very long line, such as 8 MB of JSON on one line, no longer hangs the
  window: only what a row can show is highlighted and shaped. (#57)
- `Ctrl-O` in Insert runs one Normal command and comes back to Insert, as
  in Vim, rather than jumping back to another file still in Insert. (#58)
- Visual mode has text objects (`viw`, `vip`, `vi(`), `o` for the other
  end and `J` to join; `A` in a block appends on every row, a block `c`
  reaches every row, and a block yanked or deleted pastes as a block. (#59)
- Ex commands take ranges and line numbers (`:36`, `:.,$d`, `:%sort`),
  and `:d`, `:y`, `:m`, `:t`, `:j`, `:>`, `:<`, `:norm`, `:e`, `:sp`,
  `:vs`, `:bd`, `:bn`, `:bp` and `:only` work. A command Fenix doesn't
  know says "Not an editor command" instead of doing nothing. `:s` and
  `:g` read Vim's patterns (`\(`, `\+`, `\v`), and `&` and `g&` repeat
  the last `:s`. (#60)
- Text in any encoding opens as text and saves as it came: a UTF-16 or
  Latin-1 file no longer opens in the packet decoder or the hex view, and
  a UTF-8 byte order mark is kept rather than shown. `SPC f E` reads a
  file again as another encoding, and the modeline says which it is. (#61)
- The PDF reader works once its extension is installed: PDFium now comes
  with Fenix, in the zip, the installer and the Linux packages, rather
  than a PDF opening on a blank page. (#62)
- In Visual mode a count applies to the motion after it: `v3l` selects
  three more columns and `3G` goes to line 3. (#68)
- One huge unsaved file no longer stops the session being saved: its
  text stays in its recovery file, and Fenix says so. (#69)
- `cc` and `S` keep the line's indent, as `o` and `Enter` do. (#70)
- `Enter` ends the line when there's nothing to complete, rather than a
  completion popup opened on a trigger character taking it. `Tab` and
  `Ctrl-Y` still accept. (#71)
- The debugger says why an adapter refused: lldb-dap's reason for a
  failed `SPC u e` (`error: use of undeclared identifier ...`), from the
  error's details, instead of "the adapter refused, without saying why".
  (#32)
- A debug session that has ended no longer blocks starting one in another
  project ("debug session belongs to another project"): the new one
  replaces it, as it does in the same project. (#31)

## [1.0.1] - 2026-11-03

### Added

- Linux packages with each release, a `.deb` and an `.rpm` beside the
  archive: Fenix in your applications menu and `fenix` on `PATH`.
- A [Scoop](https://scoop.sh) bucket, `tpedneault/scoop-fenix`:
  `scoop bucket add fenix https://github.com/tpedneault/scoop-fenix`, then
  `scoop install fenix`.
- The project doctor (`SPC l m`) checks that rust-analyzer can read the
  standard library: a `rustc` on `PATH` without its source (a standalone
  install ahead of rustup's) made it report errors that aren't there.

### Extension API 1.1

Everything 1.1 adds is something an extension asks of Fenix, so an
extension built for 1.0 runs as it did.

- `git-state`: the open project's repository (branch, commit, ahead and
  behind its upstream, conflicts), for an extension that asks for `git`
  events.
- `last-task`: the task that ended last, and its tests passed and failed
  when its output gives them (cargo, pytest, Jest, Vitest, `go test -v`).
- An extension may hear of agents' turns (`events = ["agents"]`) without
  `permissions.agents`. It's told a turn ended and how, but not the
  thread's title or what was said.

### Fixed

- The which-key menu no longer lists an extension's keys, with no label,
  when that extension isn't installed (`SPC g P` and `SPC g M`, the
  Review extension's).
- `q` in the Git graph view (`SPC g G`) closes it, as in the other Git
  views, rather than starting a macro recording.
- An extension's layer is drawn when its next frame is due: a model
  drawn a few times a second (the Pets extension's) moved only when
  something else redrew the window.

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

[Unreleased]: https://github.com/tpedneault/fenix/compare/v1.0.1...HEAD
[1.0.1]: https://github.com/tpedneault/fenix/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/tpedneault/fenix/releases/tag/v1.0.0
