# Plan: coding agents in Fenix, over ACP

- **Status:** reviewed 2026-10-30. Keys: `SPC a` for agents, the Agenda
  to `SPC A`. Edits saved as made. The thread is a drawn page. Gemini CLI
  and the ACP registry stay in scope
- **Scope:** Claude Code, Codex and Devin CLI, supported equally; any
  other agent that speaks the Agent Client Protocol works the same way
  (Gemini CLI, OpenCode, in-house agents)
- **Lands as:** ADR 0002 once accepted, then the milestones below, one PR
  each (or a few)

## The idea in one paragraph

Fenix becomes an **ACP client**, the way it is already an LSP and a DAP
client. An agent is a program Fenix starts and talks JSON-RPC to over its
stdio: `claude-agent-acp`, `codex-acp`, `devin acp`. The agent does the
thinking and decides what tools it uses; Fenix gives it the editor's view of
the world (unsaved buffers, diagnostics, the selection), carries out the
file writes and commands it asks for, asks you before anything the agent
wants permission for, and shows the conversation, its plan, its tool calls
and every change it made in pages you drive with the keyboard. Changes
always land in Fenix's buffers first, so each is reviewable, rejectable hunk
by hunk, and undoable. Fenix in turn offers the agent an **MCP server of its
own** (`fenix --mcp`), so the agent can ask the editor what only the editor
knows: language-server answers, diagnostics, open buffers, tasks.

One integration, and every ACP agent fits it.

### Agent-neutral by design

- **One code path.** Nothing in the client, the UI or the ledger knows
  which agent it's talking to. Behaviour follows what the agent says it can
  do (`agentCapabilities`, `authMethods`, its modes and config options),
  never its name.
- **An agent is data.** A built-in profile is a command line, the
  environment it needs and how it logs in: a row in a table (§3.4), as
  `agents.servers` rows are. Claude, Codex and Devin are three rows; adding
  a fourth needs no code.
- **The same bar for each.** Every milestone is done when it works for all
  three. Each is checked against the same conformance suite (§8).
- **Quirks are contained.** If an agent needs a workaround, it is a named
  flag on its profile (e.g. "writes to disk, not through `fs`"), tested,
  and listed in the conformance table. It is never an `if agent == …`
  branch. If an agent can't meet the bar, it ships marked *partial* or is
  dropped; that is a decision for after M0, and nothing else waits on it.

---

## 1. Decisions to make first (ADR 0002)

### 1.1 In core, not an extension

| | Core (`fenix-acp` + `fenix-gui`) | WebAssembly extension |
|---|---|---|
| Edits into buffers, with undo, review and conflicts against unsaved work | Yes: it owns buffers, undo and the disk probe | Partly: `buffer::edit`, but no review UI, no gutter marks |
| Long, concurrent sessions | Threads, like LSP/DAP | One call at a time, 1 s per page, 128 MiB cap |
| Terminals for the agent, its login in a terminal | `fenix-tasks`, `fenix-terminal` | Indirect |
| An MCP server answering with LSP results | Direct | Not reachable |
| Session restore, modeline, which-key, settings page | Native | Partial |

ACP is to agents what LSP is to languages, and LSP and DAP live in core.
Extensions still get hooks (see M9): an extension can *bring* an agent, or
*use* one, without the protocol living in WebAssembly.

### 1.2 ACP v1, with the official schema types and our own threaded client

- **Protocol version 1** (stable). v2 is still a draft. It drops the
  client's fs and terminal methods and makes `session/prompt` return before
  the turn ends. The client's internal model (§3.2) is built on upserts so
  v2 can be added later without rewriting the UI.
- **Types:** `agent-client-protocol-schema` (Apache-2.0, serde only),
  pinned exactly (`=1.13.x`), as `fenix-lsp` uses `lsp-types`.
- **Not** the `agent-client-protocol` SDK crate: it's `futures`-based and
  would bring an executor into a codebase that is threads and `mpsc`
  throughout. The client is about 600 lines in the shape of `LspClient`.

### 1.3 Edits: two ways in, one ledger

Agents change files in one of two ways, and both are first-class:

- **Through Fenix** (`fs/write_text_file`, for agents that use ACP's `fs`
  capability):
  - Fenix advertises `fs.readTextFile` and `fs.writeTextFile`.
  - Reads answer with the buffer's text, unsaved edits included.
  - Writes go into the buffer as one undo step, then are saved (§1.5).
- **On disk, by the agent itself.** This is what Claude Code's adapter
  does: claude-agent-acp 0.99.1 never calls `fs/*` or `terminal/*`.
  - Its CLI reads, writes and runs Bash on its own.
  - It *announces* each edit (a `tool_call` of kind `edit`, with the path
    and a diff), and only then writes.
  - Fenix snapshots the file when the edit is announced, and picks the
    change up from disk (§5.3).

Either way, every change goes into the thread's **change ledger** against a
baseline, so it can be reviewed and rejected hunk by hunk. The UI never
asks which way a change came in.

For agents that don't read through Fenix, your **unsaved edits** are
invisible to them. `agents.save_before_prompt` (`ask`, the default, or
`always` or `never`) offers to save the project's dirty buffers before a
prompt goes out. An `@`-mentioned file is sent with its buffer text, saved
or not.

### 1.4 Keys: `SPC a` is Agents; the Agenda moves to `SPC A`

Agents are core and used all day, so they get the lowercase key. The
Agenda is an extension (`fenix.agenda`, in fenix-extensions), and moves
to `SPC A`, with the same letters under it (`SPC A a`, `SPC A b`,
`SPC A n`…).

The move takes care, because an extension's keys sit *between* Fenix's
and yours (`keymap.rs`, `an_extensions_keys_sit_between_fenixs_and_yours`).
An Agenda that still binds `SPC a` would cover the agents' keys:

1. **Agenda 3.0.0** (major: it changes muscle memory) moves its keymap to
   `SPC A`. It also updates `extension.toml`'s comments, the README and the
   CHANGELOG, with a line telling users who want the old keys how to rebind
   them in `keybindings.toml`. It ships in fenix-extensions' registry
   **before** the Fenix release that brings agents.
2. **In Fenix:**
   - `default_keymap.toml` gets the `SPC a` "agents" group.
   - `docs/KEYBINDINGS.md`'s Agenda rows move to `SPC A`.
   - `dashboard.rs`' "SPC a a" hint for the Agenda becomes "SPC A a".
   - `app.rs`'s `keybindings.toml` starter example (`SPC a k`, unbind)
     changes to a key that still exists.
3. **An older Agenda (≤ 2.x) installed:**
   - Its `SPC a` still wins, by the layering rule. Fenix reports the clash
     once, through `keymap_conflicts`, and the Extensions page offers the
     update.
   - The agents stay reachable from `SPC :`.
   - Nothing breaks, and nothing is silently taken from the user.
4. **Your own `keybindings.toml`** still wins over both, as always.

### 1.5 Agents' edits are saved as they're made

`agents.write = "save"` is the default:

- The agent builds and tests with its own shell, which reads the disk. If
  its edits sat unsaved in buffers, it would test code that isn't what it
  just wrote, and loop on that.
- The safety comes from elsewhere:
  - the change ledger (§5.3: review, reject hunk by hunk, reject all);
  - one undo step per write;
  - the guard that never overwrites your own unsaved edits without asking.
- `agents.write = "buffer"` stays available, for whoever wants nothing on
  disk until they save.

