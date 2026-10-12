# The control channel

A running Fenix listens on a channel only you can reach, and anything
you run can use it to ask Fenix to open files at a line, open a project,
or say what it's showing. `fenix somefile` uses it to hand the file to
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
{ "protocol": 1, "version": "1.1.0", "pid": 9732, "capabilities": ["fenix/open", "fenix/openProject", "fenix/state"] }
```

`capabilities` lists the methods this Fenix implements besides
`fenix/hello`. Check it rather than the version: a method missing from it
answers `-32601`. `client` is for Fenix's diagnostics; `version` and
`pid` may be left out.

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

## Errors

| Code | Meaning |
|---|---|
| `-32700` | The body isn't JSON |
| `-32600` | Not a JSON-RPC 2.0 request, or a message the framing refuses |
| `-32601` | No such method |
| `-32602` | The params don't fit the method |
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
