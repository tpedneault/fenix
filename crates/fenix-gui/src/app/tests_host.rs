//! The test explorer's host (plan `docs/plans/test-explorer.md`, step 3):
//! Fenix running test providers. It finds the tests of a project's files
//! (`tests-scan`), lists and runs them (`tests-plan`, each step's command
//! through `fenix_tasks::TaskRunner`, its output to `tests-read` in
//! batches, its end to `tests-step-ended`), and puts what comes back into
//! the project's `fenix_tests::Model`, which decides everything else:
//! what's asked, what runs next, what each report means. Behind
//! `tests.enabled`: off, nothing here calls a provider.
//!
//! # Its parts
//!
//! - **Plain functions** at the top, tested on their own: the API's
//!   records turned into the model's and back (`model_found`,
//!   `api_found`, `model_reports`, `api_ask`), every path a provider
//!   gives mapped to the project's (`model_path`) and back
//!   (`guest_path`), an ask held to its bytes (`capped_ask`), a step's
//!   command line as Windows counts it (`command_line`), escape codes
//!   taken out of a line (`without_escapes`), a step's lines batched
//!   (`Batch`), and how a step ended (`step_end`).
//! - **`TestsHost`**, the state: one `ProjectTests` per project root --
//!   its model, its providers, the call under way for its run, the step
//!   under way (`StepRun`: its `TaskRunner`, its lines waiting, whether a
//!   `tests-read` is out), its *Test output* buffer, its first scan --
//!   and, across projects, the calls out by token, the replies and step
//!   events waiting, and the one `RunIds` every model draws from.
//! - **The pump** (`App::tests_pump`): applies the replies and step
//!   events waiting, then drives each project: a step's lines to
//!   `tests-read`, its end to `tests-step-ended`, else the model's `next`
//!   -- a plan to ask for, a step to start, steps to tell they won't
//!   start, a run that ended. One call for a project's run is out at a
//!   time; a reply or a step's event comes back into `tests_reply` and
//!   `tests_step_event`, which queue it and pump again. Re-entered while
//!   it pumps, it returns: the outer loop takes what was queued.
//! - **The tick** (`App::tests_tick`, from `plan_wake`): what's due by
//!   the clock -- a buffer's scan once typing pauses, a run on save, a
//!   run's timeout, the next file of a first scan -- and the pump.
//! - **In the file** (`file`, step 5): the gutter's test column and its
//!   marks for the lines drawn, a click on one, the right click's items,
//!   a result after a test's line, failures as problems under
//!   `fenix:#tests`, worked out when the model changes.
//! - **Debugging a test** (`debug`, step 6): `tests-launch` asked, a
//!   `build-first` plan run as the launch's run, `tests-launch` asked
//!   again, and the debug session started from the launch it gives.
//! - **The page's host** (`page`, step 4): `tests.open`, the tests page's
//!   actions, its head counted at most ten times a second, the modeline's
//!   item, and *Test output* beside the page; the page itself is
//!   `crate::tests_page`.
//!
//! # Where it meets the rest of Fenix
//!
//! `wasm_host` sends its calls (`tests_call`) and hands back their ends
//! (`Pending::Tests`), a provider's `test-provider-set`, an extension
//! ready, stopped, started again or switched off; `app.rs` its step
//! events (`FenixUserEvent::TestsStep`), a save, and the tick.

use super::*;
use fenix_tasks::{OutputStream, TaskEvent, TaskOutcome, TaskRunner};
use fenix_tests::{Model, ProviderId, RunId, RunIds, Selection};
use fenix_wasm::host::{Call, FoundTest, PlannedStep, Reply, TestAsk, TestKind, TestLine, TestReport, TestStepEnd};
use fenix_wasm::test_limits;
use std::collections::{BTreeSet, VecDeque};
use std::sync::mpsc;

/// How long typing pauses before an open file is scanned again, and the
/// least time between two scans of it (§6.4 "Scans").
const SCAN_PAUSE: Duration = Duration::from_millis(500);
/// How long after the last save a run on save starts (§8.4).
const SAVE_RUN_AFTER: Duration = Duration::from_secs(1);
/// The files of a project a provider's first scan reads (§6.7).
const SCAN_FILES: usize = 2_000;
/// How many files' scans go to the model at once in a first scan
/// (`Model::scan_files`: the tree is worked out once a batch).
const SCAN_BATCH: usize = 200;
/// Past this much of a step's output waiting for its provider, it's
/// given no more (§6.7).
const BEHIND_BYTES: usize = 32 << 20;
/// Nor more than this many lines: 1,024 batches.
const BEHIND_LINES: usize = 1024 * test_limits::MAX_READ_LINES;
/// How long a project's trust is taken as it was last looked at.
const TRUST_FOR: Duration = Duration::from_secs(2);
/// How often a first scan waiting on its thread looks again.
const WALK_POLL: Duration = Duration::from_millis(20);
/// How long Fenix's exit waits for steps' trees to be killed and results
/// written.
const EXIT_WAIT: Duration = Duration::from_secs(2);
/// The bytes of a `run` ask Fenix sends, its records' strings in all:
/// the cap its answers have (`test_limits::MAX_ANSWER`), so an ask of
/// 10,000 records of 10 KiB each can't fill a guest's memory.
pub(crate) const ASK_BYTES: usize = test_limits::MAX_ANSWER;

/// The test explorer's own commands: what `tests.enabled` off hides
/// (`App::command_hidden`), and nothing else -- an extension's own
/// `tests.coverage` stays offered.
pub(crate) const COMMANDS: &[&str] = &["tests.open", "tests.run_here", "tests.run_file", "tests.run_all", "tests.run_failed", "tests.run_last", "tests.debug_here", "tests.stop", "tests.output"];

// What's plain: records, paths, lines, command lines.

fn model_kind(kind: TestKind) -> fenix_tests::Kind {
    match kind {
        TestKind::Group => fenix_tests::Kind::Group,
        TestKind::File => fenix_tests::Kind::File,
        TestKind::Suite => fenix_tests::Kind::Suite,
        TestKind::Test => fenix_tests::Kind::Test,
        TestKind::Case => fenix_tests::Kind::Case,
    }
}

fn api_kind(kind: fenix_tests::Kind) -> TestKind {
    match kind {
        fenix_tests::Kind::Group => TestKind::Group,
        fenix_tests::Kind::File => TestKind::File,
        fenix_tests::Kind::Suite => TestKind::Suite,
        fenix_tests::Kind::Test => TestKind::Test,
        fenix_tests::Kind::Case => TestKind::Case,
    }
}

/// `path`'s segments, `.` and `..` worked out, joined by `/`: none when a
/// `..` would climb above its start.
fn lexical(path: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            part => out.push(part),
        }
    }
    Some(out.join("/"))
}

/// Whether `path` (with `/`) is a full path: `/x`, `//server/x`, `C:/x`.
fn is_full(path: &str) -> bool {
    let b = path.as_bytes();
    path.starts_with('/') || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
}

/// A full path with `/`, its `.` and `..` worked out, keeping its root
/// (`/`, `//`, `C:`).
fn full_lexical(path: &str) -> String {
    let (root, rest) = if let Some(rest) = path.strip_prefix("//") {
        ("//", rest)
    } else if let Some(rest) = path.strip_prefix('/') {
        ("/", rest)
    } else if path.len() >= 2 && path.as_bytes()[1] == b':' {
        let (drive, rest) = path.split_at(2);
        return format!("{drive}/{}", lexical_clamped(rest));
    } else {
        ("", path)
    };
    format!("{root}{}", lexical_clamped(rest))
}

/// `lexical`, a `..` at the top staying at the top, as a system does.
fn lexical_clamped(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            part => out.push(part),
        }
    }
    out.join("/")
}

/// `full` below `root` (both full, with `/`): the path between, with
/// `/`. Case doesn't count on Windows.
fn below(full: &str, root: &str) -> Option<String> {
    let root = root.trim_end_matches('/');
    let (head, rest) = (full.get(..root.len())?, full.get(root.len()..)?);
    let same = if cfg!(windows) { head.eq_ignore_ascii_case(root) } else { head == root };
    if !same {
        return None;
    }
    match rest.strip_prefix('/') {
        Some(rest) => Some(rest.to_string()),
        None if rest.is_empty() => Some(String::new()),
        None => None,
    }
}

/// A path a provider gave, as the model holds it (§6.4 "Paths";
/// `fenix-tests` Decisions, 1): a file of the project relative to `root`,
/// with `/`; anything else, the machine's full path. `/project/...`,
/// relative (to `base`: the project for a scan, a step's folder for a
/// report), a full path; `\` or `/`; a drive in any case on Windows.
pub(crate) fn model_path(given: &str, root: &Path, base: &Path) -> String {
    model_path_in(given, std::slice::from_ref(&root.to_path_buf()), base)
}

/// `model_path` for a project that has several spellings: `roots[0]`,
/// its canonical root, then the others it's reached by (a symlink, an
/// 8.3 name, a `subst` drive): a full path under any of them is the
/// project's.
pub(crate) fn model_path_in(given: &str, roots: &[PathBuf], base: &Path) -> String {
    let slashed = given.replace('\\', "/");
    // A verbatim path (`\\?\C:\...`, `\\?\UNC\server\...`) as the
    // plain one it names, before it's looked for under the project.
    let slashed = match slashed.strip_prefix("//?/") {
        Some(rest) => match rest.strip_prefix("UNC/") {
            Some(unc) => format!("//{unc}"),
            None => rest.to_string(),
        },
        None => slashed,
    };
    if slashed == "/project" || slashed.starts_with("/project/") {
        // A `..` out of the project is no file of it: as it was given.
        return lexical(&slashed["/project".len()..]).unwrap_or(slashed);
    }
    let full = if is_full(&slashed) { full_lexical(&slashed) } else { full_lexical(&format!("{}/{slashed}", base.to_string_lossy().replace('\\', "/"))) };
    for root in roots {
        if let Some(rel) = below(&full, &full_lexical(&root.to_string_lossy().replace('\\', "/"))).filter(|r| !r.is_empty()) {
            return rel;
        }
    }
    fenix_lsp::normalize(PathBuf::from(&full)).to_string_lossy().into_owned()
}

/// A path as the model holds it, as a file on this machine (the gutter,
/// problems, a jump): a file of the project under `root`, whatever it
/// resolves to (a symlink inside the project to a file outside it is the
/// project's); a full path as it is. None for a path held as given
/// because it climbed out of the project (`/project/../x.rs`): no place,
/// never opened.
pub(crate) fn place_path(root: &Path, path: &str) -> Option<PathBuf> {
    let slashed = path.replace('\\', "/");
    if slashed == "/project" || slashed.starts_with("/project/") {
        return None;
    }
    if is_full(&slashed) {
        return Some(PathBuf::from(path));
    }
    lexical(&slashed).filter(|rel| !rel.is_empty()).map(|rel| root.join(rel))
}

/// A path as the model holds it, as Fenix gives it back to a provider
/// (§6.4 "What Fenix gives back"): `/project/<path>` for a file of the
/// project, the full path for one outside it.
pub(crate) fn guest_path(path: &str) -> String {
    if is_full(&path.replace('\\', "/")) {
        path.to_string()
    } else {
        format!("/project/{path}")
    }
}

/// A provider's `found-test` as the model's, its path mapped from `base`.
pub(crate) fn model_found(f: FoundTest, roots: &[PathBuf], base: &Path) -> fenix_tests::Found {
    fenix_tests::Found {
        id: f.id,
        parent: f.parent,
        label: f.label,
        kind: model_kind(f.kind),
        path: f.path.map(|p| model_path_in(&p, roots, base)),
        line: f.line,
        end_line: f.end_line,
        skip: f.skip,
        template: f.template,
        data: f.data,
    }
}

/// A node as Fenix gives it back (`test-ask.run`, `tests-launch`).
pub(crate) fn api_found(f: fenix_tests::Found) -> FoundTest {
    FoundTest {
        id: f.id,
        parent: f.parent,
        label: f.label,
        kind: api_kind(f.kind),
        path: f.path.as_deref().map(guest_path),
        line: f.line,
        end_line: f.end_line,
        skip: f.skip,
        template: f.template,
        data: f.data,
    }
}

