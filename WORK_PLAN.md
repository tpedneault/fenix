# Reliability foundation

Branch: `reliability-foundation`, based on `01bb2e7`. The original checkout's
file explorer work was excluded during implementation. Milestones 1–6 form this
PR's completed scope; the follow-up roadmap below belongs in separate branches.

## Implemented in this first increment

- Shared `fenix-storage` staged writes for documents, configuration, and recovery.
  Serialization, explicit flush, and file sync finish before replacement.
  Windows uses `ReplaceFileW` for existing files to preserve destination ACLs.
  New documents use no-clobber persistence, including concurrent creation.
- Recovery continues with file watching disabled, and advances the maintenance
  deadline to avoid an expired-deadline event-loop spin.
- Unnamed documents receive persistent snapshot identities and recover without
  invented file paths. Version 2 snapshots retain support for reading version 1.
- Recovery errors produce a message and persistent modeline indicator.
- `:w <path>` saves an unnamed editable document to a new file, preserving spaces
  in paths and refusing existing destinations. Named documents still use `:w`.
- Task exit polling no longer holds the child mutex during blocking `wait()`.

## Milestone 2: task process supervision

Implemented in `fenix-tasks`, with the GUI consuming generation-tagged events:

- Windows task trees launch suspended, enter jobs, and then resume. An ownership
  job has kill-on-close semantics, so abrupt editor exit terminates its tasks.
- Cancellation and handle drop only signal a worker; neither waits nor joins.
- A single worker polls stdout/stderr, keeps per-line buffering bounded, and
  drains output before reporting one terminal outcome. Cleanup has a two-second
  deadline and reports errors, including underlying spawn and pipe errors.
- Normal leader exit terminates remaining descendants. Background services that
  must outlive a build belong in a separate terminal.
- Run identities prevent old output/completion from modifying a replacement run.
- Headless GUI tests use the same supervisor as the normal application.
- Unix process groups/nonblocking pipes are implemented but not validated on a
  Unix host in this session. Docker/LSP/DAP process lifecycles remain separate;
  this milestone covers build tasks.

Implementation uses process-wrap 10 for process groups and suspended Job Object
assignment: https://docs.rs/process-wrap/10.0.0/process_wrap/ . The additional
Windows ownership job handles host termination without running destructors.

## Milestone 3: transactional LSP text edits

- Added a pure workspace-edit planner in `fenix-lsp`: exact UTF-16 positions,
  document versions, duplicate/overlapping targets, and whole-request rejection
  of unsupported resource operations and annotated edits.
- Rename and edit-based code actions now open a read-only multi-file preview.
  Multiple actions require selection before preview. `a` / Enter applies all
  edits in memory; `q` / Escape cancels. Newline-only differences are visible.
- Requests capture buffer revisions. Preview and apply check disk state, current
  revisions, conflicts, permissions, and target identity. All unopened targets
  are loaded successfully before any document is mutated.
- Files remain unsaved, with recovery snapshots and notifications to running
  language servers. `:undo-refactor` reverses the latest transaction across all
  files, refusing if a target was edited, renamed, or closed. Per-file `u` remains
  available. Formatting uses the same range validation and one undo record.
- Refactor UI/state lives in `app/refactor.rs`, separate from the App monolith.

Scope: transactions cover in-memory text changes, not an atomic multi-file disk
save. Resource operations, annotations, command execution, and lazy code-action
resolution remain unsupported. Disk changes before a response use timestamps and
existing file fingerprints; same-metadata rewrites can evade that check. Apply
compares disk contents with the preview snapshot, but external processes are not
locked out and the normal save conflict protections still matter. File reads and
preview construction remain synchronous; large-workspace performance is follow-up.

## Validation (Windows, September 5, 2026)

The initial 463 storage/core/config/recovery/Vim tests passed on September 4.
Application Control initially blocked Tree-sitter's build script. A normal retry
subsequently compiled the GUI successfully; no policy settings were changed.

Task library: 35 passed, one ignored subprocess fixture. The fixture is explicitly
launched by lifecycle tests covering cancellation, drop, descendants retaining
pipes, orphaned descendants, output floods, invalid UTF-8/trailing output,
cleanup deadlines, pipe errors, and host exit without destructors.

The initial GUI task filter passed 13 tests. The final regression run
`cargo test --workspace --exclude fenix-docker --no-fail-fast --quiet` produced
2,287 passes, one failure, and 21 ignored tests. Its GUI suite passed 1,127 tests,
including the recovery, unnamed-save, stale-event, and spawn-error regressions;
one unrelated Docker log-follower test failed because `sh` is unavailable.
The earlier full workspace run stopped in fenix-docker: 42 passed and one failed
because standalone `echo` is unavailable. These two pre-existing test portability
issues prevent an entirely green Windows workspace run.

`cargo clippy -p fenix-tasks --all-targets -- -D warnings` passes. Removed a
redundant trim before split_whitespace in the task-config parser to satisfy that
check without changing parsing behavior. `git diff --check` passes.

