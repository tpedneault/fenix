//! `SPC h T`, the tutorial -- vimtutor for all of Fenix, offered on the
//! welcome. Lessons are data (`tutorial/*.toml`): the ones built in here,
//! and an extension's from its `tutorial` folder, so a language pack can
//! teach its own keys. Each opens a practice buffer beside the page with
//! its starting text and a task a step at a time; a step is checked by
//! what came of it -- the buffer's text, the cursor, the windows, the page
//! or picker in front -- never by the keys pressed, so any way that gets
//! there passes. A hint shows after a try that didn't, or a pause. The
//! lessons you've done are kept between runs (`state/tutorial.json`).
//!
//! A lesson file:
//!
//! ```toml
//! id = "editing.modes"       # kept as done by this
//! track = "Editing"          # lessons are listed by track
//! order = 1
//! title = "Modes and moving"
//! text = """
//! The practice buffer's text.
//! """
//!
//! [[step]]
//! say = "What to do."
//! hint = "How, for when it doesn't come."
//! cursor = [7, 9]            # and/or any of the checks on `Step`
//! ```
//!
//! A lesson about Git practises in a repository of its own, made fresh
//! each time it starts: `[repo]` names the file and what was committed
//! of it, and `text` is that file as changed since.
//!
//! ```toml
//! [repo]
//! file = "notes.txt"
//! committed = """
//! What the one commit holds.
//! """
//! ```

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use fenix_buffers::BufferId;
use fenix_window::WindowId;
use serde::Deserialize;

use crate::page::{fit, frame, wrap, Grid, Key, Page, Role};

/// The lessons built in, in the order their tracks are listed.
pub const BUILT_IN: &[(&str, &str)] = &[
    ("editing-1-modes.toml", include_str!("../tutorial/editing-1-modes.toml")),
    ("editing-2-words.toml", include_str!("../tutorial/editing-2-words.toml")),
    ("editing-3-objects.toml", include_str!("../tutorial/editing-3-objects.toml")),
    ("editing-4-visual.toml", include_str!("../tutorial/editing-4-visual.toml")),
    ("editing-5-search.toml", include_str!("../tutorial/editing-5-search.toml")),
    ("editing-6-macros.toml", include_str!("../tutorial/editing-6-macros.toml")),
    ("fenix-1-leader.toml", include_str!("../tutorial/fenix-1-leader.toml")),
    ("fenix-2-find.toml", include_str!("../tutorial/fenix-2-find.toml")),
    ("fenix-3-windows.toml", include_str!("../tutorial/fenix-3-windows.toml")),
    ("fenix-4-pages.toml", include_str!("../tutorial/fenix-4-pages.toml")),
    ("fenix-5-terminals.toml", include_str!("../tutorial/fenix-5-terminals.toml")),
    ("fenix-6-settings.toml", include_str!("../tutorial/fenix-6-settings.toml")),
    ("git-1-basics.toml", include_str!("../tutorial/git-1-basics.toml")),
];

/// The keybindings doc, shown when you're done.
pub const KEYBINDINGS: &str = include_str!("../../../docs/KEYBINDINGS.md");

/// What `mode` takes.
pub const MODES: &[&str] = &["normal", "insert", "visual", "replace", "command", "search"];
/// What `picker` takes: one of a kind, any, or none open.
pub const PICKERS: &[&str] = &["files", "commands", "bindings", "lines", "buffers", "tasks", "any", "none"];
/// What `terminal` takes: the panel open, a terminal in the focused
/// pane, one with the keyboard, one open without it, none in front.
pub const TERMINALS: &[&str] = &["panel", "pane", "focused", "unfocused", "closed"];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lesson {
    pub id: String,
    pub track: String,
    #[serde(default)]
    pub order: u32,
    pub title: String,
    /// A line on what it's about, under its title.
    #[serde(default)]
    pub about: String,
    /// The practice buffer's text to start with.
    #[serde(default)]
    pub text: String,
    #[serde(rename = "step")]
    pub steps: Vec<Step>,
    /// A repository to practise in, made fresh when it starts: the
    /// practice buffer is its file.
    #[serde(default)]
    pub repo: Option<PracticeRepo>,
    /// The file it was read from; `None` for one built in.
    #[serde(skip)]
    pub file: Option<PathBuf>,
}

/// A lesson's practice repository: `file`, committed as `committed`, then
/// changed to the lesson's `text`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PracticeRepo {
    pub file: String,
    pub committed: String,
}