fn model_location(at: fenix_wasm::host::TestLocation, roots: &[PathBuf], base: &Path) -> fenix_tests::Location {
    fenix_tests::Location { path: model_path_in(&at.path, roots, base), line: at.line, column: at.column }
}

/// A provider's reports as the model's, their paths mapped from its
/// step's folder `base`.
pub(crate) fn model_reports(reports: Vec<TestReport>, roots: &[PathBuf], base: &Path) -> Vec<fenix_tests::Report> {
    use fenix_tests::{Ended, Outcome, Report};
    use fenix_wasm::host::TestOutcome;
    reports
        .into_iter()
        .map(|r| match r {
            TestReport::Found(f) => Report::Found(model_found(f, roots, base)),
            TestReport::Started(id) => Report::Started(id),
            TestReport::Ended(e) => Report::Ended(Ended {
                id: e.id,
                outcome: match e.outcome {
                    TestOutcome::Passed => Outcome::Passed,
                    TestOutcome::Failed => Outcome::Failed,
                    TestOutcome::Skipped => Outcome::Skipped,
                    TestOutcome::Errored => Outcome::Errored,
                },
                duration_ms: e.duration_ms,
                message: e.message,
                at: e.at.map(|at| model_location(at, roots, base)),
                text: e.text,
            }),
            TestReport::Output(o) => Report::Output { id: o.id, text: o.text },
            TestReport::Broken(b) => Report::Broken { id: b.id, message: b.message, at: b.at.map(|at| model_location(at, roots, base)) },
            TestReport::Crashed(c) => Report::Crashed { id: c.id, message: c.message },
        })
        .collect()
}

fn found_bytes(f: &fenix_tests::Found) -> usize {
    let len = |s: &Option<String>| s.as_ref().map_or(0, String::len);
    f.id.len() + len(&f.parent) + f.label.len() + len(&f.path) + len(&f.skip) + len(&f.data)
}

/// An ask held to `ASK_BYTES`, as the model holds it to 10,000 records
/// (§4 "What the provider is asked"): whole fill-ins left out, the last
/// first, until it fits; then, if the nodes asked for alone don't, the
/// provider is asked `run-all` -- coarser, never fewer. A fill-in is the
/// run of records after a file or suite whose `parent` is in the ask.
pub(crate) fn capped_ask(ask: fenix_tests::Ask, max: usize) -> fenix_tests::Ask {
    let fenix_tests::Ask::Run(records) = ask else { return ask };
    let mut total: usize = records.iter().map(found_bytes).sum();
    if total <= max {
        return fenix_tests::Ask::Run(records);
    }
    let ids: HashSet<&str> = records.iter().map(|r| r.id.as_str()).collect();
    let fill_in: Vec<bool> = records.iter().map(|r| r.parent.as_deref().is_some_and(|p| ids.contains(p))).collect();
    // Each selected record and the fill-in after it.
    let mut groups: Vec<(usize, usize)> = Vec::new();
    for (i, &filled) in fill_in.iter().enumerate() {
        match (filled, groups.last_mut()) {
            (true, Some(group)) => group.1 = i + 1,
            _ => groups.push((i, i + 1)),
        }
    }
    let mut dropped = vec![false; records.len()];
    for &(start, end) in groups.iter().rev() {
        if total <= max {
            break;
        }
        for i in start + 1..end {
            total -= found_bytes(&records[i]);
            dropped[i] = true;
        }
    }
    if total > max {
        return fenix_tests::Ask::RunAll;
    }
    fenix_tests::Ask::Run(records.into_iter().zip(dropped).filter(|(_, d)| !d).map(|(r, _)| r).collect())
}

/// The model's ask as the API's.
pub(crate) fn api_ask(ask: fenix_tests::Ask) -> TestAsk {
    match ask {
        fenix_tests::Ask::Discover => TestAsk::Discover,
        fenix_tests::Ask::RunAll => TestAsk::RunAll,
        fenix_tests::Ask::Run(records) => TestAsk::Run(records.into_iter().map(api_found).collect()),
    }
}

pub(crate) fn api_step_end(end: &fenix_tests::StepEnd) -> TestStepEnd {
    match end {
        fenix_tests::StepEnd::Exited(code) => TestStepEnd::Exited(*code),
        fenix_tests::StepEnd::Stopped => TestStepEnd::Stopped,
        fenix_tests::StepEnd::TimedOut => TestStepEnd::TimedOut,
        fenix_tests::StepEnd::Failed(why) => TestStepEnd::Failed(why.clone()),
    }
}

/// How a step ended, from how its process did and why Fenix stopped it,
/// if it did: a stop or a timeout says so whatever the process did
/// meanwhile. A code is the process's 32 bits read as signed, so a
/// Windows crash's NTSTATUS is negative (§6.3).
pub(crate) fn step_end(outcome: &TaskOutcome, halt: Option<&fenix_tests::StepEnd>) -> fenix_tests::StepEnd {
    if let Some(halt) = halt {
        return halt.clone();
    }
    match outcome {
        TaskOutcome::Exited { code, .. } => fenix_tests::StepEnd::Exited(*code),
        TaskOutcome::Cancelled => fenix_tests::StepEnd::Stopped,
        TaskOutcome::Failed(why) => fenix_tests::StepEnd::Failed(format!("its output couldn't be read: {why}")),
    }
}

/// `n` with its thousands grouped: `41,203`.
pub(crate) fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// An argument's length on the command line Rust's standard library
/// gives `CreateProcessW` (its `make_command_line`), in UTF-16 units:
/// quoted when empty or holding a space or a tab; a `"` as `\"`, the
/// `\`s right before it doubled; the `\`s ending a quoted one doubled.
fn arg_units(arg: &str) -> usize {
    let quote = arg.is_empty() || arg.contains([' ', '\t']);
    let (mut n, mut backslashes) = (0, 0);
    for c in arg.encode_utf16() {
        if c == u16::from(b'\\') {
            backslashes += 1;
        } else {
            if c == u16::from(b'"') {
                n += backslashes + 1;
            }
            backslashes = 0;
        }
        n += 1;
    }
    if quote {
        n += 2 + backslashes;
    }
    n
}

/// The same, for a batch file's argument (`make_bat_command_line`):
/// quoted when empty, ending in `\`, or holding any ASCII but letters,
/// digits and `#$*+-./:?@\_`, or a control character; a `"` doubled, the
/// `\`s right before it doubled; a `%` as `%%cd:~,%`.
fn bat_arg_units(arg: &str) -> usize {
    const UNQUOTED: &str = r"#$*+-./:?@\_";
    let mut quote = arg.is_empty() || arg.ends_with('\\');
    if arg.chars().any(|c| (c.is_ascii() && !(c.is_ascii_alphanumeric() || UNQUOTED.contains(c))) || c.is_control()) {
        quote = true;
    }
    let (mut n, mut backslashes) = (0, 0);
    for c in arg.encode_utf16() {
        if c == u16::from(b'\\') {
            backslashes += 1;
        } else {
            if c == u16::from(b'"') {
                n += backslashes + 1;
            } else if c == u16::from(b'%') {
                n += "%%cd:~,".len();
            }
            backslashes = 0;
        }
        n += 1;
    }
    if quote {
        n += 2 + backslashes;
    }
    n
}

/// Whether `program` is a batch file, which Rust runs through `cmd.exe`.
fn is_batch(program: &str) -> bool {
    Path::new(program).extension().is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
}

/// A step's command line as Windows counts it (§6.5, the review of steps
/// 1 and 2's K2), on every platform, and its limit: the program as
/// resolved (its full path), always in `"`, then each argument as Rust
/// quotes it, joined by spaces: at most 32,766; for a `.cmd` or `.bat`,
/// the whole line Rust gives `cmd.exe`, at most 8,191. Why not, when it
/// can't be made one: a batch file's argument with a line break, which
/// Rust refuses.
pub(crate) fn command_line(program: &str, args: &[String]) -> Result<(usize, usize), String> {
    let units = |s: &str| s.encode_utf16().count();
    if is_batch(program) {
        if args.iter().any(|a| a.contains(['\r', '\n'])) {
            return Err("an argument with a line break can't go through cmd.exe, which runs a .cmd or .bat".into());
        }
        // `cmd.exe /e:ON /v:OFF /d /c ""<script>" <args>"`.
        let mut n = units("cmd.exe /e:ON /v:OFF /d /c \"") + units(program) + 2 + 1;
        for arg in args {
            n += 1 + bat_arg_units(arg);
        }
        return Ok((n, test_limits::MAX_BATCH_COMMAND_LINE));
    }
    let mut n = units(program) + 2;
    for arg in args {
        n += 1 + arg_units(arg);
    }
    Ok((n, test_limits::MAX_COMMAND_LINE))
}

/// Why a step's command line keeps it from starting, if it does.
pub(crate) fn command_line_refused(program: &str, args: &[String]) -> Option<String> {
    match command_line(program, args) {
        Err(why) => Some(why),
        Ok((n, max)) if n > max => {
            let through = if is_batch(program) { "; through cmd.exe, which runs a .cmd or .bat, Windows takes " } else { "; Windows takes " };
            Some(format!("its command line is {} characters{through}{}", grouped(n), grouped(max)))
        }
        Ok(_) => None,
    }
}

/// `program` as Fenix starts it: on Windows `fenix_rpc::resolve_command`'s
/// full path (a `.cmd` shim found on `PATHEXT`); elsewhere the first file
/// of that name on `PATH`, for its length, which runs the same.
pub(crate) fn resolved(program: &str) -> String {
    if cfg!(windows) || program.contains(['/', '\\']) {
        return fenix_rpc::resolve_command(program);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).map(|dir| dir.join(program)).find(|p| p.is_file()).map_or_else(|| program.to_string(), |p| p.to_string_lossy().into_owned())
}

/// Escape codes are taken out of a line as the page cleans what it draws.
pub(crate) use crate::tests_page::without_escapes;

/// A batch of a step's output as the provider is given it (§6.4
/// "Lines"), each line's stream and `cut` as the supervisor read them,
/// its escape codes taken out; and as *Test output* shows it, a cut
/// line's pieces joined.
pub(crate) fn test_lines(lines: Vec<fenix_tasks::OutputLine>) -> (Vec<TestLine>, String) {
    let mut text = String::new();
    let lines = lines
        .into_iter()
        .map(|line| {
            let clean = without_escapes(&line.text).into_owned();
            text.push_str(&clean);
            if !line.cut {
                text.push('\n');
            }
            TestLine { text: clean, stderr: line.stream == OutputStream::Stderr, cut: line.cut }
        })
        .collect();
    (lines, text)
}

/// A step's lines waiting for its provider: given at most 512 at a time
/// and 256 KiB (§6.4 "Batches").
#[derive(Default)]
pub(crate) struct Batch {
    lines: VecDeque<TestLine>,
    /// Their text, in bytes: what a batch is held to.
    pub bytes: usize,
    /// What they cost Fenix's memory: their text and each line's own
    /// size, so that empty lines aren't free.
    pub cost: usize,
}

impl Batch {
    /// What a line waiting costs, beyond its text.
    const LINE_COST: usize = std::mem::size_of::<TestLine>();

    pub fn push(&mut self, line: TestLine) {
        self.bytes += line.text.len();
        self.cost += line.text.len() + Self::LINE_COST;
        self.lines.push_back(line);
    }

    /// Whether more waits than its provider may fall behind by (§6.7):
    /// 32 MiB of what the lines cost, or 512 batches of lines.
    pub fn too_much(&self) -> bool {
        self.cost > BEHIND_BYTES || self.lines.len() > BEHIND_LINES
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.bytes = 0;
        self.cost = 0;
    }

