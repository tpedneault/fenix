# Changelog

All notable changes to Fenix are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Fenix
uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.1.0] - 2026-11-05

The agents and MCP release: Fenix's tools for coding agents in toolsets,
with rules for what each may do; pipelines of agents; drafting with
agents; extension API 1.2; and the fixes since 1.0.1.

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
- Fenix's tools come in toolsets (`core`, `lsp`, `refactor`, `git`,
  `search`, `syntax`, `tasks`, `debug`, `commands`, and one per
  extension). An agent is shown those that fit the project, can turn on
  others itself (`fenix_toolsets`, `fenix_call`), and
  `agents.mcp.toolsets` adds or removes some.
- What Fenix's tools may do is decided per class (read, show, edit,
  run, external), toolset or tool (`agents.mcp.allow`: allow, ask or
  deny). A project's rules can only be stricter. When a tool is asked
  about, Fenix asks you itself, as a card in the agent's thread, whatever
  mode the agent is in: once or for the thread. MCP clients Fenix didn't
  start may only read and show unless `client:` rules say otherwise.
- Agents can ask the language servers for implementations, call and
  type hierarchies, signature help and code actions, and have them
  rename, apply a code action or format: every edit goes to Agent
  changes, as the agent's own do.
- Agents can drive the debugger: list launches, start one, set and
  clear breakpoints (with conditions and log messages), continue, step,
  run to a line, pause and stop. Each answers at the program's next
  stop, with its stack and variables. `SPC u A` clears the breakpoints
  agents set.
- Agents get Git through Fenix (`git_status`, `git_diff`, `git_log`,
  `git_show`, `git_blame`, `git_stage`), a project search that sees
  unsaved text and a replace whose edits you review, the syntax tree
  (`syntax_node`, `syntax_query`), and tasks they can wait for:
  `run_task` answers with the task's problems, tests and output
  (`task_output` for the rest).
- `fenix --mcp` offers MCP resources (an open file as it stands, the
  problems, the selection, the debugger's stop, the repository's state,
  the last task's output, and extensions' `agent_resources`), with
  subscriptions that say when one changes, and prompts (`fix_diagnostic`,
  `explain`, `review_changes`, `write_tests`, and extensions'
  `agent_prompts`). An extension's tool tells the agent its progress,
  and an agent's cancelling reaches the extension.
- Fenix's MCP server over HTTP, for clients that only speak HTTP:
  `agents.mcp.http` keeps one running on this machine, or
  `fenix --mcp --http <address>`. Clients present the token
  `fenix mcp token` prints, and follow the `client:` rules.
- Pipelines (`SPC a P`): a prompt run through agents in turn. Fenix's
  own plans (with a gate where you approve the plan), builds, and
  reviews, sending the review's findings back to be fixed. Each step is
  a thread with its own agent, mode and toolsets; their changes are one
  review. Your own go in `pipelines/*.toml`, or a project's
  `.fenix/pipelines/`.
- `SPC a c` with the commit composer open writes the agent's message
  into it as it comes, in the project's style: its recent commits, a
  commit template, and CONTRIBUTING.md's rules on commits go along.
- `SPC h a` lists every call agents made to Fenix's tools, with the rule
  that applied.
- `fenix --mcp` tells its client when its tool list changes, passes
  cancellations on to Fenix, and speaks MCP revisions 2024-11-05 to
  2025-11-25.
- The control channel's `fenix/agent.*` methods: what `fenix --mcp`
  calls, each for the thread its bridge speaks for (`client.thread` in
  `fenix/hello`).

### Extension API 1.2

Everything 1.2 adds is a new import, or a new export Fenix calls only
when an extension has it, so an extension built for 1.0 or 1.1 runs as
it did.

- An agent tool says what it may do (`class`: `read`, `show`, `edit`,
  `run`, `external`), how long it may take (`timeout_ms`, 30 seconds
  unless it says, up to 10 minutes) and whether it's offered by default
  (`default`). Its class must fit its permissions; a tool without one is
  `run` when it `writes`, else `read`.