/// One step: what to do, a hint, and what it takes to pass -- every check
/// given must hold.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub say: String,
    #[serde(default)]
    pub hint: String,
    /// The practice buffer's whole text (trailing spaces and blank lines
    /// aside), back in Normal mode.
    pub text: Option<String>,
    /// The cursor in the practice buffer, `[line, column]` from 1.
    pub cursor: Option<[usize; 2]>,
    /// The cursor's line in the practice buffer, from 1.
    pub line: Option<usize>,
    /// The mode (`MODES`).
    pub mode: Option<String>,
    /// The windows in the workspace: `"2"`, or `"+1"`/`"-1"` from how
    /// many there were when the step began.
    pub windows: Option<String>,
    /// The focus is in another window than when the step began.
    pub moved: Option<bool>,
    /// The page in front, by kind: `projects`, `git`, `settings`,
    /// `extensions` (or a tab of it, `extensions/browse`), `welcome`,
    /// `tutorial`, `home`, an extension's (`fenix.review/request`) ... --
    /// or something true of it: the Git page's `git/staged`, `git/clean`,
    /// `git/changed`, `git/menu/commit` (a menu open, by its name).
    pub page: Option<String>,
    /// No page of this kind is open.
    pub closed: Option<String>,
    /// The picker open (`PICKERS`).
    pub picker: Option<String>,
    /// The leader's menu is open (`SPC` pressed, waiting for the next).
    pub leader: Option<bool>,
    /// Where the terminal is (`TERMINALS`).
    pub terminal: Option<String>,
}

/// A window count a step asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Windows {
    Exactly(usize),
    /// So many more (or fewer) than at the step's start.
    By(i64),
}

impl Windows {
    pub fn parse(s: &str) -> Option<Windows> {
        let s = s.trim();
        if s.starts_with('+') || s.starts_with('-') {
            s.parse::<i64>().ok().map(Windows::By)
        } else {
            s.parse::<usize>().ok().map(Windows::Exactly)
        }
    }
}

impl Step {
    fn checks(&self) -> usize {
        [self.text.is_some(), self.cursor.is_some(), self.line.is_some(), self.mode.is_some(), self.windows.is_some(), self.moved.is_some(), self.page.is_some(), self.closed.is_some(), self.picker.is_some(), self.leader.is_some(), self.terminal.is_some()]
            .iter()
            .filter(|c| **c)
            .count()
    }

    /// Whether it looks at the practice buffer.
    fn on_practice(&self) -> bool {
        self.text.is_some() || self.cursor.is_some() || self.line.is_some()
    }
}

/// What's wrong with `lesson`, if anything.
pub fn validate(lesson: &Lesson) -> Result<(), String> {
    if lesson.id.trim().is_empty() || lesson.track.trim().is_empty() || lesson.title.trim().is_empty() {
        return Err("id, track and title are needed".into());
    }
    if lesson.steps.is_empty() {
        return Err("no [[step]]".into());
    }
    for (i, step) in lesson.steps.iter().enumerate() {
        let n = i + 1;
        if step.checks() == 0 {
            return Err(format!("step {n} checks nothing"));
        }
        let one_of = |what: &str, value: &Option<String>, of: &[&str]| match value {
            Some(v) if !of.contains(&v.as_str()) => Err(format!("step {n}: {what} = \"{v}\" isn't one of {}", of.join(", "))),
            _ => Ok(()),
        };
        one_of("mode", &step.mode, MODES)?;
        one_of("picker", &step.picker, PICKERS)?;
        one_of("terminal", &step.terminal, TERMINALS)?;
        if let Some(w) = &step.windows {
            Windows::parse(w).ok_or_else(|| format!("step {n}: windows = \"{w}\" isn't a count (\"2\") or a change (\"+1\")"))?;
        }
        if step.cursor.is_some_and(|[l, c]| l == 0 || c == 0) || step.line == Some(0) {
            return Err(format!("step {n}: lines and columns count from 1"));
        }
    }
    Ok(())
}

/// Reads one lesson file's text.
pub fn parse(text: &str) -> Result<Lesson, String> {
    let lesson: Lesson = toml::from_str(text).map_err(|e| e.to_string().trim().to_string())?;
    validate(&lesson)?;
    Ok(lesson)
}