    /// The next batch: the lines waiting, up to 512 of them and 256 KiB
    /// (a line is at most 64 KiB, so a batch has one at least).
    pub fn take(&mut self) -> Vec<TestLine> {
        let mut out = Vec::new();
        let mut bytes = 0;
        while let Some(line) = self.lines.front() {
            if out.len() == test_limits::MAX_READ_LINES || (!out.is_empty() && bytes + line.text.len() > test_limits::MAX_READ_BYTES) {
                break;
            }
            bytes += line.text.len();
            out.push(self.lines.pop_front().expect("one"));
        }
        self.bytes -= bytes;
        self.cost -= bytes + out.len() * Self::LINE_COST;
        out
    }
}

/// The name of a project's results file (§4 "What's kept"): the FNV-1a
/// hash of its root, under `tests` in Fenix's state folder.
pub(crate) fn results_name(root: &Path) -> String {
    format!("{:016x}.json", fenix_tests::fnv1a_64(root.to_string_lossy().as_bytes()))
}

// The state.

/// A provider, as its extension's manifest declares it.
#[derive(Clone, Debug)]
pub(crate) struct ProviderSpec {
    pub id: ProviderId,
    pub name: String,
    pub languages: Vec<String>,
    pub markers: Vec<String>,
    pub scan: Vec<String>,
}

impl ProviderSpec {
    fn of(ext: &str, c: &fenix_extensions::manifest::TestProviderContribution) -> Self {
        ProviderSpec { id: ProviderId::new(ext, c.id.clone()), name: c.name.clone(), languages: c.languages.clone(), markers: c.markers.clone(), scan: c.scan.clone() }
    }

    /// Whether the project at `root` is one it serves: one of its markers
    /// at its top (§5).
    fn serves(&self, root: &Path) -> bool {
        !self.markers.is_empty() && fenix_extensions::manifest::ContextContribution { name: String::new(), markers: self.markers.clone(), languages: Vec::new(), pages: Vec::new() }.holds(root)
    }

    /// Whether it scans file `path` of the project at `root`, open: one
    /// of its languages, or one its `scan` patterns name.
    fn scans(&self, root: &Path, path: &Path) -> bool {
        let language = fenix_syntax::detect_language_from_path(path).map(crate::lsp::language_config_name);
        language.is_some_and(|l| self.languages.contains(&l)) || (!self.scan.is_empty() && fenix_extensions::manifest::file_matches(&self.scan, path, Some(root)))
    }
}

/// The step under way of a project's run.
pub(crate) struct StepRun {
    tag: u64,
    run: RunId,
    provider: ProviderId,
    step: u32,
    /// Its folder: what a relative path in its reports is relative to.
    cwd: PathBuf,
    /// While its process runs: dropped, or cancelled, the tree is killed.
    runner: Option<TaskRunner>,
    lines: Batch,
    /// A `tests-read` for it is out.
    reading: bool,
    /// Its `tests-step-ended` is out.
    ending: bool,
    /// How its process ended, once it has.
    ended: Option<TaskOutcome>,
    /// Why it couldn't start: its `tests-step-ended(failed)` goes at once.
    failed: Option<String>,
    /// Fenix stopped it, or it timed out: what its end says.
    halt: Option<fenix_tests::StepEnd>,
    /// Its provider fell behind its output (§6.7): it's given no more.
    behind: bool,
}

/// What a call out is for: where its answer goes.
#[derive(Clone, Debug)]
enum CallFor {
    Scan { root: PathBuf, provider: ProviderId, path: String, first: bool },
    Plan { root: PathBuf, run: RunId, provider: ProviderId },
    Read { root: PathBuf, tag: u64 },
    StepEnded { root: PathBuf, run: RunId, step: u32, end: fenix_tests::StepEnd, tag: Option<u64> },
    /// `tests-launch` for a test to debug (step 6, `debug`).
    Launch { root: PathBuf, run: RunId },
}

impl CallFor {
    fn root(&self) -> &Path {
        match self {
            CallFor::Scan { root, .. } | CallFor::Plan { root, .. } | CallFor::Read { root, .. } | CallFor::StepEnded { root, .. } | CallFor::Launch { root, .. } => root,
        }
    }
}

/// A file a first scan read, off the UI thread, or the walk's end.
enum Walked {
    File { rel: String, text: String },
    Done { total: usize, open: usize },
}

