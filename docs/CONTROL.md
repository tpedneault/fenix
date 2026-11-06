# The control channel

A running Fenix listens on a channel only you can reach, and anything
you run can use it to ask Fenix to open files at a line, open a project,
say what it's showing, run one of its commands, capture a line into the
notebook, or clock in on an agenda task. `fenix somefile` uses it to hand the file to
the Fenix already running instead of starting a second one; a launcher,
a terminal's link handler or a `git difftool` script can use it the
same way. This page is the reference for writing such a client.

It is JSON-RPC 2.0 framed with `Content-Length` headers, the same
envelope as LSP and DAP. Fenix is the server; the client speaks first.

## From the command line

Most tools need nothing more than the `fenix` command, which connects,
hands the request over and exits (or becomes the editor, when no Fenix
is running):

```text
fenix [--new-window] [--project <dir>] [--goto <file:line[:col]>] [file[:line[:col]]...]
```

| Example | Does |
|---|---|
| `fenix notes.md` | Opens `notes.md` |
| `fenix src/main.rs:12` | Opens it with the cursor on line 12 |
| `fenix C:\src\x.rs:12:3` | Line 12, column 3; the drive letter's colon is part of the path |
| `fenix --goto build.log:200` | Always reads a position, even when a file is named `build.log:200` |
| `fenix --project ~/src/fenix` | Switches to that project, as `SPC p p` does |
| `fenix --new-window a.rs` | Opens `a.rs` in another OS window of the same Fenix |
| `fenix` | Brings the running Fenix to the front |
| `fenix --notify "Build done" --body "in 41 s"` | Shows a notification in the running Fenix (`--urgency low\|normal\|critical`, `--tag`, `--progress 0-100`), as `fenix/notify` does; exits 1 when no Fenix is running |

Lines and columns count from 1. A file argument is read as a position
only when no file of exactly that name exists. Relative paths are taken
from the directory `fenix` was started in. `--` ends the flags, for a
file whose name starts with `-`; any other unknown flag is ignored,
with a warning on stderr. `fenix --help` lists the options and
`fenix --version` prints the version; both print and exit, without
reaching a running Fenix.

## Where Fenix listens

The endpoint is per user and per login session: another user on the same
machine (a Remote Desktop host, a shared Linux box) has their own, and
can't reach yours. Build the name exactly as below.

