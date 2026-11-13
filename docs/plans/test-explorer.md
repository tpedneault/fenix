# Plan: the test explorer

- **Status:** planned, 13 November 2026, for the maintainer to review.
  Recorded in [ADR 0005](../adr/0005-test-providers.md). Nothing here is
  built.
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
- **Builds on:** the extension-side design in fenix-extensions,
  [`plans/test-explorer.md`](https://github.com/tpedneault/fenix-extensions/blob/main/plans/test-explorer.md)
  (cited below as *the first design*), whose runner experiments are the
  pack side of this plan, unchanged in substance.

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
  first design's recorded `outputs/pytest/run-q.txt`), and
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
Python tool beside it; Web's Vitest and Jest).

### Ids, and matching results to what was found

An id is the provider's own, at most 1 KiB, unique within the provider
and stable across its scans, listings and runs. Fenix keys a node by
`(provider, id)`. The first design's keys are already these: Rust
`<target> <path>` (`lib geo::tests::unwraps_none`), pytest's node id,
Go `<import path> <name>`, Vitest and Jest `<file> > <names>` (first
design, §3, "Matching").

A report names a test by id. Fenix:

1. **Knows the id**: the result is that node's.
2. **Doesn't know it, and the report came after a `found` for it in
   the same run**: the `found` added it (a Go subtest, a `test.each`
   case, a case of a template the provider recognised itself, §6.6).
3. **Doesn't know it at all**: it's added under the deepest node of its
   file, else under the provider's root, marked *found in a run*, with
   no line (a macro-made Rust test that the scan can't see).

Fenix never guesses by name. Recognising `add(-1, -1) is -3` as a case
of `add(%i, %i) is %i` is the provider's business, because only the
provider knows its runner's placeholders: it reports a `found` with the
template as `parent` before the result.

**What's dropped.** A node a scan gave goes when a later scan of its file
doesn't give it. A node a listing gave goes when the provider's next
complete listing doesn't give it, unless its file was saved since that
listing began (the scan is newer than the list). A node first seen in a
run (`case`s of a template, Go's subtests) goes when its template runs
again and the run ends without naming it.

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
results. It goes through the same machinery and the same reader.

### What's kept across a restart

- **Results**, not tests: for each `(provider, id)` with a result, its
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
| `scan` | Patterns as `language_servers.files` (a name, or a path below the project; no `..`, `manifest.rs:1090-1092`). Optional. |
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
| Stopping a run | Fenix's `TaskRunner`: a job object on Windows that kills the tree, and kills it if Fenix dies; a process group on Linux (`fenix-tasks/src/supervisor.rs:67-124`, `165-210`). What the runner started dies with it. | `process::kill` kills the child only (`host.rs:2344-2350`): cargo's test binary ran 6.2 s on after its `cargo` was killed (first design, appendix). Needs a new import to fix. |
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
arguments whose stdout says what happened (first design, §2). So (A).
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
  records below carry what remote projects will need (`needs`) now.

`api_fits("1.6")` is true on this Fenix once `API` is `"1.6.0"`, and
false on 1.5.x (`manifest.rs:25-28`, its test at `1326-1327`). A pack
with `api = "1.6"` doesn't install on Fenix 1.5 (`manifest.rs:1236-1253`),
and the registry offers a 1.5 user the newest version that fits
(`fenix-extensions/src/registry.rs:158-168`): the pack's release from
before it had tests. So a pack adds tests in a minor of its own (Rust
1.3.0, say), and Fenix 1.5 users keep the one before.

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
/// (`test-report.found`). 1.6.
record found-test {
    /// The provider's own name for it, the same in what it scans, lists
    /// and reads from a run: `lib tests::adds`,
    /// `tests/test_basic.py::test_cases[negative]`,
    /// `example.com/orbit TestTable/negative_one`. At most 1 KiB, unique
    /// among its provider's.
    id: string,
    /// The group it's in, by id: none for one at its provider's top.
    parent: option<string>,
    /// What the page calls it: `adds`, `negative`, `orbit (lib)`.
    label: string,
    kind: test-kind,
    /// Its file: under `/project`, relative to the project, or a full
    /// path of the project's machine.
    path: option<string>,
    /// Its first line, and its last when it knows: from 0, as a
    /// problem's (`problem`).
    line: option<u32>,
    end-line: option<u32>,
    /// Left out by its source (`#[ignore = "slow"]`, `@skip`,
    /// `test.skip`): why, or "" when it doesn't say.
    skip: option<string>,
    /// Its cases are found only by running it: a `test.each`, a
    /// `parametrize` the scan saw, a Go test that calls `t.Run`.
    template: bool,
}

/// What a node of the tree is (`found-test`).
enum test-kind { group, file, suite, test, case }

/// What Fenix asks a provider for (`tests-plan`).
variant test-ask {
    /// Its runner's listing of every test it can find in the project.
    discover,
    /// These, as Fenix has them now: tests, cases, and groups (all
    /// under them).
    run(list<found-test>),
    /// Every test.
    run-all,
}

/// What runs, in order (`tests-plan`).
record test-plan {
    steps: list<test-step>,
}

/// One command of a plan.
record test-step {
    /// Held to `permissions.process`, as `spawn`'s are; `${data}` and
    /// `${tools}` in its args and env are filled in. Run as it is: no
    /// shell between.
    command: command,
    /// What the page says while it runs: `orbit (lib)`, `tests/`.
    label: string,
    /// The ids it runs, when it knows them; empty: what of the ask the
    /// steps before it don't cover.
    covers: list<string>,
    /// Files under `/data` its command reads (a reporter, a config):
    /// copied to the project's machine first for a remote project.
    /// Nothing is done with them for one on this machine.
    needs: list<string>,
}

/// A line of a step's output (`tests-read`).
record test-line {
    /// As UTF-8, what wasn't replaced by U+FFFD; colour codes taken out.
    text: string,
    stderr: bool,
    /// The line went on past 64 KiB: the rest is the next line given.
    cut: bool,
}

/// How a step ended (`tests-step-ended`).
variant test-step-end {
    /// Its program ended: its exit code, none for a signal (Linux).
    exited(option<s32>),
    /// Stopped: by you, by a run that replaced it, or never started
    /// because the run was stopped before it.
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
    /// Under `/project`, relative to the step's folder, or a full path
    /// of the project's machine (`C:\src\orbit\src\lib.rs`).
    path: string,
    line: u32,
    column: option<u32>,
}

/// A test that ended.
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

/// What didn't build or load: never a test's failure.
record test-broken {
    /// The group or file that didn't: none for all the step covers.
    id: option<string>,
    message: string,
    at: option<test-location>,
}

/// What a provider read in its runner's output (`tests-read`,
/// `tests-step-ended`).
variant test-report {
    /// A test or group: from a listing, or found in a run.
    found(found-test),
    /// A test started, by id.
    started(string),
    ended(test-ended),
    output(test-output),
    broken(test-broken),
    /// The process died: what it said. What the step covers that hasn't
    /// ended is errored with it.
    crashed(string),
}

/// How to debug a test (`tests-launch`).
variant test-launch {
    /// This launch, now.
    launch(launch-config),
    /// These first -- a build that says where the test's binary is --
    /// read as a run's are; then ask again, once.
    build-first(test-plan),
}
```

```wit
// interface host, 1.6

/// What its test provider `provider` (`contributes.test-providers`) is
/// in its project now: the runner it uses (`cargo nextest`), and why it
/// can't run tests there when it can't (`pytest isn't installed in
/// .venv`). Each call has Fenix list the project's tests again
/// (`tests-plan` with `discover`). 1.6.
test-provider-set: func(provider: string, runner: string, problem: option<string>) -> result<_, string>;
```

```wit
// world extension, 1.6
use types.{found-test, test-ask, test-plan, test-line, test-step-end, test-report, test-launch};

/// The tests in file `path` (a full path), whose text is `text`, as
/// it's open or on disk: read, nothing run. 1.6.
export tests-scan: func(provider: string, path: string, text: string) -> list<found-test>;
/// What to run for `ask`, as run `run`. 1.6.
export tests-plan: func(provider: string, run: u64, ask: test-ask) -> result<test-plan, string>;
/// Lines of the output of step `step` of run `run`, in order. 1.6.
export tests-read: func(provider: string, run: u64, step: u32, lines: list<test-line>) -> list<test-report>;
/// Step `step` of run `run` ended: what its reader still had. 1.6.
export tests-step-ended: func(provider: string, run: u64, step: u32, end: test-step-end) -> list<test-report>;
/// How to debug test `test`, for run `run`. 1.6.
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
providers aren't offered.

### 6.4 What Fenix promises a provider

| Promise | |
|---|---|
| Calls | `tests-scan`, `tests-plan`, `tests-read`, `tests-step-ended`, `tests-launch` are ordinary calls: pages go first (`WRITING_EXTENSIONS.md:1116-1126`), one call at a time, its deadline the extension's (`limits.call_ms`, 5 s if it doesn't say). |
| Run ids | Unique for as long as Fenix runs; a listing has one too. |
| Order | For each step: `tests-read` in output order (as `TaskRunner` gives it), then exactly one `tests-step-ended`. Every step of a plan gets its `tests-step-ended`, `stopped` for one that never started, so the provider can free what it kept. Steps run one after another, never two of one run at once. |
| Batches | A `tests-read` holds at most 512 lines and 256 KiB. While one is under way for a step, what comes is held for the next, so a step never has two calls queued. |
| Lines | Lossy UTF-8 (`supervisor.rs:404-412`); trailing `\r` dropped (`405-407`); a line past 64 KiB comes in pieces, each but the last `cut` (`436-438`, which gains the flag in step 3); ANSI CSI and OSC sequences removed. Between stdout and stderr, order is as near as `TaskRunner` keeps it: per write on Linux (`supervisor.rs:290-321`), per 10 ms tick on Windows (`322-325`). A provider reads its runner from one stream where it can. |
| Scans | One file a call, at most 1 MiB of text (a larger file isn't scanned: its tests come from the listing). After typing pauses, at most one scan per file every 500 ms. |
| Paths | Fenix maps what a provider gives (`/project/...`, relative, a full path; `\` or `/`; a drive letter in any case on Windows) to the project's files with `fenix_lsp::normalize`, as task output's places are (`app.rs:11032`). A place outside the project is kept for the details, never a problem or a mark. |
| After a trap | The extension stops (ADR 0001); its steps under way are stopped, their tests back to what they were, and the page says the provider stopped and why. |

### 6.5 What a provider may not do, and the limits

- No shell: a step's program is a program its manifest lists. `sh`,
  `cmd` or `pwsh` only if it lists them, and then the store shows that.
- No ids of another provider; no reports for a run that isn't its.
- At most 10,000 `found-test` per scan, 256 steps per plan, 10,000
  reports per call; an `id` over 1 KiB or a `label` over 256 characters
  is cut and said on its output page.
- Nothing a call can't do in a few milliseconds: a call holds a batch,
  never a whole run (§6.7).
- Fenix keeps at most: a message 4 KiB, a failure's text 64 KiB (its
  end), what a test printed 64 KiB (its end), a run's output 16 MiB (its
  end; the buffer says what went), 100,000 tests a provider a project.

### 6.6 The Rust side a pack author sees

In `fenix-extension-api`, a `tests` module with the types, as `lsp`
and `forge` are (`crates/fenix-extension-api/src/lsp.rs`, `forge.rs`),
and five methods on `Extension` with defaults, as `key_scheme_changed`
has (`src/lib.rs:232`):

```rust
pub mod tests {
    pub struct Found { pub id: String, pub parent: Option<String>, pub label: String, pub kind: Kind,
                       pub path: Option<String>, pub line: Option<u32>, pub end_line: Option<u32>,
                       pub skip: Option<String>, pub template: bool }
    pub enum Kind { Group, File, Suite, Test, Case }
    pub enum Ask { Discover, Run(Vec<Found>), RunAll }
    pub struct Plan { pub steps: Vec<Step> }
    pub struct Step { pub command: Command, pub label: String, pub covers: Vec<String>, pub needs: Vec<String> }
    pub struct Line { pub text: String, pub stderr: bool, pub cut: bool }
    pub enum StepEnd { Exited(Option<i32>), Stopped, TimedOut, Failed(String) }
    pub enum Outcome { Passed, Failed, Skipped, Errored }
    pub struct Location { pub path: String, pub line: u32, pub column: Option<u32> }
    pub struct Ended { pub id: String, pub outcome: Outcome, pub duration_ms: Option<f64>,
                       pub message: Option<String>, pub at: Option<Location>, pub text: Option<String> }
    pub enum Report { Found(Found), Started(String), Ended(Ended), Output { id: Option<String>, text: String },
                      Broken { id: Option<String>, message: String, at: Option<Location> }, Crashed(String) }
    pub enum Launch { Launch(LaunchConfig), BuildFirst(Plan) }

    /// `test-provider-set`.
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
`crates/lang-kit`, step E1):

| `TestRunner` part (first design §3) | Becomes | What changes |
|---|---|---|
| `name()` | the manifest's `name`, and `tests::provider(id, name, None)` when it changes (nextest found) | Nothing a pack writes. |
| `scans(path)`, `scan(path, text) -> Vec<Found>` | `tests_scan` | Fenix calls it (open, typing paused, saved, the `scan` patterns); lang-kit's scheduling of scans goes. `Found.path: Vec<String>` (labels root first) stays the pack's; lang-kit turns it into `id`/`parent` (a group's id is its path's labels joined by `\u{1f}`) and gives groups their own `found-test`. `line` becomes from 0. `end_line` is new and optional. |
| `list() -> Vec<Command>` | `tests_plan(Discover)`: each command a step | The commands are unchanged. |
| `plan(&Selection) -> Vec<Command>` | `tests_plan(Run(found) \| RunAll)` | `Selection::Tests(&Test)` becomes the `Found` records Fenix sends; `Selection::Files` is a `file` node among them; `All` is `RunAll`. A provider needs no index of its own: what it plans from comes with the ask. |
| `reader() -> Box<dyn Reader>`, `Reader::line`, `Reader::end` | one `Reader` per `(run, step)`, made at the step's first `tests_read`, dropped at `tests_step_ended` | `Line { text: &str, stderr }` gains `cut`. `end(code)` gets a `StepEnd`. Lines come in batches; the reader is still fed a line at a time. |
| `Report` | `tests::Report`, one for one | `Ended`'s `Failure { message, at, text }` is flattened into `Ended`; `Outcome::Skipped(reason)` is `Skipped` with `message`; `NotRun` is gone (Fenix decides "back to what it was" or `errored`); `errored` is new, for a pytest fixture or setup failure. |
| `is_case(template, reported)` | inside lang-kit's reader adapter | lang-kit keeps the run's asked templates; a reported key it doesn't know that `is_case` says belongs to one is reported as `found` (kind `case`, parent the template) before its `ended`. Fenix never matches by name. |
| `launch(&Test) -> Option<LaunchConfig>` | `tests_launch` | May return `BuildFirst(plan)`: Rust's `cargo test --no-run --message-format=json` for the test's target, read like a run, whose `compiler-artifact` gives the binary; then `Launch`. |
| `assets() -> &[(&str, &[u8])]` | unchanged: written under `/data` at start | Each step lists the ones it reads in `needs`. `${data}` in args is already filled in by Fenix (`host.rs:1165-1179`). |

What goes from lang-kit's part of the first design: the model, the
page, decorations, the problems ledger (§9.3 makes it unneeded), the
queue and stopping, the notification, the modeline item, the agent
services, `page_state` and the results file. lang-kit's part shrinks
from "design-heavy, top tier" to the adapter above (first design §12,
piece 2).

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
  *provider* could read"), saying so on the page.
- A scan is one file's text. A project's first scan of its `scan`
  files is paced: one file a call, files read off the UI thread, at
  most 2,000 files a provider (the rest come from the listing).

## 7. Discovery, and the page

### 7.1 When tests are found

| | When |
|---|---|
| Scan an open file | When it's opened (before it's first drawn, so the gutter is right at once), 500 ms after typing pauses, on save. |
| Scan the project's `scan` files | When the provider starts in the project; then a file when it changes on disk (Fenix's watch on the project, as the explorer's). |
| List (`discover`) | When the tests page first opens in a session, after a run that built (cargo's `--list` is then free: 0.18 s, first design §2), after `test-provider-set`, and on `u`. **Not** when the project opens: a listing builds (cargo, `go test -list`) or imports the tests (pytest, Vitest, Jest), and Fenix starting stays cheap (roadmap's #33). Only in a trusted project (§12); `tests.list_automatically` (on) turns the automatic ones off. Never while a run goes: it waits for it. |

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
Windows (`supervisor.rs:62`):

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
long; its unfinished tests are `errored` "timed out after 30 min", and
the provider gets `timed-out`. An agent's `run_tests` has its own wait
(§10.3), which doesn't stop the run.

### 8.4 Stale, run on save, the failed again

- **Stale**: saving a file marks the tests in it stale (dimmed), and
  its scan adds and removes tests at once. A save elsewhere marks
  nothing: what a change reaches is the runner's to know, not Fenix's.
  Agents' saved edits (`agents.write = "save"`) count as saves.
- **Run on save**: `tests.on_save`, `off` (the default), `file` (the
  saved file's tests) or `failed` (the failed again), 1 s after the last
  save, in a trusted project; a save's run replaces a waiting one. Off
  by default because a Rust or Go run builds, and the packs already run
  clippy or a vet on save in the same build folder (first design §6).
- **The failed again** (`f`, `SPC T F`): the tests whose last result is
  failed or errored, as one selection. A case is asked for as itself;
  a provider that can't name a case alone runs its template (its plan
  decides).

### 8.5 Output, build failures and test failures

- A run's output goes to a buffer of its own, *Test output*, of the
  kind Task Output is (`BufferKind::TaskOutput`), so `Enter` on a line
  that names a place goes there (`app.rs:11040-11062`). Each step starts
  with its command line and label, as a task's `$ …` header
  (`app.rs:10876`). `o` and `SPC T o` show it beside the page; it's never
  in the task pane and never clears the quickfix list.
- **Didn't build, didn't load** (`broken`): never test failures. The
  tests under it are `errored` with the message; the head says the first;
  no problems are set for it: the language server and the pack's linter
  have them already, and twice is noise.
- **Failed**: a problem at its place (§9.3).
- **Crashed**: what the step covers and hasn't ended is `errored` with
  what the provider read ("its binary crashed: SIGABRT").
- A step that ends with tests it covered and never reported: if it was
  stopped, back to what they were; if it exited, `errored` "the run
  ended without a result for it".

### 8.6 Remote projects

| | In the SSH preview today | When remote Phase 4 (tasks) lands |
|---|---|---|
| Scanning | Works: Fenix gives the text (buffers through `fenix-vfs`); the provider runs here | The same |
| The page, results kept, gutter, problems | Work, from scans | The same |
| Listing and running | "Not available on a remote project yet", as every unmoved feature says (`remote-ssh.md:563-565`) | Through the stand-in, which `TaskRunner::spawn_command` takes as it is and whose kill ends what it ran (`remote-ssh.md:372-377`). A step's `needs` are copied to `~/.cache/fenix/tests/<extension>/` there first, and `${data}` is that folder. Places in the output are the machine's paths, mapped as its language servers' URIs will be (`remote-ssh.md:363-370`). |
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
that group. In no test: "line 12 of geo.rs isn't in a test — SPC T f
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
   - `build-first(plan)`: the plan runs as a run's steps (in *Test
     output*, read by the provider, so it learns where the binary is),
     then `tests-launch` is asked again with the same run, once. Rust:
     `cargo test --no-run --message-format=json` for the test's target,
     whose `compiler-artifact` names the binary.
3. The session is the debugger's as any other: breakpoints, DEBUG, the
   drawer, the Ember Trail.

It **isn't remembered** as the project's last launch
(`debug_last_launch`, `app.rs:11174`, `11289`), so `SPC u u` doesn't
start the test again next time: `SPC T d` does. A project with its own
launch in `.fenix/tools.json` still debugs a test this way: that launch
only takes the place of the extensions' launches on `SPC u u`
(`app.rs:11169-11185`).

What each pack gives (the first design's §7 table, unchanged in
substance): Rust a launch of the test binary with `[name, "--exact",
"--nocapture"]` after a build; Python debugpy with `module = "pytest"`
and the node id; Go Delve's `mode = "test"` with `-test.run
'^TestTable$/^negative_one$'`; Vitest `run <file>:<line>`, Jest
`--runInBand --watchAll=false <file> -t '^<name>$'` under js-debug. The
first design's workaround through `debug.pick_launch` and a launch
variable (its §7, "It's fragile") isn't needed.

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
| Pure crates, unit-tested | `ARCHITECTURE.md:3-5` | `fenix-tests`: the tree from scans and listings, dropping rules, matching, every state change, groups' counts, the queue (wait, join, replace), stop and timeout, staleness, results file round trip, 10,000 tests timed |
| The host against real guests, committed as `.wasm` so tests need no wasm toolchain | `fenix-wasm/tests/host.rs:1-2`, `tests/guests/build.ps1` | A new guest, `tests/guests/provider`, a provider of a made-up runner: scans `# test name` lines, plans `fake-test-runner` with arguments, reads its `PASS name 3ms` lines. Tests: the exports found; a 1.5 guest loads with none; a manifest with providers and no exports is told; deadlines; batches; a trap mid-run |
| A fake of the external program as a binary of the workspace | `fenix-lsp`'s `fake-language-server` (`fenix-lsp/src/bin`) | `fake-test-runner` in `fenix-tasks`: prints a script's lines with delays, starts a grandchild that holds the pipes (the cargo case), writes a line that isn't UTF-8, a 100 KiB line, 100,000 lines, exits by a signal |
| Tree-kill tests | `fenix-tasks/src/tree.rs:80-117` (Unix only today) | The grandchild gone after a stop, **on Windows too**: the job object path has no such test in `tree.rs` |
| Recorded protocol fixtures | `fenix-dap/tests/adapters/*.json` | None for runners: runners' formats are the packs', and their fixtures are the packs' (the first design's recorded outputs) |
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
| 2 | **API 1.6**: the WIT (§6.3), `[[contributes.test_providers]]` and its checks, `API = "1.6.0"`, the `tests` module and `Extension`'s methods, the host's `Optional`, `Call`s and `TestProviderSet`, the provider guest, `WRITING_EXTENSIONS.md`'s section and its "This Fenix offers" line | M | `fenix-extension-api`, `fenix-wasm`, `fenix-extensions` | Medium: what's merged here is what packs build against | Guest tests in `fenix-wasm`; manifest tests; a 1.5 guest still loads |
| 3 | **Fenix runs providers**: `app/tests_host.rs`: scans, listings, runs through `TaskRunner` (and its `cut` flag), batching, reports into the model, stop, timeout, the queue, *Test output*, results kept; `tests.enabled` (off) and the commands, unbound | L | `fenix-gui`, `fenix-tasks`, `fenix-config` | High: the lifecycle, Windows kill | The guest and `fake-test-runner` in app tests; tree kill on Windows; 100,000 lines through a provider, timed |
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

- **Step 2 is the first a pack can build against.** It can start in
  parallel with step 1, since the API doesn't depend on the model.
  From step 2 to the last, nothing a pack sees changes: the WIT, the
  manifest section and the Rust module stay as merged, unless a pack's
  work finds a mistake before Fenix 1.6.0 ships (§6.2). What a pack
  can test before step 3 is its own code, on lang-kit's fake host; from
  step 3, in the nightly with `tests.enabled` on.
- Steps 4, 5 and 6 can go in parallel after 3; 7 after 4.

### In fenix-extensions

| # | Step | Needs | Owns | Done when |
|---|---|---|---|---|
| E1 | **lang-kit's adapter**: `lang-kit::tests`, the `TestRunner` trait as the first design has it with §6.6's changes, wrapped in `Extension`'s five methods; `Kit::tests`; the assets; test-kit's `Fake` answering `tests_*` calls with canned lines; the packs' engine to 1.6 | Fenix step 2 | `crates/lang-kit`, `crates/test-kit` | A made-up runner's scan, plan and reader pass through the adapter; every lang-kit pack's tests pass unchanged |
| E2 | **Rust** (cargo test's human lines and cargo's JSON, per-target plans, nextest when installed, `build-first` launches) and the recipe for the others | E1 | `extensions/rust` | The first design's four "done when" on its fixtures; then, from Fenix step 3, in a nightly |
| E3a | **Python** (the pytest plugin, `uv run` or not, `PYTHONIOENCODING=utf-8` for Windows' pipes) | E2's recipe | `extensions/python` | Its fixtures |
| E3b | **Go** (scan, `go list`, `go test -json`, build events) | E2's recipe | `extensions/go` | Its fixtures, Go before 1.24 as the first design says |
| E3c | **Web** (Vitest and Jest, both reporters) | E2's recipe | `extensions/web` | Its fixtures for each runner |

- E1 can start as soon as Fenix step 2 is merged; E2 beside it, as the
  first design planned piece 2 and 3 (its §12).
- What goes from the first design's work list: piece 1's problems
  ledger and `Host` growth (Fenix owns problems, decorations, status,
  launches), most of piece 2 (the model, page, store, agents), piece 5's
  issues for Fenix (tree kill, `debug::start`, gutter marks: all in this
  plan), and the `SPC m t` group with its D6 (Go's `tidy` keeps `t`).
- Releases: each pack one minor with `api = "1.6"`, `fenix = ">=1.6, <2"`,
  published once Fenix 1.6.0 is out. No major for any pack: no pack key
  moves.

## 16. Open questions for the maintainer

| | Question | Options | Recommended |
|---|---|---|---|
| Q1 | Does a test run count as a task for extensions (`last_task`, `task-started`, `task-ended`)? | (a) yes, named `tests: …` (§10.2); (b) a new export for extensions that want test runs (`tests-ended`, more API); (c) no | **(a)**: Pets and the like react with no change. The cost: an extension that treats every task end as "the build finished" sees test runs too. None in fenix-extensions does today, but it should be said in `WRITING_EXTENSIONS.md`. |
| Q2 | When a provider serves a file, do `SPC p r`, the lens and `run_test` go through the explorer? | (a) always; (b) a setting to keep them on the task pane | **(a)**: two ways to run the same test with results in two places is what this plan exists to end. With `tests.enabled` off they're as in 1.5. |
| Q3 | The leader group | (a) `SPC T`; (b) under `SPC p` beside `SPC p r` (`SPC p e …`, an HTTP-extension user's `SPC p r` is already taken); (c) `SPC m t` per pack, as the first design had | **(a)**: one group in every language, free everywhere, and nothing per pack; (c) is what made Go's `tidy` move in the first design. |
| Q4 | Keep results across restarts? | (a) per project in the state folder, shown stale; (b) not at all | **(a)**: a restart shouldn't forget what failed. It's off with `tests.keep_results`. |
| Q5 | API numbering against the remote plan's "API 1.6" | (a) the first to land takes 1.6; (b) reserve 1.6 for remote | **(a)**, and the remote plan's line is changed when this lands. |
| Q6 | Readers in Fenix for standard formats (JUnit XML, TAP, `go test -json`) | (a) no; (b) later, as a new export a plan can name; (c) in 1.6 | **(b) at most, later**: a pack's reader is short, and every runner's quirks are better in the pack. A ctest or Gradle provider might want JUnit; it can parse it in a reader at the step's end. |
| Q7 | Fenix's built-in `SPC p r` finder once the four packs ship | (a) keep as the fallback; (b) retire it | **(a)**: it's small, and it serves someone without the pack. |

## 17. Risks

- **The API is frozen early.** Its shape is merged at step 2, before
  a real pack has run on it in Fenix. Mitigation: the Rust pack (E2) is
  built against it in the nightly before Fenix 1.6.0, and nothing is
  published before then (§6.2). A mistake found after is a new export,
  forever.
- **Per-call cost in wasm** isn't measured: 200 calls of 512 lines for a
  100,000-line run is an estimate. If it's slow, batches grow (the API
  doesn't fix their size) before anything else changes.
- **The gutter's width.** A cell more in every file a provider serves
  shifts its text one column, once, compared with 1.5. `tests.gutter`
  turns it off; if users mind, the fallback is the debugger's slot,
  breakpoints winning.
- **Windows' ordering of stdout and stderr** is per 10 ms tick, not per
  write (`supervisor.rs:322-325`). Providers read one stream; a runner
  that splits its story across both (cargo's `Running` on stderr) loses
  only which binary a line came from, which the first design reads
  from cargo's JSON on stdout instead.
- **Python's output on Windows.** Writing to a pipe, Python before 3.15
  uses the ANSI code page, not UTF-8, unless told; non-ASCII test names
  arrive replaced. The Python pack sets `PYTHONIOENCODING=utf-8` in its
  steps (E3a). **Not verified on Windows.**
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

## 18. As built

Nothing yet.