### 1.6 The thread view is a drawn page

The conversation is a `Grid` page (`page.rs`), as Fenix's Git and
Extensions pages are, not a text buffer:

- **Cards:** messages, thinking, tool calls, diffs, terminal output, the
  plan and permission prompts are cards with their own colours, status and
  icons.
- **Folding:** thinking and long outputs fold, and `Tab` opens one.
- **Moving:** `j`/`k` move between cards, `]`/`[` between your prompts and
  the agent's answers, and `gg`/`G` go to either end.
- **Search:** `/` filters, with the page's own `Filter`.
- **Copying:** `y` yanks the focused card's text (a code block, a diff, a
  command's output), and `Y` yanks the whole answer as Markdown.
- **Full Vim on a card:** `v` opens the card read-only in a buffer
  (`open_text`), for when Vim is wanted over it.
- **The prompt** stays a real Vim buffer under the page (§3.3,
  `composer.rs`).

---

## 2. What you get, as a user

- **`SPC a a`** opens the agent panel beside your code, on this project's
  last thread or a new one with the default agent (`agents.default`, per
  project).
- **Write a prompt in a real Vim buffer.**
  - `@` completes context: files, symbols, diagnostics, the selection, the
    Git diff, another thread.
  - `/` completes the agent's own commands (`available_commands_update`).
  - `Enter` in Normal mode sends the prompt.
- **Watch it work.**
  - Replies stream in as Markdown, with thinking folded away.
  - Each tool call is a card showing what it does (read, edit, run…), its
    status, the files it touched (`Enter` opens them) and a diff or the
    command's output.
  - The agent's plan shows as a checklist.
  - The modeline shows the agent, what it is doing, and how much of its
    context is used.
- **Permission prompts** appear in the thread and in the modeline (●), with
  the agent's own options: `y` allow once, `Y` always, `n` reject, `N`
  always reject. If Fenix isn't focused, the OS is asked for attention.
- **Follow mode** (`SPC a f`): the pane beside the panel jumps to each file
  and line the agent works on.
- **Review changes** (`SPC a r`):
  - Every file the thread changed, hunk by hunk, against its baseline.
  - `a` keeps a hunk and `x` rejects it (restored in the buffer and saved).
    `A` and `X` do the same for a whole file, and `agents.reject_all`
    undoes everything the thread did.
  - Agent hunks are marked in the gutter of ordinary buffers, in their own
    colour next to Git's, and `]a` and `[a` move between them.
- **From the code:**
  - `SPC a e` asks about or edits the selection.
  - `SPC a d` fixes the diagnostic at point; "Ask the agent to fix" also
    appears in `SPC c a`'s actions.
  - `SPC a c` writes the commit message from the staged diff.
- **Threads** (`SPC a t`):
  - Every thread of the project: its agent, its title (the agent names it),
    when it last ran, and its state.
  - Picking one resumes it (`session/resume` or `session/load` when the
    agent can; otherwise its transcript opens read-only).
- **Modes and models** (`SPC a m` and `SPC a M`): the agent's own modes
  (plan, ask, auto-edit…) and config options, such as the model, offered in
  a picker.
- **Cancel** with `SPC a k`, or `C-c` in the panel.
- **Agents** (`SPC a p`):
  - Pick which agent to use.
  - A page lists the agents found on `PATH` and the ones from the ACP
    registry, and installs a registry agent with one key.
- **`SPC h a`** shows the raw ACP log of each agent, for when something goes
  wrong (as `SPC h l` does for extensions).
- **Restore:** after a restart, the panel comes back with its thread. The
  agent is started again only when you prompt it.

---

## 3. Architecture

```text
                 ┌─────────────────────── fenix-gui ───────────────────────┐
 keys ─────────► │ app/agents/  session.rs  fs.rs  terminal.rs  ledger.rs  │
                 │              page.rs (thread view)  review.rs  mention.rs│
                 └──────┬───────────────────────────────▲──────────────────┘
                        │ requests            AcpEvent  │ (EventLoopProxy,
                        ▼                               │  FenixUserEvent::Acp)
                 ┌──────────────── fenix-acp ───────────┴──────────────────┐
                 │ AcpClient (spawn, ids, $/cancel_request)  Thread reducer │
                 │ ndjson transport (fenix-rpc)   catalog   registry  store │
                 └──────┬──────────────────────────────────────────────────┘
                        │ stdio, one JSON object per line
                        ▼
   claude-agent-acp │ codex-acp │ devin acp │ gemini --acp │ …
                        │
                        │ the agent starts the MCP servers it was given:
                        ▼
                 fenix --mcp ──(control channel)──► the running Fenix
```

### 3.1 `fenix-rpc`: newline-delimited framing

ACP sends one JSON object per line. Add `read_line_message_limited` and
`write_line_message` next to the `Content-Length` functions. They work the
same way (functions over `BufRead`/`Write`), are bounded (16 MiB a message,
since agents put whole files in a diff), and don't change existing callers.
MCP over stdio uses the same framing, so the `fenix --mcp` bridge uses it
too.

### 3.2 New crate `fenix-acp` (host-agnostic and unit-tested, like `fenix-lsp`)

| Module | What |
|---|---|
| `client` | `AcpClient::spawn(AgentCommand) -> (AcpClient, Receiver<AcpEvent>)`. Process-tree supervision from `fenix-tasks` (`npx` starts node, which starts the agent; closing must kill them all; job objects on Windows), `resolve_command` for `.cmd` shims, `CREATE_NO_WINDOW`. `request`, `notify`, `respond`, `cancel_request`. A stderr thread feeds the log. `AcpEvent::{Response, Notification, AgentRequest, Disconnected, Log}` |
| `handshake` | `initialize` with Fenix's capabilities and `clientInfo`, and the agent's answer kept as `AgentInfo` (capabilities, auth methods, version) |
| `hub` | **Sessions: who is connected, which thread is bound to which session, attaching, resuming, crashes, queued prompts** (§3.5). Pure: inputs in, effects out, no threads, processes or clock of its own |
| `thread` | **The pure reducer.** `Thread::apply(SessionUpdate)` merges message and thought chunks, upserts tool calls by id, replaces the plan, commands, modes and config options, keeps usage and title. `Thread::entries()` is what the page draws. No I/O, so it is tested exhaustively |
| `catalog` | Built-in agent profiles, plus `agents.servers`, plus agents found on `PATH`, giving `AgentSpec { id, name, command, args, env, secrets }` |
| `registry` | Reads `https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json` (cached, refreshed at most hourly), picks the platform's distribution (`binary` archive with SHA-256 checked, `npx`, `uvx`) and installs into `<data>/agents/<id>/<version>` |
| `store` | Each thread kept as `<state>/agents/threads/<id>.json`: agent, `sessionId`, cwd, title, the entries, the ledger's baselines. History without the agent running |
| `bin/fake-agent` | A scripted agent for tests (as `fake-language-server` is). It plays a JSON scenario: stream text, call tools, ask permission, write files through `fs/write_text_file`, run terminals, crash on cue |

### 3.3 In `fenix-gui`: `app/agents/` (kept out of `app.rs`)

