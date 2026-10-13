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

Lines and columns count from 1. A file argument is read as a position
only when no file of exactly that name exists. Relative paths are taken
from the directory `fenix` was started in. `--` ends the flags, for a
file whose name starts with `-`; any other unknown flag is ignored.

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
  { "id": "agenda.open", "description": "Open the agenda page where it was left", "keys": "SPC a a", "external": true },
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
running first, exactly as choosing it in `SPC a t` does; the agenda is
saved at once. `taskId` may also be given as its decimal string
(`"7"`). A task that doesn't exist, or is archived, answers `-32602`.

### `fenix/agenda.clockOut`

Params: `{}`. Result: `{}`. Stops the clock, recording the time on its
task, as `Stop` in `SPC a t` does. With no clock running it does
nothing.

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