**The instance suffix.** With `FENIX_HOME` set (a portable or test
install), the name gains `-<hash>`: the 64-bit FNV-1a hash (offset basis
`0xcbf29ce484222325`, prime `0x100000001b3`) of `FENIX_HOME`'s bytes as
the OS gives them (on Windows, the WTF-8 bytes Rust's
`OsStr::as_encoded_bytes` returns; for any ordinary path that's its UTF-8),
formatted as 16 lowercase hex digits. With `FENIX_HOME` unset or empty
there's no suffix. For example, `FENIX_HOME=foobar` gives
`-85944171f73967e8`.

### Windows

A named pipe:

```text
\\.\pipe\fenix-<SID>-<session>[-<hash>]
```

- `<SID>` is the user's string SID (`S-1-5-21-...`), from the process
  token's `TokenUser`.
- `<session>` is the Terminal Services session id of the process
  (`ProcessIdToSessionId`), in decimal. The same user logged in twice
  gets two editors, each in its own session.

Fenix creates the pipe with a DACL granting that SID alone
(`D:P(A;;GA;;;<SID>)`), `FILE_FLAG_FIRST_PIPE_INSTANCE` (so it never
shares a name someone else created first) and `PIPE_REJECT_REMOTE_CLIENTS`.

A client opens it with `CreateFileW(name, GENERIC_READ | GENERIC_WRITE,
0, NULL, OPEN_EXISTING, SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
NULL)`: identification-level impersonation, so Fenix can learn who you
are and nothing more. `ERROR_FILE_NOT_FOUND` means no Fenix is running;
`ERROR_PIPE_BUSY` means try again after `WaitNamedPipeW`. Because pipe
names are machine-wide, a careful client also checks the server is you:
`GetNamedPipeServerProcessId`, then that process's token user, which is
what `fenix` itself does.

### Linux

A Unix socket in a private directory:

```text
$XDG_RUNTIME_DIR/fenix/control[-<hash>].sock
/tmp/fenix-<uid>/control[-<hash>].sock       (when XDG_RUNTIME_DIR is unset or empty)
```

Fenix creates the directory `0700`, and before using it checks that it's
a real directory (not a symlink), owned by you, and not open to group or
others; otherwise it refuses, and runs without the channel. The fallback
is always `/tmp`, whatever `TMPDIR` says, so every client agrees on it.

Beside the socket, `control[-<hash>].lock` is held with `flock` for the
server's life: the lock, not the socket file, decides which Fenix serves.
A socket left behind by a crash is replaced by the next Fenix. A client
that gets `ENOENT` or `ECONNREFUSED` should take it that no Fenix is
running.

## Messages

Each message is a header block, a blank line, and a UTF-8 JSON body:

```text
Content-Length: 87\r\n
\r\n
{"jsonrpc":"2.0","id":1,"method":"fenix/hello","params":{"protocol":1,"client":{...}}}
```

Only `Content-Length` is read; other headers are ignored. Fenix refuses a
body over 4 MiB, a header line over 1 KiB, or more than 16 header lines:
it answers with an error (id `null`, code `-32600`) and closes the
connection.

A connection carries any number of requests, answered in order; Fenix
closes its end when the client closes its own. Batches (a JSON array) are
not supported. A request without an `id` is a notification: Fenix does it
and doesn't answer.

Paths are plain strings, in the form your platform uses, and absolute: a
relative path is resolved against Fenix's own working directory, which
is rarely what you mean. Paths Fenix sends are in plain form, never with
Windows' `\\?\` prefix.

## Methods

Every connection starts with `fenix/hello`; anything else first gets
`-32002`.

### `fenix/hello`

```json
{ "protocol": 1, "client": { "name": "flint", "version": "0.7.0", "pid": 4120 } }
```

```json
{ "protocol": 1, "version": "1.1.0", "pid": 9732, "capabilities": ["fenix/open", "fenix/openProject", "fenix/state", "fenix/commands", "fenix/runCommand", "fenix/notebook.capture", "fenix/agenda.tasks", "fenix/agenda.clockIn", "fenix/agenda.clockOut"] }
```

`capabilities` lists the methods this Fenix implements besides
`fenix/hello`. Check it rather than the version: a method missing from it
answers `-32601`, and every method below appears in it only in a Fenix
that answers it (builds from before the commands, notebook and agenda
methods list only the first three). `client` is for Fenix's
diagnostics; `version` and `pid` may be left out.

On Windows, pass the `pid` you get back to `AllowSetForegroundWindow`
before a request that should bring Fenix to the front. Windows only lets
the process the user is using hand the foreground on; without it, Fenix
can only flash its taskbar button.

A protocol other than 1 answers `-32001` with
`"data": { "supported": [1] }`, and the connection is still waiting for a
hello it can accept.

### `fenix/open`

```json
{ "paths": [{ "path": "C:\\src\\x.rs", "line": 12, "column": 3 }, { "path": "C:\\src\\y.rs" }], "newWindow": false, "focus": true }
```

Result: `{}`.

Opens each file as picking it in `SPC f f` does; the last one ends up
focused. `line` and `column` (1-based, both optional) place the cursor,
and the view scrolls to it; a position past the end lands on the last
line. `newWindow` (default `false`) opens another OS window for them.
`focus` (default `true`) brings Fenix forward, un-minimizing it. An
empty `paths` with `focus` just brings Fenix forward.

A file that isn't there yet, in a folder that is, opens as a new file
named for it, created when it's saved, as `SPC f f` does. An empty
`path`, a folder, or a file in a folder that doesn't exist answers
`-32602` with the reason, and then none of the paths open.

### `fenix/openProject`

```json
{ "root": "C:\\src\\fenix" }
```

Result: `{}`.

Exactly what choosing the folder in `SPC p p` does: it becomes the
project, joins the known projects (`SPC p p`'s list, and Home's), and
the file finder opens on it. Fenix comes forward. A path that isn't a
folder answers `-32602`.

### `fenix/state`

Params: `{}`. Result, each field left out when there's nothing to report:

```json
{ "project": "C:\\src\\fenix", "file": "C:\\src\\fenix\\Cargo.toml", "line": 3 }
```

`project` is the current project's root, `file` the file in the focused
pane (absent when a page or tool is in focus) and `line` the cursor's
line in it, from 1.

While the agenda's clock runs, there's also:

```json
{ "agendaTimer": { "taskId": 7, "title": "Write the report", "since": "2026-10-12T09:30:00+02:00" } }
```

`taskId` is the task's `id` in `fenix/agenda.tasks`; `since`, when the
clock started, is RFC 3339 with the local offset.

### `fenix/commands`

Params: `{}`. Result:

```json
{ "commands": [
  { "id": "agenda.open", "description": "Open the agenda page where it was left", "keys": "SPC A a", "external": true },
  { "id": "file.save", "description": "Save the current file", "keys": "SPC f s", "external": false },
  { "id": "view.reset_font_size", "description": "Reset the body text size to the default", "external": false }
] }
```

Every command Fenix has, in the order it registers them. `keys` is the
leader sequence that runs it, written as Fenix's key menu writes it
(`SPC` for the space bar, `C-x` for Ctrl+x, keys separated by spaces);
the shortest one when there are several, and absent when no leader key
runs it. The buffer-specific `SPC m` menu isn't included.

`external` says whether `fenix/runCommand` runs it: true for commands
that make sense with nobody looking at a file in Fenix (open the
agenda, switch project, today's journal, the clock picker), false for
ones that act on what's in front of you there (save, undo, stage the
hunk under the cursor), that start from the file you're in (the Git
page of its repository, a capture linking back to it) or that need more
than the editor (quit, open an OS window). For a capture from outside,
use `fenix/notebook.capture`.