| File | What |
|---|---|
| `host.rs` | `App::agents: AgentHost`: owns `fenix_acp::Hub` and **carries out its effects**: spawns `AcpClient`s, writes their messages, serves fs/terminal/permission requests, saves threads, redraws pages. `AcpReader` forwards each client's events to `FenixUserEvent::Acp`, the `LspReader` pattern, and they go back into the hub. No session logic lives here; it's all in the hub, where it's tested |
| `fs.rs` | `fs/read_text_file` and `fs/write_text_file` (§5.1, §5.2) |
| `terminal.rs` | `terminal/*` (§5.4) |
| `ledger.rs` | Baselines, agent hunks, keep and reject (§5.3) |
| `page.rs` | `PageModel::AgentThread(ThreadId)`: the thread view, drawn on `page.rs`'s `Grid`, opened with `open_page_beside` (share setting `agents.panel_width`) |
| `composer.rs` | The prompt: a `BufferKind::Prompt` buffer in a small pane under the thread. Vim editing, prompt history (`C-p`/`C-n`), `@`/`/` completion through `fenix-completion`'s popup |
| `mention.rs` | `@` sources turned into ACP content: `resource_link` for files (always allowed), embedded `resource` with the text for the selection, diagnostics and the diff when the agent has `promptCapabilities.embeddedContext`, otherwise inlined as text. Images from the clipboard when it has `image` |
| `review.rs` | The *Agent changes* page and the gutter marks |
| `permission.rs` | Pending `session/request_permission`s, the card's keys, the modeline dot, OS attention |

### 3.4 Settings (`fenix-config` schema, section "Agents")

| Key | Kind | Default | |
|---|---|---|---|
| `agents.default` | choice | first one found | project setting too |
| `agents.servers` | map name → command line | built-ins | e.g. `codex = "codex-acp"` |
| `agents.env` | map | — | extra environment for agents (`HTTPS_PROXY`…) |
| `agents.secrets` | list | — | env vars filled from the keychain (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `WINDSURF_API_KEY`), never stored in `settings.toml` when a keychain exists |
| `agents.write` | choice `save` / `buffer` | `save` | for writes through Fenix: `buffer` leaves them unsaved for you |
| `agents.save_before_prompt` | choice `ask` / `always` / `never` | `ask` | save dirty buffers before a prompt, for agents that read the disk (§1.3) |
| `agents.follow` | bool | false | |
| `agents.panel_width` | int % | 38 | |
| `agents.mcp.fenix` | bool | true | offer `fenix --mcp` to agents |
| `agents.mcp.fenix_write` | bool | false | let the agent run Fenix commands and tasks through it |
| `agents.mcp.servers` | map | — | your own MCP servers, handed to every session |
| `agents.registry` | text | the ACP registry | `""` turns it off (locked-down machines) |
| `agents.notify` | bool | true | OS attention on permission or turn end when unfocused |
| `agents.terminal.output_limit` | int KiB | 1024 | |

Built-in profiles. Each is used if its program is found, and each can be
overridden in `agents.servers`:

| id | command | auth |
|---|---|---|
| `claude` | `claude-agent-acp`, else `npx -y @agentclientprotocol/claude-agent-acp` | Claude login or `ANTHROPIC_API_KEY` |
| `codex` | `codex-acp`, else `npx -y @agentclientprotocol/codex-acp` | ChatGPT login or `OPENAI_API_KEY` / `CODEX_API_KEY` |
| `devin` | `devin acp` | `devin auth login` (terminal auth), or `WINDSURF_API_KEY` |
| `gemini` | `gemini --acp` | Google login |

---

### 3.5 Sessions: the hub

ACP has *sessions*, which belong to a running agent process and die with
it. Fenix has **threads**: the conversation you see, kept on disk, which
outlive any process. The hub binds one to the other, and is the only
place that does.

It is written **sans-I/O**, like the `Thread` reducer: `Hub::handle(Input)
-> Vec<Effect>`. It holds no threads, processes or clock of its own. The
GUI feeds it inputs (your intents, agent events, the time) and carries out
the effects it returns. Every scenario below, crashes and races included,
is a plain unit test: a list of inputs and the effects expected.

#### The model

```rust
// fenix-acp/src/hub.rs
pub struct Hub {
    connections: HashMap<ConnId, Connection>,
    threads: HashMap<ThreadId, Slot>,
    routes: HashMap<(ConnId, SessionId), ThreadId>, // where each session/update goes
    focused: Option<ThreadId>,                       // what the panel shows
}

/// One agent process. Keyed by (agent, project root): one per agent per
/// project, any number of sessions on it.
pub struct Connection {
    key: ConnKey,                      // { agent: AgentId, root: PathBuf }
    generation: u32,                   // bumped on every (re)start
    state: ConnState,
    info: Option<AgentInfo>,           // capabilities, auth methods, after initialize
    pending: HashMap<RequestId, Pending>, // what each request we sent was for
    restarts: VecDeque<Instant>,       // for the backoff
    idle_since: Option<Instant>,       // no live session: stopped after agents.idle_minutes
}

pub enum ConnState {
    Spawning,
    Initializing,
    NeedsAuth(Vec<AuthMethod>),
    Authenticating(AuthMethodId),
    Ready,
    Dead { reason: String, retry_at: Option<Instant> },
}

/// A thread, and where it stands with its agent.
pub struct Slot {
    meta: ThreadMeta, // id, agent, root, session_id: Option<SessionId>, title, created, updated
    thread: Thread,   // the reducer: entries, plan, modes, config options, usage
    link: Link,
    turn: Turn,
    queue: VecDeque<Prompt>, // prompts written while it couldn't send them yet
}

pub enum Link {
    Detached,                                     // on disk only; no process involved
    Attaching { conn: ConnId, how: Attach },       // session/new, /resume or /load in flight
    Live { conn: ConnId, session: SessionId },
    ReadOnly(Why),                                // the agent can't take it back (§ resuming)
}

pub enum Attach { New, Resume, Load }

pub enum Turn {
    Idle,
    Prompting { request: RequestId },
    Cancelling { request: RequestId },
}

pub enum Input {
    User(Intent), // NewThread, Focus, Prompt, Cancel, SetMode, SetOption, Close,
                  // Delete, Import, Authenticate, Restart, Permission answer…
    Agent { conn: ConnId, generation: u32, event: AcpEvent },
    Spawned { conn: ConnId, generation: u32, result: Result<(), String> },
    Served { conn: ConnId, request: RequestId, reply: Result<Value, RpcError> },
    Tick(Instant),
}

pub enum Effect {
    Spawn { conn: ConnId, generation: u32, spec: AgentSpec, root: PathBuf },
    Stop { conn: ConnId },                              // kill the process tree
    Send { conn: ConnId, message: Outgoing },
    Serve { conn: ConnId, request: RequestId, thread: ThreadId, what: ClientRequest },
                                                        // fs/*, terminal/*, permission: the GUI answers with Input::Served
    Withdraw { thread: ThreadId, request: RequestId },  // a card or terminal no longer wanted
    Save(ThreadId),
    Redraw(ThreadId),
    Status,                                             // modeline: who's working, who waits on you
    Attention(ThreadId),                                // OS attention when Fenix isn't focused
}
```

#### Starting a thread

1. `Intent::NewThread { agent, root }` makes a `Slot` (`Detached`, a new
   `ThreadId`) and focuses it.
2. The hub looks up the connection for `(agent, root)`:
   - **None:** `Effect::Spawn`, then `initialize` when it's up.
   - **`NeedsAuth`:** the thread shows the login card (§4, auth).
   - **`Ready`:** `session/new` straight away.