/// Every lesson: those built in, then each of `dirs`' `*.toml` (the
/// installed extensions' `tutorial` folders), listed by track -- the
/// built-in tracks first -- and each track by `order`. What couldn't be
/// read is said, by file.
pub fn load(dirs: &[PathBuf]) -> (Vec<Lesson>, Vec<String>) {
    let mut lessons = Vec::new();
    let mut problems = Vec::new();
    for (name, text) in BUILT_IN {
        match parse(text) {
            Ok(lesson) => lessons.push(lesson),
            Err(e) => problems.push(format!("{name}: {e}")),
        }
    }
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut files: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "toml")).collect();
        files.sort();
        for file in files {
            let read = std::fs::read_to_string(&file).map_err(|e| e.to_string()).and_then(|text| parse(&text));
            match read {
                Ok(lesson) if lessons.iter().any(|l: &Lesson| l.id == lesson.id) => problems.push(format!("{}: there's a lesson {} already", file.display(), lesson.id)),
                Ok(mut lesson) => {
                    lesson.file = Some(file);
                    lessons.push(lesson);
                }
                Err(e) => problems.push(format!("{}: {e}", file.display())),
            }
        }
    }
    let tracks: Vec<String> = {
        let mut seen: Vec<String> = Vec::new();
        for l in &lessons {
            if !seen.contains(&l.track) {
                seen.push(l.track.clone());
            }
        }
        seen
    };
    // Built-in tracks in their order, then the rest by name.
    let rank = |l: &Lesson| {
        let at = tracks.iter().position(|t| *t == l.track).unwrap_or(0);
        let built_in = lessons.iter().any(|b| b.file.is_none() && b.track == l.track);
        (!built_in, if built_in { String::new() } else { l.track.clone() }, at, l.order)
    };
    let mut ranked: Vec<_> = lessons.iter().map(|l| (rank(l), l.clone())).collect();
    ranked.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.title.cmp(&b.1.title)));
    (ranked.into_iter().map(|(_, l)| l).collect(), problems)
}

/// The lessons done, as kept in `path`.
pub fn load_done(path: &Path) -> BTreeSet<String> {
    fenix_storage::state::read::<BTreeSet<String>>(path, "done").ok().flatten().unwrap_or_default()
}

pub fn save_done(path: &Path, done: &BTreeSet<String>) -> std::io::Result<()> {
    fenix_storage::state::write(path, "done", done)
}

/// How the terminal is, for `terminal`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Term {
    /// The panel is open.
    pub panel: bool,
    /// The focused pane is a terminal.
    pub pane: bool,
    /// A terminal has the keyboard.
    pub keyboard: bool,
}

/// What the editor looks like, for a step to be checked against.
#[derive(Debug, Clone)]
pub struct Seen {
    /// The practice buffer's text and cursor (line, column from 1), when
    /// it's the focused buffer.
    pub practice: Option<(String, (usize, usize))>,
    pub mode: &'static str,
    /// Normal mode, with nothing half-typed (an operator, a count).
    pub idle: bool,
    pub windows: usize,
    pub pane: WindowId,
    /// The focused page's kind and, for some, where it is (`extensions/
    /// browse`).
    pub page: Option<(String, Option<String>)>,
    /// Every open page's kind.
    pub pages: Vec<String>,
    pub picker: Option<&'static str>,
    pub leader: bool,
    pub term: Term,
    /// What's true of the page in front (`git/staged`, ...).
    pub facts: Vec<String>,
}

/// How things stood when a step began.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Start {
    pub windows: usize,
    pub pane: WindowId,
}

/// `text` as a step compares it: trailing spaces and blank lines aside.
fn plain(text: &str) -> String {
    text.lines().map(str::trim_end).collect::<Vec<_>>().join("\n").trim_end_matches('\n').to_string()
}

/// The practice text step `step` of `lesson` starts from, as the lesson
/// means it: its text with every earlier step's change made.
pub fn text_before(lesson: &Lesson, step: usize) -> &str {
    lesson.steps[..step.min(lesson.steps.len())].iter().rev().find_map(|s| s.text.as_deref()).unwrap_or(&lesson.text)
}

/// How many times each line that isn't blank is in `text`.
fn line_counts(text: &str) -> std::collections::HashMap<&str, usize> {
    let mut counts = std::collections::HashMap::new();
    for line in text.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()) {
        *counts.entry(line).or_insert(0) += 1;
    }
    counts
}

/// Whether `text` has the change a step makes from `before` to `want`:
/// every line the step adds, removes or changes is there as many times as
/// in `want`. The rest of the buffer is another step's -- done another
/// way, skipped, not done yet -- and doesn't stop this one passing.
fn has_change(before: &str, want: &str, text: &str) -> bool {
    let (before, want_counts, now) = (line_counts(before), line_counts(want), line_counts(text));
    let touched: Vec<&str> = before.keys().chain(want_counts.keys()).copied().filter(|l| before.get(l) != want_counts.get(l)).collect();
    if touched.is_empty() {
        // Only blank lines change: the whole text says.
        return plain(want) == plain(text);
    }
    touched.iter().all(|l| now.get(l) == want_counts.get(l))
}

