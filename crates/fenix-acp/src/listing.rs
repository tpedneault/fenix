//! The thread list (`SPC a t`, and every project's with `SPC a l`), laid
//! out: a pipeline run as one heading with its steps under it, in order;
//! a thread started on its own as a row of its own; by project when
//! asked, the current project first; archived threads left out unless
//! they're asked for. Plain data in, rows out, so the editor only draws
//! them.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::hub::{RunTag, ThreadId};

/// A thread as the list sees it: what the store's index says, and its
/// state now (`working`, `waiting on you`, or nothing).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub id: ThreadId,
    pub agent: String,
    pub root: PathBuf,
    pub title: String,
    /// Seconds since 1970.
    pub updated: u64,
    pub run: Option<RunTag>,
    pub archived: bool,
    /// Its state now: `working`, `waiting on you`, `read-only`, or
    /// nothing.
    pub state: String,
}

impl Listed {
    fn waiting(&self) -> bool {
        self.state.contains("waiting")
    }

    /// At work, or waiting on you: not a thread to put away.
    pub fn busy(&self) -> bool {
        self.waiting() || self.state == "working"
    }
}

/// How the list is asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Every project's, under a heading each; else the current one's.
    pub by_project: bool,
    /// Archived threads too.
    pub archived: bool,
    /// Runs folded (`Tab`), and unfolded, by run id: what you chose
    /// over what a run starts as (a run under way open, a finished one
    /// folded).
    pub folded: HashSet<String>,
    pub unfolded: HashSet<String>,
    /// A query typed: every thread a row of its own, each saying its run
    /// and project, nothing folded and no headings for projects -- the
    /// rows are sorted by how well they match, so nothing can lean on
    /// the row above it.
    pub flat: bool,
}

/// A row of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Project { root: PathBuf, name: String, threads: usize, current: bool },
    Run { id: String, pipeline: String, request: String, ended: Option<String>, updated: u64, steps: Vec<ThreadId>, folded: bool, archived: bool, waiting: bool },
    /// `mark`: its state now, or how it ended (`✓`, `■ stopped`).
    Step { id: ThreadId, run: String, step: usize, name: String, agent: String, mark: String, archived: bool, request: String, pipeline: String, project: String },
    Thread { id: ThreadId, title: String, agent: String, state: String, updated: u64, archived: bool, project: String },
}

/// A run's or a lone thread's place in a project's list.
enum Entry<'a> {
    Run(String, Vec<&'a Listed>),
    Thread(&'a Listed),
}

impl Entry<'_> {
    fn waiting(&self) -> bool {
        match self {
            Entry::Run(_, steps) => steps.iter().any(|s| s.waiting()),
            Entry::Thread(t) => t.waiting(),
        }
    }

    fn updated(&self) -> u64 {
        match self {
            Entry::Run(_, steps) => steps.iter().map(|s| s.updated).max().unwrap_or(0),
            Entry::Thread(t) => t.updated,
        }
    }
}

/// The rows for `threads`, as `options` ask, `current` being the
/// project you're in.
pub fn rows(threads: &[Listed], current: &Path, options: &Options) -> Vec<Row> {
    let shown: Vec<&Listed> = threads.iter().filter(|t| options.archived || !t.archived).filter(|t| options.by_project || t.root == current).collect();
    let mut by_root: BTreeMap<&Path, Vec<&Listed>> = BTreeMap::new();
    for t in shown {
        by_root.entry(t.root.as_path()).or_default().push(t);
    }
    let mut roots: Vec<(&Path, Vec<&Listed>)> = by_root.into_iter().collect();
    // The current project first, then the most recently active.
    roots.sort_by_key(|(root, threads)| (*root != current, std::cmp::Reverse(threads.iter().map(|t| t.updated).max().unwrap_or(0))));
    let all: Vec<&Path> = roots.iter().map(|(r, _)| *r).collect();
    let mut out = Vec::new();
    for (root, threads) in &roots {
        let name = project_name(root, &all);
        if options.by_project && !options.flat {
            out.push(Row::Project { root: root.to_path_buf(), name: name.clone(), threads: threads.len(), current: *root == current });
        }
        project_rows(threads, options, &name, &mut out);
    }
    out
}

/// A project's name as the list says it: its folder's, or with the
/// folder above when another project has the same name; none for a
/// thread with no project.
fn project_name(root: &Path, all: &[&Path]) -> String {
    let leaf = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let name = leaf(root);
    if name.is_empty() || name == "." {
        return "(no project)".to_string();
    }
    if all.iter().any(|o| *o != root && leaf(o) == name) {
        if let Some(parent) = root.parent().map(leaf).filter(|p| !p.is_empty()) {
            return format!("{parent}/{name}");
        }
    }
    name
}

