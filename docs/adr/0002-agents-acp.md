# ADR 0002: Coding agents, over the Agent Client Protocol

- **Status:** accepted 2026-10-30, for Fenix 1.0
- **Date:** 2026-10-30
- **Plan:** [`docs/plans/agents-acp.md`](../plans/agents-acp.md)
- **Spike:** `crates/fenix-acp` (`examples/chat.rs`, the `fake-agent` mock)

## Decision

Fenix is an **ACP client**, as it is an LSP and a DAP client. A coding
agent -- Claude Code's adapter (`claude-agent-acp`), Codex's
(`codex-acp`), `devin acp`, `gemini --acp`, or any other -- is a program
Fenix starts and talks JSON-RPC to over its stdio, one message per line.

- **In core.** `fenix-acp` (protocol, client, sessions, storage) is a
  host-agnostic crate like `fenix-lsp`. `fenix-gui` wires it in, in
  `app/agents/`. Extensions get hooks (`contributes.agents`, the
  `agents.ask` service), not the protocol. The extension API's limits --
  one call at a time, a second a page, no review UI over buffers -- don't
  fit a turn that runs for minutes and changes files.
- **ACP v1.** v2 is a draft. It drops the client's fs and terminal
  methods and returns from `session/prompt` before the turn ends. The
  thread reducer is built on upserts so v2 can come later.
- **Hand-written types, read leniently.** `agent-client-protocol-schema`
  was the plan, and is not used:
  - it turns on `serde_json`'s `preserve_order` for the whole workspace;
  - it brings `serde_with`, `strum`, `schemars` and `derive_more`, for
    what `protocol.rs` covers in a few hundred lines.
  - Fenix also has to read what agents send leniently, as the spec asks.
    An update kind, tool kind, content block or option it doesn't know is
    kept as `Unknown`, never a parse error.
- **Threads, not async.** The client is `LspClient`'s shape: a reader
  thread and a stderr thread per agent, events to the UI through the
  event loop proxy. The `agent-client-protocol` SDK is futures-based, and
  Fenix has no executor.
- **Sessions in one sans-I/O hub.** `fenix_acp::hub::Hub` binds Fenix's
  threads (kept on disk) to agents' sessions (which die with their
  process). It covers starting, attaching, resuming (resume, else load
  with replay, else read-only), queueing, cancelling, switching, crashes
  with backoff, idle stop and logins. It takes inputs and returns effects,
  with no processes, threads or clock of its own, so every flow is a unit
  test (`hub/tests.rs`).
- **Agent-neutral.** Nothing outside the catalog knows which agent it
  talks to:
  - behaviour follows what the agent says it can do;
  - an agent is a command line, its environment and a name (`catalog.rs`);
  - a quirk, if one is ever needed, is a named flag on the agent's entry,
    never a branch on its name.
- **Edits, two ways in, one ledger.**
  - Some agents write through Fenix (`fs/write_text_file`): into the
    buffer as one undo step, then saved.
  - Others write to disk themselves, as Claude Code's adapter does (it
    never calls `fs/*` or `terminal/*`). Fenix snapshots a file when the
    agent announces an edit to it (`tool_call` of kind `edit`), then picks
    the change up from disk.
  - Both go into the thread's change ledger, reviewable and rejectable
    hunk by hunk.
- **Keys:** `SPC a` is agents. The Agenda (an extension) moves to `SPC A`,
  as `fenix.agenda` 3.0.0, released first.
- **Saving:** an agent's edits are saved as they're made, so its own
  builds see them (`agents.write = "buffer"` to keep them unsaved).
- **The thread** is a drawn page (cards, folding, keys), and the prompt
  is a real Vim buffer.
- **Extensions hook in, the protocol stays in core.** An
  extension *brings* an agent as data -- a command line it gives with
  `agent-provide`, for an id its manifest names -- and Fenix runs it like
  any other. It *asks* one through Fenix's `agents.ask` service: a
  thread of its own, visible on `SPC a t`, every permission refused,
  since nobody is there to grant it. `agent-turn-ended` and `agents.ask`
  need `permissions.agents`: what an extension sends goes to the
  agent's provider, and what it hears is your conversation.

## The mock

All of it is built where no real agent can run. `fenix-acp`'s mock
(`src/mock/`, and the `fake-agent` binary) plays **claude-agent-acp
0.99.1** (commit `2fd46c8`). It was built from a reading of that
adapter's source, and covers:

- its capabilities, login methods, modes and config options;
- replay on load;
- its tool-call shapes and titles;
- its permission options;
- edits written straight to disk.

Flags make it crash, log out, go slow, forget sessions, write unannounced
files, and read, write and run through Fenix instead (`--fs`,
`--terminal`), so Fenix's other paths are tested too.

## Conformance

The live conformance suite (`cargo test -p fenix-acp --test conformance
-- --ignored`, with `FENIX_ACP_LIVE=claude,codex,devin`) runs the same
scenarios against each real agent. Its results go here, and any
difference from the mock becomes a fix to the mock.

| Scenario | Claude (mock) | Claude | Codex | Devin |
|---|---|---|---|---|
| Handshake, login | pass | — | — | — |
| New session, prompt, streamed answer | pass | — | — | — |
| Sees unsaved edits (fs, or save before prompt) | pass (save) | — | — | — |
| Edit a file → ledger | pass (on disk) | — | — | — |
| Create a file | pass | — | — | — |
| Run a command (with permission) | pass | — | — | — |
| Cancel mid-turn | pass | — | — | — |
| Load or resume | pass | — | — | — |
| Switch mode or model | pass | — | — | — |

— : not run yet (needs the agent installed and logged in).