/// The word around column `col` (from 1) of `line`: where it starts, and
/// which of the same words on the line it is. `None` off a word.
fn word_at(line: &str, col: usize) -> Option<(String, usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let word = |c: &char| c.is_alphanumeric() || *c == '_';
    let at = col.checked_sub(1)?;
    if !chars.get(at).is_some_and(word) {
        return None;
    }
    let start = (0..=at).rev().take_while(|i| word(&chars[*i])).last()?;
    let end = (at..chars.len()).take_while(|i| word(&chars[*i])).last()? + 1;
    let text: String = chars[start..end].iter().collect();
    // Words before it on the line that are the same word.
    let mut nth = 0;
    let mut i = 0;
    while i < start {
        if word(&chars[i]) && (i == 0 || !word(&chars[i - 1])) {
            let e = (i..chars.len()).take_while(|j| word(&chars[*j])).last().unwrap_or(i) + 1;
            if chars[i..e].iter().collect::<String>() == text {
                nth += 1;
            }
            i = e;
        } else {
            i += 1;
        }
    }
    Some((text, nth, at - start))
}

/// Whether the cursor at `(line, col)` in `text` is where a step asks,
/// `[l, c]` in `before`: there exactly, or the same spot in the same word
/// (the same one of its kind on the line) -- an earlier step done another
/// way can have moved the column.
fn at_spot(before: &str, [l, c]: [usize; 2], text: &str, (line, col): (usize, usize)) -> bool {
    if (l, c) == (line, col) {
        return true;
    }
    if l != line {
        return false;
    }
    let wanted = before.lines().nth(l - 1).and_then(|s| word_at(s, c));
    let here = text.lines().nth(line - 1).and_then(|s| word_at(s, col));
    wanted.is_some() && wanted == here
}

/// Whether `step` passes, seen `seen`, begun at `start`; `before` is the
/// practice text the step starts from (`text_before`).
pub fn passes(step: &Step, before: &str, seen: &Seen, start: Start) -> bool {
    if step.on_practice() {
        let Some((text, (line, col))) = &seen.practice else { return false };
        // A change counts once it's done: back in Normal mode.
        if step.mode.is_none() && !seen.idle {
            return false;
        }
        if step.text.as_ref().is_some_and(|want| !has_change(before, want, text)) {
            return false;
        }
        if step.cursor.is_some_and(|spot| !at_spot(before, spot, text, (*line, *col))) || step.line.is_some_and(|l| l != *line) {
            return false;
        }
    }
    if step.mode.as_ref().is_some_and(|m| m != seen.mode) {
        return false;
    }
    if let Some(want) = step.windows.as_deref().and_then(Windows::parse) {
        let ok = match want {
            Windows::Exactly(n) => seen.windows == n,
            Windows::By(d) => seen.windows as i64 - start.windows as i64 == d,
        };
        if !ok {
            return false;
        }
    }
    if step.moved.is_some_and(|m| m != (seen.pane != start.pane)) {
        return false;
    }
    if let Some(want) = &step.page {
        let here = seen.page.as_ref().is_some_and(|(kind, detail)| kind == want || detail.as_ref() == Some(want)) || seen.facts.contains(want);
        if !here {
            return false;
        }
    }
    if step.closed.as_ref().is_some_and(|kind| seen.pages.contains(kind)) {
        return false;
    }
    if let Some(want) = step.picker.as_deref() {
        let ok = match (want, seen.picker) {
            ("none", open) => open.is_none(),
            ("any", open) => open.is_some(),
            (want, open) => open == Some(want),
        };
        if !ok {
            return false;
        }
    }
    if step.leader.is_some_and(|l| l != seen.leader) {
        return false;
    }
    if let Some(want) = step.terminal.as_deref() {
        let t = seen.term;
        let ok = match want {
            "panel" => t.panel,
            "pane" => t.pane,
            "focused" => t.keyboard,
            "unfocused" => (t.panel || t.pane) && !t.keyboard,
            _ => !t.panel && !t.pane,
        };
        if !ok {
            return false;
        }
    }
    true
}

/// Whether what `step` waits to see go -- a page closed, a picker put
/// away, a terminal hidden or given up -- is there now; `None` for a step
/// that waits for nothing to go.
pub fn going_there(step: &Step, seen: &Seen) -> Option<bool> {
    if let Some(kind) = &step.closed {
        return Some(seen.pages.contains(kind));
    }
    if step.picker.as_deref() == Some("none") {
        return Some(seen.picker.is_some());
    }
    match step.terminal.as_deref() {
        Some("closed") => Some(seen.term.panel || seen.term.pane),
        Some("unfocused") => Some(seen.term.keyboard),
        _ => None,
    }
}

/// The lesson under way.
#[derive(Debug, Clone)]
pub struct Run {
    pub lesson: usize,
    pub step: usize,
    pub practice: BufferId,
    pub start: Start,
    /// When the step began, for the pause before its hint.
    pub since: Instant,
    pub hint: bool,
    /// The practice buffer's edit count last looked at, to tell a try.
    pub edits: u64,
    pub skipped: usize,
    /// What the step waits to see go (`going_there`) has been there while
    /// it ran: a page can't be closed, nor a picker put away, that was
    /// never opened.
    pub there: bool,
    /// Every step passed (or skipped).
    pub finished: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    /// Start this lesson, afresh.
    Start(usize),
    /// Back to the practice buffer.
    Practice,
    /// Skip the step under way.
    Skip,
    /// The keybindings doc.
    Keybindings,
}