The regression log is at `target/reliability-tests.log` (untracked build output).
No interactive GUI smoke test or Unix-host test was performed.

Known broader-suite issue: `fenix-docker::process::tests::
run_ndjson_skips_unparsable_lines_and_keeps_the_rest` assumes a standalone `echo`
program and fails on this Windows setup. The GUI's separate Docker log-follower
test also invokes `sh`. Neither implementation was modified by these milestones.

## Milestone 3 validation (Windows, September 5, 2026)

`cargo test --workspace --exclude fenix-docker --no-fail-fast --quiet --
--skip docker_log_follower_delivers_lines_and_stops_promptly_on_drop` passed:
2,301 tests, zero failures, 21 ignored. The GUI portion passed 1,134 tests with
six ignored and the one known shell-dependent test filtered out. This excludes
the entire fenix-docker package for the previously documented portability issue.
Log: `target/milestone3-tests.log`.

After the final cursor-clamping adjustment and Ex-command regression test,
`cargo test -p fenix-gui -p fenix-vim refactor --quiet` passed all eight tests.
The LSP package passed all 41 tests. `cargo clippy -p fenix-lsp --lib -- -D warnings`
passes. `cargo build -p fenix-gui --quiet` produced `target/debug/fenix.exe`
(with the existing JiraBadgeColor dead-code warning). Its all-targets Clippy check reports three pre-existing `cmp_owned` warnings
in client protocol tests. `git diff --check` passes. Interactive GUI and live
language-server smoke tests remain outstanding.

## Milestone 4: Windows CI and regression gates

Implemented `.github/workflows/ci.yml` and the shared PowerShell 7 runner
`scripts/ci.ps1`. Pushes, pull requests, and manual runs trigger two checks:

- **Windows workspace**: every workspace test without package/test-name
  exclusions, strict Clippy for storage/tasks/LSP, and a debug editor build.
  The checkout path includes a space. Successful jobs retain `fenix.exe`.
- **Linux reliability**: the filesystem, core/config/recovery, task process,
  RPC/LSP/DAP, and Vim suites, plus the same strict lint gate.

Both jobs use `--locked`, record toolchain versions, retain logs and per-step
exit codes/timings, and have job timeouts. Actions are pinned to verified commit
hashes. Tokens are read-only and checkout credentials are not persisted.

Replaced standalone `echo` and `sh` fixtures with isolated Rust test subprocesses.
The Docker log follower test now verifies both output lines before checking drop.
Added a save regression for Unicode paths with spaces and CRLF content. Fixed
three existing test-only LSP comparison warnings so all-target Clippy can gate
that crate. The old milestone 2/3 Docker test exclusions are no longer needed.

### Validation (Windows, September 5, 2026)

`./scripts/ci.ps1 -Suite Workspace` passed every gate: **2,346 tests passed,
zero failures, 23 ignored**, followed by strict scoped Clippy and GUI build.
The ignored count includes subprocess fixtures run explicitly by their parent
tests and existing live-integration tests. Log directory:
`target/ci/workspace-20260905-084542-3da742db/`.

`./scripts/ci.ps1 -Suite Reliability` also passed locally on Windows, validating
the smaller package selection and lint command. This is not a Linux-host run.

Actionlint 1.7.12 validated the workflow with no errors. Failure injection for
each of test, Clippy, and build confirmed the script exits unsuccessfully and
records the underlying exit code (7) rather than swallowing it through logging.

The workflow has not been pushed or executed on hosted runners, and Linux
execution is not locally verified. After the first hosted run, repository rules
must require **Windows workspace** and **Linux reliability** to enforce the merge
gate; branch-protection settings were not changed. See `docs/CI.md` for commands,
coverage, prerequisites, and artifact retention. Live GUI testing remains separate.

## Milestone 5: project ownership and structured tool environments

- Language servers are keyed by canonical project root and language. Each
  connection has a generation ID, so stale responses/disconnects cannot affect
  replacement sessions. Diagnostics and refactor targets are checked against
  the originating project. Root aliases reuse a session.
- `.fenix/tools.json` adds literal executable, argument-array, working-directory,
  and child-environment settings for LSP, DAP, and tasks. Missing configuration
  keeps existing defaults/INI behavior; malformed configuration reports errors.
  Windows executable lookup uses the child's PATH/PATHEXT and never mutates the
  editor environment. Explicit PATH overrides cannot fall back to the parent.
- `:lsp-restart` reloads the current project's servers. Failed/unavailable servers
  wait for explicit restart instead of repeatedly spawning during frame updates.
- Task rerun history is per project. Task output paths use the configured cwd.
  Cross-project cancellation/replacement is guarded while a task is active.
- Debugger launch arguments, cwd, and environment are captured at startup, including
  previously ignored legacy launch args. Adapter configuration is separate from
  debuggee configuration. Controls and breakpoints respect project ownership;
  relative source paths use launch cwd and stale adapter events are ignored.
