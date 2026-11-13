# Plan: the test explorer

- **Status:** planned, 13 November 2026, for the maintainer to review;
  revised the same day after the packs' review of it (§18). Recorded in
  [ADR 0005](../adr/0005-test-providers.md). Steps 1 to 3 are built (§20).
- **Scope:** a page that lists a project's tests before anything has
  run, runs one, a file, a group, the failed or all of them, and shows
  each result as it ends: on the page, in the gutter, after the test's
  line, and as a problem at the assertion. One key debugs the test the
  cursor is in. Fenix owns all of that; a language pack says how its
  runner finds, runs and reports tests, as a **test provider** (extension
  API 1.6).
- **Lands as:** the steps in §15, each one landable and tested. Until
  the last, the explorer is behind `tests.enabled` (off), and with it
  off every key, lens, task and agent tool behaves exactly as in 1.5.
- **Builds on:** the packs' side in fenix-extensions,
  [`plans/test-explorer.md`](https://github.com/tpedneault/fenix-extensions/blob/main/plans/test-explorer.md)
  (cited below as *the packs' plan*). It began as an explorer built
  entirely in extensions (*the first design*) and was rewritten for this
  split; it keeps the first design's runner experiments (its §3 and
  appendix, the recorded outputs in fenix-extensions
  `plans/test-explorer-fixtures/`), and its §11 is the review of this
  plan that §18 answers. Steps 1 to 3 are built (§20); the review of
  them is §19.

Paths are in this repository unless they say `fenix-extensions`. Every
claim about what Fenix does today cites the tree at `b5c3765`.

## 1. What it is, for whom, and what it isn't

You open a project and `SPC T T` shows its tests, by provider, target
or package, file and suite, before anything has run. `r` on a test, a
file or a group runs it; results come in as each test ends. A failure
is a problem at its assertion's line, a ✗ in the gutter beside the
test, and its message and traceback beside the tree. `SPC T t` runs the
test the cursor is in from any file, `SPC T F` the failed again, `SPC T
d` debugs the test the cursor is in. It works the same for every
language a pack serves, because the page, the model, the keys, the
gutter and the lifecycle are Fenix's, and a pack only says how its
runner lists, runs and reports.

It's for anyone who runs tests while editing: today they run a task
(`SPC p t`), read its output, and walk the quickfix list. It's also for
the coding agents, which get the list, the failures and a way to run a
set, with results they can read as data (§10).

| Not part of it | Why not |
|---|---|
| Coverage | Another tool with other output (lcov, `cargo llvm-cov`) and its own way of showing it. A plan of its own if wanted. |
| Watch mode (`vitest`, `jest --watch`, `cargo watch`) | A process that never ends doesn't fit a run that ends. Run on save (§8) is the cheap part of it. |
| Writing tests | The agents' `write_tests` prompt ([AGENTS.md](../AGENTS.md#fenix-as-a-tool)) and pipelines' *Tests first* do that. |
| Benchmarks (`cargo bench`, `go test -bench`) | No pass or fail. They stay tasks. |
| A runner of Fenix's own | The project's runner runs; the explorer reads what it says. |
| Snapshot updates (`-u`), retries, sharding, `xdist` workers | Flags a provider can add to its plans later; nothing in the API stops it. |
| Test runners Fenix knows itself | Runner knowledge is the packs' (ADR 0005). Fenix keeps today's `SPC p r` finder only as the fallback where no provider serves a file (§2). |

## 2. What exists today, and what each becomes

| Today | Where | After this lands |
|---|---|---|
| `SPC p r` runs the test the cursor is in, alone, in Task Output: Rust `#[test]` as `cargo test --message-format=json -p <pkg> [--lib\|--test x] -- <path> --exact`, pytest as `pytest <file>::<names>`, Go as `go test -run ^Name$ ./dir` | `default_keymap.toml:453-455`; `app.rs:10751-10794`; `fenix-tasks/src/test_at.rs:236-254`, `302-335`, `339-350` | **Through the explorer** when a test provider serves the file: the same as `SPC T t`. Where none does, as today (§16, Q7). The key stays. |
| `SPC p T` reruns the last task, including `SPC p r`'s test | `default_keymap.toml:443-445`; `app.rs:10815-10822`, `10872`; `FEATURES.md:1583` | Reruns what `SPC p t` or `SPC p r` last ran, so `SPC p r` then `SPC p T` still does what it did: a test run when `SPC p r` went to the explorer. |
| Fenix's own "▶ Run test" lens, over the lines `fenix_tasks::test_lines` finds (text, not tree-sitter, not a server); runs as `SPC p r` | `app/code_lens.rs:218-246`, `678-687`; `docs/plans/code-lens.md:181-186`; `lsp.code_lens_tests`, `schema.rs:451` | Over the tests the file's provider found, "▶ Run test \| Debug", running `tests.run_here` / `tests.debug_here` at that line. Without a provider, as today. |
| rust-analyzer's `runSingle` lens runs cargo in Task Output; its `debugSingle` says Fenix can't debug from a lens | `app/code_lens.rs:710-744`; `fenix-lsp/src/code_lens.rs:266-267` | On a line where the explorer has a test, the explorer runs or debugs it. Elsewhere (a binary's `▶ Run`, a doc test the provider didn't list) as today. |
| Agents' `run_test {path, line}`: `SPC p r`'s test, in the task pane, answered with the task's report | `agent_tools.rs:196`; `app/agent_toolsets.rs:170-184`, `328-338` | Through the explorer when a provider serves the file, answered with the test's result as data (§10.3). Else as today. |
| `last_task()` and its test counts, read from the task's output text: cargo's `test result:`, a summary line that **starts with `=`** (pytest), Jest's and Vitest's `Tests`, Go's `--- PASS` | `app.rs:11116-11120`; `fenix-tasks/src/tests_count.rs:8-39` | Unchanged for tasks. A test run also sets it, with exact counts from the model (§10.2). |
| Fenix's notification when a task ends (`notifications.tasks`: all, long, failed, never) | `app/notifications.rs:366-398`; `schema.rs:525` | The same setting governs test runs: one notification per run, never a task's and a run's for the same thing (§10.1). |
| Task Output: one task at a time, in a workspace of its own, its errors in the quickfix list | `app.rs:10860-10910`, `10886-10888`, `10873` | Unchanged. A test run is not a task: it doesn't take the task pane, doesn't clear the quickfix list, and runs beside a build. Its output has its own buffer (§7). |
| Debugging one test: "follow-up work" | `docs/plans/code-lens.md:392-398` | The explorer's hand-off (§9). |
| The debugger's `preLaunchTask` waits on the task pane | `app.rs:11302-11331`, `11126` | Unchanged: a test run never starts a waiting debug session. |

The rule that comes out of it: **for a given file, there is one way to
run its tests, and its results land in one place.** A file a provider
serves goes through the explorer from every door (`SPC T t`, `SPC p r`,
the lens, rust-analyzer's lens, the gutter, the agents). A file no
provider serves keeps today's task path, and the explorer doesn't show
it. The two never apply to the same file.

Two facts found while reading, which the first design took on trust:

- **`pytest -q` isn't counted.** Its last line is `4 failed, 7 passed,
  1 skipped, 1 xfailed, 1 error in 0.03s`, with no `=` around it (the
  first design's recorded `outputs/pytest/run-q.txt`, fenix-extensions
  `plans/test-explorer-fixtures/`), and
  `tests_count.rs:36` wants the line to start with `=`. `SPC p r` and
  the default `pytest` task don't pass `-q` (`test_at.rs:334`,
  `defs.rs:41-44`), so only a project's own `-q` task is affected.
- **`SPC p r`'s Go test isn't counted when it passes.** It runs `go
  test -run ^Name$` without `-v` (`test_at.rs:349`), which prints no
  `--- PASS` line, so `test_counts` gives none on a pass and `(0, 1)` on
  a failure.

Neither is fixed here: a test run's counts come from the model (§10.2).
Both are small fixes to `tests_count.rs` on their own.

## 3. Shape of the work

```text
                 Fenix                                       a language pack
┌───────────────────────────────────────────────┐    ┌────────────────────────────┐
│ the model (fenix-tests): tests, ids, states,  │    │ scan a file's text         │
│ runs, queue, matching, staleness, results kept│◄───┤   tests-scan               │
│                                               │    │ say what to run            │
│ the run lifecycle: TaskRunner (process tree,  │◄───┤   tests-plan → commands    │
│ cancel, timeout), output buffer, batching     │    │ read its runner's lines    │
│                                               ├───►│   tests-read, -step-ended  │
│ the page, the gutter, inline results,         │    │ say how to debug one       │
│ problems, the debugger hand-off, modeline,    │◄───┤   tests-launch             │
│ notification, last_task, agents, control      │    │ (reporters under /data)    │
└───────────────────────────────────────────────┘    └────────────────────────────┘
```

Fenix spawns every runner. The provider never sees a process: it gets a
file's text and gives tests, gets an ask and gives commands, gets its
commands' output in batches of lines and gives reports. That's shape
(A), declarative, chosen over (B), a provider that spawns and reports
through host calls. §6.1 says why.

## 4. The model

The model is Fenix's, native, in a new host-agnostic crate,
`fenix-tests` (sans-I/O, as `fenix-acp`'s `Hub` is, `ARCHITECTURE.md:63`):
it takes what providers and runs say and gives back what changed. The
GUI side is `app/tests_host.rs`.

### Tests and groups

Every node is a `found-test` (§6.3) a provider gave: an `id`, its
`parent` by id, a `label`, a `kind` and, when known, a file and lines.

| Kind | Examples | Runs |
|---|---|---|
| `group` | a Cargo target `orbit (lib)`, a Go package `example.com/orbit`, a Jest project | everything under it |
| `file` | `src/lib.rs`, `tests/test_basic.py`, `src/orbit.test.js` | everything in it |
| `suite` | a Rust `mod tests`, a pytest class, a `describe` | everything in it |
| `test` | `tests::adds`, `test_adds`, `TestAdds`, `it("adds")` | itself |
| `case` | `test_cases[negative]`, `TestTable/negative_one`, `add(-1, -1) is -3` | itself, when the provider can name it alone; else its template |

Providers are the roots: one top row per provider that serves the
project, shown only when there are two or more (a Rust workspace with a
Python tool beside it). A provider that serves several roots of one
project (a Cargo workspace's members, a `go.work`'s modules, a
monorepo's packages) shows them as its own `group`s, each step with
its own `cwd`; Fenix needs nothing more for it. A provider that said
it has nothing to run in the project (`test-provider-set`, §6.3) has
no row there.

**Two providers in one file.** Each scans the files of its own
`languages` and `scan`, so two can find tests in one file (a pack's
pytest and another extension's, `rust` and `rust.nextest`). Both are
kept, each under its root. In the file (the gutter, the lens, `SPC T
t`, `SPC T d`), a line's mark is the worst state of the tests that start
there, and the test the cursor is in (§9.4) is the innermost of all of
theirs; on a tie, the provider whose id sorts first. The two never
share a node: ids are per provider.

### Ids, and matching results to what was found

An id is the provider's own, at most 1 KiB, unique within the provider
and stable across its scans, listings and runs. Fenix keys a node by
`(provider, id)`. The packs' keys are already these: Rust
`<package>/<target> <path>` (`orbit/lib geo::tests::unwraps_none`),
pytest's node id, Go `<import path> <name>`, Vitest and Jest `<file> >
<names>` (the packs' plan, §4.4).

A provider is keyed by its extension's id and its own (`fenix.rust`'s
`rust`): two extensions that both declare `rust` are two providers,
whose trees and results never meet.

A report names a test by id. Fenix:

1. **Knows the id**: the result is that node's.
2. **Doesn't know it, and the report came after a `found` for it in
   the same run**: the `found` added it (a Go subtest, a `test.each`
   case, a case of a template the provider recognised itself, §6.6).
3. **Doesn't know it at all**: an `ended` adds it as a `test` labelled
   by its id, under the deepest node of the file its `at` names, else
   under the provider's root, marked *found in a run*, with no line (a
   macro-made Rust test that the scan can't see). A `started` or an
   `output` for an id Fenix doesn't know adds nothing: the output stays
   in the run's buffer. A `broken` or `crashed` naming an unknown id is
   taken as naming none (all the step covers), and said on the
   provider's output page.

An id over 1 KiB is cut the same way wherever it comes (a scan, a
listing, a report, a `parent`, a step's `covers`), so a provider that
sends the same long id everywhere still matches, and two that share
their first KiB stay two; the cut is said on its output page once. The
cut (§19, finding 9): an id of at most 1,024 bytes is as it is; a
longer one is its first 1,007 bytes, moved back to a character's start
when that falls inside one, then `#`, then the FNV-1a 64-bit hash
(offset basis `0xcbf29ce484222325`, prime `0x100000001b3`) of the whole
id's UTF-8 bytes as 16 lowercase hex digits: 1,021 to 1,024 bytes, and
cutting it again changes nothing. The host and the model share one
implementation (`fenix_tests::cut_id`); the test vectors are
`WRITING_EXTENSIONS.md`'s.

Fenix never guesses by name. Recognising `add(-1, -1) is -3` as a case
of `add(%i, %i) is %i` is the provider's business, because only the
provider knows its runner's placeholders: it reports a `found` with the
template as `parent` before the result.

**A `found` for an id Fenix has.** A node keeps what each of its three
sources last said of it: the scan of a file, the provider's listing, a
run. Within one source a later `found` replaces that source's record
whole (a file scanned again, a listing that lists it again). What the
page, the gutter and the doors use is, field by field, the scan's,
else the listing's, else the run's, a field left `none` falling through
to the next source: so cargo's `--list`, which gives no line and can't
tell `src/lib.rs`'s inline `mod tests` from `src/tests.rs`, never takes
a scanned test's line or file away. A node's place -- `path`, `line`
and `end_line` -- falls through as one, from the first source that
gives a path or a line, so a listing's file is never paired with a
run's line (§19). `label`, `kind`, `parent` and
`template`, which are never `none`, come from the first of the three,
in that order, that has the node; `data` falls through as the others.
`skip` is the one exception: where a scan has the node, its `skip` is
taken as it is, `none` included, since the scan reads the source that
says it. So a listing that doesn't mark ignored tests (cargo's) leaves
the scan's `#[ignore]` standing, and removing the `#[ignore]` isn't
undone by an older listing that marked it (nextest's).

**What's dropped.** Each source drops its own record; a node goes when
no source has it any more.

- A scan's record goes when a later scan of its file doesn't give it.
  A group that several files' scans give (a target, a package) is kept
  while any of their last scans gives it.
- A listing's record goes when the provider's next **complete** listing
  doesn't give it, unless its file was saved since that listing began
  (the scan is newer than the list). A listing is complete when every
  step of its plan `exited`, whatever the code (pytest's 5 when it
  collected nothing is a complete, empty listing), and no `broken` or
  `crashed` was reported. One that isn't adds and updates, and drops
  nothing.
- A run's record (`case`s of a template, Go's subtests, a macro-made
  test) goes when its template, or its group, runs again and the run
  ends complete, as a listing is, without naming it. A node that goes
  this way goes with the result its run judged it, uncounted.

A node that goes for any other reason -- a test cut and pasted back
across two scans, a scan of half-typed text -- has its result kept as
one read at start is: shown stale when the node is found again, and
written to the results file meanwhile (§19, finding 8).

**A result** is the run's last `ended` for the id: a later `ended` in
the same run, in the same step or a later one, replaces the earlier,
and the counts, the marks and the problem follow it (pytest's call
passed then its teardown failed; a test `crashed` errored, then
reported after all). Readers that can hold a result until it's whole
should (libtest's `FAILED` comes long before its text), but needn't.

### States

| State | Means | Gutter, page |
|---|---|---|
| unknown | never run, or no result kept | `?`, muted |
| queued | a run has it, not started | `·`, muted |
| running | the runner said it started; or, for a runner that doesn't say, its step started | `●`, accent |
| passed | | `✓`, good |
| failed | its assertion, panic or exception failed | `✗`, bad |
| skipped | the runner skipped it (`#[ignore]`, `@skip`, `t.Skip`, `test.skip`, xfail) | `○`, muted, with the reason |
| errored | it couldn't run to a result: its target didn't build, its file didn't load, its setup or fixture failed, its process crashed before it ended, the run timed out | `!`, bad, with why |

**Stale** is a flag over the last result, not a state: the mark stays,
dimmed, so you still see what it was. A test is stale when its file
has been saved since its result, or the result came from before Fenix
restarted.

A run that's stopped, by you or by a run that replaces it, puts each of
its queued and running tests back to what it was before the run, and
says on the page head that it was stopped. Nothing is marked as having
failed because you stopped it.

**A group's state** is worked out from its children as results come:
failed or errored if any is, running if any runs, passed if all that
ran passed, else unknown. Counts are kept per group as results land,
never summed per frame.

### Runs

A **run** is a selection (ids, a file, the failed, all) over one project,
turned into plans, one per provider that has a test in it, and their
steps run in turn. One run goes at a time per project (§8); a run has
an id that the provider sees (§6.4).

A **listing** (`discover`) is a run whose steps report `found`, not
results. It goes through the same machinery and the same reader; its
steps' `covers` mean nothing, and an `ended` in it is ignored.

**What the provider is asked.** A `run` ask holds the nodes selected
(the topmost of them), in display order, each as the model has it now
(the merged fields, its `data`, its `parent` the one Fenix put it
under), and after each `file` or `suite` among them its *fill-in*:
every node Fenix has under it, the suites between included, in display
order, so a provider whose runner filters by substring (libtest's
`geo::` also runs `other::geo::`) can name them exactly. Never under a
`group`: a target or a package is named to its runner as itself. Every
fill-in record's `parent` is in the ask, and no selected node's is: so
**a record whose `parent` is in the ask is a fill-in, one whose
`parent` isn't was selected**, and a provider tells them apart without
reading ids (§19, finding 3). An ask holds at most 10,000 records: the
nodes selected first, then each fill-in in display order while it fits,
whole or not at all (a later, smaller one may still fit); one left out
has its runner's filter run it, which runs at least those. A selection
of more than 10,000 nodes is asked as their parents, coarser but never
fewer; the whole project, as `run-all`. A node whose id was cut and
whose `data` isn't kept can't be named to its runner: it's never asked
for itself, but as its nearest ancestor that can be, and a fill-in that
holds one is left out (§19, finding 2).

### What's kept across a restart

- **Results**, not tests: for each `(extension, provider, id)` with a result, its
  state, duration, when, message, place and failure text (cut to 4
  KiB), in `tests/<hash of the project's root>.json` in Fenix's state
  folder (where `agent-usage.json` is, `AGENTS.md`, "Usage"), written
  through `fenix-storage` at a run's end, at most 8 MiB. Read when the
  project's first provider starts. Shown stale until the test runs
  again. `tests.keep_results` (on) turns this off.
- **The tree** is found again (scans are instant; a listing runs when
  §7.1 says), so a test that's gone isn't shown from a stale file.
- Output isn't kept.

## 5. The manifest: `[[contributes.test_providers]]`

```toml
[engine]
fenix = ">=1.6, <2"
api = "1.6"                    # test providers need 1.6

[[contributes.test_providers]]
id = "rust"                    # its own name (rust, for fenix.rust), or one
                               # starting with it: rust.nextest
name = "cargo test"            # shown until test-provider-set says otherwise
languages = ["rust"]           # open files of these are scanned (tests-scan),
                               # and their tests run from any door
markers = ["Cargo.toml"]       # projects it serves: one holding one of these
                               # at its top
scan = ["src/**/*.rs", "tests/**/*.rs", "benches/**/*.rs"]
                               # files of the project scanned when they aren't
                               # open: patterns below the project; none: only
                               # open files and its listing
```

| Field | Rule (checked in `Manifest::check`, beside the agents', `manifest.rs:1100-1112`) |
|---|---|
| `id` | Lowercase letters, digits, `-`, `_`, `.`; the extension's short name or starting with it plus `.`, as an agent's id must (`manifest.rs:1106-1108`). Unique in the manifest. |
| `name` | Not empty. |
| `languages`, `markers` | At least one of them. Fenix's language names; markers as a context's (no `..`, no leading `/`, `manifest.rs:1122-1124`). |
| `scan` | Patterns as `language_servers.files` (a name, or a path below the project; no `..`, `manifest.rs:1090-1092`), matched as `file_matches` does (`manifest.rs:408-415`), on a walk that honours the project's ignores (§6.7). Optional. |
| `engine.api` | 1.6 or later when there's a test provider: "test providers need engine.api 1.6". |

It's a `Vec<TestProviderContribution>` in `Contributes`
(`manifest.rs:523-579`), `deny_unknown_fields` as its siblings.

**Permissions.** Nothing new. A plan's commands are held to
`permissions.process`, as `spawn`'s and `tasks-set`'s are
(`fenix-wasm/src/host.rs:1183-1210`, `1348-1356`;
`app/wasm_host.rs:427-431`), and work in `/project` (`filesystem =
"project-read"`) or `/data`. All four packs already list their runners
and read the project (fenix-extensions `extensions/{rust,python,go,web}/extension.toml`,
`[permissions]`). The store shows a test provider among what an
extension adds, as it shows a language server ("Runs tests: cargo
test"), beside its permissions before install
(`WRITING_EXTENSIONS.md:1096-1097`).

**Starting.** A provider's extension is started as its other
contributions start it (`WRITING_EXTENSIONS.md:1048-1055`): a file of
one of its languages opened, or a project with one of its markers. Fenix
also starts it when the tests page opens in a project its markers
match, as a page or a service starts one when used.

## 6. The extension API: test providers (1.6)

### 6.1 Declarative, not imperative

| | (A) Declarative: the provider gives commands, Fenix runs them and gives it their output | (B) Imperative: the provider spawns and reports through host calls |
|---|---|---|
| Stopping a run | Fenix's `TaskRunner`: a job object on Windows that kills the tree, and kills it if Fenix dies; a process group on Linux (`fenix-tasks/src/supervisor.rs:67-124`, `165-210`). What the runner started dies with it. | `process::kill` kills the child only (`host.rs:2344-2350`): cargo's test binary ran 6.2 s on after its `cargo` was killed (the packs' plan, appendix, "Stopping cargo"). Needs a new import to fix. |
| Output | `TaskRunner` reads both pipes, lossy UTF-8, lines cut at 64 KiB, `\r` dropped, in batches (`supervisor.rs:17`, `404-450`) | `spawn`'s readers stop a stream at its first line that isn't UTF-8 and have no line limit (`host.rs:1567-1580`); one `process-output` event per line |
| Calls into the extension for 100,000 lines | ~200 (§6.7) | 100,000 |
| Remote projects (SSH) | Fenix runs the commands through the stand-in, as tasks will in remote Phase 4 (`docs/plans/remote-ssh.md:372-377`) | Waits for API work that makes an extension's `spawn` run there (remote Phase 5, `remote-ssh.md:524`) |
| The raw output | Fenix has it: the run's output buffer, `Enter` on a place | The extension would have to give it back |
| Timeouts, the queue, two runs at once | Fenix's, one place | Each pack's, or lang-kit's |
| What the provider must get right | Its commands, and parsing a line at a time | All of the above, per pack |
| What Fenix must know | Nothing about any runner | Nothing about any runner |

(B) gives the pack more freedom (a runner that must be driven over a
socket, a REST API) at the cost of every serious limit the first design
found. (A) loses nothing the five runners need: each is a program with
arguments whose stdout says what happened (the packs' plan, §3). So (A).
A provider can still `spawn` what it likes for its own purposes, but
only a plan's steps are test runs.

Fenix doesn't parse runner formats itself (JUnit XML, TAP, `go test
-json`), even standard ones: a reader is a few dozen lines in a pack,
and every runner's quirks (pytest's progress dots before a line, Go
before 1.24) are the pack's. Built-in readers could come later as a
field the plan sets; 1.6 doesn't have one (§16, Q6).

### 6.2 What's added, and the rules it keeps

API **1.6**, the minor after 1.5 (`manifest.rs:14`). Following the
rule in `WRITING_EXTENSIONS.md:1134-1147` and `world.wit:37-43`:

- **New exports** in `world extension` (not `extension-base`), which
  Fenix calls only when the component has them: found with
  `get_typed_func` into `Optional`, as `service-cancelled` and
  `key-scheme-changed` are (`fenix-wasm/src/host.rs:37-43`, `660-675`,
  `3923-3935`).
- **One new import** in `host`, which an older extension never calls.
- **New records and variants** in `interface types`, used only by the
  new functions. **No new case of `event`.**
- **Nothing in these types changes within 1.x.** The component model
  checks an export's type exactly, so a record that grew a field in 1.7
  would make a 1.6 guest's `tests-plan` look missing. A later need is a
  new export (`tests-plan-2`), never a changed record. That's why the
  records below carry what remote projects will need (`needs`) now, and
  what the packs' review asked for (`data`, `list-after`, a crash's
  scope; §18).

`api_fits("1.6")` is true on this Fenix once `API` is `"1.6.0"`, and
false on 1.5.x (`manifest.rs:25-28`, its test at `1326-1327`). A pack
with `api = "1.6"` doesn't install on Fenix 1.5 (`manifest.rs:1236-1253`),
and the registry offers a 1.5 user the newest version that fits
(`fenix-extensions/src/registry.rs:158-168`): the pack's release from
before it had tests. So a pack adds tests in a minor of its own (Rust
1.3.0, say), and Fenix 1.5 users keep the one before.

**A component built on the 1.6 crate that says `api = "1.5"`.** Every
extension rebuilt on the 1.6 crate exports the five functions (§6.6);
the 24 kit users in fenix-extensions that run no tests will be rebuilt
so when the kits move (the packs' plan, §7, D-A2), and whether they can
keep `api = "1.5"` depends on whether such a component starts on Fenix
1.5. Read from the code, it does:

- Fenix 1.5 instantiates with a `Linker` holding WASI and 1.5's
  `extension-base` imports (`fenix-wasm/src/host.rs:3755-3757`, `3802`),
  then looks up only the exports it knows (`Extension::new`, `3803`;
  `Optional::of`, `667-674`). Exports it doesn't know are never looked
  at.
- The component imports only the functions its core module calls:
  wit-component builds each imported instance from the module's own
  imports (wit-component 0.262, `encoding/world.rs:582-597`; the crate
  uses wit-bindgen 0.63, `fenix-extension-api/Cargo.toml:14`). A pack
  that never calls `tests::provider` doesn't import
  `test-provider-set`; the linker drops the unused wrapper.
- The new records reach the component as type exports of the imported
  `types` instance, which the matching skips; only a function missing
  from the linker fails (`matching.rs:47-51`, `145-165`, in wasmtime
  36.0.20, the version whose source was at hand; Fenix uses wasmtime 50,
  `fenix-wasm/Cargo.toml:22`).

So it should start, and Fenix 1.6 treats it as a 1.5 extension with no
provider (its manifest names none). Because that last point was read in
another wasmtime than Fenix's, step 2 settles it with a test (§15): a
guest built on the 1.6 crate, instantiated against 1.5's world.

**The number.** The remote-projects plan names an "API 1.6" for its
Phase 5 (`remote-ssh.md:524`, `561`). Whichever lands first takes 1.6;
this is likely to be first, and that plan's addition becomes 1.7 (§16,
Q5).

**Before it's frozen.** The API is frozen when a Fenix release carries
it. Between step 2 (§15) and that release it's in the nightly only, and
may still change by agreement with the packs' work; no pack with `api =
"1.6"` is published to the store before Fenix 1.6.0 is out.

### 6.3 The WIT

```wit
// interface types, 1.6

/// What a test provider found: a test, or a group of them, from a
/// file's text (`tests-scan`), its runner's listing, or a run
/// (`test-report.found`). A `found` for an id Fenix has already is
/// merged with it (§4). 1.6.
record found-test {
    /// The provider's own name for it, the same in what it scans, lists
    /// and reads from a run: `orbit/lib tests::adds`,
    /// `tests/test_basic.py::test_cases[negative]`,
    /// `example.com/orbit TestTable/negative_one`. Unique among its
    /// provider's. One over 1 KiB is cut, the same way wherever it
    /// comes: its first bytes up to 1,007, back to a character's start,
    /// then `#` and the FNV-1a 64-bit hash of the whole id's UTF-8 as 16
    /// lowercase hex digits; Fenix gives it back so, and the provider
    /// names it to its runner by its `data`. A `parent`, a `covers` id
    /// and a report's id are cut the same way.
    id: string,
    /// The group it's in, by id: none for one at its provider's top.
    /// Given back, the one Fenix put it under: none when Fenix put it at
    /// the top (its parent never found).
    parent: option<string>,
    /// What the page calls it: `adds`, `negative`, `orbit (lib)`. At
    /// most 256 characters.
    label: string,
    kind: test-kind,
    /// Its file: `/project/<path>`; or relative, to the project in a
    /// scan's answer and to its step's folder in a run's or a listing's
    /// report; or a full path of the project's machine. In a scan's
    /// answer, none for a test or case is the file scanned. Given back,
    /// `/project/<path>` with `/` for a file of the project, whatever
    /// form it came in, and a full path for one outside it.
    path: option<string>,
    /// Its first line, and its last when it knows: from 0, as a
    /// problem's (`problem`). Given back, with `path`, from the first
    /// of its scan, its listing and a run that gives a path or a line:
    /// a scan's may be of unsaved text.
    line: option<u32>,
    end-line: option<u32>,
    /// Left out by its source (`#[ignore = "slow"]`, `@skip`,
    /// `test.skip`): why, or "" when it doesn't say.
    skip: option<string>,
    /// Its cases are found only by running it: a `test.each`, a
    /// `parametrize` the scan saw, a Go test that calls `t.Run`.
    template: bool,
    /// The provider's own, at most 4 KiB: never shown, given back as it
    /// was wherever Fenix gives this node back (`test-ask.run`,
    /// `tests-launch`). What its runner calls it when the id can't say
    /// (a pytest id over 1 KiB, a describe named `a > b`). It can come
    /// back none: one over 4 KiB isn't kept, nor any past 16 MiB of a
    /// provider's, nor across a restart until a scan, listing or run
    /// gives it again; a source that gave none, when the one that gave
    /// it has dropped the node. A provider names a node by its id when
    /// its `data` is none; a node whose id was cut and whose `data`
    /// isn't kept is never given back for itself (§4, "Review of steps
    /// 1 and 2").
    data: option<string>,
}

/// What a node of the tree is (`found-test`).
enum test-kind { group, file, suite, test, case }

/// What Fenix asks a provider for (`tests-plan`).
variant test-ask {
    /// Its runner's listing of every test it can find in the project.
    discover,
    /// These, as Fenix has them now: tests, cases, and groups (all
    /// under them). After each `file` or `suite` asked comes its
    /// fill-in: every node Fenix has under it, suites included, so each
    /// one's `parent` is in the ask, while the ask stays within 10,000
    /// records; a fill-in is whole or left out, and one asked with
    /// nothing after it is planned by its runner's own filter for it
    /// (`geo::`, a file's path). A record whose `parent` is in the ask
    /// is a fill-in; one whose `parent` isn't was asked for itself.
    /// Never more than 10,000 records: a larger selection is asked as
    /// their parents.
    run(list<found-test>),
    /// Every test.
    run-all,
}

/// What runs, in order (`tests-plan`).
record test-plan {
    steps: list<test-step>,
    /// Its listing costs little once this plan has run (it built what
    /// the listing builds: cargo's `--list` after `cargo test`), so
    /// Fenix lists it after, as an automatic listing (§7.1). Ignored in
    /// a listing's own plan.
    list-after: bool,
}

/// One command of a plan.
record test-step {
    /// Held to `permissions.process`, as `spawn`'s are; `${data}` and
    /// `${tools}` in its args and env are filled in. Run as it is: no
    /// shell between. Its command line, as Windows quotes it, at most
    /// 32,766 characters (§6.5).
    command: command,
    /// What the page says while it runs: `orbit (lib)`, `tests/`. At
    /// most 256 characters.
    label: string,
    /// The ids it runs, when it knows them: a group's, a file's or a
    /// suite's stands for everything Fenix has under it. Empty: it
    /// claims nothing of its own; what of the ask no step names is the
    /// plan's, judged when the plan's last step has ended (§8.5).
    /// Ignored in a listing.
    covers: list<string>,
    /// Files under `/data` its command reads (a reporter, a config, a
    /// file of ids written for this run): each must be there when the
    /// step starts. For a remote project each is copied to the
    /// project's machine before the step, every time, keeping its path
    /// below `/data` (§8.6).
    needs: list<string>,
}

/// A line of a step's output (`tests-read`).
record test-line {
    /// As UTF-8, what wasn't replaced by U+FFFD; colour codes taken out.
    text: string,
    stderr: bool,
    /// The line went on past 64 KiB: the rest is the next line of the
    /// same stream (lines of the other may come between, and a later
    /// call). Every piece but the last is cut; a line of 64 KiB or less,
    /// at a stream's end too, never is.
    cut: bool,
}

/// How a step ended (`tests-step-ended`).
variant test-step-end {
    /// Its program ended: its exit code, none for a signal (Linux). On
    /// Windows a code is the process's 32 bits read as signed: a crash's
    /// NTSTATUS (`0xC0000005`) is negative.
    exited(option<s32>),
    /// Stopped: by you, by a run that replaced it, by its provider
    /// starting again, or never started because the run was stopped
    /// before it.
    stopped,
    /// It ran past `tests.timeout_minutes`, and was stopped.
    timed-out,
    /// It couldn't start, or its output couldn't be read: why.
    failed(string),
}

/// How a test ended (`test-ended`).
enum test-outcome { passed, failed, skipped, errored }

/// A place in a file: from 0, columns in characters.
record test-location {
    /// `/project/<path>`; relative, to the step's folder; or a full
    /// path of the project's machine (`C:\src\orbit\src\lib.rs`).
    path: string,
    line: u32,
    column: option<u32>,
}

/// A test that ended. The last `ended` for an id in a run is its
/// result (§4).
record test-ended {
    id: string,
    outcome: test-outcome,
    /// How long it took, when its runner says.
    duration-ms: option<f64>,
    /// One line: a failure's message, a skip's reason, why it errored.
    message: option<string>,
    /// Where it failed: the assertion, the panic, the deepest frame in
    /// the project.
    at: option<test-location>,
    /// All of it: the diff, the traceback, what it printed.
    text: option<string>,
}

/// What a test printed, or the run did (`id` none).
record test-output {
    id: option<string>,
    text: string,
}

/// What didn't build or load: never a test's failure. What's under it
/// and has no result in this run yet is errored with it.
record test-broken {
    /// The group or file that didn't: none for all the step covers.
    id: option<string>,
    message: string,
    at: option<test-location>,
}

/// A process that died, and what it held (`test-report.crashed`).
record test-crash {
    /// The group whose process it was (a test binary's target, a Go
    /// package): what's under it and has no result in this run yet is
    /// errored with it. None: all the step covers that has none.
    id: option<string>,
    /// What it said: `its binary crashed: SIGABRT`.
    message: string,
}

/// What a provider read in its runner's output (`tests-read`,
/// `tests-step-ended`). Fenix applies them in order.
variant test-report {
    /// A test or group: from a listing, or found in a run.
    found(found-test),
    /// A test started, by id.
    started(string),
    ended(test-ended),
    output(test-output),
    broken(test-broken),
    /// A process died.
    crashed(test-crash),
}

/// How to debug a test (`tests-launch`).
variant test-launch {
    /// This launch, now.
    launch(launch-config),
    /// These first -- a build that says where the test's binary is --
    /// read as a run's are, as the run `tests-launch` was asked with:
    /// its steps' `tests-read` and `tests-step-ended` carry that run.
    /// Then `tests-launch` is asked again, once, with the same run, if
    /// every step exited 0 and nothing was `broken` or `crashed` (§11).
    build-first(test-plan),
}
```

```wit
// interface host, 1.6

/// What its test provider `provider` (`[[contributes.test_providers]]`)
/// is in its project now: the runner it uses (`cargo nextest`, at most
/// 256 characters), and why it can't run tests there when it can't
/// (`pytest isn't installed in .venv`, at most 4 KiB). `runner` empty
/// with no `problem`: it has nothing to run in this project, and isn't
/// shown or asked there until a later call says otherwise. A call that
/// says what Fenix has already changes nothing; one that changes it has
/// Fenix list the project's tests again, as an automatic listing (§7.1).
/// Fenix keeps what it said for the project until Fenix exits, across
/// the extension stopping and starting again; it calls at each start
/// all the same, since it isn't told what Fenix has. 1.6.
test-provider-set: func(provider: string, runner: string, problem: option<string>) -> result<_, string>;
```

```wit
// world extension, 1.6
use types.{found-test, test-ask, test-plan, test-line, test-step-end, test-report, test-launch};

/// The tests in file `path` (`/project/<path>`, with `/`), whose text
/// is `text`, as it's open or on disk: read, nothing run. The answer
/// replaces the file's last scan: an empty one drops its tests, so a
/// scanner that can't read half-typed text answers what it last read
/// well. 1.6.
export tests-scan: func(provider: string, path: string, text: string) -> list<found-test>;
/// What to run for `ask`, as run `run`. 1.6.
export tests-plan: func(provider: string, run: u64, ask: test-ask) -> result<test-plan, string>;
/// Lines of the output of step `step` of run `run`, in order. 1.6.
export tests-read: func(provider: string, run: u64, step: u32, lines: list<test-line>) -> list<test-report>;
/// Step `step` of run `run` ended: what its reader still had. 1.6.
export tests-step-ended: func(provider: string, run: u64, step: u32, end: test-step-end) -> list<test-report>;
/// How to debug test `test`, for run `run`: a run of its own, which a
/// `build-first` plan runs as, and the second ask carries. 1.6.
export tests-launch: func(provider: string, run: u64, test: found-test) -> result<test-launch, string>;
```

**Host side.** `fenix-wasm`'s `bindgen!` is of `extension-base`
(`host.rs:42`), which generates only the types its world reaches.
`key-scheme-changed` works because `key-scheme` is also reached through
`host.editor-keys`; these records aren't reached from `host`. So step 2
adds a second, types-only `bindgen!` of world `extension` (or `use`s
them in `host`), and `Optional` gains five `TypedFunc`s. Five `Call`
variants (`TestsScan`, `TestsPlan`, `TestsRead`, `TestsStepEnded`,
`TestsLaunch`), none `is_page` (`host.rs:654-658`), each with a `Reply`
carrying its answer; `Request::TestProviderSet` for the import. An
extension whose manifest names a provider but whose component lacks
`tests-scan` and `tests-plan` is told so on its output page and its
providers aren't offered. In `stale_call` (`app/wasm_host.rs:451-467`)
the five are `Deliver` when they're about the project the instance
runs in, and a scan or a plan for another project starts it again there
first (`Restart`), as §6.4's "One project" says.

### 6.4 What Fenix promises a provider

| Promise | |
|---|---|
| Calls | `tests-scan`, `tests-plan`, `tests-read`, `tests-step-ended`, `tests-launch` are ordinary calls: pages go first (`WRITING_EXTENSIONS.md:1116-1126`), one call at a time, its deadline the extension's (`limits.call_ms`, 5 s if it doesn't say). |
| Run ids | Unique for as long as Fenix runs, across projects: every project's model draws them from the host's one count. A listing has one too, and so does a debug launch: its `build-first` plan runs as the run its first `tests-launch` was asked with, and the second ask carries it (§11). |
| Order | For each step: `tests-read` in output order (as `TaskRunner` gives it), then exactly one `tests-step-ended`, after every line Fenix gives that step. Every step of a plan gets its `tests-step-ended`, `stopped` for one that never started, so the provider can free what it kept. Steps run one after another, never two of one run at once: step *n* + 1's first call comes after step *n*'s `tests-step-ended`. A run's providers go in turn, each plan whole. A call's reports are applied in their order, and before the next call's. |
| Batches | A `tests-read` holds at most 512 lines and 256 KiB. While one is under way for a step, what comes is held for the next, so a step never has two calls queued. |
| Stopping | A stop (`x`, `SPC T k`, a replacing run, the timeout) kills the step's tree at once. A call under way isn't cut short: it ends, and its reports are applied, since what a runner said before it was killed is true. Lines read but not yet given are not given (they're in *Test output*). Then `tests-step-ended(stopped)` (or `timed-out`), whose reports are applied too, then `stopped` for each step not started. |
| Silence | Fenix calls a provider only with lines or an end: never on a timer, and there's no heartbeat. A step that prints nothing for an hour is waiting, as far as Fenix knows, until it ends, you stop it, or `tests.timeout_minutes` passes (§8.3). |
| Lines | Lossy UTF-8 (`supervisor.rs:404-412`); trailing `\r` dropped (`405-407`); a line past 64 KiB comes in pieces of at most 64 KiB, cut at a character's start, each but the last `cut`, the rest of each the next line *of the same stream*: the other stream's lines, and a call's end, may come between (`436-438`, which gains the flag in step 3); a line of 64 KiB or less is never `cut`, nor one that ends with its stream; ANSI CSI and OSC sequences removed. Between stdout and stderr, order is as near as `TaskRunner` keeps it: per write on Linux (`supervisor.rs:290-321`), per 10 ms tick on Windows (`322-325`). A provider reads its runner from one stream where it can. |
| Asks | A `run` ask is at most 10,000 records, its files and suites followed by their fill-ins while they fit: a record whose `parent` is in the ask is a fill-in (§4, "What the provider is asked"). |
| What Fenix gives back | A node in an ask or to `tests-launch` is as Fenix has it, merged (§4): `path` `/project/<path>` with `/` for a file of the project, whatever form the provider gave (the model holds the project's relative form; the host turns it back), a full path for one outside it; `line` and `end-line` with that path, from the first of scan, listing and run that gives either (a scan's may be of unsaved text); `parent` the one Fenix put it under; `data` as it was, or none (over 4 KiB, past 16 MiB, after a restart until found again, or the source that gave it gone); `id` cut (§4) when it was over 1 KiB, and then never given back for itself without its `data`. |
| Scans | One file a call, at most 1 MiB of text (a larger file isn't scanned: its tests come from the listing), its path `/project/<path>` with `/`, as Fenix names files to extensions elsewhere (`app/wasm_host.rs:1176`, `1231`). Only files of the project the instance runs in. After typing pauses, at most one scan per file every 500 ms. A scan's answer replaces that file's last scan: an empty one drops its tests, so a scanner that can't read half-typed text answers what it last read well. |
| What a provider said | `test-provider-set`'s runner and problem are kept per project until Fenix exits, across the extension stopping and starting again; a provider calls at each start all the same, and on a change. |
| Paths | Fenix maps what a provider gives (`/project/...`, relative, a full path; `\` or `/`; a drive letter in any case on Windows) to the project's files with `fenix_lsp::normalize`, as task output's places are (`app.rs:11032`). A relative path is the project's in a scan's answer, and the step's `cwd`'s (the project when it has none) in a report: a `found` as much as a `test-location`. A place outside the project is kept for the details, never a problem or a mark. |
| One project | An extension's instance runs in one project: its `/project` is fixed while it runs (`app/wasm_host.rs:243-249`), and Fenix asks a provider only about that project. Moving to another project starts a language pack of that project again there at once (`app/wasm_host.rs:1157-1170`), any other extension when it's next used, which a scan or `SPC T T` there is. A run of the old instance under way is then stopped as after a trap (below): its tests back to what they were, the page saying `stopped: cargo test started again in orbit-cli`. The new instance is never asked about the old run. A run in a project you've left goes on while its instance does. |
| After a trap | The extension stops (ADR 0001); its steps under way are stopped, their tests back to what they were, and the page says the provider stopped and why. No call goes to it for them. |

### 6.5 What a provider may not do, and the limits

- No shell: a step's program is a program its manifest lists. `sh`,
  `cmd` or `pwsh` only if it lists them, and then the store shows that.
- No ids of another provider; no reports for a run that isn't its.
- Nothing a call can't do in a few milliseconds: a call holds a batch,
  never a whole run (§6.7).
- **Sizes**, both ways. What's over a limit is cut (a string) or left
  out (a list's rest), and said once on the provider's output page.
  Only a step fails instead: one whose command line is over its limit,
  or whose `needs` aren't there (below).

| What | At most |
|---|---|
| A call's answer, in all | 16 MiB; past that its reports are left out from where it went over |
| `tests-scan`'s answer | 10,000 `found-test` |
| `found-test` | `id` and `parent` 1 KiB; `label` 256 characters; `path` 4 KiB; `skip` 1 KiB; `data` 4 KiB, and 16 MiB of `data` kept a provider (past that a node's isn't kept, and isn't given back) |
| A `run` ask (Fenix's) | 10,000 records |
| `test-plan` | 256 steps; 10,000 ids in all its steps' `covers`; 32 `needs` a step, each a path under `/data` of at most 4 KiB, 16 MiB of files in all |
| A step's `command` | its command line as Windows quotes it (the program, then each argument quoted as `CommandLineToArgvW` reads it, joined by spaces) at most 32,766 characters, on every platform; on Windows a program that resolves to a `.cmd` or `.bat`, 8,191 (below); its `env` 64 KiB in all; `label` 256 characters |
| A `tests-read` (Fenix's) | 512 lines and 256 KiB |
| Reports | 10,000 a call; `message` 4 KiB, `text` 64 KiB (its end), an `output`'s `text` 64 KiB (its end), a location's `path` 4 KiB |
| `tests-plan`'s and `tests-launch`'s error, `failed`'s reason | 4 KiB |
| `tests-launch`'s `launch-config` | 1 MiB in all, its `attributes` included |
| `test-provider-set` | `runner` 256 characters; `problem` 4 KiB |
| What Fenix keeps | a message 4 KiB, a failure's text 64 KiB (its end), what a test printed 64 KiB (its end), a run's output 16 MiB (its end; the buffer says what went), 100,000 tests a provider a project |

- **The command line.** On Windows Fenix starts a step's program
  directly, with `CreateProcessW`, through Rust's `Command`
  (`fenix-tasks/src/supervisor.rs:62`, `67-94`), whose command line is
  at most 32,767 characters with its terminating null. The one
  exception is a program that `fenix_rpc::resolve_command` finds as a
  `.cmd` or `.bat` on `PATHEXT` (`fenix-rpc/src/lib.rs:45-61`, an
  `npx` or a `node_modules/.bin` shim): Rust runs a batch file through
  `cmd.exe /c`, whose line is at most 8,191, and refuses an argument it
  can't quote safely for it. A step over its limit doesn't start: it
  ends `failed("its command line is 41,203 characters; Windows takes
  32,766")`, its tests errored with that. The 32,766 is checked on Linux
  too, so a provider's long selection fails in its author's tests, not
  first on a user's Windows; the packs split long selections themselves
  (the packs' plan, §4.3, `split`).

  What's counted (§19): the command line as Rust's standard library
  builds it for `CreateProcessW`, after `${data}` and `${tools}` are
  filled in -- the program as Fenix resolved it (its full path), always
  in `"`, then each argument, in `"` when it's empty or holds a space or
  a tab, a `"` in it as `\"` with the `\`s right before it doubled, and
  the `\`s ending a quoted one doubled; joined by one space. For a
  `.cmd` or `.bat`, the whole line Rust gives `cmd.exe` (`cmd.exe /e:ON
  /v:OFF /d /c` and its quoting for batch files included). A pack can't
  know the program's full path or `${data}`'s length: it counts them
  generously and keeps a margin.
- **`needs`** are checked on every platform: one that isn't a file under
  `/data` when its step starts ends the step `failed("needs
  /data/lang-kit/tests/fenix_tests.py, which isn't there")`, so a
  provider that forgets to write its reporter learns it here, not first
  on a remote project. A need is a `/data/...` path with `/`: one over 4
  KiB, with a `\`, or not naming a file below `/data` fails its step
  the same way, never cut or mended into another path (§19).

### 6.6 The Rust side a pack author sees

In `fenix-extension-api`, a `tests` module with the types, as `lsp`
and `forge` are (`crates/fenix-extension-api/src/lsp.rs`, `forge.rs`),
and five methods on `Extension` with defaults, as `key_scheme_changed`
has (`src/lib.rs:232`):

```rust
pub mod tests {
    pub struct Found { pub id: String, pub parent: Option<String>, pub label: String, pub kind: Kind,
                       pub path: Option<String>, pub line: Option<u32>, pub end_line: Option<u32>,
                       pub skip: Option<String>, pub template: bool, pub data: Option<String> }
    pub enum Kind { Group, File, Suite, Test, Case }
    pub enum Ask { Discover, Run(Vec<Found>), RunAll }
    pub struct Plan { pub steps: Vec<Step>, pub list_after: bool }
    pub struct Step { pub command: Command, pub label: String, pub covers: Vec<String>, pub needs: Vec<String> }
    pub struct Line { pub text: String, pub stderr: bool, pub cut: bool }
    pub enum StepEnd { Exited(Option<i32>), Stopped, TimedOut, Failed(String) }
    pub enum Outcome { Passed, Failed, Skipped, Errored }
    pub struct Location { pub path: String, pub line: u32, pub column: Option<u32> }
    pub struct Ended { pub id: String, pub outcome: Outcome, pub duration_ms: Option<f64>,
                       pub message: Option<String>, pub at: Option<Location>, pub text: Option<String> }
    pub enum Report { Found(Found), Started(String), Ended(Ended), Output { id: Option<String>, text: String },
                      Broken { id: Option<String>, message: String, at: Option<Location> },
                      Crashed { id: Option<String>, message: String } }
    pub enum Launch { Launch(LaunchConfig), BuildFirst(Plan) }

    /// `test-provider-set`: `runner` "" and no `problem` is "nothing to run in this project".
    pub fn provider(provider: &str, runner: &str, problem: Option<&str>) -> Result<(), String>;
}

pub trait Extension {
    // ... as now, and:
    fn tests_scan(&mut self, _provider: &str, _path: &str, _text: &str) -> Vec<tests::Found> { Vec::new() }
    fn tests_plan(&mut self, _provider: &str, _run: u64, _ask: tests::Ask) -> Result<tests::Plan, String> { Err("no tests here".into()) }
    fn tests_read(&mut self, _provider: &str, _run: u64, _step: u32, _lines: Vec<tests::Line>) -> Vec<tests::Report> { Vec::new() }
    fn tests_step_ended(&mut self, _provider: &str, _run: u64, _step: u32, _end: tests::StepEnd) -> Vec<tests::Report> { Vec::new() }
    fn tests_launch(&mut self, _provider: &str, _run: u64, _test: tests::Found) -> Result<tests::Launch, String> { Err("no debugging for tests here".into()) }
}
```

Every extension rebuilt on the 1.6 crate exports these (the crate
exports the whole `extension` world, `src/lib.rs:51-58`), with the
defaults; only the manifest makes it a provider.

**The first design's `TestRunner`, on this API.** lang-kit keeps the
trait and wraps it in these five methods (fenix-extensions
`crates/lang-kit`, step E1a). The trait as the packs now write it, with
its own changes, is the packs' plan's §4.2; this table is what moved
from the first design onto the API.

| `TestRunner` part (the first design) | Becomes | What changes |
|---|---|---|
| `name()` | the manifest's `name`, and `tests::provider(id, runner, problem)` when what it says changes (nextest found) | `runner()` in the packs' plan; lang-kit calls the import only on a change. |
| `scans(path)`, `scan(path, text) -> Vec<Found>` | `tests_scan` | Fenix calls it (open, typing paused, saved, the `scan` patterns); lang-kit's scheduling of scans goes. `Found.path: Vec<String>` (labels root first) becomes `groups: Vec<Group>`, the pack's; lang-kit turns it into `id`/`parent` (a group's id is its labels root first, each preceded by U+001F, so it can never equal a test's key) and gives groups their own `found-test`. `line` becomes from 0. `end_line` is new and optional. |
| `list() -> Vec<Command>` | `tests_plan(Discover)`: each command a step | The commands are unchanged. |
| `plan(&Selection) -> Vec<Command>` | `tests_plan(Run(found) \| RunAll)` | `Selection::Tests(&Test)` becomes the `Found` records Fenix sends, with their `data`; `Selection::Files` is a `file` node among them, followed by the tests Fenix has under it (§4); `All` is `RunAll`. A provider needs no index of its own: what it plans from comes with the ask. A plan that builds what its listing needs says `list_after`. |
| `reader() -> Box<dyn Reader>`, `Reader::line`, `Reader::end` | one `Reader` per `(run, step)`, made at the step's first `tests_read`, dropped at `tests_step_ended` | `Line { text: &str, stderr }` gains `cut`. `end(code)` gets a `StepEnd`. Lines come in batches; the reader is still fed a line at a time. |
| `Report` | `tests::Report`, one for one | `Ended`'s `Failure { message, at, text }` is flattened into `Ended`; `Outcome::Skipped(reason)` is `Skipped` with `message`; `NotRun` is gone (Fenix decides "back to what it was" or `errored`); `errored` is new, for a pytest fixture or setup failure; `Crashed` names the group whose process died. |
| `is_case(template, reported)` | inside lang-kit's reader adapter | lang-kit keeps the run's asked templates; a reported key it doesn't know that `is_case` says belongs to one is reported as `found` (kind `case`, parent the template) before its `ended`. Fenix never matches by name. |
| `launch(&Test) -> Option<LaunchConfig>` | `tests_launch` | May return `BuildFirst(plan)`: Rust's `cargo test --no-run --message-format=json` for the test's target, read like a run, whose `compiler-artifact` gives the binary; then `Launch`. |
| `assets() -> &[(&str, &[u8])]` | unchanged: written under `/data` at start | Each step lists the ones it reads in `needs`. `${data}` in args is already filled in by Fenix (`host.rs:1165-1179`). |

What goes from lang-kit's part of the first design: the model, the
page, decorations, the problems ledger (§9.3 makes it unneeded), the
queue and stopping, the notification, the modeline item, the agent
services, `page_state` and the results file. What's left is the
adapter above and the test kit's driver (the packs' plan, §4.3, §4.5,
§8.1's E1a).

### 6.7 Volume and the millisecond rule

- `TaskRunner` already batches: it reads at most 64 KiB a stream a tick
  and sends a `Vec<OutputLine>` (`supervisor.rs:265`, `414-450`).
  Fenix appends them to the run's output buffer and gives them to the
  provider 512 lines at a time, so 100,000 lines is about 200 calls,
  each parsing 512 lines (one `serde_json` value or one pattern each:
  microseconds natively, **not measured** in wasm; step 3 measures it).
- A provider that falls behind doesn't make Fenix's memory grow without
  end: past 32 MiB of lines waiting for it, Fenix stops giving that
  step's lines, keeps them in the output buffer only, and ends the
  step's unreported tests `errored` ("its output was more than
  *provider* could read"), saying so on the page; its
  `tests-step-ended` still comes when it ends.
- A scan is one file's text. A project's first scan of its `scan`
  files is paced: one file a call, files read off the UI thread, at
  most 2,000 files a provider (the rest come from the listing, or when
  they're opened; the page's provider row says `2,000 of 3,412 files
  scanned`).
- **The walk honours the project's ignores**, as Search's does
  (`fenix-project/src/search.rs:288-308`; a remote project's,
  `fenix-remote/src/search.rs:76-81`): hidden files and folders are
  left out, as are what `.gitignore`, `.ignore`, the global and the
  repository's excludes leave out, read from the parents too and with
  or without a Git repository, and `.git` never. So `.venv`, `target/`
  and `node_modules` aren't walked where the project ignores them (Cargo
  and the usual templates do), and a provider's `scan` can be names
  (`test_*.py`, `*.test.*`) rather than paths that dodge them. An open
  file is scanned whatever the ignores say.

## 7. Discovery, and the page

### 7.1 When tests are found

| | When |
|---|---|
| Scan an open file | When it's opened (before it's first drawn, so the gutter is right at once), 500 ms after typing pauses, on save. |
| Scan the project's `scan` files | When the provider starts in the project; then a file when it changes on disk (Fenix's watch on the project, as the explorer's). Through the project's ignores (§6.7). |
| List (`discover`) | When the tests page first opens in a session; after a run whose provider's plan said `list-after` (it built what the listing builds: cargo's `--list` is then 0.18 s against 20.9 s cold, the packs' plan §3), once that plan's steps all `exited` and nothing was `broken`; after a `test-provider-set` that changed what Fenix had, once the page has opened in the session (before, the page's opening lists anyway); and on `u`. **Not** when the project opens: a listing builds (cargo, `go test -list`) or imports the tests (pytest, Vitest, Jest), and Fenix starting stays cheap (roadmap's #33). Only in a trusted project (§12); `tests.list_automatically` (on) turns the automatic ones off. Never while a run goes: it waits for it. |

### 7.2 The page

The page is Fenix's own, `PageModel::Tests` (`app/pages.rs:107-144`),
laid out by a pure `tests_page.rs` as the Search page is by
`search_page.rs`, opened in the focused pane (`app/pages.rs:513-528`).
Its tutorial and focus name is `tests` (`app/tutorial_host.rs:221-250`,
`app/wasm_host.rs:1250-1279` gain the case). Wide (90 columns or more),
the details are on the right; narrow, under the tree. `›` marks the
focused row, which Fenix draws as a background.

At 100×30, a Rust workspace with a Python tool, after a run:

```text
Tests  orbit       ✓ 31  ✗ 4  ! 1  ○ 2  ? 12        ran 14:02 for 4.1 s · 1 file saved since
────────────────────────────────────────────────────────────────────────────────────────────────────
All 50  Failed 5  Not run 14                                                       / to filter
▾ cargo test                            ✗ 3          │ tests::case_negative                ✗ failed
  ▾ orbit (lib)                         ✗ 3          │ src/lib.rs:45 · cargo test · ran 14:02
    ▾ src/geo/mod.rs  geo::tests        ✗ 1          │
        dist_is_symmetric               ✓            │ assertion `left == right` failed
        unwraps_none                    ✗            │   left: -2
    ▾ src/lib.rs  tests                 ✗ 2          │  right: -3
        adds                            ✓            │ at src/lib.rs:43:5
      › case_negative                   ✗            │
        fails_on_purpose                ✗            │ ---- tests::case_negative stdout ----
        ignored_one                     ○ slow       │ thread 'tests::case_negative' (9756)
        panics_as_it_should             ✓            │ panicked at src/lib.rs:43:5:
  ▸ orbit (test it)                     ✓ 4          │ assertion `left == right` failed
▾ pytest                                ✗ 1  ! 1     │   left: -2
  ▾ tools/tests/test_basic.py           ✗ 1  ! 1     │  right: -3
      test_adds                         ✓     0.0 ms │
      test_fails_on_purpose             ✗     0.3 ms │
      test_uses_broken_fixture          ! setup      │
    ▾ test_cases                        ✓ 3   0.6 ms │
        small                           ✓     0.0 ms │
        negative                        ✓     0.4 ms │
        strings [odd] id                ✓     0.0 ms │
  ▸ tools/tests/test_cli.py             ? 12         │
                                                     │
────────────────────────────────────────────────────────────────────────────────────────────────────
r run  R all  f the failed  . again  d debug  Enter go to it  o output  / filter  x stop  u list
```

At 80×24, while a run goes:

```text
Tests  orbit       running 12 of 40 · ✗ 1                   x stops it
────────────────────────────────────────────────────────────────────────────────
All 40  Failed 1  Not run 28                                      / to filter
▾ orbit (lib)                                                       ● 7  ✗ 1
  ▾ src/geo/mod.rs  geo::tests                                           ✗ 1
      dist_is_symmetric                                                    ✓
      unwraps_none                                                         ✗
  ▾ src/lib.rs  tests                                                    ● 5
    › adds                                                                 ●
      case_negative                                                        ·
      case_small                                                           ·
▸ orbit (test it)                                                          · 4
────────────────────────────────────────────────────────────────────────────────
tests::adds                                                           ● running
src/lib.rs:12 · cargo test · last passed 13:51 (stale)



r run  R all  f failed  . again  d debug  Enter go  o output  / filter  x stop
```

- **The head**: the project, the counts (✓ passed, ✗ failed, !
  errored, ○ skipped, ? not run), when the last run was and how long it
  took, and how many of its files were saved since. While a run goes:
  `running 12 of 40 · ✗ 1 · x stops it`, then `then: 3 tests` when one
  waits. When a target didn't build: `orbit (lib) didn't build:
  src/lib.rs:19 mismatched types · o shows it`, in bad, the marks kept
  as they were. A provider that said why it can't run: its row says so,
  with `SPC p h` for the doctor.
- **The tree**: a group's detail is its failures and errors, else its
  count; a test's its mark and time. Groups start closed except those
  holding a failure, so a large project opens on what's wrong. Stale
  marks are dimmed.
- **The tabs**: All, Failed (failed and errored, with their groups), Not
  run (unknown and skipped). `1` `2` `3`.
- **The details**: the name, the place, the provider, when it ran; the
  message; then the failure's text and what it printed, scrolled with
  `J` `K`. Narrow, they show the first lines; `o` has the rest.
- **The filter** (`/`): typed on the page's field (`KEYBINDINGS.md:722-733`),
  matching labels and ids as you type, case-insensitive, words in any
  order; matches show with their groups opened; Esc clears it, Enter
  keeps it and goes to the tree.

### 7.3 Keys on the page

The same with Vim keys and standard keys: a page's letters act where
you can't type (ADR 0004, decision 8). They follow the Search page's
(`KEYBINDINGS.md:641-670`).

| Keys | Does | On |
|---|---|---|
| `j` `k` ↑ ↓, `gg` `G` | Next, previous, first, last row | tree |
| `l` →, `h` ← | Open; close, or go to the group | tree |
| `Tab` / `za`, `zM` / `zR` | Fold or open a group; fold all, open all | tree |
| `Enter` | Go to the test's line; on a group, fold or open it | tree |
| `r` | Run the focused test, case, file or group | |
| `R` | Run all | |
| `f` | Run the failed and errored again | |
| `.` | Run the last run again | |
| `d` | Debug the focused test (§9) | a test or case |
| `x` | Stop the run | while one runs |
| `o` | The run's output, beside (§8.5) | |
| `y` | Copy the failure: name, place, message | a failed test |
| `n` / `N` | The next, previous failure | |
| `/` | Filter | |
| `1` `2` `3` | All, Failed, Not run | |
| `J` / `K` | Scroll the details | |
| `u` | List again | |
| `Esc` | Clear the filter | while there's one |
| `q` | Close the page | |

Space stays the leader and `q` closes, as on every page
(`WRITING_EXTENSIONS.md:81`). Checked against the Search page's and Git
pages' own keys: the same letters mean the same thing where they're on
both (`j k gg G Tab za zM zR / Enter q`); `r`, `f`, `x`, `n` are Search's
for its fields and leaving out, on another page.

### 7.4 The leader: `SPC T`

`SPC T` is free in Fenix (`default_keymap.toml` has no `SPC T`) and in
every extension in fenix-extensions (their `keymaps/*.toml`: the
uppercase groups taken are `A`, `C`, `P`). An uppercase group for a
whole area has a precedent: the Agenda's `SPC A` (ADR 0002). With
standard keys it's `Ctrl+K T …` (ADR 0004, decision 5).

| Keys | Does | Command |
|---|---|---|
| `SPC T T` | The tests page | `tests.open` |
| `SPC T t` | Run the test the cursor is in | `tests.run_here` |
| `SPC T f` | Run this file's tests | `tests.run_file` |
| `SPC T a` | Run all | `tests.run_all` |
| `SPC T F` | Run the failed again | `tests.run_failed` |
| `SPC T l` | Run the last run again | `tests.run_last` |
| `SPC T d` | Debug the test the cursor is in | `tests.debug_here` |
| `SPC T k` | Stop the run (as `SPC p k`, `SPC a k`) | `tests.stop` |
| `SPC T o` | The last run's output | `tests.output` |

`SPC p r` stays bound to `task.run_test_at_cursor` (`default_keymap.toml:453-455`),
which goes to `tests.run_here` when a provider serves the file (§2).
Note that an installed HTTP extension already takes `SPC p r` as a group
of its own (fenix-extensions `extensions/http/keymaps/http.toml:5`), and
an extension's keys go over Fenix's (`keymap.rs:1331-1344`): for its
users `SPC p r` isn't "run test" today. `SPC T t` is.

**No chords of their own** with standard keys. VS Code's are two-key
sequences (`Ctrl+; A`), which a `[[keys]]` entry, one chord, can't be
(ADR 0004, decision 4); `Ctrl+K T t` is as short. A flavour can bind
chords to these commands. Nothing in the debugger's F-keys moves.

The which-key menu files them under a group "tests"; the palette
lists them under "Tests". All are in `CommandRegistry`, so `SPC h c`
describes them and both schemes bind the same commands (ADR 0004,
decision 3).

### 7.5 With 10,000 tests

- **The model is native**, not in an extension's 128 MiB: a node is a
  few hundred bytes, 10,000 a few MB.
- **The page lays out only open groups' rows**, from the model's per-
  group counts. While a run goes it's laid out again at most ten times
  a second, not per result.
- **Groups start closed**; only failures' groups open.
- **The filter** matches 10,000 labels natively in well under a
  millisecond (an estimate; step 8 measures, as code lens measured a
  20,000-line file, `docs/plans/code-lens.md:440-449`).
- **The gutter and inline marks** are worked out for open buffers' lines
  in view only.

### 7.6 Where it sits, and the session

- **Beside the task pane and the debugger.** Task Output opens a
  workspace of its own (`app.rs:10886-10888`); the debugger a dock under
  the panes (`app.rs:11345`). The tests page is a page in the
  pane you're in, like Search; its output (§8.5) opens beside it. A run
  started from a file (`SPC T t`, the lens, the gutter) doesn't open
  the page: the gutter, the modeline and the notification say how it
  went.
- **The session.** Like Fenix's own pages, the page isn't restored:
  its pane comes back as Home (`SESSION_RESTORATION.md:29-30`). The
  results come back from the results file (§4), and `SPC T T` reopens
  the page on the tab, filter and open groups it had, kept per project
  for the session.

## 8. Running

### 8.1 Queue and concurrency

- **One run at a time per project.** cargo locks its build folder, and
  two runs of the same tests would fight over the marks.
- A request while one runs **waits**, the head saying `then: 3 tests`;
  another request joins the one waiting (the union of the two), so at
  most one waits. `f` or `R` while the same run is going does nothing.
- A run's providers go in turn; a provider's steps in order.
- **A test run isn't a task**: it runs beside one in the task pane, and
  beside an extension's background tasks, without taking their slots
  (`host.rs:84-85`, `1924-1934`).

### 8.2 Stopping a run, on Windows and on Linux

Every step runs through `fenix_tasks::TaskRunner::spawn_command`, the
task pane's own supervisor (`supervisor.rs:67-124`), with
`fenix_rpc::resolve_command` for the program so a `.cmd` shim runs on
Windows (`supervisor.rs:62`). The program is started directly, never
through a shell; only a `.cmd` or `.bat` goes through `cmd.exe`, as Rust
runs batch files, with that line's shorter limit (§6.5):

- **Windows**: the step's tree is in a job object; cancelling terminates
  the job, so cargo's test binary, Go's test binaries and Vitest's and
  Jest's workers go with it. A second job with
  `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` kills the tree if Fenix itself
  dies without running destructors (`supervisor.rs:165-210`). No console
  window (`CREATE_NO_WINDOW`, `supervisor.rs:78-85`). There's no
  Ctrl-C to send to a program without a console: a stop is a kill.
- **Linux**: the step leads a process group, killed as a group
  (`supervisor.rs:86-89`). A child that leaves the group (`setsid`, a
  daemon) escapes, as it does from a task today.
- **Both**: a task "owns its descendants even when the leader exits
  normally" (`supervisor.rs:246-253`): a server a test started and
  forgot dies with the step. That's said in FEATURES for tasks
  (`FEATURES.md:1584-1592`) and holds for test runs.

`x`, `SPC T k`, a replacing run, or the timeout cancel the step under
way; its tests and those of the steps not started go back to what they
were (§4), the provider gets `stopped` for each step, and the page says
`stopped`. Cleanup has `TaskRunner`'s two-second deadline
(`supervisor.rs:16`, `269-285`).

### 8.3 Timeouts

`tests.timeout_minutes` (0, none) stops a run that has gone on that
long; its unfinished tests are `errored` "timed out after 30 min", the
step under way gets `timed-out` and those not started `stopped`. It's
the only clock: a step that prints nothing isn't stopped for it, and
no provider is called on a timer (§6.4, "Silence"). An agent's
`run_tests` has its own wait (§10.3), which doesn't stop the run.

### 8.4 Stale, run on save, the failed again

- **Stale**: saving a file marks the tests in it stale (dimmed), and
  its scan adds and removes tests at once. A save elsewhere marks
  nothing: what a change reaches is the runner's to know, not Fenix's.
  Agents' saved edits (`agents.write = "save"`) count as saves.
- **Run on save**: `tests.on_save`, `off` (the default), `file` (the
  saved file's tests) or `failed` (the failed again), 1 s after the last
  save, in a trusted project; a save's run replaces a waiting one. Off
  by default because a Rust or Go run builds, and the packs already run
  clippy or a vet on save in the same build folder, and a run waits
  behind it (the packs' plan, §6.1, "Weak spots").
- **The failed again** (`f`, `SPC T F`): the tests whose last result is
  failed or errored, as one selection. A case is asked for as itself;
  a provider that can't name a case alone runs its template (its plan
  decides).
- **Unsaved files.** A run doesn't save anything, as a task and `SPC p
  r` don't (`app.rs:10860-10903`; `SPC p r` finds the test in the
  buffer, unsaved edits and all, and runs what's on disk,
  `app.rs:10768-10778`). The runner reads the disk; the ask's lines can
  be the buffer's (the newest scan), so a provider names tests to its
  runner by name where it can, not by `file:line` (Vitest, the packs'
  plan §6.4). When a file in the selection has unsaved edits, the head
  says `src/lib.rs has unsaved edits: the runner reads it as last saved`, and a
  test of that file the run ends without a result for goes back to what
  it was, not `errored`, with the same reason (a test typed and not yet
  saved). Whether to offer to save first is Q8 (§16).

### 8.5 Output, build failures and test failures

- A run's output goes to a buffer of its own, *Test output*, of the
  kind Task Output is (`BufferKind::TaskOutput`), so `Enter` on a line
  that names a place goes there (`app.rs:11040-11062`). Each step starts
  with its command line and label, as a task's `$ …` header
  (`app.rs:10876`). `o` and `SPC T o` show it beside the page; it's never
  in the task pane and never clears the quickfix list.
- **Didn't build, didn't load** (`broken`): never test failures. The
  tests under it that have no result in the run yet are `errored` with
  the message (§19, finding 7); the head says the first;
  no problems are set for it: the language server and the pack's linter
  have them already, and twice is noise.
- **Failed**: a problem at its place (§9.3).
- **Crashed**: what's under the `crashed` report's `id` (a target, a
  package) and has no result in this run yet is `errored` with what the
  provider read ("its binary crashed: SIGABRT"); with no `id`, what the
  step covers. `cargo test --workspace --no-fail-fast` goes on to the
  next binary after one aborts, and `go test ./...` to the other
  packages, so a crash scoped to its binary leaves the tests of the
  binaries not yet started to their own results. A later `ended` for an
  errored test replaces it (§4).
- **What a step covers**: the ids in its `covers`, a group's, file's or
  suite's standing for every node Fenix has under it, and what it finds
  under them during the step. A step with empty `covers` claims nothing
  of its own. What of the ask no step's `covers` names is the plan's:
  it goes running when the first step with empty `covers` starts, and
  is judged when the plan's last step has ended. So a plan of one step
  per package, each covering its package's group, judges each package
  as its step ends, and a plan whose steps say nothing judges the whole
  ask at its end, never at its first step's.
- Tests a step covered, or the plan's rest, that ended without a
  result: if the step was stopped, back to what they were; if it
  exited, `errored` "the run ended without a result for it", except a
  test of a file with unsaved edits (§8.4).

### 8.6 Remote projects

| | In the SSH preview today | When remote Phase 4 (tasks) lands |
|---|---|---|
| Scanning | Works: Fenix gives the text (buffers through `fenix-vfs`); the provider runs here | The same |
| The page, results kept, gutter, problems | Work, from scans | The same |
| Listing and running | "Not available on a remote project yet", as every unmoved feature says (`remote-ssh.md:563-565`) | Through the stand-in, which `TaskRunner::spawn_command` takes as it is and whose kill ends what it ran (`remote-ssh.md:372-377`). A step's `needs` are copied there before the step, every time (a file of ids written for this run is new each run), each keeping its path below `/data`: `/data/lang-kit/tests/fenix_tests.py` lands at `~/.cache/fenix/tests/<extension>/lang-kit/tests/fenix_tests.py`, and `${data}` in the step is `~/.cache/fenix/tests/<extension>`, so `${data}/lang-kit/tests/fenix_tests.py` names it on both machines. Places in the output are the machine's paths, mapped as its language servers' URIs will be (`remote-ssh.md:363-370`). |
| Debugging a test | Not available | With the debugger's Phase 4 |

The provider stays local throughout: it never touches the remote
machine, which is what (A) buys (§6.1).

## 9. In the file

### 9.1 The gutter

Today the gutter holds, left to right: the Git change bar (3 px, at
2 px), the debugger's square for a breakpoint or the stop's bar (half a
line, at 8 px), then the line numbers, the fold column (a file's, with
line numbers and `editor.fold_markers` on), and the Ember Trail's stripe at its inner
edge (`app.rs:22259-22339`; widths in `gutter_chars`, `app.rs:18772-18806`,
the fold column at `18805` and `app/editor_ui.rs:186-191`).

A file whose provider found tests in it gets **one more cell, the test
column**, on the left of the line numbers, the way the fold column is
one more cell: on the line where each test or group starts, its state's
glyph (`✓ ✗ ! ○ ● · ?`) in its colour, dimmed when stale. It's there as
soon as the scan finds a test, before the file is first drawn, so text
doesn't move when the first result comes. Breakpoints and the Git bar
keep their places. `tests.gutter` (on) turns the column off.

- A click on a mark runs that test or group; a right click adds *Run
  test*, *Debug test* and *Show in Tests* to the menu, beside *Toggle
  breakpoint* (`app/mouse.rs:791`).
- `SPC T t` and `SPC T d` on the line do the same from the keyboard.
- The overview ruler shows failures as the problems they are
  (`FEATURES.md:1677-1680`).

### 9.2 After the line

Decorations Fenix owns, `fenix.tests`, as inlay hints are
`fenix.inlay_hints` (`app/inlay_hints.rs:24`; `docs/plans/inlay-hints.md:12-21`):
after a test's first line, a failed or errored one's ` ✗ assertion
failed: two and two` (cut at 48 characters, in bad), and with
`tests.inline = "all"` a passed one's ` ✓ 3 ms` (muted). `tests.inline`
is `failed` by default (`all`, `failed`, `off`): the gutter already
says a test passed.

### 9.3 Failures as problems

- A failed test's failure is a problem (an error) at its `at`, else at
  the test's own line. Its source is the provider's runner (`cargo
  test`), its code the test's label.
- **Its own list.** Fenix keeps problems per publisher and path, each
  publisher's replacing only its own (`fenix-lsp/src/fanout.rs:348-366`),
  an extension's under `<ext>:#problems` (`app/wasm_host.rs:433-437`,
  `2209-2266`). The explorer publishes under its own key,
  `LspServer::Additional("fenix:#tests")` for the project
  (`app/tool_sessions.rs:7-19`). So a pack's linter (`diagnostics-set`)
  and the explorer can never wipe each other's: the first design's
  problems ledger in lang-kit isn't needed.
- **Cleared** when that test runs again, whatever the result; when its
  file is saved (the line may not be the line any more); when the test
  is gone from the model; when `tests.problems` is turned off.
- A place outside the project (a library's frame) puts it at the test's
  own line. A helper's assertion failing in two tests is two problems,
  each with its test's name.
- No fixes.

### 9.4 The test the cursor is in

`tests.run_here` and `tests.debug_here` take the focused file's tests
from the model (scanned from the buffer, so a new, unsaved test counts
once scanned) and pick the innermost test whose lines hold the cursor
(`line` to `end-line`); with no `end-line`, the last that starts at or
above the cursor within its parent's lines. On a group's first line,
that group. With two providers in the file, the innermost of all their
tests (§4, "Two providers in one file"). In no test: "line 12 of geo.rs isn't in a test — SPC T f
runs this file's".

### 9.5 The lens

With `lsp.code_lens` on, Fenix's own lens (`app/code_lens.rs:218-246`)
comes from the model for a file a provider serves: "▶ Run test |
Debug" over each test and group, running `tests.run_here` and
`tests.debug_here` at its line, under the same rule that a lens starts a
program only in a trusted project (`app/code_lens.rs:648-655`).
rust-analyzer's run and debug lenses on such a line do the same (§2).
`lsp.code_lens_tests`' description changes to "over each test the
file's test provider found, or Fenix finds (a Rust #[test], a pytest
test, a Go TestXxx)".

## 10. Telling you

### 10.1 The modeline and the notification

- **The modeline** has Fenix's own item, as background work has
  (`FEATURES.md:1664-1665`): `tests 12/40 ✗ 1` while a run goes; `✗ 3
  tests` after one with failures, until they pass; nothing when all
  passed, so a green project is quiet.
- **One notification per run**, under `notifications.tasks` (all,
  long, failed, never, `schema.rs:525`), whose description becomes
  "when a task or a test run ends". It's `task_notify`'s rule and shape
  (`app/notifications.rs:366-398`) with the source `tests` (so
  `notifications.rules` can mute it apart, `schema.rs:523`) and the tag
  `tests`:

| When | Summary | Body | Urgency | Actions |
|---|---|---|---|---|
| A run ended, as `notifications.tasks` says | `Tests passed` / `3 tests failed` | `41 passed in 1 min 12 s` / `tests::case_negative and 2 more · 41 passed` | low / normal | *Show the failures* (`tests.open`), *Run the failed again* (`tests.run_failed`) |
| A target didn't build | `The tests didn't build` | its first error | normal | *Show it* (`tests.output`) |

A test run isn't a task in the task pane, so no task notification comes
for it; a task that runs tests (`SPC p t` → `cargo test`) keeps its own.
There are never two for one run.

### 10.2 `last_task`, and extensions that react to tests

When a test run ends, Fenix sets `last_task` as `finish_task_run` does
(`app.rs:11116-11120`), with the name `tests: <what ran>` (`tests:
src/lib.rs`, `tests: the failed`), `ok` true when nothing failed or
errored, and `tests` the model's exact `(passed, failed)` (failed counts
errored). Extensions that ask for `tasks` hear `task-started` when it
starts and `task-ended` when it ends, as for a task
(`app.rs:10899`, `app/wasm_host.rs:1120-1121`). So Pets, which reacts to
tests through `last_task` when a task ends (fenix-extensions
`extensions/pets/src/lib.rs:304-305`; a party from enough passing tests,
`src/brain.rs:652-661`), reacts to test runs with no
change. The debugger's `preLaunchTask` wait (`app.rs:11126`) is not told:
a test run never starts a debug session waiting on a task. Q1 in §16
asks whether to do this at all.

### 10.3 The agents' tools

In the `tasks` toolset (`agent_tools.rs:126`), whose description already
says "tasks and tests", offered when the project has tasks or a test
provider:

| Tool | Class | Input | Answers |
|---|---|---|---|
| `tests` | read | `path?`, `state?` (`failed`, `errored`, `not_run`, ...), `filter?` | Each test: `provider`, `id`, `path`, `line`, `state`, `durationMs`, `stale`; at most 500, saying how many more |
| `test_failures` | read | `path?` | Each failed or errored test: `provider`, `id`, its place, message, and text (cut at 4 KiB each, `agents.mcp.max_answer_kib` in all) |
| `run_tests` | run | `tests?` (`[{provider, id}]`), `path?`, `failed?`, `all?`, `timeout?` | Waits for the run (10 minutes by default, as `run_task`, `app/agent_toolsets.rs:19`), then the counts and the failures as `test_failures`; past its time, "still running" with what's in |
| `run_test` | run | `path`, `line`, `timeout?` | As now, but where a provider serves the file it runs through the explorer and answers `{ "test": {provider, id}, "state", "durationMs", "message", "at", "text", "output" }` (output's end, 4 KiB) instead of the task's report |
| `debug_test` | run | `path`, `line` | In the `debug` toolset: debugs the test there (§11) and answers at the first stop, as `debug_start` does (`agent_tools.rs:202`) |

Their descriptions stay under 300 characters, and the default list is
checked against ADR 0003's 12 KiB budget (decision 5). `run_tests` and
`debug_test` are asked about by default, as every `run` tool is
(`AGENTS.md`, "What they may do"). Answers name only the project's tests
and what they printed.

### 10.4 The control channel

Beside `fenix/tasks` and `fenix/tasks.run` (`CONTROL.md:375-385`):

- `fenix/tests`: params `{ "path"?, "state"? }`. `{ "root", "tests": [{
  "provider", "id", "label", "kind", "path", "line", "state",
  "durationMs", "stale" }] }`, at most 5,000.
- `fenix/tests.failures`: params `{ "path"? }`, as `test_failures`.
- `fenix/tests.run`: params `{ "tests"?, "path"?, "failed"?, "all"? }`:
  starts a run and answers `{ "run" }` at once. Refused (`-32602`) unless
  `agents.mcp.fenix_write` is on, as `fenix/tasks.run` is.

MCP clients reach the tools through `fenix/agent.call` as every tool
(ADR 0003, decision 1).

## 11. Debugging a test

1. `tests.debug_here`, `d` on the page, a lens's *Debug* or `debug_test`
   opens the test's file at its line (`debug_start_from` takes the
   adapter from the focused buffer's language, `app.rs:11238-11241`).
2. Fenix calls `tests-launch(provider, run, test)`:
   - `launch(config)`: a `launch-config` as `debug::launches` gives one
     (`world.wit:930-946`), turned into a `dap::Template`
     (`dap.rs:154-175`) as the extensions' launches are, and started with
     `debug_start_from` (`app.rs:11231-11298`): its variables filled in,
     its `preLaunchTask` run first, then the session. No picker.
   - `build-first(plan)`: the plan runs as a run's steps, as the run
     `tests-launch` was first asked with -- Fenix draws that id from the
     host's one count before the first ask -- (in *Test output*, read by
     the provider, so it learns where the binary is), then `tests-launch`
     is asked again with the same run, once, and
     only if every step `exited` with 0 and nothing was `broken` or
     `crashed`. Otherwise no session starts: the modeline and the page's
     head say `orbit (lib) didn't build: src/lib.rs:19 mismatched types
     · o shows it` (the first `broken`'s message), or the step's end,
     and *Test output* has the rest. A second `build-first` from the
     second ask is taken as a refusal ("it asked to build twice").
     `ended` reports in a build-first plan are ignored: it runs no test.
     Rust: `cargo test --no-run --message-format=json` for the test's
     target, whose `compiler-artifact` names the binary.
3. The session is the debugger's as any other: breakpoints, DEBUG, the
   drawer, the Ember Trail.

It **isn't remembered** as the project's last launch
(`debug_last_launch`, `app.rs:11174`, `11289`), so `SPC u u` doesn't
start the test again next time: `SPC T d` does. A project with its own
launch in `.fenix/tools.json` still debugs a test this way: that launch
only takes the place of the extensions' launches on `SPC u u`
(`app.rs:11169-11185`).

What each pack gives (the packs' plan, §5, "Debugging", and its §6
briefs): Rust a launch of the test binary with `[name, "--exact",
"--nocapture"]` after a build; Python debugpy with `module = "pytest"`
and the node id; Go Delve's `mode = "test"` with `-test.run
'^TestTable$/^negative_one$'`; Vitest `run <file>:<line>`, Jest
`--runInBand --watchAll=false <file> -t '^<name>$'` under js-debug. The
first design's workaround through `debug.pick_launch` and a launch
variable ("It's fragile", which the packs' plan §5 recalls) isn't
needed.

## 12. Trust

A listing runs project code: pytest imports the test modules, Vitest
and Jest load them, cargo and Go build them (build scripts, `go
generate` aren't run, but `build.rs` is). A lens may start a program
only in a trusted project (`app/code_lens.rs:648-655`; `SPC p y`,
`app/trust.rs:1-17`). So:

- What runs **without a key** — the automatic listings (§7.1), run on
  save — runs only in a trusted project. In one that isn't, the page
  says "u lists them (runs pytest --collect-only), once you trust this
  project: SPC p y".
- What you ask for with a key or a click (`r`, `SPC T t`, the gutter)
  runs, as `SPC p r` runs today. The lens keeps its own rule.
- An agent's `run_tests` is held to the agents' rules (§10.3).

## 13. Settings

In "Completion & LSP" (`Category::Completion`, `schema.rs:121`), where
the debugger's are (`schema.rs:476`). No `tests.` setting exists
(`schema.rs`, `grep '"tests\.'`).

| Setting | Kind | Default | |
|---|---|---|---|
| `tests.enabled` | bool | off until step 8, then on | The test explorer: the page, the gutter column, results in the file, `SPC p r` and the lens through it. Off: every door as in 1.5. |
| `tests.list_automatically` | bool | on | List a project's tests with its runner when the page opens and after a run that built, in a trusted project. Off: only on `u`. |
| `tests.on_save` | choice `off`, `file`, `failed` | `off` | Run tests when a file is saved. |
| `tests.problems` | bool | on | Failures as problems at their place. |
| `tests.inline` | choice `all`, `failed`, `off` | `failed` | Results after a test's line. |
| `tests.gutter` | bool | on | The test column in the gutter. |
| `tests.timeout_minutes` | int 0–1440 | 0 | Stop a run that has gone on this long; 0, never. |
| `tests.keep_results` | bool | on | Keep the last results across restarts (shown stale). |

`notifications.tasks` and `notifications.rules` change only in their
descriptions (§10.1). Commands: `tests.open`, `tests.run_here`,
`tests.run_file`, `tests.run_all`, `tests.run_failed`, `tests.run_last`,
`tests.debug_here`, `tests.stop`, `tests.output`; no `tests.` or
`test.` command exists (`commands.rs`; the `test.*` ids in the tree are
test extensions', `test.hello`), and no extension in fenix-extensions
claims a `tests` namespace.

## 14. Testing it

How Fenix tests features of this kind, and what this one needs:

| How Fenix does it | Where | For this |
|---|---|---|
| Pure crates, unit-tested | `ARCHITECTURE.md:3-5` | `fenix-tests`: the tree from scans and listings, a `found` merged per source and each source's dropping rule (a group several scans give; a listing that isn't complete), matching (an unknown id; a cut id), the last `ended` winning, every state change, groups' counts, what a step covers (group ids, empty `covers`, the plan's rest), a crash's scope, an ask's files and suites filled in and coarsened past 10,000, two providers in one file, the queue (wait, join, replace), stop and timeout, unsaved files, staleness, results file round trip, 10,000 tests timed |
| The host against real guests, committed as `.wasm` so tests need no wasm toolchain | `fenix-wasm/tests/host.rs:1-2`, `tests/guests/build.ps1` | A new guest, `tests/guests/provider`, a provider of a made-up runner: scans `# test name` lines, plans `fake-test-runner` with arguments, reads its `PASS name 3ms` lines. Tests: the exports found; a 1.5 guest loads with none; a manifest with providers and no exports is told; deadlines; batches; a trap mid-run. And the other way, a guest built on the 1.6 crate, `api = "1.5"`, no provider (`tests/guests/plain16`), instantiated against 1.5's world: a copy of `v1.5.2`'s `wit/` under `fenix-wasm/tests/wit-1.5/`, a `bindgen!` of its `extension-base` in the test, its `Linker` as `host.rs:3755-3757` makes it, then `activate` and a command called (F15, §6.2). It stays, for every later minor against the one before |
| A fake of the external program as a binary of the workspace | `fenix-lsp`'s `fake-language-server` (`fenix-lsp/src/bin`) | `fake-test-runner` in `fenix-tasks`: prints a script's lines with delays, starts a grandchild that holds the pipes (the cargo case), writes a line that isn't UTF-8, a 100 KiB line, 100,000 lines, exits by a signal, is silent for a while. With it: a stop mid-batch (the call under way finishes, then `stopped`); a command line over 32,766 characters as Windows quotes it, refused on Linux too; a `needs` that isn't there; `list-after` listing after; `test-provider-set` with the same answer listing nothing, and `""` hiding the provider; the extension started again in another project stopping its run |
| Tree-kill tests | `fenix-tasks/src/tree.rs:80-117` (Unix only today) | The grandchild gone after a stop, **on Windows too**: the job object path has no such test in `tree.rs` |
| Recorded protocol fixtures | `fenix-dap/tests/adapters/*.json` | None for runners: runners' formats are the packs', and their fixtures are the packs' (the recorded outputs, fenix-extensions `plans/test-explorer-fixtures/`) |
| The app headless, pages read as text | `page.rs:1-7`; `app.rs` tests (`run_test_at_runs_the_test_a_line_is_in`, `app.rs:32200`); `app/agent_toolsets.rs` `call(...)` | The page at 100×30 and 80×24 (empty, running, failures, a build that failed, filtered, 10,000); `SPC p r` and the lens routed or not by whether a provider serves the file; the gutter column and marks; problems under `fenix:#tests` beside an extension's `diagnostics-set` for the same file, neither wiping the other; the notification once; `last_task`; the agent tools and control methods |
| Both key schemes | `app/standard_tests.rs` (ADR 0004, consequences) | The page's keys and `SPC T` / `Ctrl+K T` in both |
| The keymap's conflict test | `keymap.rs:776-818`, `1059` | `SPC T` added with no conflict |
| The tool list's budget | ADR 0003, decision 5 | Re-run with the new tools |

What only a real Fenix shows, for the nightly: the feel of results
coming in, a stop on Windows while cargo's test binary runs, the
gutter's column beside breakpoints and folds, and 10,000 real tests.

## 15. The steps

Sizes as Fenix's plans write them: S a few days, M one to two weeks, L
more. Each step is a pull request with its tests, leaves `master`
releasable, and changes nothing a user sees while `tests.enabled` is
off (it doesn't exist before step 3; it's off until step 8).

| # | Step | Size | Crates | Risk | Verified by |
|---|---|---|---|---|---|
| 0 | This plan and ADR 0005 | S | docs | — | Review |
| 1 | **The model**: `fenix-tests` (§4), pure, no UI | M | `fenix-tests` (new) | Low | Its unit tests |
| 2 | **API 1.6**: the WIT (§6.3), `[[contributes.test_providers]]` and its checks, `API = "1.6.0"`, the `tests` module and `Extension`'s methods, the host's `Optional`, `Call`s and `TestProviderSet`, the provider guest, `WRITING_EXTENSIONS.md`'s section and its "This Fenix offers" line | M | `fenix-extension-api`, `fenix-wasm`, `fenix-extensions` | Medium: what's merged here is what packs build against | Guest tests in `fenix-wasm`; manifest tests; a 1.5 guest still loads; a 1.6-built guest with `api = "1.5"` instantiates against 1.5's world (§14), its result written in §6.2; the WIT checked against §18's table, and against lang-kit's mirror of it (E1b compiles with no change to E1a) |
| 3 | **Fenix runs providers**: `app/tests_host.rs`: scans (the project walk through its ignores), listings (`list-after`, `test-provider-set`), runs through `TaskRunner` (and its `cut` flag), a step's command line and `needs` checked, batching, reports into the model, stop (mid-call, and on the instance starting again), timeout, the queue, unsaved files, *Test output*, results kept; `tests.enabled` (off) and the commands, unbound | L | `fenix-gui`, `fenix-tasks`, `fenix-config` | High: the lifecycle, Windows kill | The guest and `fake-test-runner` in app tests; tree kill on Windows; 100,000 lines through a provider, timed |
| 4 | **The page**: `tests_page.rs`, `PageModel::Tests`, its keys, `SPC T`, the modeline item, tutorial and focus names | M | `fenix-gui` | Medium | Pages as text at both sizes; both schemes; keymap conflicts |
| 5 | **In the file**: the gutter column, inline results, problems under `fenix:#tests`, the right-click items, staleness | M | `fenix-gui` | Medium: the gutter's width | Problems beside an extension's; gutter widths and hits; a frame's cost |
| 6 | **Debugging a test**: `tests-launch`, `build-first`, `debug_start_from` without the picker or remembering | S | `fenix-gui` | Medium | App tests with a recorded adapter |
| 7 | **One story**: `SPC p r`, `SPC p T`, the lenses (Fenix's and rust-analyzer's), `run_test` through the explorer where a provider serves the file; `last_task` and `task-started`/`-ended`; the notification; `tests`, `test_failures`, `run_tests`, `debug_test`; `fenix/tests*` | M | `fenix-gui`, `fenix-control` | Medium: changes doors people use | Routed and fallback paths both; the tool budget; the notification once |
| 8 | **On**: `tests.enabled` defaults on, run on save, trust gating checked, the 10,000-test measurement, FEATURES, KEYBINDINGS, CONFIGURATION, AGENTS, CONTROL, CHANGELOG, "As built" here | S | `fenix-gui`, docs | Low | The nightly, with a pack, for a few days (`RELEASING.md:38-41`) |
| 9 | **Remote**: runs through the stand-in, `needs` copied | S | `fenix-gui` | After remote Phase 4 | Against `fake-ssh` and a real `sshd` (`dev/remote-ssh/check.sh`) |

```text
0 ─ 1 ─┬─ 3 ─┬─ 4 ─┬─ 7 ─ 8
       2 ────┘     ├─ 5 ─┤
                   └─ 6 ─┘          9 after remote Phase 4
```

- **Step 2 is the first a pack can build against**, though the packs
  start before it, on §6.3 and §6.6 as written here (E1a). It can start
  in parallel with step 1, since the API doesn't depend on the model.
  From step 2 to the last, nothing a pack sees changes: the WIT, the
  manifest section and the Rust module stay as merged, unless a pack's
  work finds a mistake before Fenix 1.6.0 ships (§6.2). What a pack
  can test before step 3 is its own code, on the test kit's driver;
  from step 3, in the nightly with `tests.enabled` on.
- Steps 4, 5 and 6 can go in parallel after 3; 7 after 4.

### In fenix-extensions

The packs' plan's §8.1 has the pieces and their owners; this is them as
they line up with Fenix's steps.

| # | Step | Size | Needs | Owns | Done when |
|---|---|---|---|---|---|
| E1a | **lang-kit's adapter, on `main`**: the `TestRunner` trait (the packs' plan, §4.2), `Kit`'s five methods, readers per step, cut lines joined, groups' ids, what it remembers, `split` for long command lines, assets and `needs`, the runner told on a change; the test kit's driver, a stand-in for Fenix's side with this plan's rules; all against `lang-kit/src/tests/api.rs`, a copy of §6.3's types as §6.6 writes them | M | Nothing of Fenix's: this plan's §6.3 and §6.6 | `crates/lang-kit`, `crates/test-kit` | A made-up runner passes through every rule of the adapter and the driver; every lang-kit pack's tests pass unchanged |
| E1b | **On the branch `api-1.6`**: the kits' `fenix-extension-api` pinned by `rev` to step 2's merge commit, the 30 locks moved, the mirror replaced by `pub use fenix_extension_api::tests::*`, `test_exports!()`, the harness's `tests_*`, `xtask validate` checking `[[contributes.test_providers]]` as §5 does | S | Fenix step 2 merged (or its pull request's head) | the kits' `Cargo.toml` and locks, `xtask` | The branch's CI passes, the mirror compiled away with no change to E1a's code (a difference is a finding for step 2) |
| E2 | **Rust** (cargo test's human lines and cargo's JSON, one step per package, nextest's libtest JSON when installed, `build-first` launches) and the recipe for the others | M | E1a; its manifest and exports on the branch after E1b | `extensions/rust`, `plans/test-explorer-recipe.md` | The packs' plan §6.1's 1 to 6 on the driver; its 7 in a nightly from Fenix step 3 (debugging from step 6) |
| E3a | **Python** (the pytest plugin, `uv run` or not, `PYTHONIOENCODING=utf-8` for Windows' pipes) | S | E2's recipe | `extensions/python` | Its brief's, on the driver; then in a nightly |
| E3b | **Go** (scan, `go list`, `go test -json -fullpath`, build events) | S | E2's recipe | `extensions/go` | Its brief's, Go before 1.24 not verified |
| E3c | **Web** (Vitest and Jest, both reporters, one provider) | M | E2's recipe | `extensions/web` | Its brief's, for each runner |
| E4 | **Releases**: the kits to `tag = "v1.6.0"`, the branch merged, Rust, Python, Go and Web 1.3.0 | S | Fenix 1.6.0 tagged; E2, E3a-c | the kits' pins, every lock, the four packs | The packs' plan, §9 |

- **Against an unreleased API** (the packs' plan, §8.2): until step 2,
  everything but the exports and the manifests is on `main`, against
  the mirror, every pin still `v1.5.0`. From step 2, the branch
  `api-1.6`, pinned by `rev` (not a branch, so the lock says what was
  built); a change to the API before 1.6.0 moves the `rev`, in one
  commit. At 1.6.0, the pins move to the tag and the branch is merged.
- E1a starts now; E2 beside it once E1a's trait is merged as a stub.
  E3a, E3b and E3c in parallel after E2's recipe.
- What goes from the first design's work list: its problems ledger and
  `Host` growth (Fenix owns problems, decorations, status, launches),
  most of its model, page, store and agents, its issues for Fenix (tree
  kill, `debug::start`, gutter marks: all in this plan), and the `SPC m
  t` group with its D6 (Go's `tidy` keeps `t`).
- Releases: each of the four one minor with `api = "1.6"`, `fenix =
  ">=1.6, <2"`, published once Fenix 1.6.0 is out. No major for any
  pack: no pack key moves. The 24 kit users that run no tests aren't
  released for it (the packs' plan, D-A2); whether their next release
  can keep `api = "1.5"` is step 2's test (§6.2).

## 16. Open questions for the maintainer

| | Question | Options | Recommended |
|---|---|---|---|
| Q1 | Does a test run count as a task for extensions (`last_task`, `task-started`, `task-ended`)? | (a) yes, named `tests: …` (§10.2); (b) a new export for extensions that want test runs (`tests-ended`, more API); (c) no | **(a)**: Pets and the like react with no change. The cost: an extension that treats every task end as "the build finished" sees test runs too. None in fenix-extensions does today, but it should be said in `WRITING_EXTENSIONS.md`. |
| Q2 | When a provider serves a file, do `SPC p r`, the lens and `run_test` go through the explorer? | (a) always; (b) a setting to keep them on the task pane | **(a)**: two ways to run the same test with results in two places is what this plan exists to end. With `tests.enabled` off they're as in 1.5. |
| Q3 | The leader group | (a) `SPC T`; (b) under `SPC p` beside `SPC p r` (`SPC p e …`, an HTTP-extension user's `SPC p r` is already taken); (c) `SPC m t` per pack, as the first design had | **(a)**: one group in every language, free everywhere, and nothing per pack; (c) is what made Go's `tidy` move in the first design. |
| Q4 | Keep results across restarts? | (a) per project in the state folder, shown stale; (b) not at all | **(a)**: a restart shouldn't forget what failed. It's off with `tests.keep_results`. |
| Q5 | API numbering against the remote plan's "API 1.6" | (a) the first to land takes 1.6; (b) reserve 1.6 for remote | **(a)**, and the remote plan's line is changed when this lands. |
| Q6 | Readers in Fenix for standard formats (JUnit XML, TAP, `go test -json`) | (a) no; (b) later, as a new export a plan can name; (c) in 1.6 | **(b) at most, later**: a pack's reader is short, and every runner's quirks are better in the pack. A ctest or Gradle provider might want JUnit; it can read it a line at a time, as a step of its own that prints the file, without parsing it whole in one call. |
| Q7 | Fenix's built-in `SPC p r` finder once the four packs ship | (a) keep as the fallback; (b) retire it | **(a)**: it's small, and it serves someone without the pack. |
| Q8 | Does a run save the files it runs first? (F18) | (a) no, as a task and `SPC p r` don't; the head says which were unsaved, and a test the run didn't see goes back to what it was (§8.4); (b) `tests.save_before_run`, `ask`, `always` or `never`, as `agents.save_before_prompt` is (`schema.rs:492`); (c) always save the selection's files | **(a)** for 1.6: it's what every door does today, and `editor.auto_save` (`schema.rs:422`) already serves who wants saving. (b) is a setting and no API, so it can come in a minor if the head's line proves to be read too late. |
| Q9 | What "trusted" means for what runs without a key (§12): automatic listings and run on save run where `project_is_trusted` allows, which is `Verdict::Nothing` (a project naming no programs, a fresh clone) as well as `Trusted`, as for the lens's programs (`app/code_lens.rs:648-655`). A save in a never-trusted repository can then run `cargo test` or `pytest` (`build.rs`, `conftest.py`) with `tests.on_save` on | (a) as now: `Nothing` counts, as for lenses; (b) only `Trusted`: a project's own "may run tests on its own" decision (`SPC p y`), asked once; (c) (b) for run on save, (a) for listings | To be ruled before step 8 ("trust gating checked"). Step 3 keeps (a), pinned by `what_runs_without_a_key_runs_only_in_a_trusted_project`; (b) changes one function (`tests_trusted`). Raised by the review of step 3, S4 |

## 17. Risks

- **The API is frozen early.** Its shape is merged at step 2, before
  a real pack has run on it in Fenix. Mitigation: the packs' author
  reviewed it against five runners' recorded output before step 2 (§18),
  the packs build against it on a branch pinned to step 2's commit
  before Fenix 1.6.0 (E1b), and nothing is published before then
  (§6.2). A mistake found after the release is a new export, forever.
- **A 1.6-built component on Fenix 1.5** is read to load (§6.2), not
  yet seen to. If step 2's test shows it doesn't, the 24 kit users that
  run no tests either raise their floor to `api = "1.6"` at their next
  release, or stay on the 1.5 crate until they do; the packs' plan's
  D-A2 waits on it, and nothing is released on the assumption.
- **Per-call cost in wasm** isn't measured: 200 calls of 512 lines for a
  100,000-line run is an estimate. If it's slow, batches grow (the API
  doesn't fix their size) before anything else changes.
- **The gutter's width.** A cell more in every file a provider serves
  shifts its text one column, once, compared with 1.5. `tests.gutter`
  turns it off; if users mind, the fallback is the debugger's slot,
  breakpoints winning.
- **Windows' ordering of stdout and stderr** is per 10 ms tick, not per
  write (`supervisor.rs:322-325`). Providers read one stream; a runner
  that splits its story across both (cargo's `Running` on stderr, its
  tests' lines on stdout) can't use the order between them; the Rust
  pack pairs each binary's stdout segment with the stderr header of the
  same rank, each stream's own order being kept (the packs' plan,
  §6.1, "Which binary a line is from").
- **Python's output on Windows.** Writing to a pipe, Python before 3.15
  uses the ANSI code page, not UTF-8, unless told, so pytest's own text
  in *Test output* (a non-ASCII test name in its summary, what a test
  printed) arrives replaced. What the provider reads doesn't: the
  pack's plugin writes its lines with `json.dumps`, escaped ASCII (the
  packs' plan, §3, "Checked again"). The Python pack sets
  `PYTHONIOENCODING=utf-8` in its steps for the rest (E3a). **Not
  verified on Windows.**
- **Listing runs code** (§12). Gated on trust, and said in the packs'
  READMEs, as other editors' explorers do the same.
- **cargo's lock**: a run waits behind rust-analyzer's or clippy's build
  ("Blocking waiting for file lock"). The head says what the step's
  output says; nothing to fix in Fenix.
- **Two paths for one test** remain in the code: the explorer and the
  `SPC p r` fallback. They never apply to the same file (§2), but both
  need tests.
- **fenix-gui grows.** `app.rs` is 39,000 lines; the host goes in
  `app/tests_host.rs` and the page in `tests_page.rs`, the model in its
  own crate, as the agents' and the debugger's are.

What would make it not worth building: if the packs can't keep ids
stable between scan, listing and run for Rust and Python, the two the
first design found hardest (macro-made tests, `parametrize` ids), then
matching, staleness and "run the failed" all degrade to a task with
nicer marks, and Task Output plus `SPC p r` is most of that already.
The first design's samples say they can for every `#[test] fn` and
every node id; macro-made Rust tests are "found in a run". And if few
people run tests from the editor at all, the cost is a page, a crate
and an API minor kept for 1.x. The agents' tools are the part most
likely to be used either way.

## 18. Review, 13 November 2026

The packs' plan (§11, "For Fenix's plan") reviewed this plan as the
implementer of a provider, with five runners' recorded output in hand,
and raised F1 to F19. Each was checked against this plan, Fenix's source
and, for a runner's behaviour, the recorded outputs (fenix-extensions
`plans/test-explorer-fixtures/outputs/`). This is what the packs' side
checks its copied types against.

### The WIT, against the version reviewed

`git diff` of §6.3 against `HEAD` before this revision (`1dfae56`, the plan as `354ea08` committed it):
four changes, all in `interface types`. No export or import changes its
name, its parameters or its result; the world's `use` line is the same;
no case of `event`.

| Record | Change | For |
|---|---|---|
| `found-test` | **New field, last**: `data: option<string>` | F6 |
| `test-plan` | **New field, last**: `list-after: bool` | F7 |
| `test-crash` | **New record**: `id: option<string>`, `message: string` | F4 |
| `test-report` | **Case changed**: `crashed(string)` is `crashed(test-crash)` | F4 |

Everything else in §6.3 changed in its doc comments only, which define
behaviour the types already carried: `found-test.id`'s example and
`label`'s limit; `found-test.path` and `test-location.path`'s relative
forms (F10); `test-ask.run`'s files and suites filled in, and its limit
(F2); `test-step.command`'s length (F11), `covers` (F1), `needs` (F13);
`test-step-end.exited` on Windows and `stopped` by a restart (found in
revision, F12); `test-ended` (F5); `test-report`'s order; `build-first`
(F19); `test-provider-set`'s name of the manifest section, its limits,
"nothing to run here" and when it lists (F8, F17); `tests-scan`'s path
(F10). The Rust side (§6.6) follows: `Found.data`, `Plan.list_after`,
`Report::Crashed { id, message }`.

### The findings

| | Finding | Ruling | Why | Where |
|---|---|---|---|---|
| F1 | `covers` can't name a group: the first step claims the whole ask | **Accepted, and one more rule** | A group's, file's or suite's id in `covers` stands for what's under it, as proposed; and an empty `covers` now claims nothing of its own, the ask's unnamed rest being judged when the plan's last step ends, so even a plan whose steps say nothing never errors at its first step's end. No field: `covers` was already a list of ids | §6.3 `test-step`; §8.5 |
| F2 | Asking for a group sends the group alone; libtest's filter is a substring | **Accepted, bounded** | Fenix adds what it has under each asked `file` or `suite`, never under a `group` (a target or package is named as itself), within a new limit of 10,000 records an ask, which the ask lacked; past it the provider's own filter, which runs at least those, is right | §4 "What the provider is asked"; §6.3 `test-ask`; §6.4; §6.5 |
| F3 | What a `found` for a known id does | **Accepted, with precedence** | Each node keeps a record per source (scan, listing, run); fields fall through `none` from scan to listing to run, except `skip`, which is the scan's where it has the node; each source drops its own. Also defines a group that several scans give, which the dropping rule didn't | §4 |
| F4 | `crashed` has no scope | **Accepted** | `cargo test --workspace --no-fail-fast` goes on after a binary aborts (the abort recorded in `outputs/rust/crash-json.stderr`; the next binary's run is cargo's documented `--no-fail-fast`, not in the recordings), and `go test ./...` runs other packages. `crashed` carries `test-crash { id, message }`, `id` as `test-broken`'s | §6.3; §8.5 |
| F5 | A second `ended` for one id | **Accepted** | The last `ended` in a run is the result, counts and problems following it. pytest's case is read from the plugin, not recorded: `pytest_runtest_logreport` reports a teardown that failed after the call's pass, two `result` lines for one id (`reporters/fenix_tests.py`; the recordings have only setup failures, `outputs/pytest/run-plugin.stdout`) | §4 "A result"; §6.3 |
| F6 | `found-test` carries nothing of the provider's | **Accepted** | Concrete: a pytest id over 1 KiB can't be named to pytest, a describe named `a > b` can't be split back. `data: option<string>`, 4 KiB, given back in `run` and `tests-launch`, merged as any field, 16 MiB kept a provider so 100,000 tests can't hold 400 MB | §6.3; §6.5; §6.6 |
| F7 | "After a run that built" can't be known | **Accepted** | Fenix can't tell a build happened; `test-plan.list-after` lets the provider say so (cargo's listing 0.18 s warm against 20.9 s cold, the packs' plan §3), applied once the plan's steps all exited and nothing was `broken`, under the automatic listings' rules | §6.3; §7.1 |
| F8 | `test-provider-set` relists on every call; no "not here" | **Accepted** | A call that says what Fenix has changes nothing; a change lists as an automatic listing would, once the page has opened; `runner` "" with no problem hides the provider in that project. A sentinel, not a new type: the signature stays | §6.3 import; §4; §7.1 |
| F9 | The project scan has no exclusions | **Accepted** | `file_matches` matches a name at any depth (`manifest.rs:408-415`). The walk now honours the project's ignores as Search's does (`fenix-project/src/search.rs:288-308`, hidden files included, so `.venv`), and the page says when the 2,000-file cap is reached | §5; §6.7; §7.1 |
| F10 | Path forms differ in each direction | **Accepted** | `tests-scan` gives `/project/<path>` with `/`, as Fenix names files to extensions already (`app/wasm_host.rs:1176`); a relative path is the project's in a scan's answer and the step's `cwd`'s in any report | §6.3; §6.4 |
| F11 | A command line's length isn't a stated limit | **Accepted, numbers checked** | Fenix starts a step's program directly through Rust's `Command`, i.e. `CreateProcessW`, 32,767 characters with its null (`supervisor.rs:62`, `67-94`); only a `.cmd`/`.bat` found on `PATHEXT` (`fenix-rpc/src/lib.rs:45-61`) goes through `cmd.exe`, 8,191. Over its limit a step fails without starting, and the 32,766 is checked on Linux too | §6.3; §6.5; §8.2 |
| F12 | A run whose provider starts again for another project | **Accepted** | One worker per extension, its `/project` fixed (`app/wasm_host.rs:243-249`); a pack of the project you move to restarts at once (`1157-1170`). The old instance's run is stopped as after a trap; the new one is never asked about it; Fenix asks an instance only about its project | §6.3 (doc); §6.4 |
| F13 | Where `needs` land remotely | **Accepted** | Copied before each step, every time, keeping their path below `/data`; and checked on every platform, so a missing one fails locally | §6.3; §6.5; §8.6 |
| F14 | A "complete" listing isn't defined | **Accepted** | Every step `exited`, any code; no `broken` or `crashed`. Used for a run's records too | §4 |
| F15 | A 1.6-built component with `api = "1.5"` on Fenix 1.5 | **Accepted: read to load, and a test to settle it** | It imports only what it calls (wit-component), the new records are type exports the matching skips, and the 1.5 host never looks at exports it doesn't know (§6.2, with the lines). Read in wasmtime 36's source, not Fenix's 50, so step 2 adds a permanent test against 1.5's world | §6.2; §14; §15 step 2; §17 |
| F16 | The Python-encoding wording | **Accepted** | The plugin's lines are `json.dumps`, escaped ASCII (`reporters/fenix_tests.py`); what's at risk is pytest's own text in *Test output* | §17 |
| F17 | Names and numbers | **Accepted, all** | `[[contributes.test_providers]]` in the import's doc; `orbit/lib tests::adds`; group ids preceded by U+001F; §15's table as E1a, E1b, E2, E3a-c, E4 with the branch the packs build on; the packs' plan cited by its own sections, with "the first design" kept for what the first version found | The header; §4; §6.3; §6.6; §15 |
| F18 | A run reads the disk; a scan may be the buffer's | **Accepted, Fenix's side only** | No door saves today: a task (`app.rs:10860-10903`), `SPC p r`, which reads the buffer and runs the disk (`app.rs:10768-10778`). A run doesn't either; the head names the unsaved files and a test the run didn't see goes back to what it was. Saving first is Q8 | §8.4; §8.5; §16 Q8 |
| F19 | After a `build-first` that failed | **Accepted** | Asked again only when every step exited 0 and nothing was `broken` or `crashed`; else no session, the head says why. A second `build-first` is a refusal | §6.3; §11 |

None was rejected. F1's and F2's fixes are narrower or wider than
proposed where the proposal left a case open; the rest are as proposed,
with the evidence above.

### Found in revision

Looked for as the implementer of steps 1 to 3; none needed a new type.

| | What | Settled | Where |
|---|---|---|---|
| R1 | A `run` ask had no size limit: "the failed" of 100,000 tests, each up to ~10 KiB of strings, would be more than a guest's 128 MiB | 10,000 records, coarsened to parents past that (with F2) | §4; §6.5 |
| R2 | Not every string and list had a cap (`parent`, `path`, `skip`, `covers`, `needs`, a step's `env`, `runner`, `problem`, errors, a launch, a call's whole answer) | The limits table | §6.5 |
| R3 | Several roots in one project (a workspace, `go.work`, a monorepo) | Groups and each step's `cwd`; nothing more | §4 |
| R4 | Two providers finding tests in one file | Both kept; in the file, worst mark, innermost test, then provider id | §4; §9.4 |
| R5 | A stop while a `tests-read` is under way | The call finishes and its reports stand; lines not given aren't; then `stopped` | §6.4 |
| R6 | A step silent for a long time | No heartbeat, no call on a timer; only the run's timeout. The step under way gets `timed-out`, later ones `stopped` | §6.4; §8.3 |
| R7 | Order between batches, step ends, steps and providers | Stated: lines, then the end; step by step; provider by provider; reports in order | §6.4 |
| R8 | A report naming an id Fenix doesn't know, other than `ended` | `started`, `output`: nothing added; `broken`, `crashed`: as for the step | §4 |
| R9 | A cut id matching again | Cut the same way everywhere | §4 |
| R10 | Windows exit codes in an `s32` | A crash's NTSTATUS is negative | §6.3 `test-step-end` |
| R11 | Calls to an extension running in another project (`stale_call`) | `Deliver` for its own project; another project's scan or plan restarts it first | §6.3 host side |
| R12 | `ended` in a listing or a `build-first` plan | Ignored | §4; §11 |
| R13 | The provider falling behind: was its step still ended? | Yes, `tests-step-ended` comes as usual | §6.7 |

`tests.timeout_minutes`, the 512-line batch and every promise in §6.4
are Fenix's to change after step 2 without touching a type. What would
need a type after step 2, and isn't here because no runner showed a
case: a per-step timeout, a provider-driven cancel, built-in readers
(Q6), a runner driven over a socket (§6.1, shape B).

## 19. Review of steps 1 and 2

An independent review read steps 1 (`fenix-tests`) and 2 (API 1.6) as
built, with probe tests against the model, before the API is frozen by
Fenix 1.6.0. The packs' kit and its stand-in for Fenix, written against
this plan, then listed fourteen rules the plan left open. Each finding
was checked against the code (the probes run again) before it was
ruled on. **No WIT type, field, case, export or import changed**: every
ruling below is a rule of meaning, written into `world.wit`'s doc
comments (and §6.3, which stays word for word the same), `tests.rs`'s,
`WRITING_EXTENSIONS.md`'s "Test providers", §4, §6.4, §6.5, §11, and
the model's Decisions (`fenix-tests/src/lib.rs`, 1 to 36), where the
model implements it.

### What blocked the freeze

| | Finding | Ruling | Why | Where |
|---|---|---|---|---|
| 1 | Which run a `build-first` plan runs under; run ids counted per project | **Accepted.** Run ids come from one count the host makes and gives every project's model (`RunIds`, `Model::with_run_ids`), so they're unique while Fenix runs, across projects. The host draws the launch's id (`Model::run_id`) before the first `tests-launch`; the `build-first` plan runs as that run (`request_build(run, ...)`), and the second `tests-launch` carries it | §11 already meant "the same run"; the model couldn't honour it with its own counter. The provider keeps per-run state (its reader) keyed by `run`, so one id across the launch, its build and the second ask is what it can use | `test-launch.build-first`, `tests-launch` (WIT, `tests.rs`); §6.4 "Run ids"; §11; Decisions 34; `run_ids_are_unique_across_projects`, `a_build_first_plan_runs_as_the_launchs_run` |
| 2 | What Fenix gives back in an ask and to `tests-launch` | **Accepted, one stated form.** `path`: `/project/<path>` with `/` for a file of the project, whatever form the provider gave (relative, `\`, a full path), and a full path outside it -- the form F10 gives `tests-scan`. The model holds the project's relative form (Decisions 1); the host turns it back (step 3). `line`, `end-line`: with that path, from the first of scan, listing and run that gives either, so possibly from unsaved text. `parent`: the one Fenix put it under. `data`: as given, or none -- over 4 KiB, past 16 MiB, after a restart until found again, or when only a source that gave none still has the node. A provider plans from `id` when `data` is none; Fenix never gives back for itself a node whose id was cut and whose `data` isn't kept: an ask has its nearest nameable ancestor instead, a fill-in holding one is left out, and it isn't debugged | The provider gave the path in one of several forms Fenix doesn't keep; one form is simplest to read back. Only a cut id needs `data` to be named; "never alone without it" is the smallest guarantee that keeps a pack correct, and an ancestor runs at least the node ("coarser, never fewer", as R1) | `found-test` (WIT, `tests.rs`); §4 "What the provider is asked"; §6.4 "What Fenix gives back"; Decisions 1, 20, 32; `TestNode::given_back`, `TestNode::can_be_named`; `a_cut_id_without_its_data_is_asked_as_its_parent`, `a_records_parent_is_where_fenix_put_it` |
| 3 | A fill-in can't be told from the next selected node | **Accepted, the first option.** A fill-in holds every node under its file or suite, the suites between included, so each fill-in record's `parent` is in the ask; a selected node's never is (the selection is its topmost nodes). Rule for a provider: *a record whose `parent` is in the ask is a fill-in; one whose `parent` isn't was selected.* Fill-ins are whole or absent, take the room left after the selected nodes in display order, and a later smaller one may fit | No id parsing; holds with the 10,000 cap and with coarsening, both of which keep the selection topmost. Giving `parent` as Fenix placed it (none at the top) keeps the rule exact for a node whose parent was never found or made a loop | `test-ask.run` (WIT, `tests.rs`); §4; §6.4 "Asks"; Decisions 20; `a_fill_in_holds_the_suites_between_so_every_parent_is_in_the_ask`, `past_the_cap_a_selected_test_is_still_told_from_a_fill_in` |
| 4 | "May keep `api = "1.5"`" depends on what's linked | **Accepted.** A component built on the 1.6 crate may say `api = "1.5"` only when it doesn't import `test-provider-set`: when no code reachable from an export calls `tests::provider`, even behind a condition that never holds. Only 1.6 on 1.5 is promised, because only it is tested | The reviewer's probe: a call behind `if said > 1_000_000` is linked, and 1.5's linker refuses the component | CHANGELOG; `WRITING_EXTENSIONS.md` (Test providers, and the versioning paragraph, no longer general); `tests.rs`'s module doc and `provider`'s; `fenix-wasm/tests/f15.rs` now asserts the five `tests-*` exports and no `test-provider-set` import |
| 9 | Cut ids collide | **Accepted.** An id of at most 1,024 bytes is as it is; a longer one is its first 1,007 bytes (back to a character's start), `#`, and the FNV-1a 64-bit hash of the whole id's UTF-8 as 16 lowercase hex digits: 1,021 to 1,024 bytes, idempotent. One implementation, `fenix_tests::cut_id`, which `fenix_wasm::test_limits` calls (`fenix-wasm` now depends on `fenix-tests`, whose only dependency is serde: no cycle); test vectors in both crates' tests and in `WRITING_EXTENSIONS.md` | A packs' kit computes it in a dozen lines with no dependency; 64 bits among a provider's 100,000 ids leave a collision about one in 10⁹; a prefix keeps the cut id readable on the page and in *Test output* | `found-test.id` (WIT, `tests.rs`); §4 "Ids"; §6.5; Decisions 33; `a_long_id_is_cut_to_a_prefix_and_a_hash_of_the_whole`, `two_long_ids_that_share_a_kib_stay_two`, `a_long_id_is_cut_to_a_prefix_and_a_hash_as_the_explorer_cuts_it` (fenix-wasm) |

### The model's bugs and costs

| | Finding | Ruling | Where |
|---|---|---|---|
| 5 | An unknown id's `ended` rebuilt the tree, and searched every id | **Fixed.** The test is added without working the tree out (once at the call's end), and "the deepest node of the file" is looked for in each provider's index of groups, files and suites by path, kept as merged fields change. 10,000 such results with `at`, in batches of 512: 26 s before (release), about 1 s in a debug build now | Decisions 21; `ten_thousand_unknown_ids_cost_a_build_a_batch` |
| 6 | `test_at` quadratic with no end lines | **Fixed**: one pass for each family's latest start. 5,000 tests: 65 ms before (release), under 4 ms in a debug build | `the_test_at_a_line_among_5000_without_end_lines` |
| 7 | A crash or `broken` with no id errored what passed earlier | **Fixed, and the rule widened**: a `broken` or `crashed`, with an id or none, errors only what has no result in this run yet. "Hasn't ended in this step" in the WIT became "has no result in this run yet" | `test-broken`, `test-crash` (WIT); §8.5; Decisions 11; `a_crash_or_broken_with_no_id_leaves_what_ended_earlier_in_the_run`, `a_broken_naming_a_group_leaves_its_tests_that_ended_in_an_earlier_step` |
| 8 | A test that leaves every source loses its result | **Fixed**: its result is kept as a restored one, stale when it's found again, and in the results file meanwhile. Packs are told an empty scan drops the file's tests, so a scanner that can't parse half-typed text answers its last good scan | §4 "What's dropped"; `tests-scan` (WIT), `tests_scan` (crate); `WRITING_EXTENSIONS.md`; Decisions 29; `a_test_that_leaves_every_source_and_comes_back_keeps_its_result` |
| 10 | The model cut `data` the host drops | **Fixed**: the model drops it too | Decisions 26; `data_over_4_kib_isnt_kept_rather_than_cut` |

### Listed for step 3, done now

| What | Ruling | Where |
|---|---|---|
| Providers keyed by their short id | A provider is `ProviderId { extension, provider }` throughout the model; the results file keeps both (version 2: a version 1 file is refused, "its version is 1", not misread) | §4; Decisions 18, 35; `two_extensions_providers_of_one_id_are_two`, `a_results_file_of_version_1_is_refused` |
| A stop during a plan, then a plan error | Errors nothing: the tests go back to what they were | Decisions 12; `a_plan_error_after_a_stop_errors_nothing` |
| Location fell through field by field | As one: `path`, `line`, `end-line` from the first source that gives a path or a line | §4; Decisions 30; `a_nodes_place_falls_through_as_one` |
| `summary()` re-resolved the waiting selection each frame | Counted once a revision | Decisions 36; `the_heads_waiting_count_follows_the_model` |
| `errored_under` quadratic | A step's scope, and what of it is without a result, worked out once per step and tree shape; with an id, what's under it only | Decisions 36; `ten_thousand_broken_reports_in_one_call` |
| A wrongly typed `tests-*` export called "built before 1.6" | Said as "its tests-scan isn't API 1.6's (another type: built on a draft of 1.6?)", on its calls and its output page | `fenix-wasm` `host.rs`; `a_tests_export_of_another_type_is_said_as_that` |
| A `needs` entry over 4 KiB was cut into another path | Not cut: the step fails, with why. `PlannedStep.needs` is `Vec<Result<PathBuf, String>>` | `test_limits::need`; `a_need_is_a_data_path_or_its_step_fails` |
| `\`-separated `needs` were accepted | **Refused**: the API promises `needs` as `/data/...` paths, a remote project's copy keeps the path as given, and accepting `\` now would be a promise for 1.x. Other calls' paths still take `\`, as they did | §6.5; `test_limits::need` |
| A marker or `scan` pattern with a drive (`C:\x`, `C:/x`); a provider id ending in `.` | **Left as they are.** The same checks pass the same values in `contexts`' markers, `language_servers.files` and `agents`' ids; tightening one section alone would be inconsistent, and tightening the others could refuse an extension that loads today. Worth doing for all of them together | `fenix-extensions` `manifest.rs`, unchanged |

### Notes

| What | Ruling |
|---|---|
| `use fenix_extension_api::tests;` beside a pack's own `#[cfg(test)] mod tests` is E0255 | The name stays (§6.6, as `lsp` and `forge`); `WRITING_EXTENSIONS.md` says to import its items, or the module under another name |
| CHANGELOG said "Fenix runs the commands" | Reworded for what exists after step 2: the API, for extension authors; the explorer that calls it is being built |
| f15 never asserted the exports | It does now (finding 4) |

### What the packs' kit asked

| | Question | Ruling | Model |
|---|---|---|---|
| K1 | How an id over 1 KiB is cut | Finding 9 above, with its test vectors | Decisions 33 |
| K2 | What the command-line limits count | The line as Rust's standard library builds it for `CreateProcessW`, after `${data}` and `${tools}` are filled in: the program as resolved (full path) always quoted, each argument quoted when empty or holding a space or tab (`"` as `\"`, the `\`s before a `"` and ending a quoted argument doubled), joined by spaces; for a `.cmd`/`.bat`, the whole line Rust gives `cmd.exe`. A pack keeps a margin for the path and `${data}` | Step 3 (§6.5) |
| K3 | A cut line's pieces and the other stream | A piece's rest is the next line *of the same stream*; the other stream's lines, and a call's end, may come between; `cut` is on every piece but the last. A provider keeps one pending piece per stream | Step 3 (§6.4 "Lines") |
| K4 | A line of exactly 64 KiB at a stream's end | Not `cut`: only a line longer than 64 KiB is split, and a line ending with its stream is whole | Step 3 (§6.4 "Lines") |
| K5 | `crashed`/`broken` with no id in a step whose `covers` is empty | Its scope is the plan's rest (what of the ask no step names), less what has a result in the run | Decisions 11 |
| K6 | Does `broken` error tests that already ended in this run | No: `broken` and `crashed` alike, with or without an id, error only what has no result in this run (finding 7) | Decisions 11 |
| K7 | A template whose cases end but it doesn't | Not errored: once its cases are found it isn't counted, and its state is its cases'. Synthesising its result isn't needed; report its own `ended` only when the runner gives one (Go's parent test) | Decisions 7; `a_template_whose_cases_ended_needs_no_result_of_its_own` |
| K8 | After a step Fenix failed to start; a plan of no steps | The plan goes on to its next step, the failed step's tests errored with why. A plan of no steps has its ask judged at once: errored "the run ended without a result for it" (unsaved files' tests back to what they were) | Decisions 13; `a_step_that_couldnt_start_errors_its_tests_and_the_plan_goes_on` |
| K9 | A `found` in a `build-first` plan; a `broken` in a listing | Ignored; makes the listing incomplete (it drops nothing) and changes no result | Decisions 17 |
| K10 | A `found` before its parent; an id twice in a scan | At its provider's top until the parent comes, then under it; the later record replaces the earlier | Decisions 6, 3 |
| K11 | A group with failed and errored children | Failed (failed before errored); both are counted | Decisions 8 |
| K12 | What an ask leaves out past 10,000 | Whole fill-ins, in display order, the first that doesn't fit and any later that doesn't; never part of one, never a selected node (finding 3) | Decisions 20 |
| K13 | Does the runner survive the extension starting again | Yes: kept per project until Fenix exits. Call at each start all the same (Fenix doesn't say what it has) and on a change; a call repeating it changes nothing, so the kit's way is right | `what_a_provider_said_it_is_survives_its_extension_starting_again` |
| K14 | A case a complete run dropped that it had just errored | It goes with that result, uncounted in the run's end | Decisions 31; `a_case_dropped_by_a_complete_run_goes_with_its_result` |

### Carried to step 3

- Filling `Spec::test_providers` where `fenix-gui` makes the spec (until
  then every `test-provider-set` on the nightly fails).
- The host's side of the rulings: giving a node's path back as
  `/project/<path>`; drawing a launch's run id with `run_id` and passing
  it to `request_build`; refusing to debug a node `can_be_named` says no
  to; one `RunIds` for every project; K2's command-line count; K3's and
  K4's line pieces (`supervisor.rs` gains the flag); a need's `Err` as
  the step's `failed`.
- Cleaning labels and messages of newlines and escape codes before the
  page draws them (step 4).
- An answer is lifted from the guest whole before a limit applies (as
  for every import today): bounded by the guest's memory, not measured.
  Noted, not changed.

None of the rulings changed a type: the API can be frozen as it stands,
with these meanings.

## 20. As built

### Step 2, API 1.6

**The WIT is §6.3's**, types, names, fields, cases and signatures alike,
with §18's four changes. Its doc comments are §6.3's word for word; one
comment heads the new types in `interface types`, saying that its §
references are to this plan and that the pack author's account is
`WRITING_EXTENSIONS.md`'s "Test providers". The world's `use` line and
the five exports follow 1.2's and 1.5's in `world extension`; the import
ends `interface host`. `world.wit`'s header now says 1.6, and what 1.5
and 1.6 added.

**F15 holds** (§6.2, §17): `fenix-wasm/tests/f15.rs` instantiates
`guests/plain16.wasm`, built on the 1.6 crate with nothing of 1.6 used,
against a copy of `v1.5.2`'s `wit/` (`tests/wit-1.5/world.wit`, its
`bindgen!` of `extension-base`, and every one of its 125 `host` imports
written out in `tests/wit-1.5/host.rs`), with WASI and those imports in
the `Linker` as 1.5's `run` makes it; then `activate` and a command run.
It passes in wasmtime 50. The component imports from `host` only what it
calls (the test checks there's no `test-provider-set` among them), and
the new records don't stand in the way. Its pair,
`a_guest_that_uses_1_6_says_what_fenix_1_5_lacks`, has the provider
guest refused by the same linker: "component imports instance
`fenix:extension/host@1.0.0`, but a matching implementation was not
found in the linker: instance export `test-provider-set` has the wrong
type: function implementation is missing". So the 24 kit users that run
no tests may keep `api = "1.5"` when rebuilt on the 1.6 crate. The other
way, `guests/keys.wasm`, built on the 1.5 crate, loads on 1.6 and
answers each of the five calls that it has no such export.

**No second `bindgen!`.** §6.3's "Host side" was wrong: wasmtime 50's
`bindgen!` of `extension-base` generates every type of `interface types`,
whether `extension-base` reaches it or not, so `fenix-wasm` has the new
records from its one `bindgen!`, the same WIT the guest crate's is.

**The Rust side** is §6.6's names, fields and signatures. Its types are
the crate's own, not the bindings' renamed, because §6.6's `Report` has
struct variants where the bindings have a record each
(`TestReport::Output(TestOutput)`); `export!` converts between them
(`tests::exports`, hidden). What §6.6 didn't say: every type derives
`Debug`, `Clone` and `PartialEq`, those without an `f64` or a `Command`
`Eq` too, `Kind` and `Outcome` `Copy` and `Hash`; `Found`, `Plan`,
`Line` and `Location` are `Default`, `Kind`'s default `Test`; and `From`
between them and the bindings' types. A mirror with fewer derives
compiles against it. The crate's own unit tests in `lib.rs` moved from
`mod tests` to `mod checks`, for the name.

**The limits** are `fenix_wasm::test_limits`: every number of §6.5's
table, for the host and step 3. The worker holds each answer to them as
it comes back (`scanned`, `planned`, `reports`, `launch`, `error`,
`provider`), and says each kind of cut once while the instance runs
(`tests: an id over 1 KiB was cut to 1 KiB (said once)`). Where §6.5
left a choice:

- `found-test.data` over 4 KiB is dropped, not cut: given back cut it
  wouldn't be "as it was".
- A `launch-config` over 1 MiB is refused (`tests-launch`'s answer is an
  `Err`): cut, it wouldn't be a launch.
- KiB limits cut by bytes at a character's start, "characters" limits by
  characters; nothing is added to what's cut. `text` and an `output`'s
  text keep their end. A step's `env` loses the variable that crosses
  64 KiB and those after it. A call's 16 MiB is the bytes of its
  strings.
- A `test-ended.duration-ms` that's NaN, endless or negative is none.
- The limits on what Fenix sends (an ask's 10,000, a batch's 512 lines
  and 256 KiB, a scan's 1 MiB) are constants for step 3; the worker
  doesn't hold Fenix to them.

**What the replies carry.** `Reply::Scanned`, `Reply::Reports`,
`Reply::Planned(Result<Planned, String>)` and
`Reply::Launched(Result<Launching, String>)`. A `PlannedStep` is the
step as given (held to its limits), its command held to
`permissions.process` and its folder as `spawn`'s is (`Result<(Checked,
Env), String>`, `${data}` and `${tools}` filled in), and each of its
`needs` as a path of this machine's when it's one under `/data`. A step
whose program it may not run isn't refused by the worker: what becomes
of it is step 3's, as are a command line's length and whether `needs`
are there when the step starts. A `launch` is checked as `launches-set`'s
are (the same code, `checked_launch`).

**Providers the component doesn't have.** `Spec::test_providers` holds
the manifest's provider ids; **step 3 fills it** where `fenix-gui` makes
the spec (this step left `fenix-gui` alone but for one empty match arm,
`Request::TestProviderSet { .. } => {}`). With it set and no
`tests-scan` and `tests-plan`, the worker says so on the output page as
it starts; any of the five calls to a component without that export ends
`Err("it has no tests-scan: it was built for an extension API before
1.6, ...")`, which is how step 3 knows not to offer it. `test-provider-set`
refuses an id that isn't in `Spec::test_providers`.

**The manifest**: `TestProviderContribution` as §5 has it. A provider in
an extension whose `engine.api` is under 1.6, or that has none (one
that runs no code), is refused. Languages aren't checked against
Fenix's names, as a language server's aren't; a marker or a `scan`
pattern may not be empty, hold `..` or start with `/`.

**Test guests.** `guests/provider` (every export and the import, a
reader with cut lines, a provider that traps in each export and one
that never returns) and `guests/plain16`, built with Rust 1.97.0 here
for want of 1.98.0. `build.ps1` builds them and `counter`, and no longer
`hello` and `keys`: those are kept as built on older APIs (1.2's WIT,
the 1.5 crate) to show those still run, and `hello` doesn't build on the
current WIT since 1.5 added an export it lacks.

### Step 3, Fenix runs providers

**Where it is.** `crates/fenix-gui/src/app/tests_host.rs`, in three
layers:

- *Plain functions*, tested alone: the API's records to the model's and
  back (`model_found`, `api_found`, `model_reports`, `api_ask`); every
  path a provider gives mapped to the project's relative form
  (`model_path`: `/project/...`, relative to the project in a scan and
  to the step's folder in a report, a full path; `\` or `/`; a drive in
  any case on Windows) and back to `/project/<path>` (`guest_path`); an
  ask held to its bytes (`capped_ask`); a step's command line counted
  as Rust builds it for `CreateProcessW` (`command_line`); escape codes
  taken out (`without_escapes`); the batches (`Batch`); a step's end
  (`step_end`).
- *State*, `TestsHost` (the `App`'s `tests`): one `ProjectTests` per
  project root -- its `fenix_tests::Model` (every one drawing on the
  host's one `RunIds`), its providers, the plan under way's steps as
  the provider gave them (command and `needs`, beside what the model
  keeps), the one call out for its run, the step under way (`StepRun`:
  its `TaskRunner`, its lines waiting, a `tests-read` out or not, how its
  process ended, why Fenix stopped it), *Test output*, its first scans,
  a run on save due. Across projects: the calls out by token with what
  each is for (`CallFor`), the replies and step events waiting, what
  each provider said of each project, the extensions with no provider's
  exports.
- *The pump* (`tests_pump`): replies and step events applied, then each
  project driven a step (`tests_drive`): a failed step's end, else a
  batch of its lines to `tests-read`, else its end once its process has
  ended, else a step not started told `stopped`, else `Model::next` --
  `Plan` asked, `Step` started, `NotStarted` queued, `Ended` written to
  *Test output* and the results file. Replies re-enter through
  `tests_reply` (from `wasm_host`'s `Pending::Tests`), step events
  through `tests_step_event` (`FenixUserEvent::TestsStep`, or an inbox
  without an event loop); each queues and pumps, and a pump re-entered
  returns, the outer loop taking what was queued. *The tick*
  (`tests_tick`, from `plan_wake`) does what's due by the clock: an open
  file's scan, a run on save, a run's timeout, a first scan's next file.

**Its hooks elsewhere**: in `app.rs` the module, the field, the event
and its handler, the tick in `plan_wake` and a save in `after_saved`
(19 lines); in `wasm_host.rs` `Pending::Tests`, its end and a stopped
extension's answer, `test-provider-set`, an extension ready, stopped,
started again or switched off, `Spec::test_providers` from the
manifest, and three small accessors (34 lines); seven commands in
`commands.rs`. Elsewhere: `fenix-tasks`' `OutputLine.cut` (below) and
`fake_runner` (feature `fake-runner`, with the binary
`fake-test-runner`); the settings in `fenix-config`. Nothing in
`fenix-tests` or `fenix-wasm` changed: what the host needed was there.

**Where the plan was silent, or read so:**

- **An ask's bytes**: 16 MiB, the cap answers have (`MAX_ANSWER`). Past
  it, whole fill-ins are left out, the last first, then the provider is
  asked `run-all` -- coarser, never fewer, as past 10,000 records. In the
  host, from the ask's own shape (a record whose `parent` is in the ask
  is a fill-in), not in the model.
- **The 16 MiB answer cap** was already the worker's (`test_limits`'
  `within`, on every scan and report list): the host relies on it.
- **One project**: `stale_call` is unchanged. The host asks an
  extension about project P only when its instance runs in P; a scan or
  a plan for the project you're in restarts one running elsewhere
  there first; anything else for another project isn't asked (a plan
  then stops its provider's turn, "can't be asked"). A run of the old
  instance is stopped as after a trap by the restart itself, wherever it
  was, but for the project the restart is for.
- **Which providers a project has**: those whose markers are at its
  top, and one whose language or `scan` names a file of it that's
  opened.
- **A call that fails** (a trap, a deadline, a stopped extension) is its
  provider stopped, as after a trap. One that fails with "it has no
  tests-..." or "isn't API 1.6's" also makes the extension no provider
  until Fenix restarts or it's reinstalled: its providers leave every
  model, and its output page says so once.
- **How a step ended**: a stop or a timeout is what its end says,
  whatever its process did meanwhile; `TaskRunner`'s own failure (a pipe
  that couldn't be read, cleanup past two seconds) is `failed("its
  output couldn't be read: ...")`.
- **A first scan** runs at each start of a provider's extension in its
  project, through the project's ignores as Search's walk (the `ignore`
  crate with Search's options; `fenix-gui` gains it, already in the
  lock), skipping files open in a buffer (scanned from it), 200 files'
  scans to the model at a time. A remote project has no walk, and its
  runs and listings say they aren't available yet. The cap reached is
  said on the provider's output page and kept (`scanned_of`) for the
  page.
- **A file changed on disk** isn't rescanned unless it's open (Fenix's
  disk poll reloads its buffer, which is rescanned) or opened: Fenix has
  no watch on a project's other files. This differs from §7.1.
- **Run on save**: the files saved in the second before it each asked
  as a file selection, the first replacing a waiting run, the rest
  joining it.
- ***Test output*** is one buffer per project, begun again at each run's
  first action; `tests.output` opens it in the focused pane (beside the
  page with step 4). Each step's line is its command line and label;
  the run's end says its counts, or why it stopped, and the files with
  unsaved edits. `Enter` on a place in it resolves as Task Output's
  fallback does, against the project, not the step's folder.
- **`tests.enabled` turned off** while a run goes stops it. No setting
  of step 3 is per project.
- **A results file** is read when the project's model is made (its
  first provider), written after every run's end, through
  `fenix_storage::write`.
- **Commands** registered, with no keys: `tests.run_here`, `run_file`,
  `run_all`, `run_failed`, `run_last`, `stop`, `output`. `tests.open` and
  `tests.debug_here` wait for their steps.

**`cut`**: `fenix-tasks`' pipe reader now cuts a line past 64 KiB only
when a byte past it that isn't its end comes, at a character's start,
marking every piece but the last; a line of exactly 64 KiB (before
`\n`, `\r\n` or its stream's end) was cut before, with an empty line
after it, and is whole now.

**The tree kill**, tested on both platforms with bounded waits: the
app test `a_stop_kills_the_steps_tree_and_puts_its_tests_back` (the
runner's `child` holds the pipes and lives two minutes; after a stop
it's gone within ten seconds, `OpenProcess`/`WaitForSingleObject` on
Windows, `kill(pid, 0)` and `/proc` on Unix); `fenix-tasks`'
`tests/fake_runner.rs` the same through `TaskRunner` alone; and
`tree.rs` gains the Windows test it lacked, `ProcessTree` over the
harness's own fixture, no shell.

**`fake-test-runner`** is `fenix_tasks::fake_runner`. `cargo test -p
fenix-gui` doesn't build a dependency's binary, so the app's tests run
the same code as their harness (`--ignored --exact ... --`, as the
other fixtures are), through a test-only `program_for`.

**Measured**: 100,000 lines through the provider guest, read in 196
batches, in about 1.3 s in a debug build on Linux.

**Not tested in the app**: `list-after`. The guest's `run-all` always
has a step whose `needs` aren't there, so its listing after never comes;
the model's own tests hold it, and the host does nothing of its own for
it. A provider that traps is tested at its plan, the guest trapping in
every export.

**For the steps after**: `tests_page_opened`, `tests_list`,
`tests_replace` and `scanned_of` are there for the page; the model's
`problems`, `marks` and `test_at` for the file; `Model::run_id` and
`request_build` are untouched, but `tests-launch` has no `CallFor` yet
and a `Reply::Launched` would be taken as a failed call; the run's end
(`tests_run_ended`) is where the notification and `last_task` go.

**After the review of step 3.** Fixed, each with a test:

- *Falling behind* (M1) counts what the lines waiting cost -- their text
  and each line's own size -- and at most 1,024 batches of them, so a
  runner's empty lines aren't free (`empty_lines_count_toward_falling_behind`).
- *A start is a start* (M2): an extension that starts again makes its
  first scans again (`a_restarted_extension_scans_the_project_again`).
- *Switched off* (S1): an extension not installed is never asked, and the
  open files forget its providers (`nothing_is_asked_of_an_extension_switched_off`).
- *One project, several spellings* (S2): roots are canonicalized once
  each (cached, no file system call per frame), every other spelling met
  -- the one a file was opened by, the instance's `/project` -- kept as an
  alias, a path under any of them the project's; a step's folder is
  canonicalized as it starts (`a_project_reached_through_a_symlink_has_its_files`,
  Unix).
- *Exit* (S3): `exiting` cancels every step and waits up to two seconds
  for its tree to be killed and the results written; dropped alone, a
  `TaskRunner` only asks its thread, and on Linux Fenix was gone first
  (`fenix_exiting_kills_a_steps_tree`). So §6.1's "what the runner
  started dies with it" holds when Fenix exits too.
- *Fail closed* (S5): a model is made with its automatic listings shut,
  then set from trust at once (`what_runs_without_a_key_runs_only_in_a_trusted_project`,
  which also pins today's trust gating, Q9).
- *The UI thread* (S6): trust looked at once in two seconds, or when a
  file of the project's `.fenix` is saved (`a_projects_trust_is_looked_at_once_a_while`);
  a buffer over 1 MiB of characters isn't copied for a scan; the
  results file read on a thread when the model is made, applied when it
  comes (a run's end waits for it before writing), and written by one
  writer thread in turn.
- *Off mid-run* (S7): the run is stopped once and driven to its end, its
  steps told `stopped`; then nothing more (`turning_the_explorer_off_ends_its_run`).
- *First scans* (S8): the cap counts the files scanned, those open
  (scanned from their buffers) among them (`a_first_scans_cap_counts_what_was_scanned`);
  a walk waiting on its thread has the tick wake within 20 ms
  (`a_walk_waiting_wakes_the_loop`); a walk let go stops its thread
  mid-tree (`a_walk_let_go_stops`); a walk's scan is marked out before
  it's sent.
- On Windows a step runs with `NoDefaultCurrentDirectoryInExePath=1`,
  so a `.cmd` shim's `cmd.exe` doesn't look in the project first for
  what it runs (one line in the step's spawn).

Left for the step that owns each, from the same review:

- **Step 4**: *Test output*'s `Enter` resolves against the project, not
  the step's folder (a step in `/data` or a sub-crate jumps wrong);
  `output_lines` counts a cut line's pieces as lines, so trimming past
  16 MiB drops more than it counted (cosmetic); `tests_replace`,
  `tests_list` and `tests_page_opened` need the `tests.enabled` gate when
  the page calls them; `list-after` gets its app test with the page's
  listing.
- **Step 5**: `/project/../x.rs` is held "as given", a full path naming a
  `/project` folder: treat it as no place, never open it; a verbatim
  path (`\\?\C:\...`) is normalized after the project test, not before;
  a symlink inside the project to a file outside it counts as the
  project's (canonicalize before marks or a jump); `set_unsaved(rel,
  true)` isn't cleared when a dirty buffer is closed unsaved; a file
  changed on disk and not open isn't rescanned (a watch, or the disk
  poll, before the gutter relies on it).
- **Step 6**: `Reply::Launched` needs its `CallFor::Launch`; today it
  would be taken as a failed call and stop the provider.
- **Step 8**: the timeout counts by `SystemTime`, so a clock step can end
  a run early or late (`Instant` for the timeout; the model's stamps can
  stay wall time); an open file seen before a provider's extension was
  installed isn't scanned until reopened; `ids_sorted_by_path` each wake
  (cheap today); Q9, what "trusted" means; `C:foo` (drive-relative) reads
  as `C:/foo`.
