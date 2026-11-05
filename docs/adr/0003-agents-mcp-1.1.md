# ADR 0003: Fenix's tools for agents, 1.1

- **Status:** accepted 2026-11-05, for Fenix 1.1
- **Plan:** [`docs/plans/agents-mcp-1.1.md`](../plans/agents-mcp-1.1.md)
- **Builds on:** [ADR 0002](0002-agents-acp.md)

## Decisions

1. **One way in, decided in the editor.** Every tool, resource and
   prompt an MCP bridge offers is a `fenix/agent.*` control call
   (`fenix/agent.call` for tools), not a control method per tool. The
   editor looks up the tool's class and toolset, applies the caller's
   rules and asks when they say so, in one place
   (`crates/fenix-gui/src/app/agent_calls.rs`); the rules and toolsets
   are host-agnostic and unit-tested (`crates/fenix-gui/src/agent_tools.rs`).
   The bridge (`fenix --mcp`) decides nothing. The older per-tool
   methods (`fenix/diagnostics`, `fenix/lsp`, …) stay for other clients.
2. **The caller is the connection's.** `fenix/hello` names the thread a
   bridge speaks for (`client.thread`), and the server fills it into
   every `fenix/agent.*` call, so a caller can't claim another's thread
   in its params. A caller with no thread Fenix knows is a client of
   yours, with rules of its own (`client:`), refused anything but
   reading and showing by default.
3. **Classes and rules replace one switch.** `read`, `show`, `edit`,
   `run`, `external`; `allow`, `ask`, `deny` per class, `toolset:<name>`
   or tool, the most specific winning, a project's rules only tightening.
   `agents.mcp.fenix_write` and `fenix_auto_read` are read as the rules
   they meant.
4. **Fenix asks, not the agent.** A tool whose rule is `ask` puts a
   card in the caller's thread (`Hub::ask_fenix`), answered with the
   same keys as an agent's own question; the answer comes back to the
   editor (`Effect::FenixAnswered`), never to the agent. The agent's own
   question about a Fenix tool is answered yes for the user
   (`Hub::set_fenix_tools_guarded`), so there's one question, not two.
   "For this thread" answers last as long as Fenix runs.
5. **Toolsets keep the list short.** The default list for a typical
   project is checked against a 12 KiB budget in a test. Toolsets that
   are off are found through `fenix_toolsets`, and called through
   `fenix_call` by clients that don't re-read their list.
6. **News by long poll.** The control channel is request and answer. A
   bridge's second connection waits on `fenix/agent.wait`, which the
   editor answers when there's news (the tool list changed, a resource
   was updated, progress) or after 20 seconds; the bridge turns it into
   MCP notifications. Calls through `fenix/agent.*` may take up to 30
   minutes (a question to the user, a task waited for); other methods
   keep their 30 seconds.
7. **MCP revisions.** The bridge answers `initialize` with the client's
   revision when it's one of 2024-11-05, 2025-03-26, 2025-06-18 or
   2025-11-25, else the newest; `structuredContent` only from
   2025-06-18.

## What the conformance run must record

Beside ADR 0002's table, for each agent: whether it re-reads its tool
list on `notifications/tools/list_changed`, reads resources, subscribes
to them, offers prompts, shows progress, honours plan mode for a
read-only ask, and sends `session/request_permission` for MCP tools.

| Behaviour | Claude | Codex | Devin |
|---|---|---|---|
| Re-reads tools on `list_changed` | — | — | — |
| Reads resources | — | — | — |
| Subscribes to resources | — | — | — |
| Offers MCP prompts | — | — | — |
| Shows progress | — | — | — |
| Plan mode honoured | — | — | — |
| Asks permission for MCP tools | — | — | — |

— : not run yet.