3. When `session/new` answers, the route `(conn, sessionId) → thread` is
   added and the link becomes `Live`. The reducer takes the modes and
   config options. `Effect::Save`.
4. Prompts written in the meantime wait in `queue`, shown as *sending…*,
   and go out in order once the thread is `Live` and `Idle`.

#### Prompting, and prompting again

- **`Idle` and `Live`:** `session/prompt`, then `Prompting`.
- **`Prompting`:** a new prompt is **queued, not sent**. ACP allows one turn
  at a time per session. It shows as *queued*, and `C-c` on it takes it
  back. It goes when the turn ends.
- **`Detached`:** the hub attaches first (below), and the prompt waits in
  the queue.
- **Ending a turn:**
  - The `session/prompt` response gives `stopReason`, and the turn goes back
    to `Idle`.
  - `max_tokens`, `max_turn_requests` and `refusal` leave a notice entry.
  - `cancelled` marks the turn's last entry as cancelled.
- **Late updates:** an update that arrives *after* the response but
  belongs to the same session is applied. Agents may flush late, and ACP
  allows it.

#### Cancelling

- `Intent::Cancel` sends `session/cancel` (a notification) and moves the
  turn to `Cancelling`.
- Every permission request still pending for that session is answered
  `cancelled` and its card withdrawn, as ACP requires.
- Terminals the turn started are killed.
- The turn ends when the agent answers the prompt (`cancelled`, normally).
  If it hasn't answered after 10 s (`Tick`), the hub ends the turn itself
  and leaves a notice. It doesn't kill the agent.

#### Switching

There are four kinds of switching, and each is handled differently:

| You switch | What happens to sessions |
|---|---|
| **The thread the panel shows** (`SPC a t`, `]t`/`[t` in the panel) | Nothing. `focused` changes, and that's all. Other threads keep running in the background: streaming, editing, waiting on you. The modeline shows them ("2 working · 1 waiting"), and the thread picker lists waiting threads first. A thread that asks for permission while unfocused raises `Attention` |
| **Mode or model** (`SPC a m`, `SPC a M`) | `session/set_mode` or `session/set_config_option` on the same session, allowed mid-turn. Shown at once, put back if the agent refuses. `current_mode_update` and `config_option_update` from the agent (it may switch itself, e.g. leaving plan mode) win |
| **Project** (`SPC p p`) | Nothing stops. The panel shows the new project's threads. The old project's connections stay until they've had no live session for `agents.idle_minutes` (10), then `Stop` |
| **Agent**, for a thread | Not possible in place: a session belongs to its agent. **Hand off** (`SPC a h`, M7) starts a new thread with the other agent. Its first prompt carries the old thread's transcript (an embedded resource, trimmed to `agents.handoff_kib`, 64) and a line saying where it came from. Both threads stay in the list, linked |

#### Resuming

A thread is `Detached` after a restart of Fenix, a crash of its agent, an
idle stop, or an import. It attaches again **lazily**: when you prompt it,
or when you focus it in the panel (so its modes and commands are there
when you look). It never attaches at startup.

The hub picks the best way the agent offers:

1. **`session/resume`** (`sessionCapabilities.resume`): the agent takes the
   session back without replaying it. The thread's stored entries are what
   you see. This is the fastest.
2. **`session/load`** (`loadSession`): the agent replays the whole
   conversation as `session/update`s before it answers.
   - The hub puts the thread in **replay**: the entries are rebuilt from
     the replay, into a fresh `Thread`, so nothing is duplicated.
   - When `session/load` answers, the rebuilt thread replaces the shown
     one in one step. Until then the stored one stays on screen.
   - If the replay brought nothing back (an agent that loads without
     replaying), the stored entries are kept.
   - The change ledger is Fenix's, not the agent's, and is never touched by
     a replay.
3. **Neither, or the agent no longer knows the session** (an error, or an
   empty load):
   - The link becomes `ReadOnly`. The transcript stays readable and
     reviewable.
   - The thread offers **Continue in a new session**: `session/new`, with
     the transcript handed over as in a hand-off, to the same agent.

`session/list` (`sessionCapabilities.list`) fills the Threads page's
**Import** (`i`): sessions the agent has for this project's `cwd` that Fenix
doesn't know, such as ones started in the agent's own CLI in a terminal.
Picking one makes a `Detached` thread with that `sessionId`, which
attaches as above. Its title and date come from the list.

#### Closing and deleting

- **Closing a thread** (its page closed, or archived from the Threads page)
  sends `session/close` when the agent has it. The thread becomes
  `Detached` and stays on disk. A connection with no live session left
  starts its idle clock.
- **Deleting** sends `session/delete` when offered, removes the route and
  deletes the stored file. It asks first.

#### When an agent dies

A `Disconnected` event, or one from an old generation, means the process
is gone:

1. The connection becomes `Dead`.
2. Each `Live` thread on it becomes `Detached`, keeping its `sessionId`.
3. Each turn in progress is ended with an *interrupted* entry.
4. Pending permission cards and served fs or terminal requests are
   withdrawn. Terminals are killed.
5. **Restart only when wanted.** It restarts right away if a thread on it
   has a queued prompt or is focused. Otherwise it waits until one is.
6. **Backoff:** 1 s, 2 s, 4 s. After 3 crashes in 60 s it stays `Dead` with
   the last stderr lines, and `SPC a R` restarts it by hand.
7. Threads re-attach as in *Resuming*.

Events tagged with an old `generation` are dropped. So is any request the
agent makes for a `sessionId` the hub has no route for (answered `-32602`).

#### Where requests from the agent go

`fs/*`, `terminal/*` and `session/request_permission` all carry a
`sessionId`:

1. The route gives the thread, and the thread gives the scope: its root
   and additional directories.
2. `Effect::Serve` asks the GUI to do the work: buffers, files,
   `TaskRunner`, the permission card.
3. The GUI answers with `Input::Served`.
4. The hub turns that into the JSON-RPC response. If the turn was cancelled
   or the agent restarted meanwhile, it drops the answer.

#### What's kept on disk (`store`)

- `<state>/agents/index.json`: one line of metadata per thread (agent,
  root, title, `sessionId`, updated, state), enough for the Threads page
  and the picker without reading every thread.
- `<state>/agents/threads/<id>.json`, versioned (`"version": 1`):
  - the meta;
  - the entries as the reducer has them;
  - the ledger's baselines.
- **When it's written:** at the end of each turn, every 5 s during one,
  and on exit. Writes go through `fenix-storage`'s staged writes, so a
  crash never leaves half a file.
- **Session restore** (`session.rs`) saves the panel as a page with its
  `ThreadId`. After a restart the panel comes back on that thread,
  `Detached`, and attaches when you look at it.

## 4. The protocol, method by method