- A tool call's deadline holds, an answer given later included: past it
  the agent gets an error and the extension hears `service-cancelled`.
  An extension that stops answers every call it had.
- A long tool says how far it's got (`service-progress`).
- An extension's pages go before its tool calls and events: a slow tool
  no longer freezes them.
- `fenix/tools` lists only the tools of extensions that are switched on,
  with each one's class, toolset, deadline and whether it's offered by
  default; a stopped extension's tools are marked stopped.
- `agents::ask_with`: an ask with attachments Fenix reads (a diff,
  commits, a file with its unsaved edits, the selection, the problems,
  text), a structured answer held to a JSON Schema, its draft as it's
  written (`agent-ask-progress`), a thread kept or hidden, a timeout, and
  a handle to cancel it. An ask runs in the agent's most restrictive
  mode, and a file its turn changes anyway is put back
  (`answer.reverted`). The first ask that sends code asks you, once per
  extension and agent (`agents.forget_ask_consent` asks again).
  `agents.ask.agent`, `agents.ask.model` and `agents.ask.show` choose the
  agent, the model and whether the thread stays.
- `[[contributes.agent_resources]]` and `[[contributes.agent_prompts]]`:
  what an extension offers agents to read, and prompts.
- A tool's description is at most 300 characters and its schema 2 KiB,
  for an extension built for 1.2.