pub struct TutorialPage {
    pub lessons: Vec<Lesson>,
    /// Files that couldn't be read, and why.
    pub problems: Vec<String>,
    pub done: BTreeSet<String>,
    /// The lesson the cursor's on.
    pub focus: usize,
    pub run: Option<Run>,
}

impl TutorialPage {
    pub fn new(lessons: Vec<Lesson>, problems: Vec<String>, done: BTreeSet<String>) -> Self {
        // The first not done yet.
        let focus = lessons.iter().position(|l| !done.contains(&l.id)).unwrap_or(0);
        TutorialPage { lessons, problems, done, focus, run: None }
    }

    /// The step under way, while a lesson is.
    pub fn step(&self) -> Option<&Step> {
        let run = self.run.as_ref().filter(|r| !r.finished)?;
        self.lessons.get(run.lesson)?.steps.get(run.step)
    }

    pub fn all_done(&self) -> bool {
        !self.lessons.is_empty() && self.lessons.iter().all(|l| self.done.contains(&l.id))
    }

    /// On to the next step, `start` as things stand; the lesson is done
    /// after its last. Returns whether it just finished.
    pub fn advance(&mut self, start: Start, edits: u64, skipped: bool) -> bool {
        let Some(run) = self.run.as_mut().filter(|r| !r.finished) else { return false };
        let steps = self.lessons.get(run.lesson).map_or(0, |l| l.steps.len());
        run.skipped += usize::from(skipped);
        run.step += 1;
        run.start = start;
        run.since = Instant::now();
        run.hint = false;
        run.there = false;
        run.edits = edits;
        if run.step < steps {
            return false;
        }
        run.finished = true;
        let lesson = run.lesson;
        if let Some(l) = self.lessons.get(lesson) {
            self.done.insert(l.id.clone());
        }
        // The cursor to the next lesson not done, for Enter.
        self.focus = (lesson + 1..self.lessons.len()).chain(0..lesson).find(|i| !self.done.contains(&self.lessons[*i].id)).unwrap_or(lesson);
        true
    }

    pub fn key(&mut self, key: Key) -> Action {
        let last = self.lessons.len().saturating_sub(1);
        let running = self.run.as_ref().filter(|r| !r.finished).map(|r| r.lesson);
        match key {
            Key::Char('q') | Key::Escape => return Action::Close,
            Key::Char('j') | Key::Down => self.focus = (self.focus + 1).min(last),
            Key::Char('k') | Key::Up => self.focus = self.focus.saturating_sub(1),
            Key::Char('g') => self.focus = 0,
            Key::Char('G') => self.focus = last,
            Key::Char('d') => return Action::Keybindings,
            Key::Char('s') if running.is_some() => return Action::Skip,
            Key::Char('r') => {
                if let Some(lesson) = self.run.as_ref().map(|r| r.lesson) {
                    return Action::Start(lesson);
                }
            }
            Key::Enter if self.lessons.is_empty() => {}
            Key::Enter if running == Some(self.focus) => return Action::Practice,
            Key::Enter => return Action::Start(self.focus),
            _ => {}
        }
        Action::None
    }
}

