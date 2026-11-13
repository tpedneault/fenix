# ADR 0005: The test explorer -- the surface in Fenix, runners in packs

- **Status:** proposed 2026-11-13, for Fenix 1.6 and extension API 1.6;
  revised the same day after the packs' review of the plan (plan §18)
- **Date:** 2026-11-13
- **Plan:** [`docs/plans/test-explorer.md`](../plans/test-explorer.md)
- **Builds on:** fenix-extensions' `plans/test-explorer.md`, the
  extension-side design and its experiments on cargo test, nextest,
  pytest, `go test`, Vitest and Jest (*the first design*)

## Context

People want to see a project's tests, run one or a set, and read the
failures where the code is, in every language Fenix serves. Fenix has
pieces of that already, each on its own path: `SPC p r` runs the test
the cursor is in as a task, for Rust, pytest and Go only
(`crates/fenix-tasks/src/test_at.rs`); a "▶ Run test" lens does the same
(`app/code_lens.rs:218-246`); the agents' `run_test` runs it and reads
the task's output (`app/agent_toolsets.rs:170-184`); `last_task` reads
pass and fail counts from that output's text
(`fenix-tasks/src/tests_count.rs`). Nothing lists tests, keeps results,
or debugs one (`docs/plans/code-lens.md:392-398`).

The first design built the explorer inside the language packs, on API
1.5. Every serious limit it found came from being an extension:

- It can't read a background task's output, so a run had to be its own
  `spawn` (`world.wit:992`, `fenix-wasm/src/host.rs:1924-1934`), and
  `process::kill` stops only that child: cargo's test binary ran on
  after its `cargo` was killed (`host.rs:2344-2350`). A line that isn't
  UTF-8 stops `spawn`'s reader (`host.rs:1567-1568`).
- It can't draw in the gutter, or start a debug session from a launch
  it gives without going through the picker and the last-launch memory.
- Its failures share one problem list per file with the same pack's
  linter (`app/wasm_host.rs:433-437`), so it needed a ledger in lang-kit.
- It would be a second way to run a test beside `SPC p r`, the lens and
  `run_test`, with results in another place, and its own notification
  beside the task's.
- Its keys (`SPC m t`) collided with Go's `tidy` and meant a major for
  the Go pack.

Fenix already splits the debugger and language servers this way: the
surface in core, what's language-specific (an adapter, a server, launch
files) given by packs (`WRITING_EXTENSIONS.md`, "Language packs").

## Decision

1. **Fenix owns the surface.** The tests page, the model (tests, ids,
   states, runs, what's kept), the run lifecycle (queue, stop, timeout,
   staleness, the failed again, run on save), the gutter, results after
   the line, failures as problems under a publisher of its own
   (`fenix:#tests`), the debugger hand-off, the modeline, one
   notification under `notifications.tasks`, `last_task`, the agents'
   tools and the control channel. The model is a sans-I/O crate,
   `fenix-tests`; the GUI's half `app/tests_host.rs` and `tests_page.rs`.
2. **A language pack owns the runner knowledge**, as a **test
   provider**: `[[contributes.test_providers]]` in its manifest (an id,
   a name, languages, markers, files to scan), and five optional exports
   in `world extension`.
3. **Declarative, not imperative.** A provider gives commands (a plan of
   steps) and parses their output; it never spawns a run. Fenix runs
   each step through `fenix_tasks::TaskRunner`, the task pane's own
   supervisor: a job object on Windows that kills the tree and kills it
   if Fenix dies, a process group on Linux
   (`fenix-tasks/src/supervisor.rs:67-124`, `165-210`); lossy UTF-8,
   lines cut at 64 KiB, output in batches (`supervisor.rs:404-450`).
   Fenix gives the provider its output 512 lines a call. On a remote
   project the same commands go through the stand-in, with no change to
   the provider.
4. **API 1.6**, additive under the 1.x rule
   (`WRITING_EXTENSIONS.md:1134-1147`): exports `tests-scan`,
   `tests-plan`, `tests-read`, `tests-step-ended`, `tests-launch`, found
   with `get_typed_func` as 1.2's and 1.5's are (`host.rs:660-675`); one
   import, `test-provider-set`; new records and variants only for them;
   no new case of `event`. The records don't change within 1.x: a later
   need is a new export. So they carry now what the packs' review of
   five runners found missing: a node's data of the provider's own, a
   plan's word that its listing is cheap after it, and a crash's scope
   (plan §18). A pack using it says `api = "1.6"` and doesn't
   install on Fenix 1.5, whose users the registry keeps on the pack's
   release before (`fenix-extensions/src/registry.rs:158-168`).
5. **Ids are the provider's** and stable across scan, listing and run.
   Fenix matches by id alone; a case's template is the provider's to
   recognise, reported as a `found` with its parent. A node may carry
   `data` of the provider's own (what its runner calls it, when the id
   can't say), which Fenix gives back and never shows or matches on.