| ACP | Fenix |
|---|---|
| `initialize` | Sends `protocolVersion: 1`, `clientInfo {fenix, version}`, `clientCapabilities { fs: {readTextFile, writeTextFile}, terminal: true, auth: {terminal: true} }`. Keeps the agent's capabilities. Refuses below v1 |
| `authenticate` / auth methods | `type: "agent"`: a picker of the methods, then `authenticate`. `type: "terminal"`: runs the agent's login command in a Fenix terminal pane (`fenix-terminal` gains an `env` parameter for this) and retries the session when it exits 0. `logout` in `SPC a p` |
| `session/new` | `cwd` = project root, `mcpServers` = `fenix --mcp` + `agents.mcp.servers`, `additionalDirectories` for the project's other roots when the agent has them |
| `session/load`, `session/resume`, `session/list`, `session/close`, `session/delete` | Threads page: resume, import past sessions (`list`), delete |
| `session/prompt` | The composer's content blocks. The turn ends with `stopReason`. `max_tokens`, `max_turn_requests` and `refusal` are shown as a notice on the thread |
| `session/cancel` | `SPC a k` / `C-c`. Pending permissions answer `cancelled` |
| `session/update` | Fed to the `Thread` reducer. Each variant is drawn (§2). `usage_update` goes to the modeline and `session_info_update` gives the title |
| `session/set_mode`, `session/set_config_option` | `SPC a m` / `SPC a M` pickers |
| `session/request_permission` | Permission card (§2) |
| `fs/read_text_file` | §5.1 |
| `fs/write_text_file` | §5.2 |
| `terminal/create`, `output`, `wait_for_exit`, `kill`, `release` | §5.4 |
| `$/cancel_request` | Sent when a request isn't wanted anymore. Received ones cancel what Fenix was doing (a permission card goes away) |
| `elicitation/*` | Not advertised at first; forms are M10 |

---

## 5. The hard parts

### 5.1 Reads see what you see (agents that read through Fenix)

`fs/read_text_file {path, line?, limit?}`:

- The path must be under the session's `cwd` or its `additionalDirectories`.
  Otherwise the answer is `-32602`, with a notice in the thread.
- An open buffer answers with its text (unsaved edits included). Otherwise
  the file is read from disk, which needs no new buffer.

### 5.2 Writes through Fenix go through buffers

`fs/write_text_file {path, content}`:

1. **Scope.** The scope check from §5.1 applies.
2. **Unsaved work.** If the buffer has *your* unsaved edits, a permission
   card asks "The agent wants to overwrite `x.rs`, which you've changed:
   Allow / Reject". A reject answers the agent with an error saying so. The
   agent never silently clobbers your work.
3. **Baseline.** On the thread's first touch of the file, the ledger keeps
   the text the agent started from.
4. **Apply.** The new content is applied as **one undo step**: a minimal
   diff, so cursors, folds and marks survive (the same code path as
   `replace_buffer_from_disk`'s line keeping). The file is then saved
   through `fenix-storage` (`agents.write = "save"`). The disk probe's
   fingerprint is updated so the save isn't seen as an outside change.
5. **Language servers** get `didChange`/`didSave` as for your own edits, so
   the diagnostics the agent asks for are fresh.
6. **New files** are created, with their folders. The baseline is "didn't
   exist", so rejecting the change deletes the file (to the Recycle Bin, via
   `fenix-fs`).

### 5.3 The change ledger: review and reject, whichever way the change came

- **Through `fs/write_text_file`:** the baseline and the change are known
  exactly.
- **On disk, by the agent: snapshot on announce.**
  1. When a `tool_call` or `tool_call_update` of kind `edit` (or `delete`,
     `move`) names a file, Fenix snapshots it before the agent writes. The
     file is named by `diff.path`, `locations`, or `rawInput.file_path`
     when present.
     - The snapshot is the buffer's text if the file is open, else the disk.
     - Claude's adapter announces the edit at the start of the tool's
       input stream, before it runs, and before any permission request, so
       the snapshot comes first.
     - The snapshot is taken once per file per thread.
  2. While a turn runs, files announced or in `locations` are probed every
     250 ms (not every 2 s).
  3. A change found goes into the ledger against the snapshot.
  4. Open, clean buffers reload at once (`replace_buffer_from_disk` keeps
     the lines in view).
  5. **Your unsaved work:**
     - When an edit is announced for a file you have unsaved edits in, the
       thread shows a warning card. A permission card for it says so too.
     - The buffer becomes `externally_changed`, as today, so nothing of
       yours is lost: `:w!` keeps yours, `:e!` takes the agent's, and the
       review page shows both.
  6. **Changes nobody announced:**
     - A change to a file never announced, made during a turn and found by
       the normal 2 s probe in an open buffer, is listed against the index
       (`git show :path`) when there is one.
     - Otherwise it's listed as *changed, no baseline*: visible, but not
       rejectable hunk by hunk.
- **Tool-call diffs** (Claude sends the edit's own diff up front, and the
  file's hunks once it's done) are shown on the card. The ledger trusts
  the disk, not the diff.
- **Hunks:** `fenix-diff` aligns baseline with the current text. *Keep*
  moves the baseline forward for that hunk. *Reject* writes the baseline's
  lines back into the buffer (one undo step), then saves.
- **Lifetime:** the ledger lives with the thread (persisted in `store`), so
  you can review yesterday's work. "Accept all" clears it.

### 5.4 Terminals

`terminal/create {command, args, env, cwd, outputByteLimit}`:

- Runs on `fenix-tasks`' `TaskRunner`: pipes, the process-tree kill, and
  output kept to the limit, cut from the front at a character boundary.
- `cwd` must be in scope (§5.1).
- The card in the thread shows the live tail. `Enter` on it opens the whole
  output in the bottom terminal panel's area as a read-only buffer.
- `kill` stops the tree. `release` frees it, and the output stays on the
  card.
- **The agent is responsible for asking before it runs something.** That is
  ACP's design: the permission request comes before `terminal/create`.
  Fenix adds nothing on top except the scope check.

### 5.5 `fenix --mcp`: the editor as a tool

This is an early-exit mode in `main.rs`, like `--install-extension`. It
speaks MCP (JSON-RPC, newline-delimited, over stdio: `initialize`,
`tools/list`, `tools/call`) on one side and the control channel on the
other. Hand-written, without `rmcp`, which needs tokio.

Fenix passes it to each session as
`{name: "fenix", command: <current_exe>, args: ["--mcp", "--thread", <id>]}`.
The thread id is so tool calls can be shown on the right thread.

New control methods, each listed in `capabilities`:

| Tool (MCP) | Control method | Gives |
|---|---|---|
| `diagnostics` | `fenix/diagnostics` | errors and warnings, from every server, for a file or the project |
| `definition`, `references`, `hover`, `symbols` | `fenix/lsp.*` | the language servers' answers, merged by `fenix-lsp::fanout` |
| `open_buffers` | `fenix/buffers` | open files, dirty or not, cursor and selection |
| `selection` | `fenix/state` (extended) | what you have selected |
| `open` | `fenix/open` (exists) | shows the user a file at a line |
| `run_task` | `fenix/tasks.run` | a project task, its output and errors (only with `agents.mcp.fenix_write`) |
| `run_command` | `fenix/runCommand` (exists) | a Fenix command marked `external` (only with `agents.mcp.fenix_write`) |

The control channel is already per user and per login session, so nothing
new is exposed beyond what the user's own processes could already reach.

### 5.6 Many agents, many sessions, one UI thread

- **No blocking on the UI thread.** Every read is on an `AcpReader`
  thread, and replies are sent from the UI thread through the client's
  mutex-guarded stdin, as for LSP.
- **Lifetimes.** The hub (§3.5) owns them: generations, routes, attaching
  and resuming.
- **Paging.** Long threads are paged: the page draws only visible entries,
  and `Thread` keeps a cheap per-entry height cache.

### 5.7 Security

- The agents run as you, with your rights. Fenix does not sandbox them;
  Claude's, Codex's and Devin's own permission systems do that.