pub fn layout(page: &TutorialPage, cols: usize) -> Page {
    let (left, width) = frame(cols, 100);
    let mut g = Grid::new();
    let end = g.put(1, left, "Tutorial", Role::Title);
    g.put(1, end + 2, "· Fenix's keys, a lesson at a time", Role::Muted);
    let built_in = page.lessons.iter().filter(|l| l.file.is_none());
    let count = format!("{} of {} done", built_in.clone().filter(|l| page.done.contains(&l.id)).count(), built_in.count());
    g.put(1, (left + width).saturating_sub(count.chars().count()), &count, Role::Muted);
    g.rule(2, left..left + width);
    let mut y = 3;
    let intro = if page.all_done() {
        "Every lesson done -- that's the core. The rest of the keys are in the keybindings doc (d opens it); SPC h b lists the keys bound where you are, and SPC h k says what a key does."
    } else {
        "Each lesson opens a practice buffer beside this page with a task, a step at a time. Do it any way you like: it's what comes of it that's checked. Stuck? A hint shows after a try, or a pause."
    };
    for line in wrap(intro, width) {
        g.put(y, left, &line, if page.all_done() { Role::Good } else { Role::Muted });
        y += 1;
    }
    for problem in &page.problems {
        y += 1;
        g.put(y, left, &fit(&format!("couldn't read {problem}"), width), Role::Bad);
    }

    let running = page.run.as_ref().map(|r| r.lesson);
    let mut track: Option<&str> = None;
    let mut number = 0;
    for (i, lesson) in page.lessons.iter().enumerate() {
        if track != Some(lesson.track.as_str()) {
            y += 1;
            g.heading(y, left, width, &lesson.track);
            y += 1;
            track = Some(&lesson.track);
            number = 0;
        }
        number += 1;
        if i == page.focus {
            g.focus(y, left..left + width);
        }
        let done = page.done.contains(&lesson.id);
        let (mark, role) = match (done, running == Some(i)) {
            (_, true) => ("›", Role::Accent),
            (true, false) => ("✓", Role::Good),
            _ => (" ", Role::Muted),
        };
        g.put(y, left + 1, mark, role);
        g.put(y, left + 3, &format!("{number:>2}"), Role::Muted);
        let end = g.put(y, left + 7, &fit(&lesson.title, width.saturating_sub(30)), Role::Text);
        let status = match page.run.as_ref().filter(|r| r.lesson == i) {
            Some(r) if r.finished => "done just now".to_string(),
            Some(r) => format!("step {} of {}", r.step + 1, lesson.steps.len()),
            None => format!("{} steps", lesson.steps.len()),
        };
        let x = (left + width).saturating_sub(status.chars().count()).max(end + 2);
        g.put(y, x, &status, if running == Some(i) { Role::Accent } else { Role::Muted });
        y += 1;
    }

    if let Some(run) = &page.run {
        if let Some(lesson) = page.lessons.get(run.lesson) {
            y += 1;
            g.heading(y, left, width, &lesson.title);
            y += 1;
            if !lesson.about.is_empty() {
                for line in wrap(&lesson.about, width) {
                    g.put(y, left, &line, Role::Muted);
                    y += 1;
                }
            }
            let text_w = width.saturating_sub(4).max(10);
            for (i, step) in lesson.steps.iter().enumerate() {
                let (mark, role) = if i < run.step {
                    ("✓", Role::Muted)
                } else if i == run.step && !run.finished {
                    ("›", Role::Text)
                } else {
                    (" ", Role::Muted)
                };
                g.put(y, left + 1, mark, if mark == "›" { Role::Accent } else { role });
                for line in wrap(&step.say, text_w) {
                    g.put(y, left + 4, &line, role);
                    y += 1;
                }
                if i == run.step && run.hint && !run.finished && !step.hint.is_empty() {
                    for (j, line) in wrap(&format!("hint: {}", step.hint), text_w).iter().enumerate() {
                        g.put(y, left + 4 + if j > 0 { 6 } else { 0 }, line, Role::Warn);
                        y += 1;
                    }
                }
            }
            if run.finished {
                y += 1;
                let next = page.lessons.get(page.focus).filter(|l| !page.done.contains(&l.id));
                let said = match (next, run.skipped) {
                    _ if page.all_done() => "Lesson done, and with it every lesson. d opens the keybindings doc, everything Fenix binds.".to_string(),
                    (Some(n), 0) => format!("Lesson done. Enter starts the next one, {}.", n.title),
                    (Some(n), s) => format!("Lesson done ({s} skipped). Enter starts the next one, {}.", n.title),
                    (None, _) => "Lesson done.".to_string(),
                };
                for line in wrap(&said, width) {
                    g.put(y, left, &line, Role::Good);
                    y += 1;
                }
            }
        }
    }
    let mut keys = vec![("Enter", if running.is_some_and(|l| l == page.focus) && page.step().is_some() { "back to it" } else { "start" })];
    if page.step().is_some() {
        keys.extend([("s", "skip a step"), ("r", "restart")]);
    }
    keys.extend([("d", "keybindings"), ("j k", "move"), ("q", "close")]);
    g.keys(left, width, &keys);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane() -> WindowId {
        fenix_window::WindowTree::new(0u8).focused_id()
    }

    fn seen(text: &str, cursor: (usize, usize)) -> Seen {
        Seen { practice: Some((text.into(), cursor)), mode: "normal", idle: true, windows: 2, pane: pane(), page: None, pages: Vec::new(), picker: None, leader: false, term: Term::default(), facts: Vec::new() }
    }

    #[test]
    fn every_built_in_lesson_reads() {
        let (lessons, problems) = load(&[]);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(lessons.len(), 13);
        let tracks: Vec<&str> = lessons.iter().map(|l| l.track.as_str()).collect();
        assert_eq!(tracks.iter().filter(|t| **t == "Editing").count(), 6);
        assert_eq!(&tracks[6..12], ["Fenix"; 6]);
        assert_eq!(tracks[12], "Git", "a section of its own, after Fenix");
        let ids: BTreeSet<&str> = lessons.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids.len(), 13, "ids are each their own");
        assert!(lessons.iter().all(|l| l.steps.len() >= 3), "several steps each");
    }

    #[test]
    fn a_lesson_that_checks_nothing_or_names_what_isnt_is_refused() {
        let bad = |steps: &str| parse(&format!("id = \"x\"\ntrack = \"T\"\ntitle = \"X\"\n{steps}")).unwrap_err();
        assert!(bad("[[step]]\nsay = \"do\"\n").contains("checks nothing"));
        assert!(bad("[[step]]\nsay = \"do\"\nmode = \"sideways\"\n").contains("isn't one of"));
        assert!(bad("[[step]]\nsay = \"do\"\nwindows = \"two\"\n").contains("isn't a count"));
        assert!(bad("[[step]]\nsay = \"do\"\ncursor = [0, 1]\n").contains("from 1"));
        assert!(bad("[[step]]\nsay = \"do\"\ntxet = \"a\"\n").contains("txet"), "a misspelt check isn't ignored");
        assert!(bad("").contains("step"));
    }

    #[test]
    fn an_extensions_lessons_join_after_the_built_in_tracks() {
        let dir = tempfile::tempdir().unwrap();
        let lesson = "id = \"tcl.procs\"\ntrack = \"Tcl\"\ntitle = \"Procs\"\ntext = \"proc a {} {}\"\n[[step]]\nsay = \"Delete it\"\ntext = \"\"\n";
        std::fs::write(dir.path().join("procs.toml"), lesson).unwrap();
        std::fs::write(dir.path().join("broken.toml"), "id = ").unwrap();
        std::fs::write(dir.path().join("zz-again.toml"), lesson.replace("Procs", "Again")).unwrap();
        let (lessons, problems) = load(&[dir.path().to_path_buf()]);
        assert_eq!(lessons.len(), 14);
        assert_eq!(lessons[13].id, "tcl.procs");
        assert_eq!(lessons[13].file.as_deref(), Some(dir.path().join("procs.toml").as_path()));
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("broken.toml")) && problems.iter().any(|p| p.contains("already")), "{problems:?}");
    }

    #[test]
    fn a_step_passes_on_what_came_of_it() {
        let start = Start { windows: 2, pane: pane() };
        let step = Step { say: "x".into(), text: Some("one\ntwo\n".into()), ..Step::default() };
        assert!(passes(&step, "", &seen("one  \ntwo\n\n", (1, 1)), start), "trailing spaces and lines aside");
        assert!(!passes(&step, "", &seen("one\n", (1, 1)), start));
        let mut typing = seen("one\ntwo", (1, 1));
        typing.idle = false;
        typing.mode = "insert";
        assert!(!passes(&step, "", &typing, start), "not while still typing");
        let cursor = Step { say: "x".into(), cursor: Some([2, 3]), ..Step::default() };
        assert!(passes(&cursor, "", &seen("", (2, 3)), start));
        assert!(!passes(&cursor, "", &Seen { practice: None, ..seen("", (2, 3)) }, start), "in the practice buffer");
        let split = Step { say: "x".into(), windows: Some("+1".into()), ..Step::default() };
        assert!(!passes(&split, "", &seen("", (1, 1)), start));
        assert!(passes(&split, "", &Seen { windows: 3, ..seen("", (1, 1)) }, start));
        let only = Step { windows: Some("1".into()), ..split };
        assert!(passes(&only, "", &Seen { windows: 1, ..seen("", (1, 1)) }, start));
        let browse = Step { say: "x".into(), page: Some("extensions/browse".into()), ..Step::default() };
        assert!(passes(&browse, "", &Seen { page: Some(("extensions".into(), Some("extensions/browse".into()))), ..seen("", (1, 1)) }, start));
        assert!(!passes(&browse, "", &Seen { page: Some(("extensions".into(), Some("extensions/installed".into()))), ..seen("", (1, 1)) }, start));
        let closed = Step { say: "x".into(), closed: Some("git".into()), ..Step::default() };
        assert!(!passes(&closed, "", &Seen { pages: vec!["git".into()], ..seen("", (1, 1)) }, start));
        let none = Step { say: "x".into(), picker: Some("none".into()), ..Step::default() };
        assert!(!passes(&none, "", &Seen { picker: Some("files"), ..seen("", (1, 1)) }, start));
        let unfocused = Step { say: "x".into(), terminal: Some("unfocused".into()), ..Step::default() };
        assert!(passes(&unfocused, "", &Seen { term: Term { panel: true, ..Term::default() }, ..seen("", (1, 1)) }, start));
        assert!(!passes(&unfocused, "", &Seen { term: Term { panel: true, keyboard: true, pane: false }, ..seen("", (1, 1)) }, start));
    }

    /// A step is checked on its own change: another step's -- skipped,
    /// done another way, or done early -- doesn't stop it passing. Before,
    /// skipping one step of a lesson made every later one impossible.
    #[test]
    fn a_text_step_is_checked_on_its_own_change_not_on_every_earlier_one() {
        let start = Start { windows: 1, pane: pane() };
        let lesson = parse(
            "id = \"t\"\ntrack = \"T\"\ntitle = \"T\"\ntext = \"\"\"\nthis is is a sentence.\nKeep this part. Remove this part.\nCopy me.\n\"\"\"\n\
             [[step]]\nsay = \"dw\"\ntext = \"\"\"\nthis is a sentence.\nKeep this part. Remove this part.\nCopy me.\n\"\"\"\n\
             [[step]]\nsay = \"D\"\ntext = \"\"\"\nthis is a sentence.\nKeep this part.\nCopy me.\n\"\"\"\n\
             [[step]]\nsay = \"yyp\"\ntext = \"\"\"\nthis is a sentence.\nKeep this part.\nCopy me.\nCopy me.\n\"\"\"\n",
        )
        .unwrap();
        let check = |n: usize, text: &str| passes(&lesson.steps[n], text_before(&lesson, n), &seen(text, (1, 1)), start);
        // Step 1 skipped: step 2 still passes on its own line.
        assert!(check(1, "this is is a sentence.\nKeep this part.\nCopy me.\n"));
        assert!(!check(1, "this is is a sentence.\nKeep this part. Remove this part.\nCopy me.\n"), "not before it's done");
        // Done a step early: the one before still passes when it comes.
        assert!(check(0, "this is a sentence.\nKeep this part.\nCopy me.\n"));
        // A line put twice: as many times as it should be, no more.
        assert!(check(2, "this is a sentence.\nKeep this part.\nCopy me.\nCopy me.\n"));
        assert!(!check(2, "this is a sentence.\nKeep this part.\nCopy me.\nCopy me.\nCopy me.\n"));
    }

    #[test]
    fn a_cursor_step_takes_the_same_word_where_an_earlier_change_moved_it() {
        let start = Start { windows: 1, pane: pane() };
        let before = "intro\nthe cat and the dog and the dog\n";
        let step = Step { say: "x".into(), cursor: Some([2, 29]), ..Step::default() };
        // As the lesson has it: the second dog.
        assert!(passes(&step, before, &seen(before, (2, 29)), start));
        // A step before was done another way (cat -> bird): the second dog
        // is a column later, and is still the second dog.
        let moved = "intro\nthe bird and the dog and the dog\n";
        assert!(passes(&step, before, &seen(moved, (2, 30)), start));
        assert!(!passes(&step, before, &seen(moved, (2, 18)), start), "the first dog isn't the second");
        assert!(!passes(&step, before, &seen(moved, (2, 31)), start), "the same word, but not its start");
    }

    #[test]
    fn progress_is_kept_and_the_page_moves_on() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state").join("tutorial.json");
        assert!(load_done(&path).is_empty());
        let (lessons, _) = load(&[]);
        let mut page = TutorialPage::new(lessons, Vec::new(), BTreeSet::from(["editing.modes".to_string()]));
        assert_eq!(page.focus, 1, "the first not done");
        assert_eq!(page.key(Key::Enter), Action::Start(1));
        let start = Start { windows: 2, pane: pane() };
        let practice = fenix_buffers::BufferList::new().open_scratch();
        page.run = Some(Run { lesson: 1, step: 0, practice, start, since: Instant::now(), hint: false, edits: 0, skipped: 0, there: false, finished: false });
        assert_eq!(page.key(Key::Enter), Action::Practice);
        assert_eq!(page.key(Key::Char('s')), Action::Skip);
        let steps = page.lessons[1].steps.len();
        for i in 0..steps {
            assert_eq!(page.advance(start, 0, i == 0), i + 1 == steps);
        }
        assert!(page.done.contains("editing.words"));
        assert_eq!(page.focus, 2, "on to the next");
        let text = layout(&page, 100).all_text();
        assert!(text.contains("Lesson done (1 skipped). Enter starts the next one, Operators and text objects."), "{text}");
        assert!(text.contains("2 of 13 done"), "{text}");
        save_done(&path, &page.done).unwrap();
        assert_eq!(load_done(&path), page.done);
    }

    #[test]
    fn when_every_lesson_is_done_it_says_where_the_rest_is() {
        let (lessons, _) = load(&[]);
        let done = lessons.iter().map(|l| l.id.clone()).collect();
        let mut page = TutorialPage::new(lessons, Vec::new(), done);
        let text = layout(&page, 100).all_text();
        assert!(text.contains("Every lesson done") && text.contains("keybindings doc (d opens"), "{text}");
        assert_eq!(page.key(Key::Char('d')), Action::Keybindings);
        assert!(KEYBINDINGS.contains("SPC h T"), "the doc has the tutorial's key");
    }
}
