# Remote projects over SSH

Status: Phase 0 (the spike) done 2026-11-09, on `claude/remote-ssh-spike`:
the design holds. See [Phase 0: what the spike found](#phase-0-what-the-spike-found).
Nothing is wired into Fenix yet.

The aim: open a folder on another machine as a Fenix project, over SSH,
and work in it as in a local one. Files first (browse, open, edit, save),
then terminals, Git and search, then language servers, tasks and the
debugger.

## Where Fenix stands today

Found by reading the tree at 1.4.1:

- **No remote code at all.** No SSH or SFTP dependency, no abstraction
  over the file system or over where a process runs.
- **Files are `std::fs`, everywhere.** About 1,200 direct calls, 670 of
  them in `fenix-gui`. A document is read by `fenix_core::Buffer::from_path`
  and written by `Buffer::save` (through `fenix-storage`'s staged write);
  a folder is listed by `fenix_fs::list_dir`. Many of the other calls are
  about Fenix's own files (settings, state, extensions, recovery) and
  stay local whatever the project is.
- **A file is a `PathBuf`, everywhere.** Buffers are indexed by
  `path_key`, the project is a root `PathBuf` (`App::open_project`),
  recents, sessions, recovery, LSP URIs and Git all take paths.
- **Processes are `std::process::Command`, built in about 70 places**,
  but most pass through a few doors that already take a `Command`:
  `fenix_git::process::git_command`, `fenix_project::process::quiet`
  (`rg`, `fd`, `git`), `LspClient::spawn_command`, `ProcessTree::spawn`,
  `TaskRunner::spawn_command`, the DAP client.
- **Terminals are `portable-pty`** (`fenix_terminal::Terminal`), a local
  ConPTY or pty with a `vt100` grid on top.
- **Everything is synchronous.** No async runtime outside `fenix-wasm`;
  work that may be slow goes to a thread and comes back as an event.
- The client machine has OpenSSH 10.3 (`ssh.exe`). Windows' OpenSSH has
  **no connection multiplexing** (`ControlMaster`), so every `ssh host
  cmd` is a full handshake.

## Phase 0: what the spike found

Built: `crates/fenix-remote` (the protocol and the helper) and
`crates/fenix-ssh` (session, connect and install, askpass, the stand-in),
with `fenix-ssh-spike`, a throwaway program that runs the checks below.
Run from this Windows 11 machine (OpenSSH 10.3) against Debian in a
Docker container on the same machine: **a real `sshd`, but no real
network**. Every time below has no network latency in it.

**Held up:**

| Question | Answer |
|---|---|
| Does the reserved root fail fast? | `\?\fenix-ssh\host\…`: every `std::fs` call fails with "not found" in under 0.1 ms. (A plain `\fenix-ssh\…` took 2.7 s looking for a server: the `\?\` matters.) |
| Does askpass work with Windows' `ssh.exe`? | Yes: a new host key, a password and a key's passphrase all came through the hook and were answered by the program. A wrong password ends with `ssh`'s own "Permission denied". |
| Can the helper be built from Windows and installed over `ssh`? | Yes. Static musl build with `rust-lld`, 706 KB. Installed, counted there (SHA-256) and started in 1.1 s; later connects 0.1 to 0.3 s. |
| A shell that prints a greeting? | Skipped, on the start and (after a fix) on the install. |
| Files | List, read, checked save. A save keeps the mode; a save over a file changed there is refused and leaves it alone; nothing staged is left behind. |
| One connection for everything? | stat 1.2 ms, list 1.5 ms, `git status` 7 to 9 ms over the session, against 115 to 165 ms for an `ssh` per command. |
| The stand-in (`--remote-exec`) | Gives the program's stdout, stderr and exit code, carries 1 MiB through stdin and back. 40 to 47 ms per `git status`, most of it starting a Windows process: a local `git` takes 27 to 45 ms to start here. |
| Does anything stay behind? | No. A killed program ends in 3 ms with what it started; killing the stand-in ends the remote program; ending the connection ends the helper and its programs. |
| A dead host | "Connection refused" in 2 s, as `ssh` says it. |

16 MiB saves in 0.45 s and reads in 0.3 s on tmpfs. On the container's
disk the save took 2.1 s: that is the `fsync`, not the connection.

**Learned, and to carry into Phase 1:**

- **`PathBuf` traps on the reserved root.** `root.join("/etc/passwd")`
  drops the host (`\?\fenix-ssh\etc\passwd`), and `join("a:b")` reads
  `a:` as a drive. Remote paths must be built and taken apart by
  `fenix_fs::vfs`'s own functions, never by `join` on text that came
  from the remote. `..` is resolved by `join` on a `\?\` path, which
  is what we want.
- **The control pipe can't carry the stand-in.** It is one synchronous
  duplex handle, so a waiting read holds up every write; a program's
  stdin and stdout need both at once. The spike used a loopback TCP
  port with a token. The real thing wants an overlapped named pipe (a
  Unix socket elsewhere) of its own, with the control pipe's
  same-user check.
- **A passphrase or password is asked once per `ssh` run**: three times
  on the first connect to a machine (look, install, start), once on
  each later connect, and again on every reconnect. Phase 1: remember
  the answer in memory for the life of the session so a reconnect
  doesn't ask, fold the install into two runs, and say in the docs that
  an agent (`ssh-agent`) means no question at all.
- **The remote `PATH` is the bare one** (`/usr/local/bin:/usr/bin:/bin`):
  `ssh host cmd` reads no login profile. `git` is found; a language
  server in `~/.cargo/bin` or under `nvm` would not be. Phase 1: the
  helper asks the user's login shell for its environment once
  (`$SHELL -lc env`) and starts programs with it.
- **Anything printed by the remote shell must be expected** before any
  line Fenix reads. Every line Fenix looks for is marked
  (`FENIX-REMOTE`, `FENIX-MISSING`, `FENIX-SUM`).
- **Two Rust installs on this machine.** `cargo` on `PATH` is the
  stand-alone 1.98, which has no musl target; rustup's has. The helper
  is built with rustup's `cargo` and `rustc` into a target folder of its
  own (`target/helper-build`), or the two compilers trip over each
  other's files. CI should build the helpers on Linux.
- Whole files travel in one frame (64 MiB at most) in the spike. Phase 1
  sends them in pieces, with progress and a way to cancel.

**Not tested, and still to prove on the way:**

- A real network: latency, a slow link, a connection that drops, sleep
  and wake. On a 50 ms link every figure above gains about 50 ms.
- `aarch64`, and any Linux other than Debian.
- `ProxyJump`, an agent, hardware keys, one-time codes.
- An older Windows OpenSSH (the 8.x that Windows 10 ships).
- A home folder mounted `noexec`.
- A pty, file watching and search: not in the spike.

To run it again: build the helper into `target/helpers/` as
`fenix-remote-linux-x86_64`, then

```powershell
cargo run --release -p fenix-ssh --bin fenix-ssh-spike -- HOST /path/to/a/git/repo/there
```

## The design

### One connection, one helper

Fenix starts **one** `ssh` process per remote host and talks over its
stdin and stdout to a small program on the other end, `fenix-remote`,
with `fenix-rpc`'s framed messages. Every file read, every `git`, every
terminal is a stream inside that one connection.

```
Fenix (Windows) ── ssh.exe -T host ~/.fenix/remote/1.5.0/fenix-remote ──► fenix-remote (Linux)
   fenix-ssh: session, requests, streams                files · watch · exec · pty · search
```

Why this and not the others:

| Option | Why not |
|---|---|
| `ssh host cmd` per operation | A handshake each (0.3 to 2 s) with no multiplexing on Windows. Git status alone would stall the UI. |
| An SSH library in Fenix (`russh`, `libssh2`) + SFTP | Fenix would have to redo what `ssh` already does right: `~/.ssh/config`, `ProxyJump`, the agent, hardware keys, `known_hosts`, 2FA. Each is a way to fail to connect. SFTP also has no file watching. |
| Mount it (SSHFS / WinFsp) | A driver to install; Git and language servers would run locally against a network drive, slowly and wrongly. |
| Mirror the project locally and sync | Two copies that disagree. Git, builds and terminals want the real one. |

Using the system `ssh` means a host that works in a terminal works in
Fenix, with the same config. The helper gives what SFTP can't: file
watching, atomic checked saves, ptys, process trees, and search that
doesn't depend on `rg` being installed there.

**The helper** (`crates/fenix-remote`, a binary):

- Static Linux builds, `x86_64` and `aarch64` (musl), **shipped inside
  Fenix's installer**. Nothing is downloaded on the remote, so it works
  on a machine with no Internet.
- Put in place at the first connect: Fenix asks `uname -sm`, pipes the
  matching binary to `~/.fenix/remote/<version>/fenix-remote`, checks its
  SHA-256, and starts it. Later connects find it there. Old versions are
  removed.
- Protocol version checked in the handshake. Client and helper are
  always the same release.
- Does: stat, list, read, write (temp file, fsync, rename, mode kept),
  rename, delete, mkdir, copy; watch (inotify, on the folders Fenix
  shows and the files it has open, never the whole tree); exec with
  stdin/stdout/stderr and a killable process group; pty (open, write,
  resize, kill); file listing and text search with the `ignore` and
  `grep` crates (ripgrep's own engine).
- v1 lives and dies with its connection: no daemon, nothing left
  running. Keeping terminals across a reconnect is a later phase.

### Signing in

`ssh` is run with `SSH_ASKPASS=fenix --askpass` and
`SSH_ASKPASS_REQUIRE=force`. When `ssh` needs a passphrase, a password,
a one-time code or a yes to a new host key, it runs that, which asks
the running Fenix over the control channel (`fenix-control`), and Fenix
shows the question on its prompt line. Keys and the agent need no
question. Fenix stores no password.

### What a remote file is called

A remote file keeps being a `PathBuf`, under a reserved root that no
local file can have:

```
\\?\fenix-ssh\<host>\home\tom\proj\src\main.rs      (Windows client)
/fenix-ssh/<host>/home/tom/proj/src/main.rs        (Unix client)
```

shown to you as `host:/home/tom/proj/src/main.rs`.

This is the decision the rest hangs on. The alternative, a typed
`Location { host, path }` through every buffer, picker, session and
page, is a rewrite of how 120,000 lines name a file. With the reserved
root, buffers, tabs, recents, sessions, recovery and jumps work as they
are, and code is moved over one call at a time:

- `fenix_fs::vfs` (new): `read`, `write_checked`, `metadata`, `list`,
  `rename`, `remove`, `create_dir`, `exists`. Each looks at the root: a
  local path goes to `std::fs` as today, a remote one to its session.
- `fenix_ssh::command(program, cwd)` (new): gives back a
  `std::process::Command`. For a local `cwd`, the program itself. For a
  remote one, `fenix --remote-exec <host> -- program args`, a small
  local stand-in that joins the running connection and passes
  stdin/stdout/stderr and the exit code through. So Git, `rg`, language
  servers, debug adapters and tasks keep their code: they are handed a
  `Command` and can't tell.
- A call not moved over yet fails with "not found" on a remote path. It
  can't touch a local file by mistake. (The spike checks that the
  Windows root fails at once, with no network lookup.)
- A guard test, like the one for external commands, lists the modules
  that deal in project files and fails on a new `std::fs` call there.

Limits to accept: a remote file name with `\` in it can't be shown from
Windows; names differing only in case are two files there and are kept
so (`path_key` must not fold case under the remote root).

### Not freezing

A remote call takes milliseconds on a LAN and far longer on a bad
link, and today's file calls are made where they're needed, often on
the UI thread.

- Opening a file, listing a folder, saving and Git refreshes on a
  remote project go to a worker and come back as events, with a
  "loading" state in the pane. Local projects keep the direct path.
- Every request has a deadline. A connection that stops answering is
  declared lost; nothing waits forever (the lesson of the sleep freeze).
- A small cache of metadata and listings, dropped by watch events.

### When the connection drops

- Buffers are local: no text is lost, and recovery keeps writing as
  now.
- The modeline says `host: reconnecting…`; Fenix retries with backoff,
  and at once on a network change or wake from sleep.
- A save while offline fails and says so; the buffer stays modified.
- On reconnect: watches are set again, open files are checked against
  the remote (changed there and unmodified here → reloaded; changed on
  both → the existing "changed on disk" question), language servers
  start again, terminals show `[connection lost]` and offer a new shell.

### Saving

`write_checked` sends the text with the size and mtime Fenix last saw.
The helper refuses if the file changed since (the same question as a
local file changed on disk), else writes a temp file beside it, syncs,
renames, and keeps mode and owner. Large files go in chunks with
progress; above a size limit Fenix asks first.

## Feature by feature

| Feature | On a remote project | Phase |
|---|---|---|
| Explorer | Listing, create, rename, delete, copy and move through the helper. Delete is permanent and asks (no Recycle Bin there). Copy between local and remote = upload/download. | 2 |
| Open, edit, save | `Buffer::from_path` / `save` through `vfs`. Encodings and line endings as now. | 2 |
| File picker, recents, session | Listing from the helper (`ignore`). Recents and sessions keep `host:/path`; a restored session connects when you look at it. | 2 |
| Terminal | A pty on the remote, in the project. `fenix-terminal` gets a second backend behind the same `Terminal`. | 3 |
| Git | The remote's `git`, through `fenix_ssh::command`. `fenix-git`'s 34 direct reads of `.git` go through `vfs`. Status, diff, stage, commit, log, branches, rebase. Push and fetch use the remote's credentials; agent forwarding is a per-host setting, off by default. | 3 |
| Search (`SPC /`, search page) | The helper's own search, so no `rg` needed there. Replace goes through `vfs`. | 3 |
| Language servers | Run on the remote through the stand-in. `path_to_uri`/`uri_to_path` map `file:///home/…` to the reserved root. Servers must be installed there; the project doctor checks the remote. | 4 |
| Tasks, debugger | Through the stand-in. Debug adapters over stdio only at first. | 4 |
| Port forwarding | `ssh -L` kept beside the connection, a list page. Needed for web work and for adapters that listen on a port. | 5 |
| Coding agents | The agent runs on the remote (it must be installed there); Fenix's MCP server reaches it through a forwarded stream. Until then, agents say they don't work on remote projects. | 5 |
| Extensions | Run locally. Their file calls on project paths go through `vfs`; `spawn` in a remote project runs there. Needs an API addition (which host a path is on) → API 1.6. | 5 |
| Terminals that outlive a drop | The helper as a daemon that keeps ptys and is rejoined. | 5 |
| Remote Windows or macOS hosts | macOS: one more helper build. Windows: its own work (paths, ConPTY, service). | later |

Not planned: editing as another user (`sudo`), X11.

## The work, in order

**Phase 0 — spike (a few days).** Prove the risky parts before any
plumbing: `fenix-remote` with handshake, `read`, `list`, `exec`; started
through `ssh.exe` from Windows with the askpass hook (key, passphrase,
password, new host key); the deploy step; the reserved root failing
fast in `std::fs`; latency of `git status` through the stand-in against
a real host. Throwaway UI. Stop and rethink if any of these fail.

**Phase 1 — foundation.**
- `fenix-remote-proto` (messages, shared), `fenix-remote` (the helper),
  `fenix-ssh` (session, streams, deploy, reconnect, askpass, hosts read
  from `~/.ssh/config`).
- `fenix_fs::vfs` and the remote path type's helpers; `path_key` rule.
- `fenix --remote-exec` and `fenix --askpass` in `cli.rs`.
- CI: build the two musl helpers, package them in the MSI, deb/rpm and
  the macOS bundle; an integration test against an `sshd` in a
  container (Linux CI), and the helper's own tests driven over a pipe
  with no SSH (every platform).

**Phase 2 — files.** Connect (`project.open_remote`: pick a host from
`~/.ssh/config` or type `user@host`, then browse to a folder),
explorer, open/save with the check, picker, recents, session, modeline
host badge, reconnect, async loading states, the guard test. Bindings
in both key schemes, tutorial untouched.

**Phase 3 — terminal, Git, search.** The first release worth calling
"remote projects".

**Phase 4 — language servers, tasks, debugger, remote doctor.**

**Phase 5 — forwarding, agents, extensions API 1.6, daemon.**

Each phase is one PR or a short stack, shippable on its own: until a
feature is moved over it says "not available on a remote project yet"
rather than misbehaving.

## For 1.5.0

Phases 0 to 3 are a realistic "remote projects (preview)" for 1.5.0 if
the release can wait for them; phases 4 and 5 are 1.6 material. It
doesn't touch the key-scheme work beyond adding a handful of commands
to both schemes, so it branches from
`claude/fenix-ide-keybindings-3sjjky` and can merge into it or follow
it.

## Risks

- **Scale of the move.** 1,200 file calls; perhaps a third concern
  project files. The reserved root makes it incremental and the guard
  test keeps it from regressing, but finding every one takes GUI
  passes on a real remote project.
- **UI-thread file calls.** Each one on a remote path is a possible
  stall. Phase 2 has to find them; a debug build can assert that no
  remote call is made on the UI thread.
- **Askpass on older servers and odd setups** (forced commands,
  restricted shells, `noexec` home). The helper path is configurable
  per host; a host where a program can't run is not supported.
- **inotify limits** on big trees: hence watching only what's shown and
  open.
- **Security.** The helper runs as you, with your rights there, and
  only what Fenix asks. The binary is checked by hash after upload. A
  remote project's `.fenix/settings.toml` is as trusted as a local
  project's (it can tighten, never loosen). Agent forwarding off by
  default.