6. **One way per file.** Where a provider serves a file, `SPC p r`, the
   lenses (Fenix's and rust-analyzer's), the gutter and `run_test` go
   through the explorer; where none does, `SPC p r` and the lens keep
   today's task path. A test run isn't a task: it doesn't take the task
   pane or clear the quickfix list, but it sets `last_task` with exact
   counts and tells extensions as a task's end does.
7. **Keys:** `SPC T` (`Ctrl+K T` with standard keys), free in Fenix and
   in every extension; the page's letters as the Search page's. No pack
   key changes.
8. **Behind `tests.enabled`** (off) until the explorer is whole; then on.
   With it off, every door behaves as in 1.5.

## Alternatives considered

- **All in extensions, as first designed.** Set aside for the reasons in
  the context: each limit needed a Fenix change anyway (tree kill, a
  debug start, gutter marks, a background task's output), and the
  result would still have been a second, unconnected way to run tests.
  Its runner work stands: the commands, readers, reporters, launch
  arguments and fixtures move into providers unchanged in substance
  (plan §6.6 maps its `TestRunner` onto the API).
- **All in core, with built-in runners.** Fenix would grow cargo,
  nextest, pytest, Go, Vitest and Jest knowledge (and .NET, Java after),
  each with its reporters and version quirks (Vitest's reporter API,
  Jest's `onTestCaseResult`, Go's build events from 1.24), released with
  Fenix rather than with the pack that already knows the language's
  tasks, launches and adapter. `test_at.rs` stays only as the fallback
  for a file no provider serves.
- **The imperative provider** (it spawns, and reports through host
  calls). Gives a pack more freedom, but every limit above returns:
  Fenix would need new imports to kill trees, to read the output, and a
  later one for remote projects (remote plan, Phase 5); each pack would
  own its queue and timeouts. The five runners are all a program whose
  output says what happened, which the declarative shape covers.
- **Fenix parsing standard formats** (JUnit XML, TAP, `go test -json`).
  Not in 1.6: a reader is short in a pack, and every runner's quirks
  (pytest's progress dots, Go before 1.24) belong with it. It can come
  later as a new export a plan names.
- **Discovery and runs from language servers.** rust-analyzer has
  runnables (`experimental/runnables`) and an experimental test-explorer
  protocol of its own, off by default, which runs cargo with libtest's
  unstable JSON; gopls offers run lenses (`gopls.run_tests`) whose
  results come back as messages, not per-test data; pyright,
  basedpyright and typescript-language-server offer nothing for tests.
  (As those projects document them; **not verified** in this
  session.) A base that covers one language of four, behind an
  experimental flag, isn't one. rust-analyzer's lenses are kept and
  routed through the explorer where it has the test (plan §9.5).
- **Scanning by tree-sitter queries in Fenix** (a `tests.scm` a pack
  ships, as Zed's runnables are). Fast and with no wasm call, but ids
  need code: Rust's module path from the file's place, pytest's ids,
  Go's import path. The provider's `tests-scan` keeps that in the pack;
  queries can be an optimisation later if per-file calls prove slow.
- **`SPC m t` per pack** for the keys. One group in every language is
  simpler, and costs no pack a major.

## Consequences

- Fenix grows a crate (`fenix-tests`), a page, a gutter column, an API
  minor and a handful of settings (`tests.*`), commands (`tests.*`) and
  agent tools (`tests`, `test_failures`, `run_tests`, `debug_test`).
- The API's shape is fixed once a Fenix release carries it. Until 1.6.0
  ships it's in the nightly only, and no pack using it is published
  before then.
- lang-kit's part shrinks to an adapter from its `TestRunner` to the five
  exports; the first design's problems ledger, page, model, notification
  and agent services aren't built. Go keeps `SPC m t` for `tidy`.
- Test runs share the task pane's process supervision, so a stop is the
  same kill on Windows and Linux as a task's, and a test's leftover
  children die with its step. They share Windows' limits too: a step's
  command line is held to `CreateProcessW`'s 32,766 characters on every
  platform, so a provider splits a long selection itself.
- Every extension rebuilt on the 1.6 API crate exports the five
  functions. One that keeps `api = "1.5"` starts on Fenix 1.5 only when
  no code reachable from an export calls `tests::provider`, even behind
  a condition that never holds: a component imports every function its
  code can reach, and the 1.5 host has no `test-provider-set` to give.
  Step 2 tests both: a 1.6-built guest that calls nothing new starts on
  1.5's world, and one that calls it is refused (plan §6.2, §19). An
  extension whose code can reach the call needs `api = "1.6"`.
- Every new door (`SPC p r`, the lens, `run_test`) has two paths, the
  explorer and the fallback, and both are tested.
- The remote-projects plan's "API 1.6" (`docs/plans/remote-ssh.md:524`)
  becomes 1.7 if this lands first.