- `SPC p r` runs the test the cursor is in, alone, in Task Output: a
  Rust `#[test]` (by its whole module path, in its package and test
  binary), a pytest test or test class, or a Go `TestXxx`. `SPC p T` runs
  it again. (#55)
- `SPC q r` restarts Fenix where you are, the session and unsaved text
  kept: what an extension installed or switched on since start, or a
  setting read only then, needs. The Extensions page and its messages
  say so. (#40)

### Changed

- `agents.mcp.fenix_write` and `agents.mcp.fenix_auto_read` are read as
  the rules they meant (`edit` and `run` allowed; `read` asked about),
  and `agents.mcp.allow` takes their place.
- What a coding agent says reads as Markdown rendered, in the agent
  panel: headings, emphasis, inline code, lists, quotes, tables, links
  and highlighted code blocks, laid out as the reading view lays out a
  note. Yanking a message (`y`) still copies the Markdown it said.

### Fixed

- Installing a code extension that asks for more than its sandbox
  (network, programs, secrets, your coding agents...) shows what it
  asks for and waits for your yes (#41).
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
- A Rust panic in Task Output is a place to go: `SPC p n` opens the
  file and line it names (`panicked at src\lib.rs:89:9`), not an empty
  buffer named for the whole line, and `Enter` on any line of Task Output
  that names a `path:line:col` goes there. (#19)
- `SPC f f` on a file that doesn't exist yet opens a new file by that
  name, rather than a `[No Name]` buffer with the path lost, and `SPC f s`
  creates it, and any folders it's in. (#18)
- `fenix/open` answers `-32602` with the reason for a path it can't
  open (an empty one, a folder, a file in a folder that doesn't exist),
  and opens nothing, rather than `{}` and a `[No Name]` tab. A new file
  in a folder that exists opens as `SPC f f` opens it. (#39)
- Paths read as you'd write them on Windows, with no `\\?\` and one kind
  of separator: in the refactor preview and its messages, the projects
  Fenix switches to, launch configurations handed to an extension
  (`${file}`, `${project}`), and the control channel (`fenix/state` said
  `...\ledger\src/main.rs`). (#38)
- A language server that never answers `initialize` no longer shows
  "starting" for good: after 30 seconds Fenix stops it and says so, with
  the last thing it wrote. Tool Status (`SPC l m`) lists each server that
  failed, why, and the last lines of its stderr. (#34)
- `fenix --help` lists the options (`--mcp`, the extension options and
  the rest) and `fenix --version` prints the version; both print and
  exit, on Windows too, rather than handing over to a running Fenix. An
  option Fenix doesn't know (`--projet`) is warned of. (#36)
- The commit composer, and any other blank one, opens in Insert, so what
  you type first is the message, not commands (`P` pasted the
  clipboard). One with text to edit (amend, reword) still opens in
  Normal. (#48)
- Finishing a merge (`SPC g x c`, or `c` on the Git page) commits with
  Git's prepared message (`Merge branch 'feature'`), opened in the
  composer to edit first, rather than with "Merge". (#45)
- "no known debug adapter" says where to look: the project's
  `tools.json`, an extension's tool on `PATH` (`SPC p h`), or a restart
  (`SPC q r`) for one installed since Fenix started. (#40)
- Completion no longer offers the word being typed (`clas` beside
  `class`), unless it's written elsewhere in the file too, so `Enter`
  accepts the word you meant. (#73)
- A picker closes once Enter has run what it picked: `SPC :` and a
  command that opens nothing of its own left a blank PICKER page, with
  keys going to the buffer behind it. With nothing matching, Enter keeps
  the query. (#30)
- A first setting saved to a new `settings.toml` goes after its header
  comment, not above it. (#51)
- The `"0` register holds the last yank (`yyjdd"0p` pastes the yanked
  line), and `"_` deletes without touching any register or the
  clipboard. (#26)
- Home opens on the file to resume when there is one, so the `Enter`
  its column shows resumes it rather than opening the file finder. (#49)
- `SPC g g` with a file from outside any repository in front (one left
  open after a project switch) opens the project's repository, or says
  that neither is in one, rather than nothing. (#47)
- In the Merge view, `o` and `t` on the header row, where the cursor
  starts, take the only conflicted file, as the hints above it say,
  rather than doing nothing. (#46)
- `SPC p t` lists each task once: an extension's and Fenix's own with
  the same name or command line are one row, the extension's. In a uv
  project Fenix's `pytest` and `ruff check` run through `uv run`. (#35)
- An idle window no longer copies the focused file's text and looks at
  the disk each frame to tell its language servers what they already
  know. (#74)
- The project wizard shortens a long path in the middle, keeping the
  project's own folder in view, rather than cutting off its end. (#53)
- Fenix's window comes up before its extensions do: one being let go
  while it's still loading -- starting again in another project -- is no
  longer waited for, nor are the language servers being stopped. An
  extension setting its language server starts again only that
  language's, and only when it changes, not every server of the project.
  With 40 extensions the window was held up for seconds. (#33)
- A language pack that starts with Fenix (`on_startup` and a context
  naming markers or languages) waits for a file of its languages, or a
  project of its markers, before it loads. `[activation]` can say so for
  any extension: `languages` and `markers`. (#33)
- Moving to another project starts again at once only the language
  packs for it; another extension that reads the project starts again
  there the next time it's used -- a command, a page, a service, its
  language server -- and carries on as it was if you come back first.
  Every extension used to start again at every move. (#33)
- An agent's `rename` and `apply_code_action` work with language servers
  that name document versions, as rust-analyzer does: every one was
  refused as stale.
- A pipeline's plan step stops at the plan: its agent's offer to carry
  it out no longer has it build everything, only for that to be put
  back and its summary passed on as the plan.
- A pipeline ends on the thread with its changes, so `SPC a r` finds them.
- There's one agents' dock, and it comes with you between workspaces, so
  a card an agent waits on stays in sight. Opening it in a second
  workspace made a second one that shared its thread: putting either
  away left the other a Home pane and a prompt, and the first lost its
  titles. A new workspace from the dock starts on Home, not on a thread
  with no prompt.
- A file an agent shows, or that's opened from outside, goes beside the
  dock, not over a thread and its cards.

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

[Unreleased]: https://github.com/tpedneault/fenix/compare/v1.1.0...HEAD
[1.1.0]: https://github.com/tpedneault/fenix/compare/v1.0.1...v1.1.0
[1.0.1]: https://github.com/tpedneault/fenix/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/tpedneault/fenix/releases/tag/v1.0.0
