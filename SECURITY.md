# Security

## Reporting a vulnerability

Please don't open a public issue for something that could put people who
use Fenix at risk.

Report it privately instead: on this repository's **Security** tab,
choose **Report a vulnerability**. That opens a report only the
maintainer can see. Say what you found, how to reproduce it, and which
version of Fenix and which system you saw it on.

A fix is prepared in private and
released before the details are published; you're credited in the
release notes unless you'd rather not be.

## Which versions get fixes

The latest release. Fenix is a desktop program with no long-term support
branches: a fix ships in the next release.

## What counts

Fenix runs on your machine as you, and so does a good deal of what it
starts: language servers, debug adapters, tasks, coding agents. Those
are programs you chose, with your rights, and what they do isn't a
vulnerability in Fenix. These are:

- Something that runs a program, or reads or writes a file, **without
  your having asked for it or allowed it**: by opening a file or a
  project, following a link, or looking at a document.
- A project's own programs (`.fenix/tools.json`, `.fenix/lsp/`) running
  before you've allowed them for that project (`SPC p y`).
- An extension doing what its permissions don't let it: reaching the
  network, a file, a secret or a program it didn't declare.
- An extension from the store installed or updated without a valid
  signature.
- Another program or another user on the machine getting Fenix to act
  for them through its control channel, its MCP server, or the
  listeners remote projects use.
- A remote machine (remote projects) reading or writing files on yours
  beyond what you copied to or from it.
- A secret (an API key, a token, a password typed for `ssh`) written
  somewhere it shouldn't be: a log, a session file, a crash report.

## What Fenix trusts

Worth knowing, because none of it is a bug:

- **Programs you configure.** A language server, an agent or a debug
  adapter you set up runs as you. A language server may itself run a
  project's build scripts.
- **Projects you've allowed.** Once you answer yes at `SPC p y`, that
  project's `.fenix/tools.json` is run as written, until it changes.
- **Coding agents.** An agent is a program with your rights. What it
  may do *through Fenix* is governed by `agents.mcp.allow`; what it does
  on its own is its own.
- **The remote machine, for remote projects.** Fenix's helper there
  runs as your account on that machine and is as trustworthy as that
  account.