- Fenix enforces:
  - the scope of `fs/*` and `terminal/*`;
  - never overwriting your unsaved work without asking;
  - a read-only `fenix --mcp` unless you allow more;
  - the registry's SHA-256 for binary agents.
- Secrets go from the keychain into the agent's environment and nowhere
  else. The ACP log redacts `env` values and known token shapes.
- Fenix itself sends nothing anywhere. Only the agents do.

---

## 6. Milestones

The work came in milestones, each one or more PRs with CI green. It was
built in the cloud, where no real agent runs: everything is tested
against the **mock agent** (§7), which plays Claude Code's ACP adapter,
and the live conformance suite runs on a developer's machine against
the real Claude, Codex and Devin.

### M0 — ADR and the protocol spike (S)

**Goal:** the decisions written down, and proof the pieces fit.

- `docs/adr/0002-agents-acp.md`:
  - §1's decisions: core, ACP v1, the schema crate, a threaded client,
    edits through buffers, keys, saving, the drawn page;
  - the conformance table (§8), started.
- `fenix-acp` crate skeleton, `Cargo.toml` with
  `agent-client-protocol-schema = "=1.13.x"` and `serde_json`. Licences
  checked in `THIRD-PARTY.md`.
- `fenix-rpc`: `read_line_message_limited` and `write_line_message`, with
  tests:
  - a partial line;
  - CRLF;
  - a line over the limit;
  - EOF in the middle of a line;
  - non-UTF-8.
- `fenix-acp/examples/chat.rs`: start a command, `initialize`,
  `session/new`, one prompt from stdin, and print every update. Fs and
  permission requests are answered on the console.
- The mock agent, first cut (§7): `initialize`, `session/new`,
  `session/prompt` streaming text.

**Done when:**
- The ADR is merged.
- `cargo run -p fenix-acp --example chat -- <fake-agent> --profile claude`
  holds a conversation.
- `cargo test -p fenix-rpc` passes.

### M1 — `fenix-acp`: client, reducer, hub, store (M)

**Goal:** the whole protocol and session logic, with no UI.

| PR | Contents |
|---|---|
| M1.1 client | `client.rs`: `AcpClient` on `fenix-tasks`' process-tree supervision (job object or process group). Request ids, `respond`, `notify`, `$/cancel_request` both ways, stderr to `AcpEvent::Log`, a reader thread with bounded lines. Typed helpers over the schema types for every v1 method in §4. Methods starting with `_` (agents' extensions, e.g. Claude's `_auth/status_update`) are ignored as notifications and answered `-32601` as requests. Unknown fields and `_meta` are kept, not rejected |
| M1.2 reducer | `thread.rs`: every `SessionUpdate` variant; tool calls upserted by id (an update for an unknown id makes the card); chunks merged by kind and by `messageId` when the agent sends one; plan replaced; commands, modes, config options, usage and title kept; replay mode (§3.5) |
| M1.3 hub | `hub.rs`: everything in §3.5. Connections, attach (new, resume, load, read-only), queue, cancel with its timeout, crash and backoff, idle stop, `-32000` (auth required) mid-turn moving the connection to `NeedsAuth`, routes, focus, import, close, delete |
| M1.4 store and catalog | `store.rs`: the index, thread files, versioning, staged writes. `catalog.rs`: built-in profiles (claude, codex, devin, gemini), `agents.servers`, `PATH` discovery |
| M1.5 mock | The mock agent, complete for the Claude profile (§7) |

**Tests:**
- The reducer, on every update kind, out-of-order updates, replay, and
  late chunks after a turn's end.
- The hub, table-driven, `Vec<Input> → Vec<Effect>`, for every flow in
  §3.5: a prompt while attaching, a prompt while prompting, cancel while a
  permission is pending, crash mid-turn, three crashes and `Dead`, resume
  versus load versus read-only, load with and without replay, events of an
  old generation, an unknown `sessionId`, idle stop, import.
- The client against the mock, through a real process: handshake, a turn,
  cancel, a crash on cue, an oversize line.

**Done when:** `cargo test -p fenix-acp` covers every hub flow, on Windows
and Linux.

### M2 — The panel (L)

**Goal:** working with an agent from Fenix, end to end.

| PR | Contents |
|---|---|
| M2.0 Agenda | fenix-extensions: Agenda 3.0.0 on `SPC A` (§1.4). Fenix: `KEYBINDINGS.md`, `dashboard.rs`, the `keybindings.toml` example |
| M2.1 settings and host | `fenix-config`: the `agents.*` settings (§3.4), the "Agents" section, secrets through the keychain. `app/agents/host.rs`: `AgentHost` carrying out the hub's effects, `FenixUserEvent::Acp`, `AcpReader`, the generation check |
| M2.2 thread page | `app/agents/page.rs`: the drawn thread (§1.6). Cards for user, agent, thinking, tool call (kind, status, title, locations, diff, text), plan, notice. Folding, `j`/`k`, `]`/`[`, `/`, `y`/`Y`, `v`. Paging for long threads |
| M2.3 composer | `app/agents/composer.rs`: `BufferKind::Prompt` in a pane under the page. `Enter` (Normal) sends, `C-p`/`C-n` history, queued prompts shown and takeable back |
| M2.4 files | `app/agents/fs.rs`, both ways in (§1.3). **Through Fenix:** reads from buffers, writes into buffers (one undo step, line keeping), saved per `agents.write`, the disk probe told, the unsaved-work guard, new files, the scope check. **On disk:** snapshot on announce, the 250 ms probe during a turn, clean buffers reloaded at once, the warning card for files you've changed, `agents.save_before_prompt`. The snapshots are kept for M3's ledger |
| M2.5 permissions | `app/agents/permission.rs`: the card, `y`/`Y`/`n`/`N` mapped to the agent's own options by `kind`, cancelled on cancel, modeline dot, `Attention` |
| M2.6 keys, modeline and log | The `SPC a` group: `a` panel, `n` new, `t` threads, `k` cancel, `p` agents, `R` restart. The modeline item ("claude · working", "1 waiting", context %). `SPC h a`: each connection's log with redaction |