fn project_rows(threads: &[&Listed], options: &Options, project: &str, out: &mut Vec<Row>) {
    let mut entries: Vec<Entry> = Vec::new();
    for &t in threads {
        match &t.run {
            Some(run) => match entries.iter_mut().find(|e| matches!(e, Entry::Run(id, _) if *id == run.id)) {
                Some(Entry::Run(_, steps)) => steps.push(t),
                _ => entries.push(Entry::Run(run.id.clone(), vec![t])),
            },
            None => entries.push(Entry::Thread(t)),
        }
    }
    // Waiting on you first, then the most recently active.
    entries.sort_by_key(|e| (!e.waiting(), std::cmp::Reverse(e.updated())));
    for entry in entries {
        match entry {
            Entry::Thread(t) => out.push(Row::Thread { id: t.id.clone(), title: t.title.clone(), agent: t.agent.clone(), state: t.state.clone(), updated: t.updated, archived: t.archived, project: project.to_string() }),
            Entry::Run(id, mut steps) => {
                // In the order they were started: a round of a step run
                // again comes after what came before it.
                steps.sort_by_key(|s| s.run.as_ref().map_or((0, 0), |r| (r.seq, r.step)));
                let first = steps[0].run.clone().unwrap_or_default();
                // How it ended is said on whichever step was told.
                let ended = steps.iter().find_map(|s| s.run.as_ref().and_then(|r| r.ended.clone()));
                let working = ended.is_none() && steps.iter().any(|s| s.busy());
                let folded = if options.flat {
                    false
                } else if options.folded.contains(&id) {
                    true
                } else if options.unfolded.contains(&id) {
                    false
                } else {
                    !working
                };
                let waiting = steps.iter().any(|s| s.waiting());
                out.push(Row::Run {
                    id: id.clone(),
                    pipeline: first.pipeline.clone(),
                    request: first.request.clone(),
                    ended: ended.clone(),
                    updated: steps.iter().map(|s| s.updated).max().unwrap_or(0),
                    steps: steps.iter().map(|s| s.id.clone()).collect(),
                    folded,
                    archived: steps.iter().all(|s| s.archived),
                    waiting,
                });
                if !folded {
                    let stopped = ended.as_deref().is_some_and(|e| !e.starts_with("done"));
                    let last = steps.len().saturating_sub(1);
                    let mut rounds: HashMap<usize, usize> = HashMap::new();
                    for (i, s) in steps.iter().enumerate() {
                        let tag = s.run.as_ref();
                        let step = tag.map_or(0, |r| r.step);
                        let round = *rounds.entry(step).and_modify(|n| *n += 1).or_insert(1);
                        let name = tag.map(|r| r.step_name.clone()).unwrap_or_default();
                        let name = if round > 1 { format!("{name} ({round})") } else { name };
                        // What it's doing, or how it went: the step a run
                        // stopped at stopped, the others done.
                        let mark = if !s.state.is_empty() {
                            format!("● {}", s.state)
                        } else if stopped && i == last {
                            "■ stopped".to_string()
                        } else {
                            "✓".to_string()
                        };
                        out.push(Row::Step { id: s.id.clone(), run: id.clone(), step, name, agent: s.agent.clone(), mark, archived: s.archived, request: first.request.clone(), pipeline: first.pipeline.clone(), project: project.to_string() });
                    }
                }
            }
        }
    }
}

/// A row as the list writes it while a query is typed: each one whole,
/// its run and its project said, since the rows are in the order they
/// match, not under their headings.
pub fn flat_label(row: &Row, now: u64) -> String {
    let cut = |t: &str| {
        let t = t.lines().next().unwrap_or("").trim();
        if t.chars().count() > 40 { format!("{}…", t.chars().take(39).collect::<String>()) } else { t.to_string() }
    };
    match row {
        Row::Step { step, name, agent, mark, archived, request, pipeline, project, .. } => {
            format!("{} {name} · {agent} {mark} -- {} ({pipeline}) · {project}{}", step + 1, cut(request), if *archived { " · archived" } else { "" })
        }
        Row::Thread { project, .. } => format!("{} · {project}", label(row, now).trim_start()),
        _ => label(row, now),
    }
}

/// How long ago `then` was, at `now`, short: `now`, `5 m`, `3 h`, `2 d`.
pub fn age(now: u64, then: u64) -> String {
    let secs = now.saturating_sub(then);
    match secs {
        0..=59 => "now".to_string(),
        60..=3599 => format!("{} m", secs / 60),
        3600..=86_399 => format!("{} h", secs / 3600),
        _ => format!("{} d", secs / 86_400),
    }
}

