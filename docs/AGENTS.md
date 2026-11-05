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
`agents.mcp.fenix`). The agent can ask for:

- the language servers' problems, and their definitions, references,
  hover and symbols, for any file, open or not;
- the open files, which have unsaved edits, and their text as it stands
  in Fenix, which the disk doesn't have yet;
- your selection;
- where the debugger is stopped: the stack, the variables, watches and
  the program's output.

It can show you a file too, run the project's tasks, evaluate in the
debugger, and call the extensions' tools, as your rules let it.

Beyond those, Fenix's toolsets give an agent:

- **lsp**: `implementations`, `call_hierarchy` (who calls it, or what it
  calls), `type_hierarchy`, `signature_help`, and `code_actions`, each
  with an id.
- **refactor**: `rename` by the language server, `apply_code_action`,
  and `format` (a file or some lines). These edit, so they're asked
  about unless you allow them.
- **git**: `git_status`, `git_diff` (unstaged, staged, or a branch
  against its base), `git_log` (of a branch, a path, a range or a few
  lines), `git_show`, `git_blame` (unsaved lines say so), and
  `git_stage`. Nothing commits or pushes: you do.
- **search**: `search` sees unsaved text and the project's ignores,
  which the agent's own `rg` doesn't; `replace` changes every match, as
  edits you review.
- **syntax**: `syntax_node` says what function, class or block a place
  is in, and `syntax_query` runs a tree-sitter query, in any language
  Fenix has a grammar for.
- **debug**: `debug_launches`, `debug_start`, `breakpoints`,
  `set_breakpoint` (with a condition or a log message),
  `clear_breakpoint`, `debug_continue`, `debug_step` (over, into, out),
  `debug_run_to` a line, `debug_pause`, `debug_stop`, `debug_state` and
  `debug_evaluate`. Whatever moves the program answers at its next stop,
  with the stack and variables, so an agent steps as you would.
  `SPC u A` clears the breakpoints agents set; yours stay.
- **tasks**: `run_task` waits for its task and answers how it went: its
  problems with their lines, its tests, the end of its output;
  `task_output` reads the rest.

Edits made through Fenix's tools go to Agent changes (`SPC a r`) like
the agent's own, kept or rejected hunk by hunk; a file with your
unsaved edits gets the change unsaved too. Long answers are cut at
`agents.mcp.max_answer_kib`, saying how to ask for the rest.

**Toolsets.** Fenix's tools come in toolsets, and an agent is shown
the ones that fit the project, so its list stays short:

| Toolset | Shown |
|---|---|
| `core`: problems, open files, the selection, definitions, references, hover, symbols | always |
| `search`, `syntax` | always |
| `lsp`, `refactor` | when a language server runs for the project |
| `git` | in a Git repository |
| `tasks` | when the project has tasks |
| `debug` | while you debug, or when the project has launches |
| `commands` | when you turn it on |
| each extension's | when the extension says so |

The agent can turn on the others itself (`fenix_toolsets`), and
`agents.mcp.toolsets` adds or takes some away for everyone, or for a
project: `["+commands", "-search"]`.

**What they may do.** Every tool has a class: it **reads**, **shows**
you something, **edits** (always through Agent changes, so you can
reject it), **runs** something (a task, a test, the debugger), or acts
outside this machine (**external**). `agents.mcp.allow` says, per class,
toolset or tool, whether it's `allow`ed, `ask`ed about or `deny`ed; the
most specific wins:

```toml
[agents.mcp.allow]
edit = "allow"            # Agent changes is review enough
"toolset:debug" = "allow"
run_command = "deny"
```

By default reading and showing are allowed, editing and running are
asked about, external is refused. A project's `.fenix/settings.toml`
can only make these stricter, never looser.

When a tool is asked about, Fenix asks you itself, as a card in the
agent's thread, whatever mode the agent is in: `y` allows it once, `Y`
for the rest of the thread, `n` refuses it, `N` for the rest of the
thread. The agent is told you refused, not that the tool failed. An
agent asking its own question about one of Fenix's tools is answered
yes for you, so you're asked once, by Fenix.

MCP clients Fenix didn't start (`fenix --mcp` run by another program)
may only read and show unless a `client:` rule says otherwise
(`"client:edit" = "ask"` needs a thread to ask on, so for them it means
no). `SPC h a` lists every call to Fenix's tools, with the rule that
applied.

Fenix 1.0's switches are still read: `agents.mcp.fenix_write = on`
means `edit` and `run` are allowed, `agents.mcp.fenix_auto_read = off`
that reading is asked about.

A note before a new session's first prompt tells the agent to prefer
Fenix's tools to searching text, and that more toolsets can be turned on.
You don't see it in the thread (`agents.mcp.fenix_brief`). The agent
still decides, and can use its own tools when it prefers.

Your own MCP servers (`agents.mcp.servers`) go to every session too: the
command line that starts one, or the address of one that's running.

```toml
[agents.mcp.servers]
local = "my-server --project ."
remote = "https://mcp.example.com/x Authorization=Bearer-abc"
older = "sse+https://mcp.example.com/sse"
```

An agent that can't reach servers over HTTP or SSE isn't given those.

An agent that runs its commands through Fenix (ACP terminals) gets them
run in the project, shown in its card as they go, the whole process tree
stopped on cancel (`agents.terminal.output_limit` keeps the end of the
output). Claude Code runs its own.

## Extensions

An extension can bring an agent, ask one (`agents.ask`: a prompt, and
the answer as text, in a thread of its own on `SPC a t`), and hear when
a turn ends. An ask runs in the agent's most restrictive mode (Claude
Code's *plan*), and a file its turn changes anyway is put back. The first
time an extension wants to send code with an ask -- a diff, a file, your
selection -- Fenix asks you, once for that extension and agent;
`agents.forget_ask_consent` makes them ask again. `agents.ask.agent` and
`agents.ask.model` pick the agent and model asks go to, and
`agents.ask.show = "hidden"` keeps their threads off `SPC a t`. See
[Writing extensions](WRITING_EXTENSIONS.md#coding-agents).

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