- The CI reliability selection and strict Clippy gate now include `fenix-project`.

The editor still supports one active task and one active debugger globally;
this increment prevents accidental cross-project control rather than adding
concurrent panels. Language servers do run independently for multiple projects.
Remote/WSL identities, live adapter smoke tests, and asynchronous LSP/DAP teardown
remain follow-ups. See `docs/PROJECT_TOOLS.md` for examples and precedence.

### Milestone 5 validation (Windows, September 5, 2026)

The final `./scripts/ci.ps1 -Suite Workspace` run passed all gates:
**2,360 tests passed, 0 failures, 24 ignored**, then scoped all-target Clippy
(storage/tasks/LSP/project) and a successful debug editor build. New tests cover
same-language/same-request-ID project isolation, retired connection generations,
foreign diagnostics/refactors, project task history, literal arguments, child
PATH/environment handling, structured debug launch data, and project-only restart.

Log directory: `target/ci/workspace-20260905-145726-f32a195a/`.
`git diff --check` passes. Real subprocess fixtures validate launch settings;
live language-server/debug-adapter smoke tests and Linux-host execution remain
outstanding. The existing JiraBadgeColor dead-code warning remains outside the
scoped strict lint gate.

## Follow-up work for separate branches

1. Finish persistence hardening: project-wide write paths, hard-link semantics,
   external-change races, snapshot cleanup failures, and asynchronous disk I/O.
   Atomic replacement does not preserve hard-link identity; file sync alone is
   not a guarantee against every power-loss or filesystem failure.
2. Task process supervision is implemented (see milestone 2 above). Follow-ups:
   run Unix lifecycle tests and migrate other integrations to the supervisor where
   their streaming and lifetime requirements match.
3. Transactional text workspace edits are implemented (milestone 3 above).
   Follow-ups: resource-operation lifecycle, action resolution/commands, stronger
   disk baselines, and large-refactor performance.
4. Windows CI and regression gates are implemented (milestone 4 above).
   Follow-ups: first hosted runs, required-check configuration, and expanding
   lint/format coverage once existing debt is addressed.
5. Project ownership and structured tool settings are implemented (milestone 5).
   Follow-ups: concurrent task/debug panels, remote environment identities, and
   live language-server/adapter smoke tests.
6. Session restoration is implemented (milestone 6 below). Follow-ups: native
   multi-monitor smoke testing, asynchronous checkpoints, and richer panel state.
7. Extract document, process, workspace, language, and panel ownership from App.
8. Complete Windows paths, encodings/newlines, IME, Unicode, and terminal profiles.
9. Deepen the main language's formatting/build/test/debug workflow.
10. Measure typing latency, large repositories/files, output floods, and idle use.
11. Add WSL/remote workspace services if needed.
12. Add stable integration APIs before expanding the dashboard collection.

## Milestone 6 — Session restoration

Implemented versioned, atomically replaced session checkpoints alongside config.
Documents (including unsaved and hidden buffers), shared pane identities, workspace
names, native window layouts, split ratios, focus, cursors, and scroll positions
are reconstructed with fresh runtime IDs. Explicit CLI files retain focus.

Unsaved text stays unsaved; missing dirty documents become scratch buffers and disk
conflicts survive repeated restarts. Newer recovery snapshots take precedence over
older checkpoints. Tool panels restore placeholders without restarting processes.
`:session-save` checkpoints now and `:session-quit` preserves work across exit.
Discarding quit clears session edits and recovery; failed writes keep the app open.
Invalid session files remain untouched until explicit replacement. Configuration
can disable session persistence or merge saved windows into one native window.

See `docs/SESSION_RESTORATION.md` for behavior and limitations. Windows validation completed with **2,374 tests passed, 0 failures,
24 ignored**, scoped all-target Clippy passing, and a successful debug editor
build. Twelve session regressions cover round trips, shared panes/windows, dirty
and clean buffers, missing files, repeated disk conflicts, newer recovery data,
explicit discard, invalid files, write failures, opt-out, and CLI path aliases.
Two portable layout tests cover structure/focus and invalid layouts. The existing
same-length watcher test now sets its timestamp explicitly to remove a Windows
clock-resolution assumption. The existing JiraBadgeColor dead-code warning remains.

Final logs: `target/ci/workspace-20260905-211209-78ec6968/`.
`git diff --check` passes. Native multi-monitor and Linux-host smoke tests were not
performed.

## PR integration validation

Merged `origin/master` at `b3447c4` into `reliability-foundation` without conflicts.
The merged Windows workspace passed **2,558 tests, 0 failures, 24 ignored**, scoped
all-target Clippy, and the debug editor build. `git diff --check` passed.
Logs: `target/ci/workspace-20260905-213914-d1386176/`.
The existing JiraBadgeColor warning remains; hosted Linux and native multi-monitor
smoke tests are not claimed by this local validation.