### `fenix/runCommand`

```json
{ "id": "project.switch_project" }
```

Result: `{}`, once the command has run. Fenix comes forward, as for
`fenix/open`: most external commands open a page, a picker or a prompt
for you to carry on in. A command that isn't `external`, or an id Fenix
doesn't have, answers `-32602` with a message saying which.

### `fenix/notify`

```json
{ "summary": "Nightly build failed", "body": "3 tests failed in fenix-tasks", "urgency": "critical",
  "source": "ci", "tag": "nightly", "progress": 40,
  "actions": [{ "label": "Open the log", "command": "url:https://ci.example.com/run/812" }] }
```

Result: `{ "id": 7, "shown": true }`; `"shown": false` when it was only
kept in the history (do not disturb, a rule, notifications off), and
`"muted": true` when a rule dropped it.

Shows a notification as Fenix's own are shown: a toast, the history
(`SPC o n`), and the system's notification while Fenix isn't in front,
as the user's `[notifications]` settings and rules say. Only `summary`
is needed. `urgency` is `low`, `normal` (the default) or `critical`;
`source` is what the history and the rules call it (`cli` when it's not
given); one with the same `source` and `tag` replaces the one showing;
`progress` is 0 to 100. An action's `command` is a Fenix command that
`fenix/runCommand` may run, `url:https://...`, or `open:<path>[:line]`;
others are dropped, and at most five are kept. An empty summary, or
another urgency, answers `-32602`.

### `fenix/notebook.capture`

```json
{ "text": "call the ground station about the pass", "target": "inbox" }
```

Result:

```json
{ "entry": "C:\Users\you\AppData\Roaming\fenix\data\notebook\Inbox.md" }
```

Files the line exactly as `SPC n c` does, stamped with the time: under
the notebook's Inbox note (made if it isn't there yet) with `"target":
"inbox"`, the default, or under the Log of today's journal day with
`"journal"`. `entry` is the file it went into. Fenix writes it, keeping
the notebook's index and any open buffer of that file in step, and
doesn't take the focus. Text that's empty or only whitespace answers
`-32602`; a notebook that can't be written, `-32603` with why.

### `fenix/agenda.tasks`

```json
{ "status": ["todo", "inProgress"] }
```

Result:

```json
{ "tasks": [
  { "id": 7, "title": "Write the report", "status": "inProgress", "jiraKey": "ABC-12", "elapsedMinutes": 95 },
  { "id": 9, "title": "File it", "status": "todo", "elapsedMinutes": 0 }
] }
```

The agenda's tasks, archived ones left out, in the board's order (by
status, then as arranged within it). `status` (optional) keeps those in
any of `todo`, `inProgress`, `blocked`, `done`; another value answers
`-32602`. `id` is a number, stable for the task's life and never reused.
`jiraKey` is there when the task is linked to an issue.
`elapsedMinutes` counts every minute logged on it, the running clock's
included.

### `fenix/agenda.clockIn`

```json
{ "taskId": 7 }
```

Result: `{}`. Starts the clock on the task, stopping whatever clock was
running first, exactly as choosing it in `SPC A t` does; the agenda is
saved at once. `taskId` may also be given as its decimal string
(`"7"`). A task that doesn't exist, or is archived, answers `-32602`.

### `fenix/agenda.clockOut`

