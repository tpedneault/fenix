# Project tool settings

Fenix runs one primary language server per language and canonical project root,
plus any additional servers a project declares (see below). Opening two Python
or Rust projects no longer routes their requests to the same server.
Relative aliases of a root reuse the same session. The nearest project marker
determines ownership; `.fenix/settings.toml` and `.fenix/tools.json` are also markers.

Add `.fenix/tools.json` to a project for structured commands:

```json
{
  "lsp": {
    "rust": {
      "executable": "C:\\Tools\\rust-analyzer.exe",
      "args": [],
      "env": { "RUST_LOG": "error" }
    }
  },
  "dap": {
    "python": {
      "executable": ".venv/Scripts/python.exe",
      "args": ["-m", "debugpy.adapter"]
    }
  },
  "tasks": {
    "Run report": {
      "executable": ".venv/Scripts/python.exe",
      "args": ["../scripts/report.py", "--title", "Monthly report"],
      "cwd": "reports",
      "env": { "REPORT_FORMAT": "html" }
    }
  },
  "launch": {
    "program": "scripts/report.py",
    "args": ["--title", "Debug report"],
    "cwd": "reports",
    "env": { "REPORT_FORMAT": "html" }
  }
}
```

Use lowercase language names, matching the existing LSP configuration (`python`,
`rust`, `cpp`, etc.). Each command has an `executable`, optional `args` array,
optional `cwd`, and optional `env` string map. Windows paths in JSON need doubled
backslashes; forward slashes also work. Arguments retain spaces, empty strings,
and shell metacharacters. Fenix does not split strings, expand variables, or run
an implicit shell. To use shell syntax, configure a shell executable explicitly.

Relative executable paths containing a slash, relative `cwd`, and the launch
`program` resolve against the project root. Command arguments remain literal and
relative argument paths are interpreted by the child in its working directory.
Omitting `cwd` uses the project root. Environment entries override inherited
values only for that child; they never mutate the editor's environment. Windows
bare executables and npm `.cmd` shims are resolved using the child's `PATH` and
`PATHEXT`. An explicit `PATH` override cannot silently fall back to the parent's
search path. `%PATH%`, `$PATH`, and `${workspaceFolder}` are literal strings here.

## Additional language servers

A buffer can be served by more than one language server: its language's own
(the *primary*, configured as above or built in) and any number of additional
servers. Declare them under `"lsp"` as `"additional"`, each with a `name`, the
`languages` it serves, and the same `executable`/`args`/`cwd`/`env` as any
other command. For example, rust-analyzer for Rust plus a requirements server
that also serves Markdown and Tcl:

```json
{
  "lsp": {
    "rust": { "executable": "rust-analyzer" },
    "additional": [
      {
        "name": "kiln",
        "executable": "kiln",
        "args": ["lsp"],
        "languages": ["rust", "markdown", "tcl"]
      },
      {
        "name": "spell",
        "executable": "spell-ls",
        "args": ["--stdio"],
        "languages": ["markdown"]
      }
    ]
  }
}
```

Here a `.rs` file is served by rust-analyzer and `kiln`, a Markdown file by
`kiln` and `spell` (Markdown has no primary server), and a `.tcl` file by `kiln`
alone. The per-language entries keep working exactly as before, and a project
with no `"additional"` behaves as it always has.

- **One process per server and project.** `kiln` above runs once per project
  and is told about every Rust, Markdown and Tcl buffer you open there
  (`didOpen`, `didChange`). Each server keeps its own capabilities and its own
  document versions.
- **Diagnostics** from every server are shown together; each names its source
  (the server's own `source`, else its name). A server clearing its
  diagnostics leaves the others'.
- **Completion** merges every server's items. A character a server declared a
  trigger (`.`, `-`) asks only the servers that declared it. While the popup
  stays at one place, only servers that said their list was incomplete are
  asked again as you type. Inside a comment or a string, local words are left
  out but servers are still asked, so a server can complete an ID in
  `// implements: REQ-014`. With more than one server answering, the popup
  names the server of each item.
- **Hover** (`K`) shows every server's answer, the primary's first, separated by
  a rule; empty answers are dropped, and answers show as they arrive.
- **Definition, references and code actions** are merged (locations without
  repeats); with several servers, each code action names its server, and
  applying it is checked against that server's view of the documents.
- **Rename and formatting** go to the primary only -- unless the primary can't,
  and exactly one additional server can.
- **A slow server** doesn't hold the others up: after a short deadline (0.8 s),
  `gd`, `gr` and code actions use what has arrived; a server that errors on a
  request is skipped for it.
- **A server that fails** to start or crashes is reported by name, like a
  primary's failure, and leaves the buffer's other servers running. It waits for
  `:lsp-restart` rather than retrying every frame.

Names are unique, and `languages` are lowercase language names as above. A
missing name or language, a duplicate name, or an unknown field is reported
like any other invalid `tools.json`. The project doctor (`SPC p h`) checks each
additional server's program, the tool status (`SPC l m`) lists them with what
they serve and whether they're running, and the project settings (`SPC p ,`)
edit them as `name: languages = command`.

## Precedence and lifetime

- Project LSP commands override global `[lsp] serverN` INI entries and built-in
  defaults. Existing INI entries retain their original whitespace-split behavior.
- Project DAP commands override built-in adapter commands. `launch.args`, `cwd`,
  and `env` describe the debugged program; the DAP command's corresponding fields
  describe the adapter process. Launch program/arguments fall back to the legacy
  `[launch]` section, then the current file. Adapter-specific launch behavior still
  depends on the adapter; reverse requests such as `runInTerminal` remain unsupported.
- Structured tasks override discovered/INI tasks with the same display name.
  Task discovery and each new run re-read settings. The task header reflects the
  selected executable and arguments; output paths resolve against its actual cwd.
- LSP environment and command settings stay fixed for the session. Run
  `:lsp-restart` from the project to reload them. It restarts that project's
  servers only (and re-reads its additional servers); `:lsp-restart NAME`
  restarts one -- an additional server's name, a primary server's program,
  or a language. Failed/unavailable servers wait for this command rather than
  retrying every frame. Debug settings are frozen when a session starts.

Missing `tools.json` preserves the old configuration. Invalid JSON, unknown
fields, invalid environment entries, and empty executable names produce visible
errors instead of silently falling back to a different command.

## Project boundaries

Task rerun history is per project. Fenix still supports one active task run and
one active debug session at a time. Starting a task in another project while one
is running is refused; use the existing task panel to cancel it first. Debug
continue/step/stop/watch controls refuse to operate on another project's session;
switch to its debug panel to control it. Breakpoints are sent only to the owning
project's adapter. Refactor edits and diagnostics from a language server are
checked against project ownership, and late LSP/DAP events cannot affect replacement
sessions because each connection has a generation ID.

This is local project isolation, not a remote execution environment or a sandbox.
Concurrent task/debug panels, WSL/remote environment identities, and live adapter
smoke tests remain follow-up work.
