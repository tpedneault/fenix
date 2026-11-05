# Plan: Fenix 1.1.0, agents and MCP

- **Status:** built, 2026-11-05 (M0–M10 merged into `release/v1.1.0`;
  see "Where it landed" at the end). The live conformance run (ADR 0003's
  table) and a pass in the GUI with real agents are still to do.
- **Version:** Fenix **1.1.0**, extension API **1.2**. It also ships every
  fix made since 1.0.1 and a set of open bugs (§10).
- **Builds on:** [`agents-acp.md`](agents-acp.md) (M0–M9, built) and
  [ADR 0002](../adr/0002-agents-acp.md)
- **Lands as:** ADR 0003 (the decisions in §1), then the milestones in
  §11, one PR each (or a few). The extension work lands in
  `fenix-extensions`.
- **Afterwards:** an audit of which extensions should get AI features
  (§12), once 1.1.0 is out

## The idea in one paragraph

In 1.0, an agent can *ask* Fenix things: diagnostics, the language
servers' answers, unsaved text, the debugger's stop. In 1.1 it can also
*work through* Fenix:

- rename a symbol with the language server, apply a code action, format;
- read Git, search the project and query the syntax tree;
- run a task and read back its problems;
- drive the debugger from launch to step.

Every change a tool makes lands in the thread's change ledger, as an
agent's own edits do, so `SPC a r` reviews it hunk by hunk. A
permission class on every tool decides what it may do, toolsets keep the
list an agent sees short, and the extension API grows what an extension
needs to offer tools and ask agents safely. On top of that come
**pipelines**: one agent plans, another implements, a third reviews,
and you review the result once. Extensions use `agents.ask` for the
everyday jobs: a commit message, a pull request's description, a
summary of a pull request you're about to review.

### In 1.1.0

- The MCP server, deeper (§5): semantic edits through the ledger, more
  LSP, Git, search, tasks with their problems, tree-sitter, the
  debugger, resources and subscriptions, prompts, elicitation, and an
  HTTP transport.
- Too many tools, handled: toolsets, and tools revealed as they're
  needed (§2).
- Permissions per tool, not one switch (§3).
- Extension API 1.2: deadlines, progress and cancellation, and
  `agents.ask` with options (§4).
- Pipelines (§6).
- Drafting with agents: commit messages in core; pull request
  descriptions and summaries in the Review extension (§7).
- Fixes from 1.0 (§10).

### Not in 1.1.0

Agents in their own Git worktrees, side by side; inline edits with the
diff in place; ACP v2; WSL agents; remote or background agents. They
stay in `agents-acp.md`'s "What comes after".

---

## 1. Decisions to make first (ADR 0003)

1. **Edits made by tools go through the ledger.** An LSP rename an agent
   asks for is recorded in the thread's baselines before it's applied
   (§5.2), exactly as `agents_write` records an edit made through
   Fenix. There is no second review path, and no preview page that
   holds the agent until you answer: the ledger *is* the review.
2. **Fenix asks, not the agent.** A tool that isn't allowed outright is
   asked about by Fenix itself, as a card in the thread (§3.3).
   - Agents' own permission systems differ, and some have a "bypass"
     mode.
   - Fenix's tools are Fenix's, so Fenix guards them, whatever mode the
     agent is in.
3. **A short list, revealed as needed.**
   - An agent is shown a core of about twelve tools, plus the toolsets
     that fit the project.
   - It's told the rest exist and can turn them on (§2).
   - A token budget for `tools/list` is a test that fails CI.
4. **The extension API stays one worker per extension** (ADR 0001).
   - It gets deadlines, progress and cancellation, and pages that jump
     ahead of tool calls in its queue.
   - It doesn't get guest threads or component-model async.
   - Long work answers later, through `service-answer`, which 1.2 makes
     safe (§4).
5. **Pipelines are data, run by the hub.**
   - A pipeline is a list of steps in settings. The steps run as linked
     threads, with one ledger between them.
   - The hub stays sans-I/O, so every pipeline flow is a unit test
     against the mock agent.
6. **Elicitation is ACP's, not Fenix's MCP server's.**
   - MCP elicitation is a request from a server to its *client*. For
     Fenix's MCP server, the client is the agent, which would draw the
     form in a UI that isn't on screen.
   - Fenix's own tools need no elicitation: Fenix can just ask (§3.3).
   - Questions from the agent, and from MCP servers the agent forwards,
     reach Fenix as ACP elicitation, drawn as a form in the thread (§5.10).
7. **Nothing commits, pushes or posts in 1.1.**
   - No tool commits, pushes, or posts to a forge.
   - Drafts (a commit message, a description) go to you, to edit and
     send.
   - The `external` permission class exists (§3.1) and starts at
     `deny`, so the decision can be revisited with the class already
     in place.

---

## 2. Too many tools

A tool's name, description and schema go into the agent's context on
every turn. Fenix's own tools grow from 15 to about 55 in this plan, and
each extension adds its own. A long list costs tokens and makes agents
worse at picking the right tool. So:

### 2.1 Toolsets

Every tool belongs to one toolset:

| Toolset | Tools (§5) | Offered by default |
|---|---|---|
| `core` | `diagnostics`, `open_files`, `read_buffer`, `selection`, `show_file`, `definition`, `references`, `hover`, `document_symbols`, `workspace_symbols`, `fenix_toolsets` | always |
| `lsp` | `implementations`, `call_hierarchy`, `type_hierarchy`, `signature_help`, `code_actions` | when a language server is running for the project |
| `refactor` | `rename`, `apply_code_action`, `format` | the same |
| `git` | `git_status`, `git_diff`, `git_log`, `git_show`, `git_blame`, `git_stage` | in a Git repository |
| `search` | `search`, `replace` | always |
| `syntax` | `syntax_node`, `syntax_query` | always |
| `tasks` | `tasks`, `run_task`, `task_output`, `run_test` | when the project has tasks |
| `debug` | `debug_state`, `debug_evaluate`, `debug_launches`, `debug_start`, `breakpoints`, `set_breakpoint`, `clear_breakpoint`, `debug_continue`, `debug_step`, `debug_run_to`, `debug_pause`, `debug_stop` | while a debug session runs, or the project has launches |
| `commands` | `run_command` | no |
| `<extension>` | an extension's tools, one toolset per extension | when the manifest says `default = true` and the extension is on for the project |

- `agents.mcp.toolsets` (project-settable) overrides the defaults: a
  list, `+name` to add and `-name` to remove (`["-search", "+commands"]`).
- A pipeline step can name its own toolsets (§6).

### 2.2 Revealed as needed

