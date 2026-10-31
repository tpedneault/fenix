# Coding agents

Fenix works with coding agents -- Claude Code, Codex, Devin, Gemini CLI,
and any other -- through the **Agent Client Protocol** (ACP). An agent is
a program Fenix starts and talks to over stdio, as it talks to a language
server over LSP. Every agent gets the same thread, the same review of
its changes and the same keys; nothing in Fenix depends on which one it
is.

## Setting one up

Fenix looks for each agent on `PATH`. Install the ones you want, log in
to them once in a terminal, and they're offered on `SPC a p`:

| Agent | What Fenix runs | Install | Log in |
|---|---|---|---|
| Claude Code | `claude-agent-acp`, else `npx -y @agentclientprotocol/claude-agent-acp` | `npm i -g @agentclientprotocol/claude-agent-acp` (Node 18+) | `claude` once (`/login`), or `ANTHROPIC_API_KEY` |
| Codex | `codex-acp`, else `npx -y @agentclientprotocol/codex-acp` | `npm i -g @agentclientprotocol/codex-acp` | `codex login`, or `OPENAI_API_KEY` / `CODEX_API_KEY` |
| Devin | `devin acp` | Devin's CLI | `devin auth login`, or `WINDSURF_API_KEY` |
| Gemini CLI | `gemini --acp`, else `npx -y @google/gemini-cli --acp` | `npm i -g @google/gemini-cli` | `gemini` once, or `GEMINI_API_KEY` |

- **Somewhere else**, or with other flags: `agents.servers` gives an
  agent's command line by name. A built-in's name replaces it:

  ```toml
  [agents.servers]
  devin = "/opt/devin/bin/devin acp"
  in-house = "\"C:\\Tools\\Our Agent\\agent.exe\" --acp"
  ```

- **From the ACP registry**: `SPC a p` → *Install an agent from the ACP
  registry…* lists the registry's agents for this machine. A download is
  checked against the registry's SHA-256 and unpacked into Fenix's data
  folder; a package `npx` or `uvx` runs is kept as its command line.
  Installed agents are offered beside the others, and uninstalled from
  the same page.
- **From an extension**: an extension can bring an agent of its own
  (`contributes.agents`); it's listed as soon as the extension is
  installed, and offered once the extension has started it.
- **API keys**: `SPC a s` keeps one in the system's keychain (Windows'
  Credential Manager, the macOS Keychain, the Secret Service) and gives
  it to every agent as its variable. `agents.env` sets other variables
  -- a proxy, a region -- for every agent.
- **Which one**: `agents.default` names the agent a new thread talks to;
  a project can set its own. Left empty, it's the first one found.

An agent that wants a login says so in its thread: `SPC a p` offers its
ways -- one the agent does itself, or its own login command, run in a
terminal pane under the thread -- and Fenix tries again after.

## The thread

Threads live in the agents' dock: one column at the right of the window,
beside all your panes (it never splits the one you're in), the thread
above and its prompt below. `SPC a a` brings it out on the project's
last thread, or a new one, with the prompt ready to write in; from the
dock, `SPC a a` puts it away. `SPC a A` shows or puts it away from
anywhere. The prompt offers no completions as you type; `@` and `/`
have pickers of their own. `SPC a n` always starts a new thread, in
the same dock, with the default agent (`agents.default`); `SPC a p`
starts one with another. Its title rows say which thread it is, which agent the
prompt goes to and in which mode.

- Above, the conversation: your prompts, the agent's answers, its
  thinking (folded), each tool call as a card -- what it read, the diff
  of what it edited, the output of what it ran -- and its plan.
- Below, the prompt: a buffer with Vim's keys. `Ctrl-Enter` sends it
  from Insert mode, `Enter` from Normal mode. A prompt sent while the agent works waits its turn; `SPC a k`
  cancels the turn (or takes back the waiting prompt). `↑` in an empty
  prompt brings back the one sent before, again the one before that;
  `↓` goes the other way.
- `@` in the prompt mentions a file (its text goes along, unsaved edits
  included), `@selection` (`SPC a e` puts it there), `@file` (the one
  beside the panel), `@diagnostics` and `@diff`. `/` offers the agent's
  own commands. `Ctrl-V` pastes a picture.
- When the agent asks before it edits or runs something, the question is
  a card in the thread with its answers (`y`, `n`, …), and the modeline
  says an agent is waiting on you. The same keys answer it from an empty
  prompt, without going back to the thread. When Fenix isn't in front,
  it asks for your attention (`agents.notify`).
- A command's card says what ran (`$ rg -n session`) when the agent's
  own title for it doesn't ("Ran command").
- `SPC a m` picks the agent's mode (Claude Code's *plan*, *accept
  edits*…), `SPC a M` its model and other settings.