**Tests (app tests with `event_proxy = None`, driving the mock):**
- A conversation streams onto the page.
- An edit through Fenix lands in the buffer as one undo step and is
  saved. An edit on disk (the Claude profile's way) is snapshotted first,
  then reloaded into the clean buffer.
- The guard asks when the buffer is dirty.
- A write outside the project is refused.
- Permission keys answer the right `optionId`.
- Cancel withdraws the card.
- A crash mid-turn leaves an *interrupted* entry, and the next prompt
  resumes.
- Two threads stream at once while only one is shown.

**Done when:** with the mock, a whole task (read, edit, permission, done)
runs from `SPC a a`.

### M3 — Review and safety (M)

| PR | Contents |
|---|---|
| M3.1 ledger | `app/agents/ledger.rs`: the snapshots and `fs/write_text_file` baselines from M2 turned into a ledger of hunks. Unannounced changes against the index. Stored with the thread |
| M3.2 review page | `app/agents/review.rs`: *Agent changes*, per file and hunk (`fenix-diff`, `diff_view.rs`), `k`/`x`/`K`/`X`, `:agent-reject-all`, accept all |
| M3.3 gutter and follow | Agent hunks in the gutter, `]a`/`[a`. Follow mode (`SPC a f`): the pane beside follows `locations` without taking the focus |

**Tests:**
- Reject restores the baseline in the buffer and on disk.
- Keep moves the baseline forward.
- A new file rejected goes to the Recycle Bin.
- Both ways in end up in the same ledger. The Claude profile's on-disk
  edit and the `--fs` style's `fs/write_text_file` are reviewed the same
  way.
- An unannounced change is listed against the index.
- A reject while you have unsaved edits in the file asks first.

**Done when:** every change the mock makes can be rejected hunk by hunk.

### M4 — Context, commands, modes (M)

| PR | Contents |
|---|---|
| M4.1 mentions | `app/agents/mention.rs`: `@` completion (files via `fenix-picker`, symbols, diagnostics, selection, Git diff, threads). Content blocks per `promptCapabilities` (§3.3) |
| M4.2 slash commands | `/` completion from `available_commands_update`, with its `input.hint` |
| M4.3 actions from code | `SPC a e` (selection), `SPC a d` (diagnostic at point), "Ask the agent to fix" in `SPC c a`, `SPC a c` (commit message from the staged diff, into the commit buffer) |
| M4.4 modes and options | `SPC a m` and `SPC a M`: pickers over modes and config options (model, and others by category), optimistic and reverted on refusal |
| M4.5 images | Pasting an image into the composer (`pasted_image`) when the agent takes images |

**Tests:**
- Each mention becomes the right content block, with and without
  `embeddedContext`.
- Commands update on `available_commands_update`.
- A mode change is reverted on refusal.
- The agent's own mode switch is shown.

### M5 — Terminals and login (S–M)

| PR | Contents |
|---|---|
| M5.1 terminals | `app/agents/terminal.rs`: `terminal/*` on `TaskRunner`, output limit cut at a character boundary, the card's live tail, `Enter` to the whole output, kill and release, withdraw on cancel |
| M5.2 login | `authenticate` with a picker of methods. Terminal auth in a Fenix terminal pane (`fenix-terminal::spawn_program` gains `env`), retried on exit 0. `logout` from `SPC a p` |

**Tests:**
- A terminal's output, truncation, exit status, kill and release.
- A connection in `NeedsAuth` goes to `Ready` through each auth type (the
  mock requires auth on cue).

### M6 — `fenix --mcp` (M)

| PR | Contents |
|---|---|
| M6.1 control methods | `fenix-control`: `fenix/diagnostics`, `fenix/buffers`, `fenix/selection`, `fenix/tasks`, `fenix/tasks.run`, `fenix/agentCommand`. `docs/CONTROL.md`. (Language-server queries -- definition, references, hover, symbols -- need Fenix's LSP fan-out to answer for a file that isn't focused; they're after 2.2.) |
| M6.2 the bridge | `main.rs` early-exit `--mcp`: MCP over NDJSON stdio (`initialize`, `tools/list`, `tools/call`), each tool a control call. Write tools only with `agents.mcp.fenix_write` |
| M6.3 handing it over | `session/new`/`load`/`resume` get `fenix` plus `agents.mcp.servers` as `mcpServers` |

**Tests:**
- An MCP client against `fenix --mcp` against a standalone Fenix with a
  test `FENIX_HOME`.
- The mock calls `diagnostics` through it.
- Write tools are refused unless allowed.

### M7 — Threads (M)

| PR | Contents |
|---|---|
| M7.1 threads page | `SPC a t`: the project's threads, waiting first, then by date. Title, agent, state. Open, close, delete, `i` import (`session/list`) |
| M7.2 restore | `session.rs`: the panel saved with its `ThreadId`, restored `Detached`, attached when looked at |
| M7.3 hand-off | `SPC a h`: continue with another agent (§3.5 *Switching*), and *Continue in a new session* for read-only threads |
| M7.4 polish | Titles from `session_info_update`, usage in the modeline and the thread's header, `Attention` on turn end when unfocused |

**Tests:**
- A thread survives a Fenix restart and continues: resume, load with
  replay, load without replay, read-only and continue.
- Import makes a thread from a listed session.
- A hand-off carries the trimmed transcript.

### M8 — Agents page and registry (M)

| PR | Contents |
|---|---|
| M8.1 page | `SPC a p`: agents found and configured, the default per project, the state of each connection, login and logout, restart |
| M8.2 registry | `fenix-acp::registry`: fetch, cache, platform pick, SHA-256, install, update and uninstall under `<data>/agents`. `agents.registry = ""` turns it off |

**Tests:**
- Registry parsing against a saved copy.
- Platform selection.
- A bad SHA-256 is refused.
- npx and uvx distributions become the right command.

### M9 — Extensions (M) and the release

| PR | Contents |
|---|---|
| M9.1 `contributes.agents` | An extension brings an agent from its tools folder, as it brings a language server. It shows on the agents page |
| M9.2 `agents.ask` | A service, and `fenix-extension-api` helpers: a one-shot prompt on the project's default agent, the answer as text. Event `agent_turn_ended`. API 0.8.8, `WRITING_EXTENSIONS.md` |
| M9.3 docs | `docs/AGENTS.md` (the user's guide), `FEATURES.md`, `KEYBINDINGS.md`, `CONFIGURATION.md`'s generated table, `ARCHITECTURE.md` (`fenix-acp`) |

**Done when:** the conformance suite passes against the mock, and you have
run it against the real agents you want to ship.

**Status:** M0–M9 are built and pass against the mock; the live
conformance run is recorded in ADR 0002.

### What comes after

ACP v2 when it's stable; elicitation forms; inline edits with the diff in
place; agents in their own Git worktree; WSL agents on Windows.

## 7. The mock agent

All of it was built in the cloud, where no real agent can run. The mock
makes that possible. It is `fenix-acp`'s `fake-agent` binary, built with
the tests (`env!("CARGO_BIN_EXE_fake-agent")`, as `fake-language-server`
is).

### 7.1 It plays Claude Code's adapter

The default profile, `--profile claude`, copies what
**claude-agent-acp 0.99.1** (commit `2fd46c8`, 2026-10-29) does on the
wire. I read it from the source; the details below are from there. Fenix's
code never sees the profile's name. Only the tests choose it.

| | What the mock does, as claude-agent-acp does |
|---|---|
| `initialize` | `protocolVersion: 1`. `agentInfo {name: "@agentclientprotocol/claude-agent-acp", title: "Claude Agent", version: "0.99.1"}`. `loadSession: true`. `promptCapabilities {image, embeddedContext}` (no audio). `mcpCapabilities {http, sse}`. `sessionCapabilities {list, resume, close, delete, fork, additionalDirectories}`. `auth {logout}`. `_meta.claudeCode.promptQueueing` |
| Auth methods | Only when the client sets `auth.terminal`: `claude-ai-login` "Claude Subscription" and `console-login` "Anthropic Console", `type: "terminal"`, with `args` (`--cli auth login --claudeai` / `--console`). `authenticate` on them fails ("Method not implemented."). After the handshake an `_auth/status_update` ext notification arrives, which Fenix must ignore |
| Not logged in (on cue) | `session/new` succeeds. `session/prompt` fails `-32000` (auth required). The hub must go to `NeedsAuth` |
| `session/new`, `/resume`, `/fork` | `{sessionId, modes, configOptions}`. Modes: `default` "Manual", `acceptEdits` "Accept edits", `plan` "Plan", `auto` "Auto", and `bypassPermissions` on cue. Config options in order: `mode` (category `mode`), `model` (category `model`, `default` first), `effort` (category `thought_level`), `fast` (category `model_config`; boolean only if the client says it takes booleans, else on/off). Then `available_commands_update` |
| `session/load` | Replays the stored conversation first: `user_message_chunk`s, then agent and thought chunks, tool calls with their updates, and plans. All before the response, chunks carrying `messageId`. Then `available_commands_update` |
| `session/list` | Sessions for `cwd`, newest first, `{sessionId, cwd, title, updatedAt}`. Cursor `"offset:N"`, and a bad one fails `-32602` |
| `session/close`, `/delete` | `{}`, also for an unknown id |
| A turn | `agent_thought_chunk`s, then `agent_message_chunk`s (with `messageId`), tool calls, `usage_update {used, size, cost {amount, currency: "USD"}}`, `session_info_update {title}` on the first turn. The response is `{stopReason, usage}` |
| Tool calls | First `tool_call` with `status: "pending"`, a partial `rawInput`, the non-standard `name` and `_meta.claudeCode.toolName`. Then a `tool_call_update` with the full input, progress updates `in_progress`, and the end `completed`/`failed`. Titles and kinds: `Read src/x.rs (1 - 40)` read, `Edit src/x.rs` edit, `Write src/new.rs` edit, the command for Bash (execute), `grep -n "pat" src` search, ``Find `src` `*.rs` `` search, `Fetch https://…` fetch |
| Edits | **Written straight to disk by the mock itself, never through `fs/*`.** The order is: the announce (`tool_call`, kind `edit`, diff `{path, oldText: old_string, newText: new_string}`), the permission request if the mode asks for one, then the write. After it, a `tool_call_update` with a diff per hunk and `locations {path, line}` |
| Bash | Run by the mock itself, never through `terminal/*`. Its output comes back as text content in a ```` ```console ```` fence |
| TodoWrite | A `plan` update (entries `{content, status, priority: "medium"}`), not a tool call |
| Permissions | Options `allow-once` "Yes" (allow_once), `allow-with-updates` "Yes, allow all edits during this session" (allow_always), `reject` "No" (reject_once). ExitPlanMode is a `switch_mode` tool called "Approve Plan". Its options are `exit-plan-auto`, `exit-plan-accept-edits`, `exit-plan-default` and `reject` "No, keep planning". Approving one sends `current_mode_update` and `config_option_update` |
| Modes | `default` asks for edits and Bash. `acceptEdits` asks only for Bash. `auto` and `bypassPermissions` ask for nothing. `plan` refuses edits until the plan is approved |
| `session/cancel` | Pending permission requests end as a deny. The turn's response is `{stopReason: "cancelled"}`, and running tool calls end `failed` |
| Errors | An unknown session on prompt: `-32002`. A bad mode: `-32602` "Invalid Mode: x". An ended session: `-32603` |

### 7.2 Driving it

- **Scenarios** are JSON files under `crates/fenix-acp/tests/scenarios/`.
  Each prompt's text picks a script: what to think and say, which tools to
  call on which files, when to ask permission, what to put in the plan.
  This lets a test say "prompt `edit-and-test`" and get a realistic turn.
- **Cues** are flags or env vars:
  - `--crash-on <method>` and `--crash-after <n>` updates;
  - `--logged-out`;
  - `--slow <ms>` between updates;
  - `--no-replay` (load without replaying);
  - `--forget-sessions` (resume and load fail);
  - `--unannounced-write` (writes a file it never announced);
  - `--bypass`.
- **The session store** is a folder the test gives (`--store`), so
  list/resume/load work across mock restarts, as Claude's
  `~/.claude/projects` does.
- **Other styles:**
  - `--fs` (reads and writes through `fs/*`);
  - `--terminal` (Bash through `terminal/*`);
  - `--agent-auth` (an `authenticate`-style login).

  These cover the ways other agents may work, so Fenix's other code paths
  are tested even though the Claude profile never takes them. They're the
  starting point for Codex and Devin profiles, once their real behaviour
  is known.

### 7.3 What it can't tell us

The mock is as faithful as the reading of claude-agent-acp it was built
from. It can't tell us how Codex and Devin behave, or how a new release of
the adapter changes. The live conformance suite (§8) is what you run on
your machine to find out. Its results go into the ADR's table, and any
difference becomes a fix to the mock.

## 8. Testing

- **`fenix-acp`:**
  - Reducer unit tests for every `SessionUpdate`, including out-of-order
    `tool_call_update`s and chunks after cancel.
  - Client tests against `fake-agent`: requests, errors, cancellation,
    crash mid-turn, an oversize line.
- **`fenix-gui`:**
  - App tests with `event_proxy = None`, as the debugger's are, driving a
    `fake-agent` scenario end to end.
  - Writes land in buffers with one undo step; the unsaved-work guard
    asks.
  - Reject restores the baseline; the scope check refuses paths outside the
    project.
  - Permission keys answer the right option.
- **`fenix --mcp`:** an MCP client test against a Fenix running with
  `FENIX_STANDALONE` and a test `FENIX_HOME`.
- **`fake-agent` profiles:** the fake can behave like each real agent's
  observed style (writes through `fs` or to disk; terminal or agent auth;
  with or without `loadSession`). The GUI tests run once per style, so
  each agent's way of working is covered in CI without the agent itself.
- **Conformance suite (`#[ignore]`, like the GitLab tests):** one set of
  scenarios run unchanged against each real agent, selected by an env var
  saying it's set up:
  `FENIX_ACP_LIVE=claude,codex,devin cargo test -p fenix-acp --test conformance -- --ignored`.
  The scenarios:
  1. Handshake, then log in.
  2. New session, a prompt, and a streamed answer.
  3. A file you have open with unsaved edits: the agent sees them, either
     through `fs/read_text_file` or after `agents.save_before_prompt`.
  4. Edit a file; the edit must land in the ledger.
  5. Create a file.
  6. Run a command (needs a permission request).
  7. Cancel mid-turn.
  8. Load or resume the session.
  9. Switch mode or model.

  Its output is the **conformance table**, one column per agent:
  pass, fail, or not offered by the agent. It is kept in the ADR and
  updated at each milestone. Support decisions, Devin's included, are made
  from this table.

---

## 9. Risks and open questions

1. **Each agent's real ACP behaviour is partly unverified.** For all three
   it isn't known yet which write through `fs/write_text_file` and which
   write to disk, or which support `session/load`/`resume`. Devin has the
   least public documentation: its docs disagree on permission modes, and a
   reported issue says `ACP_BACKEND=windsurf` in the environment makes it
   ignore its stored login. M0 fills in the conformance table for all three
   at once; where Devin needs more work, or gets dropped, is decided from
   that table.
2. **On-disk edits are the main path, not an exception.** Claude's adapter
   uses it, and Codex's and Devin's may too. Snapshot on announce (§5.3)
   depends on agents announcing an edit before writing it. Claude's does,
   and the conformance suite checks it for each agent.
   - An agent that writes unannounced (a Bash `sed`, a code generator)
     gets index baselines only.
   - A per-turn snapshot of the project's tracked files is the fallback if
     that proves too weak.
3. **Schema churn.** The schema crate moves fast (1.13.0 is a week old).
   Pinning it exactly and keeping the reducer on our own types confines
   updates to one module.
4. **`claude-agent-acp` is listed as proprietary in the registry.** Fenix
   only starts it, never ships it.
5. **Corporate machines.** No `npx`, a proxy, no registry access: covered
   by `agents.servers`, `agents.env` and `agents.registry = ""`. An agent
   installed by IT is found on `PATH`.