/// A provider's first scan of a project's files (§7.1).
struct Walk {
    provider: ProviderId,
    ext: String,
    files: mpsc::Receiver<Walked>,
    /// A scan of one of its files is out.
    out: bool,
    /// The files' scans not yet given to the model.
    batch: Vec<(String, Vec<fenix_tests::Found>)>,
    scanned: usize,
    /// The walk ended, with this many files matched, and this many of
    /// them open (scanned from their buffers).
    total: Option<(usize, usize)>,
    /// Set when the walk is let go: its thread stops.
    cancel: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for Walk {
    fn drop(&mut self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// One project's tests.
pub(crate) struct ProjectTests {
    pub model: Model,
    pub providers: Vec<ProviderSpec>,
    /// The plan under way: each step as its provider gave it, its command
    /// and `needs`, beside what the model keeps of it.
    plan: Option<(RunId, ProviderId, Vec<PlannedStep>)>,
    /// The call out for its run: a plan, a batch, a step's end.
    call: Option<u64>,
    /// Steps of a plan to tell they won't start.
    not_started: VecDeque<(RunId, ProviderId, u32)>,
    step: Option<StepRun>,
    /// The run under way, and when it started (ms since 1970): what the
    /// timeout counts from.
    run: Option<(RunId, u64)>,
    timed_out: bool,
    /// *Test output*: its buffer, the lengths of its lines (for keeping
    /// its last 16 MiB), and how many went.
    output: Option<BufferId>,
    output_lines: VecDeque<usize>,
    output_bytes: usize,
    output_dropped: u64,
    /// Its last line has no end yet (a cut line's first pieces): the next
    /// text goes on it, not on a line of its own.
    output_partial: bool,
    /// The failures published as problems under `fenix:#tests` (§9.3),
    /// by file as the problems are keyed: what the next look replaces.
    problems: HashMap<PathBuf, Vec<lsp_types::Diagnostic>>,
    /// Where each step's lines begin, counted from the run's first line
    /// (those dropped included), and the step's folder: what a place on
    /// one of its lines is relative to (`Enter`, step 4).
    output_steps: Vec<(u64, PathBuf)>,
    /// The head and the modeline's item, as counted at a revision of the
    /// model, at most ten times a second (step 4, `page.rs`).
    shown: Option<page::Shown>,
    /// First scans to make, and the one under way.
    walks: VecDeque<(ProviderId, String)>,
    walk: Option<Walk>,
    /// Of each provider's first scan: the files scanned, of how many.
    pub scanned_of: HashMap<ProviderId, (usize, usize)>,
    /// A run on save due, and the files saved meanwhile.
    save_run: Option<(Instant, BTreeSet<String>)>,
    /// Whether the project is trusted, and when that was looked at: looked
    /// at again after `TRUST_FOR`, or when a file of its `.fenix` is saved.
    trusted: Option<(bool, Instant)>,
    /// Its results file being read, off the UI thread.
    restoring: Option<mpsc::Receiver<String>>,
    /// A run ended while it was being read: written once it's read.
    write_due: bool,
    /// A test being got ready to debug (step 6), and what the page's
    /// head says of the last one: building it, why it didn't build.
    pub(crate) debug: Option<debug::Debugging>,
    debug_said: Option<(String, crate::page::Role)>,
    /// How each run ended, in tests.
    #[cfg(test)]
    pub(crate) ended_runs: Vec<fenix_tests::RunEnd>,
}

/// What's known of an open buffer for its scans.
struct Seen {
    path: PathBuf,
    root: PathBuf,
    rel: String,
    providers: Vec<ProviderId>,
    /// The edit count last scanned, and since when it's been as it is.
    scanned: Option<u64>,
    changed: (u64, Instant),
    last_scan: Option<Instant>,
}

/// Where the program a step names starts, in tests: `fake-test-runner` as
/// the test harness itself, with what it's started with first.
#[cfg(test)]
pub(crate) type ProgramFor = Box<dyn Fn(&str) -> Option<(String, Vec<String>, Vec<(String, String)>)>>;

/// The test explorer's host: every project's tests, the calls out, and
/// what came back.
#[derive(Default)]
pub(crate) struct TestsHost {
    run_ids: RunIds,
    pub(crate) projects: HashMap<PathBuf, ProjectTests>,
    calls: HashMap<u64, (String, CallFor)>,
    next: u64,
    replies: VecDeque<(String, u64, Result<Reply, String>)>,
    /// Step events, when there's no event loop (tests).
    pub(crate) inbox: Arc<Mutex<Vec<(u64, TaskEvent)>>>,
    events: VecDeque<(u64, TaskEvent)>,
    pumping: bool,
    /// The extensions that turned out to have no test provider's exports:
    /// not asked again while Fenix runs.
    no_tests: HashSet<String>,
    /// The extensions that turned out to have no `tests-launch`: their
    /// tests run, but aren't offered to debug (step 6).
    no_launch: HashSet<String>,
    /// How many times each extension has been ready, and the last time
    /// its first scans were asked for.
    ready: HashMap<String, u64>,
    walked: HashMap<String, u64>,
    /// What each provider said it is in each project (`test-provider-set`),
    /// kept until Fenix exits.
    said: HashMap<(PathBuf, ProviderId), (String, Option<String>)>,
    seen: HashMap<BufferId, Seen>,
    /// Each open file's project and path there, by the path its buffer
    /// has (step 5: the gutter's column, its marks, the menu): looked up
    /// as a frame is drawn, never worked out then.
    pub(crate) by_path: HashMap<PathBuf, (PathBuf, String)>,
    /// What's shown of the tests in open files (step 5, `file`): each
    /// buffer's results after its lines, and the settings they were
    /// shown under.
    inline: HashMap<BufferId, file::Inline>,
    file_settings: Option<file::Settings>,
    /// The project an extension is being started again in for a call
    /// about it: its runs aren't the old instance's.
    restart_here: Option<PathBuf>,
    /// The other spellings of each project's root (a symlink, an 8.3
    /// name): a path under one is the project's.
    aliases: HashMap<PathBuf, Vec<PathBuf>>,
    /// Paths as canonicalized, once each: no file system call per frame.
    canon: HashMap<PathBuf, PathBuf>,
    /// Results files to write, to the thread that writes them in turn,
    /// and how many it has still to write.
    writer: Option<mpsc::Sender<(PathBuf, String)>>,
    writing: Arc<std::sync::atomic::AtomicUsize>,
    /// Fenix's state folder for results files, in place of the real one
    /// (tests).
    pub(crate) state_dir: Option<PathBuf>,
    /// The tests pages closed this session, per project: `SPC T T` opens
    /// one again as it was (§7.6).
    views: HashMap<PathBuf, crate::tests_page::TestsPage>,
    /// A tests page has been opened: what turning the explorer off closes.
    page_shown: bool,
    #[cfg(test)]
    pub(crate) program_for: Option<ProgramFor>,
    /// What the page's `y` copied, in tests (no clipboard there).
    #[cfg(test)]
    pub(crate) copied: Option<String>,
    /// Added to the clock, in tests: a timeout without waiting for it.
    #[cfg(test)]
    pub(crate) skew_ms: u64,
    /// The first scan's cap, in place of 2,000 (tests).
    #[cfg(test)]
    pub(crate) scan_cap: Option<usize>,
}

impl TestsHost {
    fn token(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    fn now_ms(&self) -> u64 {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        #[cfg(test)]
        let now = now + self.skew_ms;
        now
    }
}

/// Why a provider's call may not go to its extension now.
enum Reach {
    /// It may: running in the project, or started there by the call.
    Yes,
    /// Not now: it runs in another project, or has no tests.
    No(String),
}

impl App {
    pub(super) fn tests_enabled(&self) -> bool {
        self.config.bool_value("tests.enabled") == Some(true)
    }

    /// `path` canonicalized, once: no file system call the next time.
    fn tests_canon(&mut self, path: &Path) -> PathBuf {
        if let Some(canonical) = self.tests.canon.get(path) {
            return canonical.clone();
        }
        let canonical = refactor::identity(path);
        self.tests.canon.insert(path.to_path_buf(), canonical.clone());
        canonical
    }

    /// The project of file `path`, as the host keys it: its root
    /// canonicalized, the spelling it was found by kept as another.
    fn tests_root_of(&mut self, path: &Path) -> Option<PathBuf> {
        let raw = fenix_project::find_project_root(path)?;
        let root = self.tests_canon(&raw);
        self.tests_alias(&root, &raw);
        Some(root)
    }

    /// `raw` is another spelling of the project at `root`.
    fn tests_alias(&mut self, root: &Path, raw: &Path) {
        let raw = fenix_lsp::normalize(raw.to_path_buf());
        if raw != root {
            let list = self.tests.aliases.entry(root.to_path_buf()).or_default();
            if !list.contains(&raw) {
                list.push(raw);
            }
        }
    }

    /// Every spelling of the project at `root`, the canonical first.
    fn tests_roots(&self, root: &Path) -> Vec<PathBuf> {
        std::iter::once(root.to_path_buf()).chain(self.tests.aliases.get(root).into_iter().flatten().cloned()).collect()
    }

    /// File `path`'s path in the project at `root`, as the model holds it.
    fn tests_rel(&self, root: &Path, path: &Path) -> String {
        model_path_in(&path.to_string_lossy(), &self.tests_roots(root), root)
    }

    /// Whether the project at `root` is trusted (§12): looked at again
    /// after `TRUST_FOR`, or after a file of its `.fenix` was saved, not
    /// at every wake -- looking reads and hashes those files.
    fn tests_trusted(&mut self, root: &Path) -> bool {
        if let Some((trusted, at)) = self.tests.projects.get(root).and_then(|p| p.trusted) {
            if at.elapsed() < TRUST_FOR {
                return trusted;
            }
        }
        let trusted = self.project_is_trusted(root);
        if let Some(project) = self.tests.projects.get_mut(root) {
            project.trusted = Some((trusted, Instant::now()));
        }
        trusted
    }

    /// Whether listings no key asked for may run in the project at `root`
    /// (§7.1, §12): trusted, `tests.list_automatically`, not remote.
    fn tests_automatic(&mut self, root: &Path) -> bool {
        self.config.bool_value("tests.list_automatically") != Some(false) && !fenix_vfs::is_remote(root) && self.tests_trusted(root)
    }

    /// The providers of every extension switched on, with their extension.
    fn tests_specs(&self) -> Vec<(String, ProviderSpec)> {
        self.wasm.installed.iter().filter(|e| !self.tests.no_tests.contains(&e.id)).flat_map(|e| e.manifest.contributes.test_providers.iter().map(|c| (e.id.clone(), ProviderSpec::of(&e.id, c)))).collect()
    }

    /// The project at `root`'s tests, made when it has none yet: a model
    /// drawing on the host's one `RunIds`, the providers whose markers it
    /// has, what they said of it, and its results file read.
    fn tests_project(&mut self, root: &Path) -> &mut ProjectTests {
        if !self.tests.projects.contains_key(root) {
            let mut model = Model::with_run_ids(self.tests.run_ids.clone());
            let mut providers = Vec::new();
            for (_, spec) in self.tests_specs() {
                if spec.serves(root) {
                    model.add_provider(&spec.id, &spec.name);
                    providers.push(spec);
                }
            }
            for ((r, id), (runner, problem)) in &self.tests.said {
                if r == root && providers.iter().any(|p| p.id == *id) {
                    model.provider_set(id, runner, problem.as_deref());
                }
            }
            // Fenix's choice until it's looked: no listing on its own
            // (§12, failing closed); set as it is just below.
            model.set_automatic_listings(false);
            model.set_timeout_minutes(self.config.int_value("tests.timeout_minutes").unwrap_or(0).clamp(0, 1440) as u32);
            // Its results file, read off the UI thread (up to 8 MiB).
            let restoring = (self.config.bool_value("tests.keep_results") != Some(false)).then(|| self.tests_results_file(root)).flatten().map(|file| {
                let (tx, rx) = mpsc::channel();
                let _ = std::thread::Builder::new().name("fenix-tests-results".into()).spawn(move || {
                    let small = std::fs::metadata(&file).is_ok_and(|m| m.len() <= fenix_tests::RESULTS_FILE_BYTES as u64);
                    let text = small.then(|| std::fs::read_to_string(&file).ok()).flatten().unwrap_or_default();
                    let _ = tx.send(text);
                });
                rx
            });
            let project = ProjectTests {
                model,
                providers,
                plan: None,
                call: None,
                not_started: VecDeque::new(),
                step: None,
                run: None,
                timed_out: false,
                output: None,
                output_lines: VecDeque::new(),
                output_bytes: 0,
                output_dropped: 0,
                output_partial: false,
                output_steps: Vec::new(),
                problems: HashMap::new(),
                shown: None,
                walks: VecDeque::new(),
                walk: None,
                scanned_of: HashMap::new(),
                save_run: None,
                trusted: None,
                restoring,
                write_due: false,
                debug: None,
                debug_said: None,
                #[cfg(test)]
                ended_runs: Vec::new(),
            };
            self.tests.projects.insert(root.to_path_buf(), project);
            let automatic = self.tests_automatic(root);
            self.tests.projects.get_mut(root).expect("just made").model.set_automatic_listings(automatic);
        }
        self.tests.projects.get_mut(root).expect("just made")
    }

    /// The project at `root`'s results file, once read: its results
    /// restored (shown stale), and a write that waited for it done.
    fn tests_restored(&mut self, root: &Path) {
        let Some(project) = self.tests.projects.get_mut(root) else { return };
        let Some(rx) = &project.restoring else { return };
        let text = match rx.try_recv() {
            Ok(text) => text,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => String::new(),
        };
        project.restoring = None;
        // One that can't be read is left as it is: the next run's end
        // writes a good one.
        if !text.is_empty() {
            let _ = project.model.restore_results(&text);
        }
        if std::mem::take(&mut project.write_due) {
            self.tests_write_results(root);
        }
    }

    /// The project at `root`'s results written, off the UI thread, by
    /// the one thread that writes them in turn (§4 "What's kept").
    fn tests_write_results(&mut self, root: &Path) {
        let Some(file) = self.tests_results_file(root) else { return };
        let Some(project) = self.tests.projects.get_mut(root) else { return };
        if project.restoring.is_some() {
            // Not before the old results are read: they'd be lost.
            project.write_due = true;
            return;
        }
        let text = project.model.results_file();
        if self.tests.writer.is_none() {
            let (tx, rx) = mpsc::channel::<(PathBuf, String)>();
            let writing = self.tests.writing.clone();
            let spawned = std::thread::Builder::new().name("fenix-tests-write".into()).spawn(move || {
                for (file, text) in rx {
                    let written = file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| fenix_storage::write(&file, text.as_bytes()));
                    if let Err(e) = written {
                        eprintln!("fenix: couldn't keep the test results in {}: {e}", file.display());
                    }
                    writing.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                }
            });
            if spawned.is_err() {
                return;
            }
            self.tests.writer = Some(tx);
        }
        self.tests.writing.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        if self.tests.writer.as_ref().is_some_and(|w| w.send((file, text)).is_err()) {
            self.tests.writing.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        }
    }

    /// Fenix exits (`exiting`): every step's tree killed, waited for up to
    /// two seconds -- dropped alone, a `TaskRunner` only asks its thread to,
    /// and on Linux the process would be gone first, leaving the tree
    /// running -- and the results still to write written.
    pub(super) fn tests_shutdown(&mut self) {
        let runners: Vec<TaskRunner> = self.tests.projects.values_mut().filter_map(|p| p.step.take().and_then(|s| s.runner)).collect();
        for runner in &runners {
            runner.cancel();
        }
        let deadline = Instant::now() + EXIT_WAIT;
        while Instant::now() < deadline && !(runners.iter().all(TaskRunner::is_finished) && self.tests.writing.load(std::sync::atomic::Ordering::Acquire) == 0) {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.tests.writer = None;
        for project in self.tests.projects.values_mut() {
            project.walk = None;
        }
    }

    /// Adds provider `spec` to the project at `root`, if it isn't there.
    fn tests_add_provider(&mut self, root: &Path, spec: &ProviderSpec) {
        let said = self.tests.said.get(&(root.to_path_buf(), spec.id.clone())).cloned();
        let project = self.tests_project(root);
        if project.providers.iter().any(|p| p.id == spec.id) {
            return;
        }
        project.model.add_provider(&spec.id, &spec.name);
        if let Some((runner, problem)) = said {
            project.model.provider_set(&spec.id, &runner, problem.as_deref());
        }
        project.providers.push(spec.clone());
    }

    /// The results file of the project at `root`, in Fenix's state folder.
    fn tests_results_file(&self, root: &Path) -> Option<PathBuf> {
        let dir = match &self.tests.state_dir {
            Some(dir) => Some(dir.clone()),
            None if cfg!(test) => None,
            None => fenix_storage::paths::state_file("tests"),
        }?;
        Some(dir.join(results_name(root)))
    }

    /// Whether a call about the project at `root` may go to extension
    /// `ext` now (§6.4 "One project"): its instance runs there, or none
    /// runs and it would start there. A scan or a plan for the project
    /// you're in starts one running elsewhere again here (`starts`);
    /// anything else for another project isn't asked of it.
    fn tests_reach(&mut self, ext: &str, root: &Path, starts: bool) -> Reach {
        if self.tests.no_tests.contains(ext) {
            return Reach::No("it has no test providers".into());
        }
        // Switched off or removed: nothing would answer.
        if !self.wasm.installed.iter().any(|e| e.id == ext) {
            return Reach::No("its extension isn't installed".into());
        }
        let here = self.code_project_now().map(|p| self.tests_canon(&p));
        match self.code_instance_project(ext) {
            Some(Some(project)) if self.tests_canon(&project) == root => Reach::Yes,
            Some(_) if starts && here.as_deref() == Some(root) => {
                self.tests.restart_here = Some(root.to_path_buf());
                self.restart_code_extension(ext);
                self.tests.restart_here = None;
                Reach::Yes
            }
            Some(_) => Reach::No("its extension runs in another project".into()),
            None if here.as_deref() == Some(root) => Reach::Yes,
            None => Reach::No("its extension isn't running in this project".into()),
        }
    }

    /// Sends `call` to `ext` for `what`: its answer comes to `tests_reply`.
    fn tests_send(&mut self, ext: &str, call: Call, what: CallFor) -> u64 {
        let token = self.tests.token();
        self.tests.calls.insert(token, (ext.to_string(), what));
        self.tests_call(ext, call, token);
        token
    }

    /// The extension a provider is of.
    fn tests_ext(provider: &ProviderId) -> String {
        provider.extension.to_string()
    }

    // Hooks: what the rest of Fenix tells the host.

    /// A call's end, from `wasm_host` (`Pending::Tests`).
    pub(super) fn tests_reply(&mut self, ext: &str, token: u64, result: Result<Reply, String>) {
        self.tests.replies.push_back((ext.to_string(), token, result));
        self.tests_pump();
    }

    /// A step's process said something, or ended.
    pub(super) fn tests_step_event(&mut self, tag: u64, event: TaskEvent) {
        self.tests.events.push_back((tag, event));
        self.tests_pump();
    }

    /// Extension `ext` is up: its first scans are asked for, and its
    /// `test-provider-set` heard, at the next tick.
    pub(super) fn tests_extension_ready(&mut self, ext: &str) {
        *self.tests.ready.entry(ext.to_string()).or_default() += 1;
    }

    /// Extension `ext` stopped, started again, or was switched off (§6.4
    /// "After a trap", "One project"): its calls out are let go, and in
    /// each project a run whose turn is its provider's is stopped as after
    /// a trap, `why`, its step's tree killed. A provider's tests stay;
    /// switched off (`gone`), its providers go with them.
    pub(super) fn tests_extension_gone(&mut self, ext: &str, why: &str, gone: bool) {
        self.tests.calls.retain(|_, (e, _)| e != ext);
        // Its next start is a start: its first scans are made again.
        self.tests.ready.remove(ext);
        self.tests.walked.remove(ext);
        if gone {
            for seen in self.tests.seen.values_mut() {
                seen.providers.retain(|p| p.extension != ext);
            }
        }
        let roots: Vec<PathBuf> = self.tests.projects.keys().cloned().collect();
        for root in roots {
            if self.tests.restart_here.as_ref() != Some(&root) {
                self.tests_provider_stopped(&root, ext, why);
            }
            if gone {
                let project = self.tests.projects.get_mut(&root).expect("a project");
                for provider in project.providers.iter().filter(|p| p.id.extension == ext) {
                    project.model.remove_provider(&provider.id);
                }
                project.providers.retain(|p| p.id.extension != ext);
            }
        }
        if gone {
            self.tests.no_tests.remove(ext);
            self.tests.no_launch.remove(ext);
            self.tests.said.retain(|(_, id), _| id.extension != ext);
        }
        self.tests_pump();
    }

    /// In the project at `root`, the providers of `ext` stopped: as
    /// `Model::provider_stopped` says, and the host's side -- its step's
    /// tree killed, no call to it after.
    fn tests_provider_stopped(&mut self, root: &Path, ext: &str, why: &str) {
        self.tests_debug_provider_stopped(root, ext, why);
        let Some(project) = self.tests.projects.get_mut(root) else { return };
        for provider in project.providers.iter().filter(|p| p.id.extension == ext) {
            project.model.provider_stopped(&provider.id, why);
        }
        if project.step.as_ref().is_some_and(|s| s.provider.extension == ext) {
            if let Some(runner) = project.step.take().and_then(|s| s.runner) {
                runner.cancel();
            }
        }
        if project.plan.as_ref().is_some_and(|(_, p, _)| p.extension == ext) {
            project.plan = None;
        }
        project.not_started.retain(|(_, p, _)| p.extension != ext);
        if let Some(token) = project.call {
            if self.tests.calls.get(&token).is_none_or(|(e, _)| e == ext) {
                self.tests.calls.remove(&token);
                project.call = None;
            }
        }
        if project.walk.as_ref().is_some_and(|w| w.ext == ext) {
            project.walk = None;
        }
        project.walks.retain(|(p, _)| p.extension != ext);
    }

    /// What extension `ext`'s provider `provider` said it is in its
    /// project (`test-provider-set`): kept until Fenix exits, across the
    /// extension starting again; a repeat changes nothing, `runner` ""
    /// with no problem hides it there, a change lists as §7.1 says.
    pub(super) fn tests_provider_set(&mut self, ext: &str, provider: &str, runner: &str, problem: Option<&str>) {
        let Some(raw) = self.code_instance_project(ext).flatten() else { return };
        let root = self.tests_canon(&raw);
        self.tests_alias(&root, &raw);
        let id = ProviderId::new(ext, provider);
        self.tests.said.insert((root.clone(), id.clone()), (runner.to_string(), problem.map(str::to_string)));
        if !self.tests_enabled() {
            return;
        }
        let Some(spec) = self.tests_specs().into_iter().map(|(_, s)| s).find(|s| s.id == id) else { return };
        self.tests_add_provider(&root, &spec);
        self.tests_project(&root).model.provider_set(&id, runner, problem);
        self.tests_pump();
    }

    /// File `path` was saved (§8.4): its tests stale, its scans asked for
    /// again, and a run on save due a second after the last save.
    pub(super) fn tests_file_saved(&mut self, path: &Path) {
        if !self.tests_enabled() {
            return;
        }
        let Some(root) = self.tests_root_of(path) else { return };
        let rel = self.tests_rel(&root, path);
        let Some(project) = self.tests.projects.get_mut(&root) else { return };
        project.model.file_saved(&rel);
        // A file of its `.fenix` (tools.json, lsp/): its trust is looked
        // at again.
        if rel == ".fenix" || rel.starts_with(".fenix/") {
            project.trusted = None;
        }
        for seen in self.tests.seen.values_mut().filter(|s| s.root == root && s.rel == rel) {
            seen.scanned = None;
            seen.last_scan = None;
        }
        let on_save = self.config.text_value("tests.on_save").unwrap_or("off").to_string();
        if on_save != "off" && !fenix_vfs::is_remote(&root) && self.tests_trusted(&root) {
            let project = self.tests.projects.get_mut(&root).expect("a project");
            let files = project.save_run.take().map(|(_, f)| f).unwrap_or_default();
            let mut files = files;
            files.insert(rel);
            project.save_run = Some((Instant::now() + SAVE_RUN_AFTER, files));
        }
    }

    // The tick and the pump.

    /// What's due by the clock, done; then the pump. When next it's due.
    pub(super) fn tests_tick(&mut self, now: Instant) -> Option<Instant> {
        if !self.tests_enabled() {
            self.tests_off();
            // A run still open is driven to its end; nothing else is.
            if self.tests.projects.values().any(|p| p.run.is_some() || p.step.is_some()) {
                self.tests_pump();
            }
            return None;
        }
        self.tests_sync();
        let mut wake = self.tests_scan_buffers(now);
        let mut soonest = |at: Instant| wake = Some(wake.map_or(at, |w: Instant| w.min(at)));
        let now_ms = self.tests.now_ms();
        let timeout = self.config.int_value("tests.timeout_minutes").unwrap_or(0).max(0) as u64;
        let roots: Vec<PathBuf> = self.tests.projects.keys().cloned().collect();
        for root in roots {
            self.tests_restored(&root);
            let project = self.tests.projects.get_mut(&root).expect("a project");
            if project.restoring.is_some() || project.walk.as_ref().is_some_and(|w| !w.out) || !project.walks.is_empty() {
                // A file being read on its thread: looked for again soon.
                soonest(now + WALK_POLL);
            }
            // A run past `tests.timeout_minutes` (§8.3), the only clock.
            if let Some((_, started)) = project.run {
                if timeout > 0 && !project.timed_out {
                    let due = started + timeout * 60_000;
                    if now_ms >= due {
                        project.timed_out = true;
                        if project.model.time_out() {
                            Self::tests_halt(project, fenix_tests::StepEnd::TimedOut);
                        }
                    } else {
                        soonest(now + Duration::from_millis(due - now_ms));
                    }
                }
            }
            // A run on save, a second after the last save.
            if let Some((due, _)) = &project.save_run {
                if now >= *due {
                    let (_, files) = project.save_run.take().expect("due");
                    let on_save = self.config.text_value("tests.on_save").unwrap_or("off").to_string();
                    let project = self.tests.projects.get_mut(&root).expect("a project");
                    match on_save.as_str() {
                        "failed" => {
                            project.model.request(Selection::Failed, fenix_tests::Mode::ReplaceWaiting);
                        }
                        "file" => {
                            let mut mode = fenix_tests::Mode::ReplaceWaiting;
                            for file in files {
                                project.model.request(Selection::File(file), mode);
                                mode = fenix_tests::Mode::Queue;
                            }
                        }
                        _ => {}
                    }
                } else {
                    soonest(*due);
                }
            }
        }
        self.tests_walks();
        self.tests_pump();
        wake
    }

    /// `tests.enabled` is off: a run still going is stopped, and nothing
    /// else is done.
    fn tests_off(&mut self) {
        // Stopped once; it then ends as any stopped run does, its steps
        // told `stopped` (the pump drives a run still open while off).
        let going: Vec<PathBuf> = self.tests.projects.iter().filter(|(_, p)| (p.run.is_some() || p.step.is_some()) && !p.model.summary().running.is_some_and(|r| r.stopping)).map(|(r, _)| r.clone()).collect();
        for root in going {
            self.tests_stop_in(&root, "the test explorer was turned off (tests.enabled)");
        }
        for project in self.tests.projects.values_mut() {
            project.walk = None;
            project.walks.clear();
            project.save_run = None;
            if project.debug.take().is_some() {
                project.model.drop_build();
            }
            project.debug_said = None;
        }
    }

    /// The extensions that came up since last looked: their providers in
    /// their instance's project, and the first scans they make there; and
    /// every project's settings, as they are now.
    fn tests_sync(&mut self) {
        let fresh: Vec<(String, u64)> = self.tests.ready.iter().filter(|(e, n)| self.tests.walked.get(*e) != Some(*n)).map(|(e, n)| (e.clone(), *n)).collect();
        for (ext, n) in fresh {
            self.tests.walked.insert(ext.clone(), n);
            let Some(raw) = self.code_instance_project(&ext).flatten() else { continue };
            let root = self.tests_canon(&raw);
            self.tests_alias(&root, &raw);
            let specs: Vec<ProviderSpec> = self.tests_specs().into_iter().filter(|(e, _)| *e == ext).map(|(_, s)| s).collect();
            for spec in specs {
                let known = self.tests.projects.get(&root).is_some_and(|p| p.providers.iter().any(|q| q.id == spec.id));
                if spec.serves(&root) || known {
                    self.tests_add_provider(&root, &spec);
                    if !spec.scan.is_empty() && !fenix_vfs::is_remote(&root) {
                        let project = self.tests_project(&root);
                        if !project.walks.iter().any(|(p, _)| *p == spec.id) {
                            project.walks.push_back((spec.id.clone(), ext.clone()));
                        }
                    }
                }
            }
        }
        let timeout = self.config.int_value("tests.timeout_minutes").unwrap_or(0).clamp(0, 1440) as u32;
        let roots: Vec<PathBuf> = self.tests.projects.keys().cloned().collect();
        for root in roots {
            // What runs without a key runs only in a trusted project (§12).
            let automatic = self.tests_automatic(&root);
            let project = self.tests.projects.get_mut(&root).expect("a project");
            project.model.set_automatic_listings(automatic);
            project.model.set_timeout_minutes(timeout);
        }
    }

    /// Open buffers' scans (§7.1): a file of a provider's, scanned when
    /// it's first seen, and again once typing has paused 500 ms and 500
    /// ms after its last scan; its unsaved edits told to the model. When
    /// the next is due.
    fn tests_scan_buffers(&mut self, now: Instant) -> Option<Instant> {
        let mut wake: Option<Instant> = None;
        self.tests_forget_closed();
        for id in self.buffers.ids_sorted_by_path() {
            let Some(ob) = self.buffers.get(id) else { continue };
            if !ob.kind.tracks_unsaved_changes() {
                continue;
            }
            let Some(path) = ob.buffer.path().map(Path::to_path_buf) else { continue };
            let (edits, dirty) = (ob.buffer.edit_count(), ob.buffer.is_dirty());
            if self.tests.seen.get(&id).is_none_or(|s| s.path != path) {
                // Renamed: what its old name had goes with it.
                if let Some(old) = self.tests.seen.remove(&id) {
                    self.tests_forget_seen(id, old);
                }
                let Some(root) = self.tests_root_of(&path) else { continue };
                let rel = self.tests_rel(&root, &path);
                self.tests.by_path.insert(path.clone(), (root.clone(), rel.clone()));
                let specs: Vec<ProviderSpec> = self.tests_specs().into_iter().map(|(_, s)| s).filter(|s| s.scans(&root, &path)).collect();
                let providers = specs.iter().map(|s| s.id.clone()).collect();
                self.tests.seen.insert(id, Seen { path: path.clone(), root, rel, providers, scanned: None, changed: (edits, now), last_scan: None });
            }
            let seen = self.tests.seen.get_mut(&id).expect("seen");
            if seen.providers.is_empty() {
                continue;
            }
            if seen.changed.0 != edits {
                seen.changed = (edits, now);
            }
            if seen.scanned == Some(edits) {
                continue;
            }
            let due = match (seen.scanned, seen.last_scan) {
                (None, _) | (_, None) => now,
                (Some(_), Some(last)) => (seen.changed.1 + SCAN_PAUSE).max(last + SCAN_PAUSE),
            };
            if now < due {
                wake = Some(wake.map_or(due, |w| w.min(due)));
                continue;
            }
            let (root, rel, providers) = (seen.root.clone(), seen.rel.clone(), seen.providers.clone());
            seen.scanned = Some(edits);
            seen.last_scan = Some(now);
            // Not copied when it's past what's scanned: a character is a
            // byte at least.
            let small = self.buffers.get(id).is_some_and(|ob| ob.buffer.len_chars() <= test_limits::MAX_SCAN_TEXT);
            let text = if small { self.buffers.get(id).map(|ob| ob.buffer.text()).unwrap_or_default() } else { String::new() };
            let specs: Vec<ProviderSpec> = self.tests_specs().into_iter().map(|(_, s)| s).filter(|s| providers.contains(&s.id)).collect();
            for spec in specs {
                self.tests_add_provider(&root, &spec);
            }
            self.tests_project(&root).model.set_unsaved(&rel, dirty);
            if !small || text.len() > test_limits::MAX_SCAN_TEXT {
                continue;
            }
            for provider in providers {
                let ext = Self::tests_ext(&provider);
                if let Reach::No(_) = self.tests_reach(&ext, &root, true) {
                    continue;
                }
                let call = Call::TestsScan { provider: provider.provider.to_string(), path: guest_path(&rel), text: text.clone() };
                self.tests_send(&ext, call, CallFor::Scan { root: root.clone(), provider, path: rel.clone(), first: false });
            }
        }
        wake
    }

    /// The buffers closed since last looked, forgotten.
    fn tests_forget_closed(&mut self) {
        let gone: Vec<BufferId> = self.tests.seen.keys().copied().filter(|id| self.buffers.get(*id).is_none()).collect();
        for id in gone {
            if let Some(seen) = self.tests.seen.remove(&id) {
                self.tests_forget_seen(id, seen);
            }
        }
    }

    /// What was known of buffer `id` as `seen` goes: its results after its
    /// lines, its path's entry, and its file's unsaved edits -- a buffer
    /// closed without saving leaves the file as it is on disk, which is
    /// what a runner reads (§8.4), unless another buffer on it has edits.
    fn tests_forget_seen(&mut self, id: BufferId, seen: Seen) {
        self.tests_inline_clear(id);
        if !self.tests.seen.values().any(|s| s.path == seen.path) {
            self.tests.by_path.remove(&seen.path);
        }
        let still = self.tests.seen.iter().any(|(other, s)| s.root == seen.root && s.rel == seen.rel && self.buffers.get(*other).is_some_and(|ob| ob.buffer.is_dirty()));
        if !still {
            if let Some(project) = self.tests.projects.get_mut(&seen.root) {
                project.model.set_unsaved(&seen.rel, false);
            }
        }
    }

    /// The first scans (§6.7, §7.1): one provider's at a time per project,
    /// its files walked and read off the UI thread through the project's
    /// ignores, one scan out at a time, given to the model a batch at a
    /// time; at most 2,000 files, and said when there were more.
    fn tests_walks(&mut self) {
        let roots: Vec<PathBuf> = self.tests.projects.keys().cloned().collect();
        for root in roots {
            let open: HashSet<String> = self.tests.seen.values().filter(|s| s.root == root).map(|s| s.rel.clone()).collect();
            #[cfg(test)]
            let cap = self.tests.scan_cap.unwrap_or(SCAN_FILES);
            #[cfg(not(test))]
            let cap = SCAN_FILES;
            let project = self.tests.projects.get_mut(&root).expect("a project");
            if project.walk.is_none() {
                let Some((provider, ext)) = project.walks.pop_front() else { continue };
                let Some(spec) = project.providers.iter().find(|p| p.id == provider).cloned() else { continue };
                let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let files = walk_files(&root, spec.scan, open, cap, cancel.clone());
                project.walk = Some(Walk { provider, ext, files, out: false, batch: Vec::new(), scanned: 0, total: None, cancel });
            }
            let walk = project.walk.as_mut().expect("a walk");
            if walk.out {
                continue;
            }
            let next = if walk.total.is_some() { None } else { walk.files.try_recv().ok() };
            match next {
                Some(Walked::File { rel, text }) => {
                    let (ext, provider) = (walk.ext.clone(), walk.provider.clone());
                    if let Reach::No(_) = self.tests_reach(&ext, &root, false) {
                        self.tests.projects.get_mut(&root).expect("a project").walk = None;
                        continue;
                    }
                    // Out before it's sent: an answer that comes at once
                    // (a stopped extension's) finds it so.
                    self.tests.projects.get_mut(&root).expect("a project").walk.as_mut().expect("a walk").out = true;
                    let call = Call::TestsScan { provider: provider.provider.to_string(), path: guest_path(&rel), text };
                    self.tests_send(&ext, call, CallFor::Scan { root: root.clone(), provider, path: rel, first: true });
                }
                Some(Walked::Done { total, open }) => walk.total = Some((total, open)),
                None => {}
            }
            let project = self.tests.projects.get_mut(&root).expect("a project");
            let Some(walk) = project.walk.as_mut() else { continue };
            if let (Some((total, open)), false) = (walk.total, walk.out) {
                let mut walk = project.walk.take().expect("a walk");
                project.model.scan_files(&walk.provider, std::mem::take(&mut walk.batch));
                // Those open were scanned too, from their buffers.
                let scanned = walk.scanned + open;
                project.scanned_of.insert(walk.provider.clone(), (scanned, total));
                if scanned < total {
                    let said = format!("tests: {} of {} files of {} scanned; the rest are found by its listing, or when opened", grouped(scanned), grouped(total), walk.provider.provider);
                    self.note_code_output(&walk.ext, said);
                }
            }
        }
    }

    /// Applies what came back, and drives each project's run, until
    /// nothing moves. Re-entered while it does, it returns.
    pub(super) fn tests_pump(&mut self) {
        if self.tests.pumping {
            return;
        }
        self.tests.pumping = true;
        loop {
            let mut moved = false;
            let inbox = std::mem::take(&mut *self.tests.inbox.lock().unwrap_or_else(|p| p.into_inner()));
            self.tests.events.extend(inbox);
            while let Some((ext, token, result)) = self.tests.replies.pop_front() {
                self.tests_apply_reply(&ext, token, result);
                moved = true;
            }
            while let Some((tag, event)) = self.tests.events.pop_front() {
                self.tests_apply_event(tag, event);
                moved = true;
            }
            // Off, a run still open is driven to its end, and nothing more.
            let enabled = self.tests_enabled();
            let roots: Vec<PathBuf> = self.tests.projects.iter().filter(|(_, p)| enabled || p.run.is_some() || p.step.is_some()).map(|(r, _)| r.clone()).collect();
            for root in roots {
                moved |= self.tests_drive(&root);
            }
            if !moved && self.tests.replies.is_empty() && self.tests.events.is_empty() {
                break;
            }
        }
        self.tests_notices();
        self.tests.pumping = false;
    }

    /// What each model has to say on its providers' output pages.
    fn tests_notices(&mut self) {
        let mut said = Vec::new();
        for project in self.tests.projects.values_mut() {
            for notice in project.model.take_notices() {
                said.push((notice.provider.extension.to_string(), format!("tests: {}: {}", notice.provider.provider, notice.text)));
            }
        }
        for (ext, text) in said {
            self.note_code_output(&ext, text);
        }
    }

    /// One step forward for the project at `root`'s run: whether it moved.
    fn tests_drive(&mut self, root: &Path) -> bool {
        let Some(project) = self.tests.projects.get_mut(root) else { return false };
        if project.call.is_some() {
            return false;
        }
        if let Some(step) = project.step.as_mut() {
            let (run, provider, number) = (step.run, step.provider.clone(), step.step);
            if let Some(why) = step.failed.take() {
                let end = fenix_tests::StepEnd::Failed(why);
                let tag = step.tag;
                step.ending = true;
                return self.tests_send_step_ended(root, run, &provider, number, end, Some(tag));
            }
            if step.reading || step.ending {
                return false;
            }
            if step.halt.is_none() && !step.behind && !step.lines.is_empty() {
                let lines = step.lines.take();
                step.reading = true;
                let tag = step.tag;
                let ext = Self::tests_ext(&provider);
                let call = Call::TestsRead { provider: provider.provider.to_string(), run: run.0, step: number, lines };
                let token = self.tests_send(&ext, call, CallFor::Read { root: root.to_path_buf(), tag });
                self.tests.projects.get_mut(root).expect("a project").call = Some(token);
                return true;
            }
            if let Some(outcome) = step.ended.clone() {
                // Lines read but not given after a stop are not given
                // (they're in Test output).
                let end = step_end(&outcome, step.halt.as_ref());
                step.ending = true;
                let tag = step.tag;
                return self.tests_send_step_ended(root, run, &provider, number, end, Some(tag));
            }
            return false;
        }
        if let Some((run, provider, number)) = project.not_started.pop_front() {
            return self.tests_send_step_ended(root, run, &provider, number, fenix_tests::StepEnd::Stopped, None);
        }
        let now_ms = self.tests.now_ms();
        let project = self.tests.projects.get_mut(root).expect("a project");
        let Some(action) = project.model.next(now_ms) else { return false };
        match action {
            fenix_tests::Action::Plan { run, provider, ask } => {
                self.tests_run_started(root, run, now_ms);
                let ext = Self::tests_ext(&provider);
                match self.tests_reach(&ext, root, true) {
                    Reach::Yes => {
                        let ask = api_ask(capped_ask(ask, ASK_BYTES));
                        let call = Call::TestsPlan { provider: provider.provider.to_string(), run: run.0, ask };
                        let token = self.tests_send(&ext, call, CallFor::Plan { root: root.to_path_buf(), run, provider: provider.clone() });
                        self.tests.projects.get_mut(root).expect("a project").call = Some(token);
                    }
                    Reach::No(why) => {
                        let project = self.tests.projects.get_mut(root).expect("a project");
                        project.model.provider_stopped(&provider, &format!("{} can't be asked: {why}", provider.provider));
                    }
                }
            }
            fenix_tests::Action::Step { run, provider, step, label } => {
                self.tests_debug_plan(root, run);
                self.tests_run_started(root, run, now_ms);
                self.tests_start_step(root, run, provider, step, &label);
            }
            fenix_tests::Action::NotStarted { run, provider, steps } => {
                let project = self.tests.projects.get_mut(root).expect("a project");
                project.not_started.extend(steps.map(|s| (run, provider.clone(), s)));
            }
            fenix_tests::Action::Ended(end) => self.tests_run_ended(root, end),
        }
        true
    }

    fn tests_send_step_ended(&mut self, root: &Path, run: RunId, provider: &ProviderId, step: u32, end: fenix_tests::StepEnd, tag: Option<u64>) -> bool {
        let ext = Self::tests_ext(provider);
        let call = Call::TestsStepEnded { provider: provider.provider.to_string(), run: run.0, step, end: api_step_end(&end) };
        let token = self.tests_send(&ext, call, CallFor::StepEnded { root: root.to_path_buf(), run, step, end, tag });
        self.tests.projects.get_mut(root).expect("a project").call = Some(token);
        true
    }

    /// A run's first action: *Test output* begins again, and its clock.
    fn tests_run_started(&mut self, root: &Path, run: RunId, now_ms: u64) {
        let project = self.tests.projects.get_mut(root).expect("a project");
        if project.run.is_some_and(|(r, _)| r == run) {
            return;
        }
        project.run = Some((run, now_ms));
        project.timed_out = false;
        let what = match project.model.summary().running {
            Some(p) if p.tests => "tests".to_string(),
            Some(p) if p.build => format!("a build to debug {}", project.debug.as_ref().filter(|d| d.run == run).map_or("a test", |d| d.label.as_str())),
            _ => "a listing".to_string(),
        };
        self.tests_output_reset(root, &format!("-- {what}, run {} --\n", run.0));
    }

    /// Starts step `step` of `provider`'s plan, or finds why it can't
    /// start (§6.5): its command not one it may run, a need that isn't a
    /// file under `/data` or isn't there, a command line over its limit,
    /// a remote project, a program that won't start. Then it ends at once,
    /// `failed` with why.
    fn tests_start_step(&mut self, root: &Path, run: RunId, provider: ProviderId, step: u32, label: &str) {
        let tag = self.tests.token();
        let planned = self.tests.projects.get(root).and_then(|p| p.plan.as_ref()).filter(|(r, p, _)| *r == run && *p == provider).and_then(|(_, _, steps)| steps.get(step as usize).cloned());
        let mut cwd = root.to_path_buf();
        let started: Result<TaskRunner, String> = (|| {
            let planned = planned.ok_or_else(|| "its plan isn't known".to_string())?;
            let ((program, args, dir), env) = planned.command.clone()?;
            // Its folder canonicalized, as the root is, so its reports'
            // relative paths come out the project's; the instance's own
            // spelling of the project is another of its roots.
            if let Some(raw) = self.code_instance_project(&provider.extension).flatten() {
                self.tests_alias(root, &raw);
            }
            cwd = dir.map_or_else(|| root.to_path_buf(), |d| refactor::identity(&d));
            let (program, mut prefix, extra) = self.tests_program(&program);
            prefix.extend(args);
            let args = prefix;
            self.tests_output_line(root, &format!("$ {program} {}    ({label})", args.join(" ")));
            let project = self.tests.projects.get_mut(root).expect("a project");
            let at = project.output_dropped + project.output_lines.len().saturating_sub(1) as u64;
            project.output_steps.push((at, cwd.clone()));
            for (need, given) in planned.needs.iter().zip(&planned.step.needs) {
                match need {
                    Err(why) => return Err(why.clone()),
                    Ok(path) if !path.is_file() => return Err(format!("needs {given}, which isn't there")),
                    Ok(_) => {}
                }
            }
            if let Some(why) = command_line_refused(&program, &args) {
                return Err(why);
            }
            if fenix_vfs::is_remote(root) {
                return Err("running tests isn't available on a remote project yet".into());
            }
            let mut command = std::process::Command::new(&program);
            command.args(&args).current_dir(&cwd);
            // A `.cmd` shim's `cmd.exe` would look in the step's folder (the
            // project) first for what it runs: not there.
            #[cfg(windows)]
            command.env("NoDefaultCurrentDirectoryInExePath", "1");
            command.envs(env).envs(extra);
            let sink = self.tests_sink(tag);
            TaskRunner::spawn_command(command, sink).map_err(|e| format!("couldn't start {program}: {e}"))
        })();
        let (runner, failed) = match started {
            Ok(runner) => (Some(runner), None),
            Err(why) => {
                self.tests_output_line(root, &format!("-- {label} couldn't start: {why} --"));
                (None, Some(why))
            }
        };
        let project = self.tests.projects.get_mut(root).expect("a project");
        project.step = Some(StepRun { tag, run, provider, step, cwd, runner, lines: Batch::default(), reading: false, ending: false, ended: None, failed, halt: None, behind: false });
    }

    /// Where a step's events go: the event loop, else the inbox (tests).
    fn tests_sink(&self, tag: u64) -> impl FnMut(TaskEvent) -> bool + Send + 'static {
        let proxy = self.event_proxy.clone();
        let inbox = self.tests.inbox.clone();
        move |event| match &proxy {
            Some(proxy) => proxy.send_event(FenixUserEvent::TestsStep(tag, event)).is_ok(),
            None => {
                inbox.lock().unwrap_or_else(|p| p.into_inner()).push((tag, event));
                true
            }
        }
    }

    /// The program a step names as Fenix starts it, what comes before its
    /// arguments, and what's added to its environment: resolved, or in
    /// tests the harness itself for `fake-test-runner`.
    fn tests_program(&self, program: &str) -> (String, Vec<String>, Vec<(String, String)>) {
        #[cfg(test)]
        if let Some(found) = self.tests.program_for.as_ref().and_then(|f| f(program)) {
            return found;
        }
        (resolved(program), Vec::new(), Vec::new())
    }

    /// A call's answer, applied.
    fn tests_apply_reply(&mut self, ext: &str, token: u64, result: Result<Reply, String>) {
        let Some((_, what)) = self.tests.calls.remove(&token) else { return };
        let root = what.root().to_path_buf();
        if let Some(project) = self.tests.projects.get_mut(&root) {
            if project.call == Some(token) {
                project.call = None;
            }
        }
        // The call itself failed: the extension trapped, ran past its
        // deadline, or has no such export. Its provider stopped, as after
        // a trap (§6.4); one with no exports isn't asked again.
        let reply = match result {
            Ok(reply) => reply,
            Err(why) => return self.tests_call_failed(ext, &root, &what, &why),
        };
        let roots = self.tests_roots(&root);
        let Some(project) = self.tests.projects.get_mut(&root) else { return };
        match (what, reply) {
            (CallFor::Scan { provider, path, first, .. }, Reply::Scanned(found)) => {
                let roots = self.tests_roots(&root);
                let Some(project) = self.tests.projects.get_mut(&root) else { return };
                let found: Vec<fenix_tests::Found> = found.into_iter().map(|f| model_found(f, &roots, &root)).collect();
                match project.walk.as_mut().filter(|w| first && w.provider == provider) {
                    Some(walk) => {
                        walk.out = false;
                        walk.scanned += 1;
                        walk.batch.push((path, found));
                        if walk.batch.len() >= SCAN_BATCH {
                            let batch = std::mem::take(&mut walk.batch);
                            project.model.scan_files(&provider, batch);
                        }
                    }
                    None if first => {}
                    None => project.model.scan(&provider, &path, found),
                }
            }
            (CallFor::Plan { run, provider, .. }, Reply::Planned(planned)) => {
                match planned {
                    Ok(planned) => {
                        let plan = fenix_tests::Plan { steps: planned.steps.iter().map(|s| fenix_tests::Step { label: s.step.label.clone(), covers: s.step.covers.clone() }).collect(), list_after: planned.list_after };
                        project.plan = Some((run, provider, planned.steps));
                        project.model.planned(run, Ok(plan));
                    }
                    Err(why) => project.model.planned(run, Err(why)),
                }
            }
            (CallFor::Read { tag, .. }, Reply::Reports(reports)) => {
                let Some(step) = project.step.as_mut().filter(|s| s.tag == tag) else { return };
                step.reading = false;
                let (run, number) = (step.run, step.step);
                let reports = model_reports(reports, &roots, &step.cwd);
                project.model.reports(run, number, reports);
            }
            (CallFor::StepEnded { run, step, end, tag, .. }, Reply::Reports(reports)) => {
                let cwd = project.step.as_ref().filter(|s| Some(s.tag) == tag).map_or_else(|| root.clone(), |s| s.cwd.clone());
                let reports = model_reports(reports, &roots, &cwd);
                if let Some(debugging) = project.debug.as_mut().filter(|d| d.run == run) {
                    debugging.step_ended(&end);
                }
                project.model.step_ended(run, step, end, reports);
                if tag.is_some() && project.step.as_ref().map(|s| s.tag) == tag {
                    project.step = None;
                }
            }
            (CallFor::Launch { run, .. }, Reply::Launched(launched)) => self.tests_launched(&root, run, token, launched),
            (CallFor::Scan { .. }, _) => {}
            (what, reply) => {
                let why = format!("it answered {what:?} with {reply:?}");
                self.tests_call_failed(ext, &root, &what, &why);
            }
        }
    }

    /// A call to `ext` that failed (`why`).
    fn tests_call_failed(&mut self, ext: &str, root: &Path, what: &CallFor, why: &str) {
        // Debugging alone: the provider's runs aren't touched (step 6).
        if let CallFor::Launch { run, .. } = what {
            return self.tests_launch_failed(ext, root, *run, why);
        }
        let no_exports = why.contains("it has no tests-") || why.contains("isn't API 1.6's");
        if no_exports {
            if self.tests.no_tests.insert(ext.to_string()) {
                self.note_code_output(ext, format!("tests: {why}; its test providers aren't offered"));
            }
            let roots: Vec<PathBuf> = self.tests.projects.keys().cloned().collect();
            for root in roots {
                self.tests_provider_stopped(&root, ext, why);
                let project = self.tests.projects.get_mut(&root).expect("a project");
                for provider in project.providers.iter().filter(|p| p.id.extension == ext) {
                    project.model.remove_provider(&provider.id);
                }
                project.providers.retain(|p| p.id.extension != ext);
            }
            return;
        }
        // A scan that failed changes nothing; the extension's stop, if it
        // stopped, comes as its own event.
        if let CallFor::Scan { first: true, provider, .. } = what {
            if let Some(walk) = self.tests.projects.get_mut(root).and_then(|p| p.walk.as_mut()).filter(|w| w.provider == *provider) {
                walk.out = false;
            }
            return;
        }
        if matches!(what, CallFor::Scan { .. }) {
            return;
        }
        self.tests_provider_stopped(root, ext, why);
    }

    /// A step's process said something, or ended.
    fn tests_apply_event(&mut self, tag: u64, event: TaskEvent) {
        let Some(root) = self.tests.projects.iter().find(|(_, p)| p.step.as_ref().is_some_and(|s| s.tag == tag)).map(|(r, _)| r.clone()) else { return };
        match event {
            TaskEvent::Output(lines) => {
                let (lines, text) = test_lines(lines);
                let project = self.tests.projects.get_mut(&root).expect("a project");
                let step = project.step.as_mut().expect("its step");
                let mut fell_behind = false;
                if step.halt.is_none() && !step.behind {
                    for line in lines {
                        step.lines.push(line);
                    }
                    if step.lines.too_much() {
                        step.behind = true;
                        step.lines.clear();
                        fell_behind = true;
                    }
                }
                if fell_behind {
                    let (run, number, provider) = (step.run, step.step, step.provider.clone());
                    let why = format!("its output was more than {} could read", provider.provider);
                    project.model.step_unread(run, number, &why);
                    self.note_code_output(&provider.extension, format!("tests: {why}: the rest of the step's output is in Test output only"));
                }
                self.tests_output_text(&root, &text);
            }
            TaskEvent::Finished(outcome) => {
                let project = self.tests.projects.get_mut(&root).expect("a project");
                let step = project.step.as_mut().expect("its step");
                step.runner = None;
                step.ended = Some(outcome.clone());
                let said = match &outcome {
                    TaskOutcome::Exited { code: Some(code), .. } => format!("-- exited {code} --"),
                    TaskOutcome::Exited { code: None, .. } => "-- ended by a signal --".into(),
                    TaskOutcome::Cancelled => "-- stopped --".into(),
                    TaskOutcome::Failed(why) => format!("-- {why} --"),
                };
                self.tests_output_line(&root, &said);
            }
        }
    }

    /// The step under way of a run that's been stopped or timed out: its
    /// tree killed at once; lines read but not given aren't given, and its
    /// end says why (§6.4 "Stopping").
    fn tests_halt(project: &mut ProjectTests, end: fenix_tests::StepEnd) {
        let Some(step) = project.step.as_mut() else { return };
        if step.halt.is_some() || step.failed.is_some() {
            return;
        }
        step.halt = Some(end);
        step.lines.clear();
        if let Some(runner) = &step.runner {
            runner.cancel();
        }
    }

    /// Stops the run in the project at `root`, `why`.
    fn tests_stop_in(&mut self, root: &Path, why: &str) -> bool {
        // A debug waiting its turn, or its provider's answer, goes too.
        let let_go = self.tests_debug_let_go(root, why);
        let Some(project) = self.tests.projects.get_mut(root) else { return let_go };
        let stopped = project.model.stop(why) || let_go;
        Self::tests_halt(project, fenix_tests::StepEnd::Stopped);
        project.save_run = None;
        self.tests_pump();
        stopped
    }

    /// A run ended: *Test output* says how, and the results are kept.
    fn tests_run_ended(&mut self, root: &Path, end: fenix_tests::RunEnd) {
        let project = self.tests.projects.get_mut(root).expect("a project");
        project.run = None;
        project.plan = None;
        #[cfg(test)]
        project.ended_runs.push(end.clone());
        let took = end.ended_ms.saturating_sub(end.started_ms);
        let said = match (&end.halted, &end.kind) {
            (Some(fenix_tests::Halt::Stopped(why)), _) => format!("-- stopped: {why} --"),
            (Some(fenix_tests::Halt::TimedOut), _) => "-- timed out --".to_string(),
            (None, fenix_tests::RunKind::Listing { provider }) => format!("-- {} listed, in {} ms --", provider.provider, took),
            (None, fenix_tests::RunKind::Build { .. }) if end.build_ok => format!("-- built, in {took} ms --"),
            (None, fenix_tests::RunKind::Build { .. }) => format!("-- didn't build, in {took} ms --"),
            (None, _) => format!("-- {} passed, {} failed, {} errored, {} skipped, in {} ms --", end.passed, end.failed, end.errored, end.skipped, took),
        };
        let unsaved = (!end.unsaved.is_empty()).then(|| format!("-- {} had unsaved edits: the runner read them as last saved --", end.unsaved.join(", ")));
        self.tests_output_line(root, &said);
        if let Some(unsaved) = unsaved {
            self.tests_output_line(root, &unsaved);
        }
        if let fenix_tests::RunKind::Build { .. } = end.kind {
            // Nothing of a build is a result to keep: on to the debugger.
            return self.tests_built(root, &end);
        }
        if self.config.bool_value("tests.keep_results") != Some(false) {
            self.tests_write_results(root);
        }
    }

    // Test output.

    /// The project at `root`'s *Test output*, begun again with `header`.
    fn tests_output_reset(&mut self, root: &Path, header: &str) {
        let existing = self.tests.projects.get(root).and_then(|p| p.output).filter(|b| self.buffers.get(*b).is_some());
        let buffer = match existing {
            Some(buffer) => {
                self.reset_task_output_buffer(buffer, header);
                buffer
            }
            None => self.buffers.open_task_output(header),
        };
        let project = self.tests.projects.get_mut(root).expect("a project");
        project.output = Some(buffer);
        project.output_lines = header.split_inclusive('\n').map(str::len).collect();
        project.output_bytes = header.len();
        project.output_dropped = 0;
        project.output_partial = !header.is_empty() && !header.ends_with('\n');
        project.output_steps.clear();
    }

    fn tests_output_line(&mut self, root: &Path, line: &str) {
        self.tests_output_text(root, &format!("{line}\n"));
    }

    /// `text` at the end of the project at `root`'s *Test output*, which
    /// keeps the last 16 MiB of a run and says what went (§6.5).
    fn tests_output_text(&mut self, root: &Path, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.tests.projects.get(root).and_then(|p| p.output).is_none_or(|b| self.buffers.get(b).is_none()) {
            self.tests_output_reset(root, "");
        }
        let project = self.tests.projects.get_mut(root).expect("a project");
        let buffer = project.output.expect("made");
        // A line of the buffer is one length, however many pieces of it
        // came: so trimming drops the lines it counted (§20, step 3's
        // review).
        for piece in text.split_inclusive('\n') {
            match project.output_lines.back_mut() {
                Some(last) if project.output_partial => *last += piece.len(),
                _ => project.output_lines.push_back(piece.len()),
            }
            project.output_partial = !piece.ends_with('\n');
        }
        project.output_bytes += text.len();
        let Some(ob) = self.buffers.get_mut(buffer) else { return };
        let mut end = Cursor { char_idx: ob.buffer.len_chars(), sticky_col: 0 };
        ob.buffer.insert_str(&mut end, text);
        if project.output_bytes <= test_limits::MAX_OUTPUT_KEPT + (1 << 20) {
            return;
        }
        let (mut lines, mut bytes) = (0usize, 0usize);
        while project.output_bytes - bytes > test_limits::MAX_OUTPUT_KEPT {
            let Some(len) = project.output_lines.pop_front() else { break };
            bytes += len;
            lines += 1;
        }
        project.output_bytes -= bytes;
        project.output_dropped += lines as u64;
        // The note on the first line counts as a line it keeps.
        let note_lines = usize::from(project.output_dropped > lines as u64);
        let cut_to = ob.buffer.line_start_char((lines + note_lines).min(ob.buffer.line_count()));
        let note = format!("-- {} lines before were dropped: a run keeps the last 16 MiB of its output --\n", grouped(project.output_dropped as usize));
        let mut start = Cursor::at_start();
        ob.buffer.replace_range(&mut start, 0, cut_to, &note);
    }

    // What the commands do.

    /// The focused file's project and its path there, with the explorer on
    /// and the project not remote; else why not, said.
    fn tests_here(&mut self) -> Option<(PathBuf, String)> {
        if !self.tests_enabled() {
            self.set_error("the test explorer is off (tests.enabled)");
            return None;
        }
        let Some(path) = self.open().buffer.path().map(Path::to_path_buf) else {
            self.set_error("no file here, so no project's tests");
            return None;
        };
        let Some(root) = self.tests_root_of(&path) else {
            self.set_error("this file isn't in a project");
            return None;
        };
        if fenix_vfs::is_remote(&root) {
            self.set_error("running tests isn't available on a remote project yet");
            return None;
        }
        let rel = self.tests_rel(&root, &path);
        self.tests_project(&root);
        Some((root, rel))
    }

    /// Asks the project at `root` to run `selections`, as one request.
    fn tests_request(&mut self, root: &Path, selections: Vec<Selection>, what: &str) {
        let project = self.tests_project(root);
        let mut answer = fenix_tests::Requested::Nothing;
        for selection in selections {
            let said = project.model.request(selection, fenix_tests::Mode::Queue);
            if answer == fenix_tests::Requested::Nothing {
                answer = said;
            }
        }
        match answer {
            fenix_tests::Requested::Nothing => self.set_error(format!("no tests to run: {what}")),
            fenix_tests::Requested::AlreadyGoing => self.set_message(format!("{what}: already running")),
            fenix_tests::Requested::Waits | fenix_tests::Requested::Joined => self.set_message(format!("{what}: waits for the run going")),
            _ => self.set_message(format!("running {what}")),
        }
        self.tests_pump();
    }

    /// `tests.run_here`: the test the cursor is in (§9.4).
    pub(crate) fn tests_run_here(&mut self) {
        let Some((root, rel)) = self.tests_here() else { return };
        let line = self.open().buffer.line_col(&self.cursor()).0 as u32;
        let model = &self.tests.projects[&root].model;
        let Some(key) = model.test_at(&rel, line).and_then(|id| model.node(id)).map(|n| n.key()) else {
            let name = Path::new(&rel).file_name().map_or(rel.clone(), |n| n.to_string_lossy().into_owned());
            return self.set_error(format!("line {} of {name} isn't in a test -- tests.run_file runs this file's", line + 1));
        };
        let what = key.id.clone();
        self.tests_request(&root, vec![Selection::Nodes(vec![key])], &what);
    }

    /// `tests.run_file`: this file's tests.
    pub(crate) fn tests_run_file(&mut self) {
        let Some((root, rel)) = self.tests_here() else { return };
        self.tests_request(&root, vec![Selection::File(rel.clone())], &rel);
    }

    /// `tests.run_all`.
    pub(crate) fn tests_run_all(&mut self) {
        let Some((root, _)) = self.tests_here() else { return };
        self.tests_request(&root, vec![Selection::All], "all the tests");
    }

    /// `tests.run_failed`: those whose last result failed or errored.
    pub(crate) fn tests_run_failed(&mut self) {
        let Some((root, _)) = self.tests_here() else { return };
        self.tests_request(&root, vec![Selection::Failed], "the failed");
    }

    /// `tests.run_last`: the last run again.
    pub(crate) fn tests_run_last(&mut self) {
        let Some((root, _)) = self.tests_here() else { return };
        let Some(last) = self.tests.projects[&root].model.last_selection().map(<[Selection]>::to_vec) else {
            return self.set_error("no test run yet in this project");
        };
        self.tests_request(&root, last, "the last run");
    }

    /// `tests.stop`: the run going in this project.
    pub(crate) fn tests_stop(&mut self) {
        let Some((root, _)) = self.tests_here() else { return };
        if self.tests_stop_in(&root, "stopped") {
            self.set_message("stopping the test run");
        } else {
            self.set_message("no test run to stop");
        }
    }

    /// `tests.output`: the project's *Test output*, beside its tests page
    /// when that's shown, else in the focused pane.
    pub(crate) fn tests_output(&mut self) {
        let Some(root) = self.tests_page_root() else { return };
        let page = self.find_page(|m| matches!(m, PageModel::Tests(p) if p.root == root));
        self.tests_output_beside(&root, page);
    }

    /// `selection` run in the project at `root` in place of the run going
    /// (§4: "by a run that replaces it"), whose step is stopped at once.
    #[cfg_attr(not(test), allow(dead_code))] // the doors' (step 7)
    pub(crate) fn tests_replace(&mut self, root: &Path, selection: Selection) -> fenix_tests::Requested {
        if !self.tests_enabled() {
            return fenix_tests::Requested::Nothing;
        }
        let project = self.tests_project(root);
        let answer = project.model.request(selection, fenix_tests::Mode::Replace);
        if answer == fenix_tests::Requested::Replaces {
            Self::tests_halt(project, fenix_tests::StepEnd::Stopped);
        }
        self.tests_pump();
        answer
    }

    /// A listing of the project at `root`'s providers, asked for (the
    /// page's `u`).
    pub(crate) fn tests_list(&mut self, root: &Path) {
        if !self.tests_enabled() {
            return;
        }
        self.tests_project(root).model.list(None);
        self.tests_pump();
    }

    /// The tests page opened in the project at `root`: the first time,
    /// its providers are listed, as an automatic listing.
    pub(crate) fn tests_page_opened(&mut self, root: &Path) {
        if !self.tests_enabled() {
            return;
        }
        self.tests_project(root).model.page_opened();
        self.tests_pump();
    }
}

/// Walks the project at `root` through its ignores, as Search does
/// (§6.7; `fenix-project/src/search.rs`): hidden files and folders left
/// out, `.gitignore`, `.ignore`, the global and the repository's
/// excludes honoured, read from the parents too and with or without a
/// Git repository, `.git` never. On a thread of its own, it sends each
/// file `patterns` name, but those open (scanned from their buffers),
/// read, up to 2,000 and 1 MiB each, then how many there were. The
/// channel holds a few at a time: no more is read than is scanned.
fn walk_files(root: &Path, patterns: Vec<String>, open: HashSet<String>, cap: usize, cancel: Arc<std::sync::atomic::AtomicBool>) -> mpsc::Receiver<Walked> {
    let (tx, rx) = mpsc::sync_channel(4);
    let root = root.to_path_buf();
    let spawned = std::thread::Builder::new().name("fenix-tests-walk".into()).spawn(move || {
        let mut walk = ignore::WalkBuilder::new(&root);
        walk.hidden(true).ignore(true).git_ignore(true).git_global(true).git_exclude(true).parents(true).require_git(false).sort_by_file_name(|a, b| a.cmp(b));
        walk.filter_entry(|entry| entry.file_name() != ".git");
        let (mut total, mut open_seen, mut sent) = (0, 0, 0);
        for entry in walk.build().flatten() {
            // Let go (a restart, the explorer turned off): stops here,
            // however much of the tree is left.
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            if !entry.file_type().is_some_and(|t| t.is_file()) || !fenix_extensions::manifest::file_matches(&patterns, entry.path(), Some(&root)) {
                continue;
            }
            total += 1;
            let rel = model_path(&entry.path().to_string_lossy(), &root, &root);
            // An open file is scanned from its buffer, and counts as one.
            if open.contains(&rel) {
                open_seen += 1;
                continue;
            }
            if sent + open_seen >= cap {
                continue;
            }
            sent += 1;
            let small = entry.metadata().is_ok_and(|m| m.len() <= test_limits::MAX_SCAN_TEXT as u64);
            let Some(text) = small.then(|| std::fs::read(entry.path()).ok()).flatten() else { continue };
            let text = String::from_utf8_lossy(&text).into_owned();
            if tx.send(Walked::File { rel, text }).is_err() {
                return;
            }
        }
        let _ = tx.send(Walked::Done { total, open: open_seen });
    });
    if spawned.is_err() {
        let (tx, rx) = mpsc::sync_channel(1);
        let _ = tx.send(Walked::Done { total: 0, open: 0 });
        return rx;
    }
    rx
}

mod debug;
mod file;
pub(in crate::app) use file::TestCells;
mod page;
#[cfg(test)]
mod tests;