In the thread: `j`/`k` move between cards, `Tab` opens or folds one,
`Enter` on a tool call opens the file it worked on, `y`/`Y` copy the
card or the agent's answer, `/` filters the cards, `i` goes to the
prompt, `Ctrl-C` cancels, `q` puts the dock away. [Key bindings](KEYBINDINGS.md#an-agents-thread-spc-a-a)
has them all.

## What it changes

Agents change files in one of two ways, and Fenix keeps both:

- **Through Fenix**: the edit goes into the buffer as one undo step and
  is saved as it's made (`agents.write = "save"`), so the agent's own
  builds see it; or left unsaved for you (`"buffer"`).
- **On disk, itself** -- Claude Code does this. It says which file it's
  about to edit; Fenix keeps the file as it was, and a buffer with no
  unsaved edits follows the change at once.

An edit over unsaved work of yours is asked about first. Agents read
from disk, so before a prompt Fenix offers to save the project's changed
files (`agents.save_before_prompt`).

**Agent changes** (`SPC a r`) shows every file the thread changed, hunk
by hunk, against what it was before the agent touched it -- whether the
agent said so or not (Fenix asks git at the end of each turn). `a` keeps
a hunk, `x` puts it back, `A`/`X` do the whole file; a new file rejected
goes to the Recycle Bin. `agents.accept_all` and `agents.reject_all`
(`SPC :`) do every file. Agent changes opens by your code, never in the
dock.

In the code, until kept, the lines an agent changed are tinted, and an
agent's change is handled like a git hunk: `]a`/`[a` go to the next or
previous one (across files), `SPC a y` keeps the one under the cursor,
`SPC a x` puts it back, and `SPC a v` shows what it replaced. A kept
change is an ordinary change for git from then on. Line endings are kept
as the file had them.

`SPC a f` follows the agent: the pane beside the thread shows each file
it works on, at its line.

## Threads

Threads are kept, per project. `SPC a t` lists the project's; one picked
up again continues its session where the agent can (it resumes it, or
loads it again). The dock and the prompt you hadn't sent come back with
the session after a restart -- as the dock, not as two more panes; the
agent starts again when you look.

- `SPC a i` continues a session you started in the agent's own CLI.
- `SPC a h` hands the thread to another agent: the conversation so far
  goes with its first prompt (`agents.handoff_kib`).
- A thread whose agent can't take it back is read-only; `C` continues it
  in a new session the same way.
- `SPC a D` deletes the thread, and its session where the agent can.

An agent with no thread open stops after `agents.idle_minutes`. One that
crashes is started again (after 1, 2, then 4 seconds) and its thread
resumed; after three crashes in a minute it stays down until
`SPC a R`.

## Shortcuts

- `SPC a d` asks the agent to fix the problem on the cursor's line.
- `SPC a c` has it write a commit message for what's staged, onto the
  clipboard.
- `SPC a e` asks about the selection.

## Fenix as a tool

Every session gets Fenix itself as an MCP server (`fenix --mcp`,
`agents.mcp.fenix`): the agent can ask for the language servers'
problems, the open files and which have unsaved edits, your selection,
and show you a file. With `agents.mcp.fenix_write` it can also run the
project's tasks and Fenix's commands. Your own MCP servers
(`agents.mcp.servers`) go to every session too.

An agent that runs its commands through Fenix (ACP terminals) gets them
run in the project, shown in its card as they go, the whole process tree
stopped on cancel (`agents.terminal.output_limit` keeps the end of the
output). Claude Code runs its own.

## Extensions

An extension can bring an agent, ask one (`agents.ask`: a prompt, and
the answer as text, in a thread of its own on `SPC a t`), and hear when
a turn ends. See [Writing extensions](WRITING_EXTENSIONS.md#coding-agents).

## When something's wrong

- `SPC h a` shows everything said on each connection, both ways, with
  secrets hidden.
- `SPC a p` says which agents were found, and the command each runs.
- Outside Fenix, the chat example talks to an agent through the same
  code, in a terminal:

  ```text
  cargo run -p fenix-acp --example chat -- claude-agent-acp
  cargo run -p fenix-acp --example chat -- devin acp
  cargo run -p fenix-acp --example chat -- npx -y @agentclientprotocol/codex-acp
  ```

- The conformance suite runs the same scenarios against each real agent
  installed and logged in on this machine (each in a temporary folder of
  its own); [ADR 0002](adr/0002-agents-acp.md) records what passed:

  ```text
  FENIX_ACP_LIVE=claude,codex,devin cargo test -p fenix-acp --test conformance -- --ignored --nocapture
  ```

  `FENIX_ACP_<ID>` gives an agent's command line when it isn't the one
  on `PATH` (`FENIX_ACP_DEVIN="/opt/devin/bin/devin acp"`).

The settings are under **Agents** on the settings page (`SPC ,`), and in
[Configuration](CONFIGURATION.md); the keys in
[Key bindings](KEYBINDINGS.md).