Toolsets that aren't offered can still be used. `fenix_toolsets`, in
`core`, lists every toolset with a line about each, and turns one on for
the session (`{"enable": "debug"}`).

- **Clients that support `tools.listChanged`**: Fenix sends
  `notifications/tools/list_changed`, and the toolset's tools appear as
  real tools.
- **Clients that don't**: `fenix_toolsets` answers with the toolset's
  tool descriptions, and the agent calls them through one gateway tool,
  `fenix_call {tool, arguments}`.

Which agents re-list their tools on `list_changed` is unknown. M0's
conformance run records it, per agent, in ADR 0003. The gateway
exists so that nothing depends on the answer.

The briefing (`agents.mcp.fenix_brief`) gains a line naming the toolsets
that are off, so the agent knows to look.

### 2.3 A budget, checked

- A test sums the bytes of `tools/list` for the default toolsets and
  fails over a budget: 12 KiB to start, about 3,000 tokens. A new tool
  has to fit, or go in a toolset that's off by default.
- `cargo xtask validate` (fenix-extensions) and Fenix's manifest check
  limit an extension's tool to a description of 300 characters and a
  schema of 2 KiB.
- `SPC h a` shows each session's tool list and its size, so you can see
  what an agent is being given.

---

## 3. Permissions, per tool

Today one switch, `agents.mcp.fenix_write`, gates everything that
changes something, and `agents.mcp.fenix_auto_read` lets reads through
without asking. 1.1 replaces both with classes, and a rule per class,
toolset or tool.

### 3.1 Classes

Every tool, Fenix's and extensions', has exactly one class:

| Class | What it may do | Default |
|---|---|---|
| `read` | read what Fenix knows | `allow` |
| `show` | move what's on your screen (`show_file`, `debug_start`'s pane) | `allow` |
| `edit` | change buffers and files. Every change goes through the ledger, so it can be rejected | `ask` |
| `run` | start processes: tasks, tests, Fenix commands, a debug session, a debugger step or evaluation | `ask` |
| `external` | act outside this machine: post, send, change a remote system | `deny` |

The `external` class has no tools of Fenix's in 1.1. It's there for
extensions (a Jira transition, a posted review comment), which declare
it in their manifest.

### 3.2 Rules

`agents.mcp.allow` is a map from a class, `toolset:<name>` or a tool's
name to `allow`, `ask` or `deny`. The most specific rule wins.

```toml
[agents.mcp.allow]
edit = "allow"                 # the ledger is review enough for me
"toolset:debug" = "allow"
git_stage = "ask"
jira_transition = "deny"
```

- **A project can only tighten.** `.fenix/settings.toml` can turn
  `allow` into `ask` or `deny`, never the other way. A repository you
  clone can't give its agents more than you do.
- **Per thread, "allow for this thread"** is one of a card's answers
  (`Y`). It lasts for the thread while Fenix runs.
- **External clients** (§5.11) start from stricter defaults: `edit`,
  `run` and `external` are `deny` until you allow them for that
  client.
- **Migration.**
  - `fenix_write = on` becomes `edit = "allow"` and `run = "allow"`.
  - `fenix_auto_read = off` becomes `read = "ask"`.
  - The old keys are read for one release and then dropped, with a
    note on the settings page.

### 3.3 Asking

Fenix asks about `ask` itself, whatever mode the agent is in.

- **The card.** The thread shows a card naming the tool, the arguments
  in plain words and the files they touch: "Rename `parse_line` to
  `parse_output_line` (14 places in 5 files)".
- **The answers:**
  - `y` once;
  - `Y` for this thread;
  - `n` refuse: the agent gets an error saying you refused, not that the
    tool failed;
  - `N` refuse for this thread.
- **While it waits.** The modeline says so (as it does for ACP permission
  questions), and `agents.notify` asks for your attention.
- **Read-only tools skip the agent's prompt too.** Tools whose rule is
  `allow` are still passed to `hub.set_allowed_fenix_tools`, so the
  agent doesn't ask its own question first.
- **No double asking.** A tool Fenix will ask about is left out of that
  list, and the agent's own question is answered `allow_once` by Fenix,
  which then asks you itself.

### 3.4 Knowing which thread asked

`fenix --mcp --thread <id>` is passed today, but `mcp.rs` never reads
it, so Fenix can't tell which thread a call came from. 1.1:

- **The thread goes with each call.** `mcp.rs` reads `--thread`, and its
  `fenix/hello` names the thread. Every control call then carries it.
- **Where cards and edits go.** Cards and permission answers go to that
  thread, and edits to its ledger.
- **Tool cards.** A tool call is shown as a card in its thread, as ACP
  tool calls are: the tool, what it answered (folded) and its class.
- **The log.** `SPC h a` keeps a per-thread log of every MCP call: the
  tool, the arguments, the class, the rule that applied and how long it
  took.
- **No thread.** A call with no thread (an external client) is logged
  under that client's name.

### 3.5 Extensions

- **The manifest gains a class.** `[[contributes.agent_tools]]` gets
  `class = "read" | "show" | "edit" | "run" | "external"`. `writes = true`
  without a class is read as `run` (the safe reading), and
  `writes = false` as `read`.
- **The class must fit the manifest.** `external` needs `network` or
  `sockets` in its permissions, and `edit` needs `editor = "write"` or
  `filesystem = "project-write"`.
- **One rule turns an extension off for agents.** `"toolset:jira" =
  "deny"`.
