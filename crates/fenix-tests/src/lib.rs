//! The test explorer's model (plan `docs/plans/test-explorer.md` §4,
//! ADR 0005): a project's tests as its test providers found them, their
//! states and results, and its runs. Sans-I/O, as `fenix-acp`'s `Hub`
//! is: no processes, no threads, no clock, no extension. The host
//! (`fenix-gui`'s `app/tests_host.rs`) tells it what providers and runs
//! said -- scans, listings, plans, reports, step ends, the time -- and
//! asks it what to run next and what to show. The page and the file's
//! views ask it for rows, marks, the test at a line, problems and the
//! head.
//!
//! - `records`: the model's copies of the API's test records, one for
//!   one (§6.3).
//! - `model`: the tree, its three sources per node, their merging and
//!   dropping, states and counts (§4).
//! - `run`: the queue, asks, plans, steps, reports, what a step covers,
//!   stop and timeout, listings and builds (§4 "Runs", §7.1, §8, §11).
//! - `view`: rows, the filter, tabs, gutter marks, the test the cursor
//!   is in, problems and the head (§7.2, §9).
//! - `kept`: the results file's text (§4 "What's kept").
//! - `limits`: §6.5's sizes and the one way each is cut.
//!
//! # Its callers
//!
//! **The host** (step 3) makes one `RunIds` and gives it to every
//! project's model (`Model::with_run_ids`), so a run id is unique while
//! Fenix runs. It adds providers, each a `ProviderId` -- its extension's
//! id and its own (`add_provider`, `provider_set`) -- gives scans (`scan`, `scan_files`), saves
//! (`file_saved`, `set_unsaved`) and the results file
//! (`restore_results`, `results_file`). It asks for runs and listings
//! (`request` with a `Selection` -- this test, a file, all, the failed;
//! `list`, `page_opened`, `request_build`), then drives them: it calls
//! `next(now)` whenever nothing is under way and does what the `Action`
//! says (a debug launch draws its run id with `run_id`, and a
//! `build-first` answer runs as that run, `request_build`) -- `Plan` (call `tests-plan` with the `Ask` given, which holds
//! §4's fill-in and 10,000-record cap, then `planned`), `Step` (start it,
//! then `reports` per `tests-read` and `step_ended`), `NotStarted` (send
//! `stopped` for each), `Ended` (the notification, `last_task`). `stop`,
//! `time_out` and `provider_stopped` end a run early; the host kills the
//! step under way. It turns the API's records into this crate's one for
//! one (`records`), and says on each provider's output page what
//! `take_notices` gives.
//!
//! **The page and the file's views** (steps 4 and 5) ask for `rows` (a
//! tab, a filter, which rows are open), `node` (its fields, state,
//! staleness, result, counts), `providers`, `summary` (the head),
//! `failures`; and for a file, `serves`, `marks` for the lines in view,
//! `test_at`, `in_file` (the lens) and `problems`. `revision` says when
//! to lay out again. None of these walks more than what it shows,
//! except the filter's one pass over the labels.
//!
//! # Decisions
//!
//! Where the plan is silent, or two of its statements meet, the model
//! takes the simplest reading, written down here.
//!
//! 1. **Paths are the host's** (§6.4 "Paths"). The host maps every
//!    path a provider gives (`/project/...`, relative, a full path) to
//!    the project's form, relative with `/`, before the model sees it;
//!    the model holds and compares paths in that form, as strings. A
//!    place outside the project keeps its full path, and the host tells
//!    the two apart. What the model gives back (an ask's records,
//!    `TestNode::given_back`) is in that form too: the host turns a
//!    project's file back into `/project/<path>`, the one form a provider
//!    is given (§6.4 "What Fenix gives back").
//! 2. **A scan's missing path** (§6.3 `found-test.path`: "none for a
//!    test or case is the file scanned") is the file scanned for every
//!    kind but `group`: a suite and a file are in the file they were
//!    scanned from, while a group spans files.
//! 3. **An id given twice in one answer** is one node, the later record
//!    replacing the earlier (§4: "within one source a later `found`
//!    replaces that source's record whole"). Two of Jest's `it("adds")`
//!    in one describe are one test to the model, as they are to its
//!    runner's id.
//! 4. **A group several files' scans give** (§4) takes its fields from
//!    the latest of those scans.
//! 5. **A last line before the first line** is left out.
//! 6. **A parent the provider hasn't given**, or one that would make a
//!    loop, or a chain deeper than 256, puts the node at its provider's
//!    top until the parent comes (§4: "Every node is a found-test").
//! 7. **What's counted** (§4 "A group's state"): a test or case with no
//!    test or case under it. A template counts as one test until its
//!    cases are found, then as its cases; its own result (Go's parent
//!    test) still shows on its own row.
//! 8. **A group's state** follows §4's rule with `queued` added between
//!    running and passed (the page shows a waiting group as `·`), and
//!    failed before errored. A line's mark is the worst of what starts
//!    there, worst first: failed, errored, running, queued, unknown,
//!    skipped, passed; it's dimmed when that node is stale.
//! 9. **Running** (§4 "States"): a step's tests are running when it
//!    starts, as for a runner that doesn't say; at the step's first
//!    `started`, the runner is seen to say, so the step's tests that
//!    haven't started go back to queued, and each `started` makes its
//!    test running.
//! 10. **A step judges only what the run has** (§8.5): the tests its
//!     selection holds, and what's found under them in the run. A
//!     `covers` naming a whole group for a run of one test doesn't error
//!     the group's other tests; nor does a `broken` or `crashed`.
//! 11. **What a step with empty `covers` covers**, for a `broken` or
//!     `crashed` with no id (§6.3 `test-crash`: "all the step covers"),
//!     is the plan's rest: what of the ask no step names. So a plan of
//!     one step that names nothing errors its whole ask on a crash. A
//!     `broken` or `crashed`, with an id or none, errors only what has
//!     no result in the run yet: a test that ended earlier, in this step
//!     or another, keeps its result.
//! 12. **A plan that's an error** (`tests-plan`'s `err`, §8 silent):
//!     the provider's tests in the run are errored with its message;
//!     after a stop, nothing is (they go back to what they were, §4).
//! 13. **A plan of no steps**: its ask is judged at once as if its last
//!     step exited (errored, "the run ended without a result for it").
//!     A listing of no steps isn't complete: it drops nothing.
//! 14. **A timeout** (§8.3) errors every test of the run without a
//!     result, those of steps that never started and of providers not
//!     yet asked included: "its unfinished tests are errored".
//! 15. **A provider that stops** (a trap, started again in another
//!     project, §6.4) while its turn in a run is under way stops the
//!     whole run, its tests back to what they were, and no call goes to
//!     it after; one whose turn hadn't come is left out of the run.
//! 16. **The queue** (§8.1): only the running run's tests are queued; a
//!     waiting run's keep their marks, and its selection is worked out
//!     when it starts (the failed then, not when asked). A debug
//!     launch's build goes before a waiting run; listings after it.
//! 17. **A listing's `broken` or `crashed`** makes it incomplete and
//!     changes no result; a `found` in a build is ignored, as an `ended`
//!     is (§11).
//! 18. **The results file** keeps results read at start for tests not
//!     found since, until 8 MiB pushes the oldest out: a test in a file
//!     not scanned this session keeps its result. A failure's text is
//!     kept by its end, as everywhere. A result is kept by its
//!     provider's extension, its provider and its id (version 2); a file
//!     of another version is refused whole.
//! 19. **Problems** (§9.3) come from `failed` results only (errored is
//!     "couldn't run to a result", and §8.5 sets no problem for a broken
//!     build), and not from a stale result (its line may have moved,
//!     §9.3 "Cleared ... when its file is saved") or a test being run
//!     again ("Cleared when that test runs again").
//! 20. **An ask** (§4) holds the topmost nodes selected, in display
//!     order. A file's or suite's fill-in -- every node under it, the
//!     suites between included, in display order -- follows it whole or
//!     not at all, so a provider never takes part of a file for all of
//!     it, and every fill-in record's `parent` is in the ask while a
//!     selected node's isn't. Fill-ins take the room left under 10,000
//!     in display order: one that doesn't fit is left out, and a later,
//!     smaller one may still fit. Coarsening replaces the deepest nodes
//!     by their parents first. A record's `parent` is the one the model
//!     put it under (none at its provider's top).
//! 21. **An unknown id's `ended`** goes under "the deepest node of the
//!     file its `at` names" (§4): the deepest group, file or suite of
//!     that file whose lines hold `at`'s line, else the deepest of it;
//!     between equals, the one that starts first, then the id that sorts
//!     first. They're looked for in the provider's index of them by file,
//!     and the test is added without working the tree out, which is done
//!     once at the end of the call.
//! 22. **The test at a line** (§9.4), among several that hold it: the
//!     one that starts last, then ends first, then is deepest, then
//!     the provider whose id sorts first.
//! 23. **`list-after`** (§7.1, F7) lists after a plan whose steps all
//!     exited with nothing `broken`; a `crashed` doesn't prevent it.
//! 24. **A run's records go** (§4) for tests strictly under a node the
//!     run asked for (their template or group), or every one of the
//!     provider's after a `run-all`.
//! 25. **Stale is ordered by the model's own clock**, a count of what
//!     happened, not the time: a save and a result in the same
//!     millisecond still come in order.
//! 26. **A test's `data` over 4 KiB isn't kept** (§6.5, §19): cut, it
//!     wouldn't be given back "as it was". The host drops it the same way.
//! 27. **A file's tests** (`SPC T f`) are every provider's topmost
//!     nodes, but groups, whose file is that file.
//! 28. **A listing waiting** behind a run starts after it, and after a
//!     run waiting: you asked for that run, and a listing then is
//!     still warm.
//!
//! Added after the review of steps 1 and 2 (plan, "Review of steps 1
//! and 2"):
//!
//! 29. **A test that leaves every source** -- cut and pasted back, a
//!     scan of half-typed text -- has its result kept as one read at
//!     start is: shown stale when it's found again, and in the results
//!     file meanwhile. Not for a provider removed, nor for a case its
//!     template's run dropped (31).
//! 30. **A node's place falls through as one**: `path`, `line` and
//!     `end_line` all from the first source, scan then listing then run,
//!     that gives a path or a line; never a listing's file with a run's
//!     line.
//! 31. **A case its template's complete run dropped** goes with the
//!     result the run's end judged it ("ended without a result"), which
//!     the run's counts leave out.
//! 32. **A node its provider can't name** -- its id was cut (over 1 KiB)
//!     and its `data` isn't kept (past the 16 MiB budget, or no source
//!     that has it now gave any) -- is never asked for by itself: an ask
//!     has its nearest ancestor that can be named instead, a fill-in
//!     holding one is left out, and the host doesn't debug it
//!     (`TestNode::can_be_named`). A provider can always name a node by
//!     its id up to 1 KiB, and by its `data` past it.
//! 33. **An id over 1 KiB** is its start up to 1,007 bytes, cut back to
//!     a character's start, then `#` and the FNV-1a 64-bit hash of the
//!     whole id as 16 lowercase hex digits (`cut_id`): the same wherever
//!     it comes, and two that share their first KiB stay two.
//! 34. **Run ids are the host's** (`RunIds`, shared by every project's
//!     model); a `build-first` plan runs as the run its launch's first
//!     `tests-launch` was asked with (`run_id`, `request_build`).
//! 35. **A provider is its extension and its id** (`ProviderId`): two
//!     extensions that both declare `rust` are two providers. The page
//!     orders them by id, then extension.
//! 36. **A step's scope**, and what of it has no result, are worked out
//!     once per step and shape of the tree, so that many `broken` or
//!     `crashed` reports in one call cost what's under each, not the run
//!     each; the head's count of the waiting run, once a revision.

mod kept;
mod limits;
mod model;
mod records;
mod run;
mod view;

pub use kept::ResultsFileError;
pub use limits::{cut_end, cut_id, cut_start, fnv1a_64, is_cut_id, ASK_RECORDS, DATA_BYTES, ID_BYTES, ID_PREFIX_BYTES, LABEL_CHARS, MESSAGE_BYTES, RESULTS_FILE_BYTES, TESTS_PER_PROVIDER, TEXT_BYTES};
pub use model::{Counts, Model, NodeId, Notice, ProviderId, ProviderRef, State, TestKey, TestResult};
pub use records::{Ask, Ended, Found, Kind, Location, Outcome, Plan, Report, Step, StepEnd};
pub use run::{Action, Halt, Mode, Requested, RunEnd, RunId, RunIds, RunKind, Selection};
pub use view::{Item, LastRun, Mark, Problem, Progress, ProviderView, Row, RowQuery, Summary, Tab, TestNode};

#[cfg(test)]
mod tests;