Params: `{}`. Result: `{}`. Stops the clock, recording the time on its
task, as `Stop` in `SPC A t` does. With no clock running it does
nothing.

### `fenix/diagnostics`

Params: `{ "path"?: string }`. The language servers' problems in `path`,
or in every file open in Fenix -- following unsaved edits:

```json
{ "diagnostics": [{ "path": "C:\\src\\x.rs", "line": 12, "column": 5, "endLine": 12, "severity": "error", "message": "mismatched types", "source": "rustc" }] }
```

Lines and columns from 1; `severity` is `error`, `warning`,
`information` or `hint`.

### `fenix/buffers`

Params: `{}`. The files open in Fenix:
`{ "buffers": [{ "path", "unsaved": bool, "focused": bool }] }`.

### `fenix/selection`

Params: `{}`. What's selected in the editor, or the cursor's line:
`{ "path"?, "line", "text", "selected": bool }`. While a coding agent's
panel has the focus, what was last sent to it as `@selection`.

### `fenix/tasks`

Params: `{}`. The project's tasks, as `SPC t t` lists them:
`{ "root", "tasks": [{ "name", "command", "args" }] }`.

### `fenix/tasks.run` and `fenix/agentCommand`

Params: `{ "name": string }` / `{ "id": string }`. Run a project task in
the task pane, or a Fenix command marked `external`. For the coding
agents (`fenix --mcp`'s `run_task` and `run_command`): both are refused
(`-32602`) unless `agents.mcp.fenix_write` is on.

### `fenix/buffer.read`

Params: `{ "path": string, "startLine"?: number, "endLine"?: number }`,
lines from 1. An open file as it stands in Fenix, unsaved edits and all:
`{ "path", "unsaved", "lines", "startLine", "endLine", "text", "truncated" }`.
At most 1 MiB of text comes back. A file that isn't open is refused
(`-32602`): its text is on disk.

### `fenix/lsp`

Params: `{ "kind": string, ... }`. What the language servers say,
answered once they have, or after 20 seconds with what's come. A
relative `path` is from the project. A file needn't be open; one that
isn't is read from disk and shown to its servers, which start if they
need to (`-32602` says to ask again while they do).

| `kind` | Params | Result |
|---|---|---|
| `definition`, `references` | `path`, `line`, and `symbol` or `column` | `{ "locations": [{ "path", "line", "column", "text" }], "total", "truncated" }` |
| `hover` | `path`, `line`, and `symbol` or `column` | `{ "hover": string or null }` |
| `document_symbols` | `path` | `{ "symbols": [{ "name", "kind", "line", "endLine", "depth", "detail" }] }` |
| `workspace_symbols` | `query` | `{ "symbols": [{ "name", "kind", "path", "line", "container" }], "total", "truncated" }` |

Lines and columns are from 1. `symbol` is the name on `line`, found as a
whole word, for callers that know a line but not a column. With neither,
it's the line's first non-blank. Lists stop at 200.

### `fenix/debug.state` and `fenix/debug.evaluate`

`fenix/debug.state`: params `{}`. `{ "session": false }` with no debug
session, else `{ "session": true, "name", "ended", "stopped",
"selectedFrame", "frames": [{ "function", "path", "line" }], "scopes":
[{ "scope", "variables": [{ "name", "value", "children"? }] }],
"watches", "console" }`: what the drawer has, and the last 40 lines of
the program's output.

`fenix/debug.evaluate`: params `{ "expression": string }`, evaluated in
the selected frame: `{ "result", "type" }`. It can run code in the
program, so it's refused (`-32602`) unless `agents.mcp.fenix_write` is
on, and when the debugger isn't stopped.

### `fenix/tools` and `fenix/tools.call`

`fenix/tools`: params `{}`. What extensions offer agents
(`contributes.agent_tools`): `{ "tools": [{ "name", "description",
"inputSchema", "extension", "writes" }] }`.

`fenix/tools.call`: params `{ "name": string, "arguments"?: object }`.
Runs one: the result is what its service answers, as JSON if it is, else
as a string. One with `"writes": true` is refused (`-32602`) unless
`agents.mcp.fenix_write` is on.

### `fenix/agent.*`

What MCP bridges (`fenix --mcp`) use. Each call is made for the
connection's caller: the `client.name` it said hello with and, for a
bridge Fenix started, the agent thread (`client.thread` in
`fenix/hello`'s params). A caller with no thread, or one Fenix doesn't
know, is a client of yours: the `client:` rules apply to it. Every tool
has a class (`read`, `show`, `edit`, `run`, `external`) and a toolset,
and `agents.mcp.allow` says what each may do ([Coding agents](AGENTS.md#fenix-as-a-tool)).

- `fenix/agent.tools`: params `{}`. The tools the caller is offered
  now, MCP's shape: `{ "tools": [{ "name", "description", "inputSchema" }] }`.
  A tool its toolset doesn't show, or the rules refuse, isn't listed.
- `fenix/agent.call`: params `{ "call": string, "tool": string,
  "arguments"?: object, "progressToken"? }`. Runs one, Fenix's or an
  extension's: `{ "answer": ... }`, with `"toolsChanged": true` when it
  turned a toolset on or off. A tool the rules refuse is `-32602`; one
  they ask about waits for your answer on the caller's thread. `call` is
  the caller's id for it, for `fenix/agent.cancel`. `fenix_call`'s
  `{ "tool", "arguments" }` is the tool it names, decided as that one.
- `fenix/agent.cancel`: params `{ "call": string }`. The caller gave up
  on it: a question about it is taken back, and the call answers
  `cancelled`.
- `fenix/agent.wait`: params `{ "since": number }`. News since that
  revision, answered when there is some or after 20 seconds: `{
  "revision", "events": [{ "kind": "tools" | "resources" | "prompts" |
  "resource" | "progress" | "log", ... }] }`. `since: 0` answers at once
  with the revision to wait from.
- `fenix/agent.resources`, `fenix/agent.read`, `fenix/agent.subscribe`,
  `fenix/agent.prompts`, `fenix/agent.prompt`: MCP's resources and
  prompts, in MCP's shapes.

## `fenix --mcp`

`fenix --mcp` is an MCP server on stdio (JSON-RPC, a message a line).
Each tool, resource and prompt is a `fenix/agent.*` call to the Fenix
that's running, which decides by its rules; the bridge decides nothing
itself. It speaks MCP revisions 2024-11-05 to 2025-11-25, and tells its
client when the tool list changes. Fenix hands it to the coding agents it
starts as `fenix --mcp --thread <id>` (`agents.mcp.fenix`); any MCP
client can run it, with a Fenix running to answer, and is then a client
of yours.

## Fenix's files

Tools should change the agenda and notebook through these methods, not
by writing `agenda.json` or the notebook's files: Fenix saves the agenda
whole, so a change written behind its back can be lost, and the
notebook's index assumes Fenix made every change. Reading them is fine.
Fenix does read `agenda.json` again when it changes on disk, for the
agenda that roams with a Windows profile between machines, but that is
a safety net, not a way in.

## Errors

| Code | Meaning |
|---|---|
| `-32700` | The body isn't JSON |
| `-32600` | Not a JSON-RPC 2.0 request, or a message the framing refuses |
| `-32601` | No such method |
| `-32602` | The params don't fit the method, or name something Fenix doesn't have (a command, a task) or won't do from outside (a command that isn't `external`) |
| `-32603` | Fenix failed inside, or didn't answer within 30 seconds |
| `-32001` | Unsupported protocol (`data.supported` lists the ones that are) |
| `-32002` | Send `fenix/hello` first |

## Older `fenix` binaries

Before this protocol, a later launch sent its arguments one per line and
closed the connection. For one release Fenix still accepts that on the
new endpoint: a connection whose first bytes aren't `Content-Length:` is
read to the end (at most 1 MiB) and opened as a `fenix/open` with focus,
with `--new-window` honoured, any other line starting with `-` ignored,
and no reply. An empty one brings Fenix forward. New clients should use
the protocol.

## A minimal client

On Linux, with Python:

```python
import json, os, socket

def call(sock, id, method, params):
    body = json.dumps({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).encode()
    sock.sendall(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    f = sock.makefile("rb")
    length = int(f.readline().split(b":")[1]); f.readline()
    return json.loads(f.read(length))

runtime = os.environ.get("XDG_RUNTIME_DIR")
folder = os.path.join(runtime, "fenix") if runtime else f"/tmp/fenix-{os.getuid()}"
with socket.socket(socket.AF_UNIX) as s:
    s.connect(os.path.join(folder, "control.sock"))
    print(call(s, 1, "fenix/hello", {"protocol": 1, "client": {"name": "example", "version": "1", "pid": os.getpid()}}))
    print(call(s, 2, "fenix/state", {}))
```

`crates/fenix-control` is Fenix's own implementation of both ends, in
Rust, with no dependency on the rest of Fenix beyond `fenix-rpc`.
