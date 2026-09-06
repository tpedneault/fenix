# Project tool settings

Fenix runs one language server per language and canonical project root. Opening
two Python or Rust projects no longer routes their requests to the same server.
Relative aliases of a root reuse the same session. The nearest project marker
determines ownership; `.fenix/project.ini` and `.fenix/tools.json` are also markers.

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
  servers only. Failed/unavailable servers wait for this command rather than
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