- **Installing asks first (#41).** Installing a code extension shows its
  permissions and agent tools, with their classes, and waits for you.

---

## 4. Extension API 1.2

What the survey of `fenix-wasm` and `wasm_host.rs` found, and what 1.2
does about it. 1.2 only adds (new imports, new exports called only when
present, no new `event` cases), so an extension built for 1.0 or 1.1
runs as it did.

### 4.1 What's wrong today

| Problem | Where | Effect on agents |
|---|---|---|
| One worker per extension: tool calls, pages, keys and events share one queue | `host.rs` `run()` | a slow tool freezes the extension's pages, and a page render delays a tool |
| A tool's deadline is `limits.call_ms`: 5 s by default, at most 60 s | `manifest.rs`, `wasm_host.rs` | too short for a query or a request to a slow API |
| An answer given later (`service-answer`) has no deadline on Fenix's side | `serving` cleared only in `code_answered` | an agent's tool call can hang forever |
| When an extension stops, queued `Serve` calls are never answered | `Event::Stopped` handling | the agent waits forever |
| `control_tools` lists every installed extension's tools, on or not for the project | `control_agent.rs` | agents see tools that can't work here |
| `tools/list` drops extension tools silently when Fenix can't be reached | `mcp.rs` | an agent sees fewer tools, with no reason given |
| `agents.ask` takes a prompt and an agent, nothing else; no streaming, no cancel; `thread` isn't passed back | `agents/extensions.rs`, guest `agents::ask` | not enough for drafting (§7) |
| "Don't change any files" is a note in the prompt; permission requests are refused, but an agent that writes to disk itself isn't stopped | `agents_ask` | an ask can change your files |
| Docs say "this Fenix offers 1.0", the WIT header says 1.1 but its package 1.0.0, and the WIT says `agents` events need `permissions.agents` (they don't) | `WRITING_EXTENSIONS.md`, `world.wit` | confusion |

### 4.2 Deadlines that hold

- **A timeout for each tool.** `[[contributes.agent_tools]]` gets
  `timeout_ms`: 30 s by default, up to 10 min. It covers the whole call,
  including an answer given later. Past it, the agent gets an error,
  `serving` is cleared, and the extension is told (`service-cancelled`,
  §4.4).
- **`call_ms` keeps its meaning.** It still limits how long the *guest
  code* runs before it returns. A long tool returns `None` at once and
  answers later, from a `spawn`, an HTTP reply or a timer.
  `WRITING_EXTENSIONS.md` shows this as the pattern for tools.
- **A stopped extension answers its callers.** Queued `Serve` calls and
  open `serving` entries are answered with "the extension stopped:
  <reason>", as queued commands already are.

### 4.3 Pages first

The worker's queue gets two lanes: render, page state, keys and paste,
then everything else. A tool call waiting behind a render is delayed by
at most one render (100 ticks). A render no longer waits behind a queue
of tool calls and events. The worker stays single: no guest threads.

### 4.4 Progress and cancellation

- **Progress.** A new import, `service-progress(id, message, fraction?)`,
  becomes MCP `notifications/progress`, when the call carried a
  `progressToken`, and updates the tool's card in the thread.
- **Cancellation.** A new optional export, `service-cancelled(id)`, is
  called when the agent cancels (`notifications/cancelled`), the turn
  is cancelled, or the timeout passes. An extension built before 1.2
  doesn't have it, and its late answer is dropped.

### 4.5 Listing that's true

- **Only extensions that can answer.** `fenix/tools` lists the tools of
  extensions that are installed, enabled and on for the project.
- **Stopped extensions.** A stopped extension's tools are listed, marked
  stopped, until it starts again.
- **Changes are announced.** Installing, enabling, disabling or a project
  switch sends `tools/list_changed` (§2.2).
- **No silent drops.** When Fenix can't be reached, `tools/list` still
  answers with Fenix's own tools, and the next `fenix_toolsets` says why
  the extensions' tools are missing.

### 4.6 `agents.ask`, with options

The guest gets `agents::ask_with(Ask)`; `agents::ask(prompt, agent)`
stays and means `Ask::new(prompt).agent(agent)`. The service is
`agents.ask` as now, and the new fields are optional:

```rust
let ask = Ask::new("Write this pull request's description.")
    .agent(None)                              // agents.ask.agent, else agents.default
    .attach(Attach::diff(DiffOf::Range { base: "origin/main", head: "HEAD" }))
    .attach(Attach::commits("origin/main..HEAD"))
    .attach(Attach::file(".github/pull_request_template.md"))
    .answer_as(schema)                        // JSON Schema: {title, body}
    .stream(true)                             // agent-ask-progress, as it writes
    .show(Show::Thread)                       // or Show::Hidden: no thread on SPC a t
    .timeout(Duration::from_secs(120));
let handle = agents::ask_with(ask)?;          // handle.cancel()
let answer = handle.await?;                   // answer.json, answer.text, answer.thread
```

- **Attachments are resolved by Fenix**, so an extension needs no file
  or process permission to send a diff:
  - `diff` (staged, unstaged, a range);
  - `commits` (a range's messages);
  - `file` (in the project, with unsaved edits);
  - `selection`;
  - `diagnostics`;
  - `text` (anything the extension already has).
- **Each attachment is its own block.** It goes to the agent as an ACP
  resource block, as `@` mentions do, never pasted into the prompt.
- **Structured answers.** `answer_as` asks for one JSON block matching
  the schema. Fenix finds it in the answer, validates it, and asks again
  once when it doesn't match. `answer.json` is then that value, or the
  call is an error.
- **Streaming.** A new optional export, `agent-ask-progress(id, text)`,
  is told the answer as it grows, for a draft that fills in as you
  watch.
- **Cancel.** `handle.cancel()` cancels the turn and closes its thread.
- **Read-only for real.**
  - The ask runs in the agent's most restrictive mode, if it offers one
    (Claude Code's *plan*, for example).
  - Permission requests are refused, as now.
  - Any file the turn changes anyway is put back from the ledger at the
    end, and the answer says so (`answer.reverted`).
- **Consent per extension.** The first ask that carries code (a diff, a
  file) asks you once: "The Review extension wants to send this branch's
  diff to Claude Code." The answer is kept per extension and agent, and
  managed on the extension's page.
- **Settings.**
  - `agents.ask.agent` and `agents.ask.model` pick the agent and model
    that answers asks, so drafts can go to a quick, cheap model.
  - `agents.ask.show` (`thread` or `hidden`) is the default for `Show`.

### 4.7 Resources and prompts from extensions

- `[[contributes.agent_resources]]`: a URI template
  (`fenix://ext/jira/issue/{key}`) and a service that reads it.
- `[[contributes.agent_prompts]]`: a name, a description, arguments and
  a service that writes the prompt's messages.

Both follow `agent_tools`' shape, toolset and rules (§2, §3).

### 4.8 Making them, testing them

- **A test kit.** `fenix-extension-api` gets a `testing` module that calls
  a guest's tool or service with arguments and checks its answer, under
  the same deadlines, without Fenix.
- **Validation.** `cargo xtask validate` checks tools' classes, sizes
  (§2.3) and schemas.
- **The docs match the code.** `WRITING_EXTENSIONS.md` says 1.2. The WIT
  header and package version are made to agree, and the `agents` event
  comment is corrected.

---

## 5. The MCP server, deeper

Every tool goes through one control method, `fenix/agent.call`
(ADR 0003): a tool is a row in `agent_tools.rs`'s `TOOLS`, with its
toolset and class, and its work is done in the editor
(`app/agent_calls.rs` routes it). `CONTROL.md` documents the
`fenix/agent.*` methods.

### 5.1 Plumbing first

- **The thread goes with each call** (§3.4).
- **Protocol versions.** `mcp.rs` answers with the newest MCP revision
  both sides know. Today it echoes whatever the client asks for.
- **What `initialize` declares.** It declares `tools.listChanged`,
  `resources` (`subscribe`, `listChanged`), `prompts` and `logging`.
  Today it declares only `tools`.
- **The edit window.** The bridge keeps its control connection open and
  listens for control notifications (`fenix/notify`). That's how list
  changes, resource updates and progress reach the agent.
- **Errors that say what to do next.** A tool's error says what to try:
  "no language server for .tcl files; try `search`".
- **Paths.** Paths are written as Fenix writes them, never `\\?\`
  (#38). A tool given a path it can't open says so (#39).

### 5.2 Semantic edits, through the ledger (`refactor`)

| Tool | Class | What it does |
|---|---|---|
| `rename {path, line, symbol \| column, new_name}` | edit | an LSP rename, applied, recorded in the ledger. Answers the files and places changed |
| `code_actions {path, line, endLine?}` | read | the code actions on those lines, the language servers' and extensions' fixes (`request_code_action`'s merge), each with an id for this session |
| `apply_code_action {id}` | edit | applies one. A command-only action runs as `run` |
| `format {path, startLine?, endLine?}` | edit | the project's formatter, as `SPC c f` runs it (`format_buffer`, `format_selection`, an extension's formatter) |

- **Tools work by path, not by focus.** Rename, code actions and
  formatting work only on the focused buffer and cursor today. Each gets
  a path-and-position form (`LspTarget { buffer, position }`), which the
  keys' own commands then use too.
- **Applying an edit.**
  1. `workspace_edit::prepare` checks the whole edit first, as now.
  2. For each file it touches, `baselines[thread][path].or_insert(before)`
     records the text before the edit, the same entry that
     `agents_write` makes.
  3. The edit is applied to the buffers as one undo step and saved,
     following `agents.write`.
  4. `SPC a r` then shows it with the agent's other changes.
- **One thing at a time.** A file with your unsaved edits is asked about
  first, as an agent's write is. Two edits from one tool call are one
  undo step.
- **No preview page.** The refactor preview page (`preview_lsp_edit`,
  one at a time) stays for your own renames. Tools don't use it.

### 5.3 More from the language servers (`lsp`)

`fenix_lsp::fanout::Feature` gains `Implementation`, `CallHierarchy`
(prepare, incoming, outgoing), `TypeHierarchy` (prepare, supertypes,
subtypes) and `SignatureHelp`, which so far is only a capability check.
Each is offered only when a server for the file says it can answer, and
merged across servers like the others.

| Tool | Gives |
|---|---|
| `implementations {path, line, symbol \| column}` | where a trait, interface or abstract method is implemented |
| `call_hierarchy {path, line, symbol \| column, direction: in \| out, depth?}` | who calls it, or what it calls, to a depth (at most 3) |
| `type_hierarchy {path, line, symbol \| column, direction: super \| sub}` | its supertypes or subtypes |
| `signature_help {path, line, column}` | the signature and the active parameter at a call |

These make "go to implementation" and "show call hierarchy" commands of
Fenix's own too (`SPC c i`, `SPC c h`). That's not required for the
tools, but it's cheap once the requests exist.

### 5.4 Git (`git`)

All from `fenix-git`, which runs `git`:

| Tool | Class | From |
|---|---|---|
| `git_status` | read | `status_and_files`, with the branch, upstream, ahead and behind |
| `git_diff {path?, staged?, base?}` | read | `file_diff`, `diff_refs`. Unstaged by default; `base` for a branch's whole change |
| `git_log {path?, range?, max?}` | read | `log`, `line_log` |
| `git_show {commit, path?}` | read | `commit_files`, `commit_file_diff` |
| `git_blame {path, startLine?, endLine?}` | read | `blame`, with the buffer's text so unsaved lines say so |
| `git_stage {path, hunks?}` | edit | `stage_file`, or `apply_patch(Stage)` with `hunk_patch` |

- **No commit or push** (§1, decision 7). The commit message is drafted
  instead (§7.1).
- **Answers are capped.** Diffs and logs are cut to
  `agents.mcp.max_answer_kib` (64 KiB by default), and the cut says how
  to get the rest (a path, a range).

### 5.5 Search (`search`)

| Tool | Class | From |
|---|---|---|
| `search {pattern, regex?, case?, glob?, max?}` | read | `fenix_project::search::search`, with unsaved buffers, the project's ignores, and results with their line |
| `replace {pattern, replacement, regex?, glob?, preserve_case?}` | edit | `fenix_project::search::replace` per file, through the ledger (§5.2) |

`search` sees unsaved text and the project's ignores, which an agent's
own `rg` doesn't.

### 5.6 The syntax tree (`syntax`)

`fenix-syntax` keeps its tree and queries private today. It gets a small
public API:

- `node_at(byte) -> Node`
- `ancestors(node)`
- `query(source) -> Matches`, with a budget on matches and time

| Tool | Gives |
|---|---|
| `syntax_node {path, line, column}` | the node there and its parents: kind, range and the first line of each. "What function, class, block am I in?" |
| `syntax_query {path \| glob, query, max?}` | a tree-sitter query's captures with their lines, in a file or across files of one language |

They work with the built-in grammars and extensions' (`LanguageId::Grammar`),
which is most use in languages with no good language server: Tcl,
SCOS-2000 scripts, Ada.

### 5.7 Tasks and tests (`tasks`)

| Tool | Class | What it does |
|---|---|---|
| `tasks` | read | as now |
| `run_task {name, wait?, timeout?}` | run | runs it in the task pane. With `wait` (the default), it answers when the task ends: the exit code, the problems found (`fenix_tasks::parse_line`'s quickfix entries, with path, line and message), the tests passed and failed (`test_counts`) and the last 200 lines |
| `task_output {name?, startLine?, endLine?}` | read | the output of the last run, or part of it |
| `run_test {path, line}` | run | the test under that line, as `SPC p r` will run it (#55, a new key) |

- **What it waits on.** `TasksRun` answers `{started}` today. `wait` uses
  the task session's end event and its quickfix list, which
  `run_task_as` clears at the start.
- **Better problems.** Rust panics become proper problems (#19), so a
  test that panics points at its line.

### 5.8 The debugger (`debug`)

| Tool | Class | From |
|---|---|---|
| `debug_state` | read | as now |
| `debug_launches` | read | `ProjectTools::read(root).launch`, `templates_for`, `wasm.launches`: name, type and program |
| `debug_start {launch}` | run | `debug_start_from(Template)` |
| `breakpoints` | read | `App.breakpoints`, with each one's condition, log message and whether it was verified |
| `set_breakpoint {path, line, condition?, log?}` | run | `debug_breakpoint_on_off`, `debug_set_breakpoint_part`, then `send_set_breakpoints_for` |
| `clear_breakpoint {path, line}` | run | the same |
| `debug_continue`, `debug_step {over \| into \| out}`, `debug_pause`, `debug_stop` | run | `debug_step_over`… `debug_stop`, `debug_pause` |
| `debug_run_to {path, line}` | run | `debug_run_to_cursor`'s logic, given a path and line instead of the cursor |
| `debug_evaluate {expression}` | run | as now |

- **Movement waits for the stop.** Continue, step and run-to answer when
  the program next stops, ends or a timeout passes (10 s by default),
  with the same state `debug_state` gives. An agent's loop is then one
  call per step, not a step and a poll.
- **Agents' breakpoints are marked.** A breakpoint set by an agent is
  marked in the gutter (another shade, the thread on hover). Its
  breakpoints are cleared when its thread is deleted, or with
  `SPC u A` (a new key). Yours are never touched without asking.
- **Fixes first.** The debugger bugs the agent would hit are fixed
  first: an adapter's error is lost (#32), and an ended session blocks
  another project's (#31).

### 5.9 Resources and subscriptions

| URI | What |
|---|---|
| `fenix://buffer/{path}` | an open file as it stands, with unsaved edits |
| `fenix://diagnostics` and `fenix://diagnostics/{path}` | the problems, the project's or a file's |
| `fenix://selection` | your selection |
| `fenix://debug/state` | the debugger's stop |
| `fenix://git/status` | the repository's state |
| `fenix://task/{name}/output` | a task's last output |

- **Listing.** `resources/list` lists the open buffers and the fixed ones.
  `resources/templates/list` gives the templates.
- **Subscriptions.** `resources/subscribe` registers with Fenix over the
  bridge's connection (`fenix/subscribe`). Fenix sends
  `notifications/resources/updated` when a file's diagnostics change,
  you save, the debugger stops, or a task ends. Updates are coalesced
  to one a second per resource.
- **Same rules as tools.** Resources are `read` and follow the same rules
  (§3).
- **Not every agent uses them.** M0 records which agents read resources
  and which subscribe. A tool exists for everything a resource gives,
  so nothing depends on them.

### 5.10 Prompts and elicitation

**Prompts.** MCP prompts are workflows an agent's client offers as
commands; Claude Code shows them as slash commands. Fenix's:

| Prompt | Arguments | Writes |
|---|---|---|
| `fix_diagnostic` | path, line | the problem, its code and the language server's fixes, asking for the smallest fix |
| `explain` | path, startLine, endLine | the code and its symbols' hovers, asking for an explanation |
| `review_changes` | `base?` | the thread's ledger, or a branch's diff, asking for a review with findings by file and line |
| `write_tests` | path, line | the function, its callers and the project's test layout |

Extensions add their own (§4.7). In Fenix itself, the prompt's `/`
picker lists them beside the agent's own commands. `SPC a d` becomes
`fix_diagnostic`.

**Elicitation, through ACP.** ACP's elicitation (unstable today) lets
an agent, or an MCP server it forwards for, ask the user for structured
input. Fenix draws it as a form card in the thread:

- text, numbers, booleans and choices;
- `Tab` between fields, `Enter` to send, `Esc` to decline.

It's offered in `clientCapabilities` only once the conformance run shows
an agent sending it. The mock gains it first. If ACP's shape changes
before 1.1.0, it ships off by default.

### 5.11 Over HTTP, for other clients

`fenix --mcp` on stdio already works for any client with Fenix running,
Claude Desktop or Claude Code in a terminal among them. 1.1 documents
it ("Using Fenix from other MCP clients" in `AGENTS.md`) and adds
Streamable HTTP for clients that only speak HTTP, or run in a container:

- **Starting it.** `fenix --mcp --http 127.0.0.1:0` serves the same
  bridge over Streamable HTTP. With `agents.mcp.http = on`, Fenix itself
  keeps one running on loopback.
- **Security.**
  - A bearer token is written to Fenix's data folder, readable only by
    you, and `fenix mcp token` prints it.
  - The server binds only to loopback and checks `Origin`, which the
    MCP spec asks for against DNS rebinding.
- **Each client is a principal.** It has a name (from `clientInfo`) and
  its own rules (§3.2), starting strict.
- **You see who's connected.** The modeline shows when one is, and `SPC
  h a` logs it.
- `fenix --help` and `--version` print something at last (#36), and
  list `--mcp`.

---

## 6. Pipelines

A pipeline runs a prompt through several agents in turn: one plans,
another implements, a third reviews. You see one conversation and review
one set of changes at the end.

### 6.1 As data

```toml
[agents.pipelines.plan-build-review]
title = "Plan, build, review"

[[agents.pipelines.plan-build-review.steps]]
name = "plan"
agent = "claude"
mode = "plan"
prompt = "Plan this change. Don't edit files.\n\n{input}"
toolsets = ["core", "lsp", "git", "search", "syntax"]
passes = "plan"                 # its ACP plan, else its last answer
gate = "ask"                    # you approve the plan before step 2

[[agents.pipelines.plan-build-review.steps]]
name = "build"
agent = "codex"
prompt = "Carry out this plan.\n\n{previous}"
toolsets = ["core", "lsp", "refactor", "git", "tasks"]
passes = "changes"              # the ledger's diff

[[agents.pipelines.plan-build-review.steps]]
name = "review"
agent = "claude"
mode = "plan"
prompt = "Review these changes against the plan.\n\n{plan}\n\n{previous}"
toolsets = ["core", "lsp", "git", "tasks"]
answer = "findings"             # structured, below
on_findings = "build"           # back to build, with the findings
max_rounds = 2
```

- **Where they live.** Pipelines are in settings, and a project can
  define its own. Fenix ships the one above, under the name `review`,
  with every agent left as `agents.default`.
- **What each step gets.** `{input}`, `{previous}` (what the step before
  passed) and `{<step name>}` (what that step passed), as ACP resource
  blocks, like the hand-off transcript today
  (`fenix://handoff/transcript.md`).
- **What a step passes on (`passes`):**
  - `answer`, its last answer;
  - `plan`, its ACP plan entries, else its answer;
  - `changes`, the ledger's diff for the pipeline;
  - `transcript`, its whole conversation (`agents.handoff_kib`).
- **Findings.** `answer = "findings"` uses the same structured answer as
  `agents.ask` (§4.6): a JSON list of `{path, line, severity, message}`.
  Findings are attached to the ledger's hunks and shown in Agent changes
  (`SPC a r`) beside the lines they're about.

### 6.2 In the hub

- **A pipeline is a thread group.** The hub gets a `Pipeline` above its
  threads: an id, the steps, the current one, the rounds so far, and
  what each step passed.
- **Each step is a thread of its own** (its own agent and session),
  linked to the group.
- **Sans-I/O, like the rest of the hub.** It takes inputs ("step N's turn
  ended", "you approved the gate") and returns effects ("start a
  thread with this agent and these blocks", "ask at the gate").
  `hub/tests.rs` covers every flow with the mock:
  - a gate approved, and a gate refused;
  - findings that loop back, and stop after `max_rounds`;
  - a step's agent crashing, and a step needing a login;
  - cancelling in the middle;
  - a session resumed after a restart.
- **One ledger.** Every step's changes go into the pipeline's ledger, so
  you review the outcome once, not each step's part.
- **Read-only steps are enforced.** A step with `mode = "plan"` and no
  `refactor` toolset has its writes refused (as `agents.ask`'s are), and
  anything it writes to disk anyway is put back at the end of its turn.
- **Toolsets per step** are set through `agents.mcp.toolsets` for that
  step's session (§2.1).

### 6.3 What you see

- **Starting one.** `SPC a P` picks a pipeline and opens the prompt. It
  starts when you send.
- **The dock.** The title row shows the steps, `plan ✓ → build ● →
  review`, and the thread is the current step's. In the thread, `[s`/`]s` move
  between steps' threads.
- **The gate.** A gate is a card with the step's output: `y` to go on,
  `e` to edit what's passed on first (in a buffer), `n` to stop there.
- **Stopping.** `SPC a k` cancels the running step and stops the
  pipeline. `SPC a t` lists pipelines as one entry, with their steps
  under it.
- **Extensions hear it.** `agent_turn_ended` gets the pipeline and step
  (new optional fields), so Pets can cheer the review that finds
  nothing.

---

## 7. Drafting with agents

### 7.1 Commit messages, in core

- **Draft in the composer.** The Git commit composer gets "Draft with
  agent": `SPC a c` while it's open.
  - It asks `agents.ask.agent` with the staged diff and the last 20
    commit subjects, as an example of the project's style.
  - The message streams into the composer's buffer, as one undo step.
  - You edit it and commit as always.
- **`SPC a c` elsewhere** fills the composer if it's open, and keeps
  copying to the clipboard when it isn't.
- **It honours the project's rules.** A `.gitmessage` and
  `commit.template` are passed along, and so is the project's
  `CONTRIBUTING.md` section on commits, when there is one.
- **Fixed alongside:** the composer opens in Insert mode (#48), and
  finishing a merge keeps Git's message (#45).

### 7.2 Pull requests, in the Review extension (`fenix-extensions`)

All through `agents::ask_with` (§4.6), with `api = "1.2"` and
`permissions.agents`:

- **A new pull request's description.**
  - The "Open a pull request for this branch" form gets "Draft with
    agent".
  - It sends the branch's diff against its base, its commits, the
    repository's pull request template (`.github/pull_request_template.md`
    and the other places, mirrored heading by heading), and any linked
    issue's title.
  - The title and body come back as one structured answer, streamed
    into the form's fields, for you to edit and submit.
- **A summary of a pull request you're reviewing.**
  - The review page's header gets "Summarize" (`S`).
  - It gives what the change does, what's risky, and which files to
    read first.
  - The summary is kept per head commit, so opening the request again
    doesn't ask again.
- **A merge request on GitLab** works the same way: the forge crate
  gives the extension the diff and template.
- **Asked once.** Sending the diff is asked about once (§4.6, consent).
  `review.agent` picks an agent for the extension, else
  `agents.ask.agent`.
- **Read-only.** No review comment is posted for you. A drafted reply to
  a thread is a stretch goal, and still lands in your held comments, not
  on the forge.

---

## 8. What changes, crate by crate

| Crate | Changes |
|---|---|
| `fenix-control` | new methods (§5) and `fenix/notify`, `fenix/subscribe`; the thread in `fenix/hello` |
| `fenix-gui/src/mcp.rs` | toolsets, `fenix_toolsets`, `fenix_call`, list changes, resources, prompts, progress, version negotiation, `--thread`, `--http` |
| `fenix-gui/src/app/control_*.rs` | the new methods, rules and asking (§3), tool cards |
| `fenix-gui/src/app/agents/` | pipelines' UI, the tool permission card, ask v2, consent, ledger recording for tool edits |
| `fenix-acp` (`hub`) | `Pipeline`, per-session toolsets, elicitation, `allow_once` passthrough |
| `fenix-acp` (`mock`) | an MCP client (it calls Fenix's tools from a script), `list_changed` handling on and off, elicitation, plan mode for read-only steps |
| `fenix-lsp` | `Implementation`, `CallHierarchy`, `TypeHierarchy`, `SignatureHelp` in the fan-out |
| `fenix-syntax` | `node_at`, `ancestors`, `query` |
| `fenix-tasks` | Rust panics as problems (#19); a task's end with its problems, for `wait` |
| `fenix-config` | `agents.mcp.allow`, `toolsets`, `max_answer_kib`, `http`, `agents.ask.*`, `agents.pipelines`; migration of `fenix_write` and `fenix_auto_read` |
| `fenix-extensions` (`manifest.rs`) | `class`, `timeout_ms`, `default`, `agent_resources`, `agent_prompts`; API `1.2.0` |
| `fenix-wasm` | two-lane queue, deadlines on later answers, answering on stop, `service-progress`, `service-cancelled`, `agent-ask-progress` |
| `fenix-extension-api` | `ask_with`, `Attach`, `AskHandle`, the testing kit, WIT 1.2 |
| `fenix-extensions` repo | Review's drafting (§7.2); `xtask validate` checks for tools; extensions moved to classes |

---

## 9. Testing

- **Unit tests, sans-I/O:**
  - the rules' resolution, including project tightening and migration;
  - toolset selection;
  - the pipeline hub flows (§6.2);
  - findings attached to hunks;
  - finding and validating a structured answer.
- **The control methods.** Each new control method gets a unit test
  beside the existing ones in `control_agent.rs`, with the text it
  answers: a long list cut, merged servers, a missing server's error.
- **MCP end to end.**
  - The mock agent calls Fenix's tools from a script (`--mcp-script`),
    so the whole path is tested in CI: agent → bridge → control channel
    → editor → ledger → review.
  - The scripts cover an ask that's refused, an allow for the thread, a
    rename, the gateway, and a list change.
- **The extension host.** A guest in `fenix-wasm/tests/guests` covers a
  deadline that passes, an answer after the extension stops, progress,
  cancellation, and pages answered while a tool waits.
- **Budget.** The `tools/list` size test (§2.3).
- **Conformance, live.** ADR 0002's table gets filled in, and ADR 0003
  adds columns for what this plan depends on:
  - `list_changed`, resources, subscriptions and prompts;
  - progress and elicitation;
  - plan mode honoured;
  - permission passthrough.
- **By hand, before the release.** A pass of each pipeline with Claude
  and Codex, and drafting a description on a real pull request.

---

## 10. Fixes from 1.0, in 1.1.0

### 10.1 Made since 1.0.1, not yet in the changelog

These are merged but missing from `[Unreleased]`. They go under
**Fixed** in 1.1.0's entry:

- One document per file, however its path is spelled (#56)
- A very long line no longer hangs the window (#57)
- `Ctrl-O` in Insert runs one Normal command (#58)
- Visual mode: text objects, `o`, `J`, and blocks (#59); a count applies
  to the motion after it (#68)
- Ex commands: ranges, line numbers, and an error for what doesn't exist (#60)
- Text in any encoding opens as text and saves as it came (#61)
- PDFium ships with Fenix, so the PDF reader works when installed (#62)
- One huge unsaved file no longer stops the session being saved (#69)
- `cc` and `S` keep the line's indent (#70)
- `Enter` ends the line when there's nothing to complete (#71)
- (CI only, no entry: the orphan fixture waits for its child, #67)

### 10.2 Open bugs, fixed in 1.1.0

**Must: the agent and MCP work depends on them.**

| Issue | Why it's needed |
|---|---|
| #39 `fenix/open` reports success for paths it can't open | `show_file`'s answer |
| #38 `\\?\` paths and mixed separators in the UI and control channel | every path an agent is given |
| #34 a language server that never initializes shows "starting" forever | the LSP tools would wait on it |
| #32 debugger adapter errors are lost | the debug tools' errors |
| #31 an ended debug session blocks another project's | `debug_start` |
| #19 Rust panics become broken quickfix entries | `run_task`'s problems |
| #55 run the test under the cursor | `run_test` |
| #41 installing a code extension asks for no confirmation | §3.5 |
| #40 extensions and their settings need a restart, with no message | tool lists that change (§4.5) |
| #33 6.7 s at startup with many extensions; all restart on a project switch | extension tools start on demand |
| #48 the commit composer opens in Normal mode | §7.1 |
| #45 finishing a merge commits "Merge", not Git's message | §7.1 |
| #36 `fenix --version` and `--help` print nothing | §5.11, external clients |

**Should: quality, in the same release when time allows.**

- Performance: #74 idle redraws, #65 stuck on the splash.
- Editing: #73 completion offers the word being typed; #26 the `"0` and
  `"_` registers.
- Pages: #30 a blank PICKER page; #29 Tool Status and Task Output left
  behind; #75 a PDF that can't open leaves a blank page; #72 extension
  pages can't have `Esc` or `:`; #50 the tab strip doesn't scroll to the
  active tab.
- Projects and tools: #43 and #35 Tool Status, the doctor and duplicate
  rows; #52 the doctor's "(Project)"; #53 long paths in the wizard; #51
  a first setting written above the header comment; #49 Home's resume
  hint; #18 `SPC f f` on a path that doesn't exist.
- Git: #47 Git keys do nothing outside the project; #46 the merge view's
  header row.
- Accessibility: #42 the terminal panel's focus; #37 Unicode input via
  `SendInput`; #17 right-to-left text.

**Later (1.2): Vim features.** #22, #23 and #25 are missing motions,
operators and Insert keys, not regressions.

### 10.3 The release itself

- **Versions.** The workspace and `fenix-gui` go to 1.1.0, and the
  extension API to 1.2.0.
- **The changelog.** 1.1.0's entry starts from `[Unreleased]` (the MCP
  additions already there), adds §10.1, and has its own "Extension
  API 1.2" section, as 1.0.1's had.
- **The docs.**
  - `AGENTS.md`: toolsets, rules, pipelines, drafting, other clients.
  - `CONTROL.md`: the new methods.
  - `WRITING_EXTENSIONS.md`: 1.2, classes, deadlines, `ask_with`,
    resources and prompts.
  - `CONFIGURATION.md`: the new settings.
  - `KEYBINDINGS.md`: `SPC a P`, `[s`/`]s` in a thread, `SPC p r`,
    `SPC u A`, `SPC c i`/`SPC c h`.
- **The order of releases.** Fenix 1.1.0 first. Then Review 2.x, with
  `fenix = ">=1.1"` and `api = "1.2"`. Then any extension moved to
  classes. Extensions that keep `writes` need no release.

---

## 11. Milestones

Sizes as in `agents-acp.md`: S (days), M (a week or two), L (more).
Fixes (§10) run alongside throughout, each its own small PR. The "must"
fixes are done before the milestone that needs them.

| # | Milestone | Size | Needs | Done when |
|---|---|---|---|---|
| M0 | **ADR 0003 and the live run.** Decisions §1. The conformance suite run on Claude, Codex and Devin, recording ADR 0002's table and the new columns (§9). `mcp.rs` plumbing (§5.1): `--thread`, version negotiation, `fenix/notify` | M | — | ADR accepted; the table filled; tool calls show on their thread |
| M1 | **Permissions and toolsets.** Classes, `agents.mcp.allow`, projects only tightening, migration, Fenix's own card, per-thread allow, the per-thread log (§3). Toolsets, `fenix_toolsets`, `fenix_call`, `list_changed`, the budget test (§2). Install confirmation (#41) | M | M0 | every existing tool has a class; the mock calls one through the gateway and one after a list change; the budget test is in CI |
| M2 | **Extension API 1.2.** Deadlines and answers on stop, the two-lane queue, progress and cancellation, true listing, `class`/`timeout_ms`/`default`, the testing kit, docs fixed (§4.1–4.5, 4.8) | M | M1 | the host tests in §9 pass; an extension built for 1.0 still runs |
| M3 | **Semantic edits.** Path-based rename, code actions and formatting; tool edits into the ledger (§5.2). The LSP additions and their commands (§5.3) | M–L | M1 | a rename by the mock shows in `SPC a r` and rejects hunk by hunk |
| M4 | **Git, search, syntax, tasks.** §5.4–5.7, with #19 and #55 | M | M1 | each tool's control test; `run_task` waits and answers its problems |
| M5 | **The debugger.** §5.8, with #31 and #32 | M | M1 | the mock sets a breakpoint, starts, steps and evaluates in a test adapter |
| M6 | **`agents.ask` with options.** `ask_with`, attachments, structured answers, streaming, cancel, read-only for real, consent, `agents.ask.*` (§4.6) | M | M2 | a guest test drafts JSON from a diff; an ask that writes is put back |
| M7 | **Drafting.** The commit composer's draft (§7.1), with #48 and #45; Review's description and summary (§7.2) in `fenix-extensions` | M | M6 | drafting works on a real repository and pull request with Claude and Codex |
| M8 | **Pipelines.** The hub's `Pipeline`, settings, gates, findings on hunks, the dock's steps, `SPC a P` (§6) | L | M1, M6 (structured answers) | the hub tests in §6.2; one full run with real agents |
| M9 | **Resources, prompts, elicitation.** §5.9–5.10; extensions' resources and prompts (§4.7) | M | M2 | subscriptions update the mock; prompts in the `/` picker; elicitation behind its flag if ACP's isn't settled |
| M10 | **Other clients.** `--http`, the token, Origin checks, clients as principals, the modeline, `--help` (§5.11, #36) | S–M | M1 | Claude Code in a terminal, and an HTTP-only client, use Fenix's tools under strict rules |
| M11 | **The release.** The "should" fixes that made it, docs, changelog, conformance run again, extensions released in order (§10.3) | S | all | 1.1.0 tagged; Review 2.x in the registry |

The critical path is M0 → M1 → M2 → M6 → M8. M3, M4, M5, M9 and M10 can
go in parallel once M1 is in.

---

## 12. Next: which extensions get AI

Once 1.1.0 is out, each extension in `fenix-extensions` is looked at
against the same questions, and the answer is recorded in its
`plans/` file:

1. **Does it know something an agent can't find on its own?** Live state
   (a database's schema, a container's logs, the debugger), a system
   behind a login Fenix holds (Jira, a forge), a decoder for a format
   (CCSDS packets, MIB tables, Parquet), or a reader for a binary (PDF).
   If not, it gets no tools.
2. **Would an agent call it often enough to earn its place?** Count the
   tools and their bytes against the budget (§2.3). Most should be one
   toolset, off by default, found through `fenix_toolsets`.
3. **What class is each tool?** Anything `external` needs a reason, a
   `deny` default and a clear card.
4. **Would `agents.ask` make one of its jobs shorter?** A summary, a
   draft, an explanation. These are features for you, not tools for
   the agent.
5. **What could its answers carry into the agent?** A Jira description
   or a pull request comment is text someone else wrote. Tools that
   return it say so in their description, and pipelines' review steps
   are told to treat it as data.

To look at first, by what they'd add:

- **Tools for agents:** Database (schema, read-only queries), HTTP (run
  a request, its response), Containers (state, logs), Jira (an issue,
  a search), SCOS-2000 MIB and CCSDS (look up a telecommand or
  parameter, decode a packet), the language packs (the doctor's checks,
  the project's checks and their fixes), Arduino (boards, the serial
  monitor's output), PDF (a page's text, search), CSV and Parquet (a
  table's schema and stats), Diagrams (render and report errors).
- **`agents.ask` features:** Review (done in 1.1), Jira (draft an issue
  from a selection or a failing test), Agenda (a timesheet's summary),
  Notebook (a day's journal summarized), Regex (explain a pattern,
  write one from examples), Data tools (types from a sample, done
  better).
- **Likely nothing:** themes, snippets, keymaps, Pets, Pomodoro,
  Discord, Colours, Images, Hex, VNC.

The result is a plan per extension and a release of each that
changes, against API 1.2.

---

## 13. Risks and open questions

1. **Agents may ignore `list_changed`, resources or prompts.**
   - The gateway (§2.2) and tool equivalents (§5.9) mean nothing depends
     on them.
   - M0's run says how much each is worth building on.
2. **Prompt injection through tool answers.**
   - Text from a pull request, Jira, a web response or a file can carry
     instructions.
   - Fenix can't fix that in the agent. It can make sure such tools are
     marked, `external` never defaults to `allow`, and nothing commits,
     pushes or posts (§1, decision 7).
3. **Double asking, or none.**
   - Fenix asking for its tools while the agent also asks could mean
     two questions.
   - Answering the agent's own question `allow_once` and asking ourselves
     (§3.3) depends on each agent asking through ACP, which the
     conformance run checks.
4. **ACP elicitation is unstable.** It ships behind a flag if it moves
   (§5.10).
5. **The budget may be too tight, or too loose.** 12 KiB is a first
   guess. M1 measures how Claude and Codex do with 12, 20 and 40 tools
   on the same tasks, and adjusts.
6. **One worker per extension** may still be too little for an extension
   with heavy tools and a busy page. The two lanes and later answers
   should do. If not, a second worker for tools is an ADR of its own,
   not part of 1.1.
7. **Cost.** Pipelines and drafts spend the user's tokens.
   - Every pipeline step and every ask shows its agent and model.
   - `agents.ask.model` lets drafts go to a cheap one.
   - Pipelines stop after `max_rounds`.

---

## Where it landed

Built as planned, with these differences:

- **One control method for every tool.** Tools are rows in
  `agent_tools.rs`, all called through `fenix/agent.call` (ADR 0003),
  not a control method each.
- **"For this thread" is `Y`/`N`**, the keys agents' own questions use,
  and lasts while Fenix runs, not across restarts.
- **Extension tools are listed** for every installed, enabled
  extension. Fenix has no notion of an extension being on for one
  project and not another.
- **The extension API test kit** (§4.8) is host-side tests in
  `fenix-wasm`, not a `testing` module: calling a guest natively needs
  the host imports mocked.
- **Review's drafts** are 1.1.0 of the extension, not 2.x: nothing
  breaks for those who have it.

Not built in 1.1.0, built since for 1.1.x:

- ACP elicitation (§5.10), now that ACP's shape for it is stable: forms
  as cards answered in a compose pane, and links opened in the browser.
- `SPC c i` / `SPC c h` as Fenix's own commands.
- In a pipeline: findings shown beside their hunks in Agent changes, the
  dock's step strip and `[s`/`]s`, and editing at a gate (`e`).
- The modeline naming the MCP clients connected over HTTP.
- Agents' breakpoints in a gutter shade of their own, kept across
  restarts (`SPC u A` clears them).
- `cargo xtask validate` checking agent tools' classes, permissions and
  sizes in `fenix-extensions`.

Still for later:

- "For this thread" allows lasting across restarts.
- A plan step's agent asks its own approval of its plan (Claude's
  `ExitPlanMode`) before the pipeline's gate; Fenix could answer it
  itself, once what a real agent does with each answer is checked.