/// A row as the list writes it: left-aligned, what matters first, so a
/// narrow pane cuts the end.
pub fn label(row: &Row, now: u64) -> String {
    let archived = |a: bool| if a { " · archived" } else { "" };
    match row {
        Row::Project { name, threads, current, .. } => {
            format!("{name}{} · {threads} thread{}", if *current { " (this project)" } else { "" }, if *threads == 1 { "" } else { "s" })
        }
        Row::Run { pipeline, request, ended, updated, steps, folded, archived: a, waiting, .. } => {
            let how = match ended.as_deref() {
                Some(e) if e.starts_with("done") => "done",
                Some(e) if e.starts_with("stopped") => "stopped",
                Some(_) => "ended",
                None if *waiting => "waiting on you",
                None => "running",
            };
            let asked = request.lines().next().unwrap_or("").trim();
            let asked: String = if asked.chars().count() > 60 { format!("{}…", asked.chars().take(59).collect::<String>()) } else { asked.to_string() };
            let count = if *folded { format!("  ({} step{})", steps.len(), if steps.len() == 1 { "" } else { "s" }) } else { String::new() };
            format!("{} {asked} · {pipeline} · {how} · {}{}{count}", if *folded { "▸" } else { "▾" }, age(now, *updated), archived(*a))
        }
        Row::Step { step, name, agent, mark, archived: a, .. } => {
            format!("    {} {name}  {agent}  {mark}{}", step + 1, archived(*a))
        }
        Row::Thread { title, agent, state, updated, archived: a, .. } => {
            let state = if state.is_empty() { String::new() } else { format!(" · {state}") };
            format!("  {title} · {agent}{state} · {}{}", age(now, *updated), archived(*a))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(id: &str, root: &str, updated: u64) -> Listed {
        Listed { id: ThreadId(id.into()), agent: "claude".into(), root: PathBuf::from(root), title: format!("{id} title"), updated, run: None, archived: false, state: String::new() }
    }

    fn step(id: &str, run: &str, n: usize, name: &str, updated: u64) -> Listed {
        let tag = RunTag { id: run.into(), pipeline: "Review and fix".into(), request: "Fix the login test\nand more".into(), step: n, step_name: name.into(), seq: n, started: 1, ended: None, ledger: None };
        Listed { run: Some(tag), ..thread(id, "/p", updated) }
    }

    fn ids(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Row::Project { root, .. } => format!("P {}", root.display()),
                Row::Run { id, .. } => format!("R {id}"),
                Row::Step { id, .. } => format!("  {}", id.0),
                Row::Thread { id, .. } => id.0.clone(),
            })
            .collect()
    }

    #[test]
    fn a_run_is_one_entry_its_steps_under_it_in_order() {
        let mut build = step("b", "r1", 1, "build", 30);
        build.state = "working".into();
        let threads = vec![thread("lone", "/p", 20), step("p", "r1", 0, "plan", 10), build, thread("other", "/q", 99)];
        let rows = rows(&threads, Path::new("/p"), &Options::default());
        assert_eq!(ids(&rows), ["R r1", "  p", "  b", "lone"], "running: unfolded; the newest first; /q's left out");
        assert_eq!(label(&rows[0], 30), "▾ Fix the login test · Review and fix · running · now");
        assert_eq!(label(&rows[1], 30), "    1 plan  claude  ✓");
        assert_eq!(label(&rows[2], 30), "    2 build  claude  ● working");
        assert_eq!(label(&rows[3], 3620), "  lone title · claude · 1 h");
    }

    #[test]
    fn a_finished_run_is_folded_until_unfolded_and_says_how_it_ended() {
        let mut review = step("v", "r1", 1, "review", 50);
        review.run.as_mut().unwrap().ended = Some("done: all good".into());
        let threads = vec![step("p", "r1", 0, "plan", 10), review];
        let mut options = Options::default();
        let folded = rows(&threads, Path::new("/p"), &options);
        assert_eq!(ids(&folded), ["R r1"]);
        assert_eq!(label(&folded[0], 50 + 2 * 86_400), "▸ Fix the login test · Review and fix · done · 2 d  (2 steps)");
        options.unfolded.insert("r1".into());
        assert_eq!(ids(&rows(&threads, Path::new("/p"), &options)), ["R r1", "  p", "  v"]);
        options.folded.insert("r1".into());
        assert_eq!(ids(&rows(&threads, Path::new("/p"), &options)), ["R r1"], "folded wins");
    }

    #[test]
    fn archived_threads_are_left_out_unless_asked_for() {
        let mut gone = thread("gone", "/p", 50);
        gone.archived = true;
        let mut old_run = vec![step("p", "r1", 0, "plan", 10), step("b", "r1", 1, "build", 11)];
        old_run.iter_mut().for_each(|s| s.archived = true);
        let mut threads = vec![thread("kept", "/p", 5), gone];
        threads.extend(old_run);
        let options = Options::default();
        assert_eq!(ids(&rows(&threads, Path::new("/p"), &options)), ["kept"]);
        let options = Options { archived: true, ..Options::default() };
        let all = rows(&threads, Path::new("/p"), &options);
        assert_eq!(ids(&all), ["gone", "R r1", "kept"]);
        assert!(label(&all[0], 50).ends_with("archived"));
        assert!(matches!(all[1], Row::Run { archived: true, .. }));
    }

    #[test]
    fn by_project_every_project_s_under_a_heading_the_current_first() {
        let mut waiting = thread("w", "/q", 1);
        waiting.state = "waiting on you".into();
        let threads = vec![thread("a", "/p", 10), thread("b", "/q", 90), waiting, thread("c", "/r", 50)];
        let options = Options { by_project: true, ..Options::default() };
        let rows = rows(&threads, Path::new("/p"), &options);
        assert_eq!(ids(&rows), ["P /p", "a", "P /q", "w", "b", "P /r", "c"], "this project, then by activity; waiting first within one");
        assert_eq!(label(&rows[0], 0), "p (this project) · 1 thread");
        assert_eq!(label(&rows[2], 0), "q · 2 threads");
    }

    #[test]
    fn rounds_come_in_the_order_they_ran_named_by_their_round() {
        let mut threads = Vec::new();
        for (seq, (n, name)) in [(0, "plan"), (1, "build"), (2, "review"), (1, "build"), (2, "review")].into_iter().enumerate() {
            let mut t = step(&format!("t{seq}"), "r1", n, name, 100 - seq as u64);
            t.run.as_mut().unwrap().seq = seq;
            threads.push(t);
        }
        threads[4].run.as_mut().unwrap().ended = Some("stopped: still fails".into());
        let options = Options { unfolded: ["r1".to_string()].into(), ..Options::default() };
        let rows = rows(&threads, Path::new("/p"), &options);
        assert_eq!(ids(&rows), ["R r1", "  t0", "  t1", "  t2", "  t3", "  t4"]);
        let labels: Vec<String> = rows.iter().map(|r| label(r, 0)).collect();
        assert!(labels[4].contains("2 build (2)"), "{labels:?}");
        assert!(labels[5].contains("3 review (2)") && labels[5].contains("■ stopped"), "the step it stopped at: {labels:?}");
        assert!(labels[3].contains('✓'), "{labels:?}");
    }

    #[test]
    fn a_query_lists_each_thread_whole_with_its_run_and_project() {
        let threads = vec![step("p", "r1", 0, "plan", 10), thread("lone", "/p", 5)];
        let options = Options { flat: true, by_project: true, ..Options::default() };
        let rows = rows(&threads, Path::new("/p"), &options);
        assert_eq!(ids(&rows), ["R r1", "  p", "lone"], "no project headings; nothing folded");
        assert_eq!(flat_label(&rows[1], 10), "1 plan · claude ✓ -- Fix the login test (Review and fix) · p");
        assert!(flat_label(&rows[2], 10).ends_with("· p"));
    }

    #[test]
    fn projects_with_the_same_name_are_told_apart() {
        let threads = vec![thread("a", "/a/api", 1), thread("b", "/b/api", 2), thread("c", "", 3)];
        let options = Options { by_project: true, ..Options::default() };
        let names: Vec<String> = rows(&threads, Path::new("/a/api"), &options).iter().filter_map(|r| if let Row::Project { name, .. } = r { Some(name.clone()) } else { None }).collect();
        assert_eq!(names, ["a/api", "(no project)", "b/api"]);
    }

    #[test]
    fn a_long_request_is_cut_and_nothing_is_an_empty_list() {
        let mut s = step("p", "r1", 0, "plan", 1);
        s.run.as_mut().unwrap().request = "x".repeat(200);
        s.state = "working".into();
        let rows = rows(&[s], Path::new("/p"), &Options::default());
        let heading = label(&rows[0], 1);
        assert!(heading.chars().count() < 120 && heading.contains('…'), "{heading}");
        assert!(super::rows(&[], Path::new("/p"), &Options::default()).is_empty());
        assert_eq!(age(100, 200), "now", "a clock that went back");
    }
}
