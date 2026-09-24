//! `SPC p c`, the new-project wizard: pick a template, say where, answer
//! its questions, review exactly what will be written and run, then
//! watch it happen. Four steps and a run log, all one page buffer.
//!
//! Everything here is plain data -- the wizard's state, what a key does
//! to it, and the page it lays out as text plus colour spans -- so the
//! whole flow is testable without a window. `App` (see `app/projects.rs`)
//! owns the buffer, writes `layout`'s text into it, and does the parts
//! that touch the world: writing files, running the commands, and
//! registering and opening the result.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::page::{fit, frame, wrap, Grid, Key, Page, Role};
use fenix_project::template::{Answer, Answers, AskKind, Hook, Plan, Step as CommandStep, Template};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Template,
    Location,
    Options,
    Review,
    Creating,
}

/// What `App` has to do after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    /// Write the files and start the first command.
    Create,
    /// Run the failed step again.
    Retry,
    /// Leave the failed step and go on with the next.
    Skip,
    /// Register and open what's there.
    Finish,
    /// Browse for the folder to create it in.
    BrowseParent,
}

/// A row that `Enter`/`Space`/`h`/`l` act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// Index into `Wizard::templates`.
    Template(usize),
    Name,
    Parent,
    GitInit,
    GitCommit,
    Register,
    /// Index into the chosen template's asks.
    Ask(usize),
    /// A `many` ask's option: (ask, option).
    ManyOption(usize, usize),
    Continue,
    Create,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Pending,
    Running,
    Done(Duration),
    Failed(String),
    Skipped,
}

#[derive(Debug, Clone)]
pub enum RunWork {
    WriteFiles,
    /// The files that wait for the template's commands (see
    /// `Plan::after_files`).
    WriteAfterFiles,
    Command(CommandStep),
}

#[derive(Debug, Clone)]
pub struct RunStep {
    pub label: String,
    pub work: RunWork,
    pub status: Status,
    /// The most recent output lines, capped.
    pub output: Vec<String>,
}

/// How many of a step's output lines the log keeps.
const OUTPUT_LINES: usize = 6;

#[derive(Debug, Clone)]
pub struct Run {
    pub steps: Vec<RunStep>,
    /// `C-c`: stop after the running step.
    pub cancel_requested: bool,
}

impl Run {
    pub fn running(&self) -> Option<usize> {
        self.steps.iter().position(|s| s.status == Status::Running)
    }

    pub fn failed(&self) -> Option<usize> {
        self.steps.iter().position(|s| matches!(s.status, Status::Failed(_)))
    }

    /// The next step to start, if the run should go on.
    pub fn next_pending(&self) -> Option<usize> {
        if self.cancel_requested || self.failed().is_some() || self.running().is_some() {
            return None;
        }
        self.steps.iter().position(|s| s.status == Status::Pending)
    }

    pub fn finished(&self) -> bool {
        self.running().is_none() && self.failed().is_none() && (self.cancel_requested || self.steps.iter().all(|s| s.status != Status::Pending))
    }

    pub fn push_output(&mut self, step: usize, line: String) {
        if let Some(step) = self.steps.get_mut(step) {
            step.output.push(line);
            if step.output.len() > OUTPUT_LINES {
                step.output.remove(0);
            }
        }
    }
}

pub struct Wizard {
    pub templates: Vec<Template>,
    /// Templates that failed to load, shown at the foot of the list.
    pub errors: Vec<String>,
    /// Whether each program a template needs was found on PATH.
    pub found: BTreeMap<String, bool>,
    pub step: Step,
    pub template: usize,
    pub name: String,
    pub parent: String,
    pub git_init: bool,
    pub git_commit: bool,
    pub register: bool,
    pub answers: Answers,
    /// Index into `fields()` of the focused row.
    pub focus: usize,
    /// A text field being typed into: its text so far.
    pub editing: Option<String>,
    /// Why the current step can't go on, shown under it.
    pub problem: Option<String>,
    pub plan: Option<Plan>,
    /// How many of `plan.steps` are the template's own -- the rest are
    /// the wizard's git steps, which come after the late files.
    template_steps: usize,
    pub run: Option<Run>,
    /// The git repository the new project would land inside, if any --
    /// then `git init` defaults off: the repository already covers it.
    pub repository: Option<PathBuf>,
    /// Whether you've set the git rows yourself; if so, moving the
    /// project in or out of a repository leaves them alone.
    git_touched: bool,
}

/// The order groups are listed in; anything else ("Yours") comes after.
const GROUPS: [&str; 7] = ["Python", "Systems", "Web", "Embedded", "Mission", "Scripting", "Start from"];

impl Wizard {
    pub fn new(templates: Vec<Template>, errors: Vec<String>, found: BTreeMap<String, bool>, parent: &Path) -> Self {
        let mut wizard = Wizard {
            templates,
            errors,
            found,
            step: Step::Template,
            template: 0,
            name: String::new(),
            parent: parent.display().to_string(),
            git_init: true,
            git_commit: true,
            register: true,
            answers: Answers::new(),
            focus: 0,
            editing: None,
            problem: None,
            plan: None,
            template_steps: 0,
            run: None,
            repository: None,
            git_touched: false,
        };
        wizard.refresh_location();
        if let Some(&first) = wizard.template_order().first() {
            wizard.template = first;
        }
        wizard
    }

    /// Template indices in display order: by group, then as loaded.
    pub fn template_order(&self) -> Vec<usize> {
        let rank = |group: &str| GROUPS.iter().position(|g| *g == group).unwrap_or(GROUPS.len());
        let mut order: Vec<usize> = (0..self.templates.len()).collect();
        order.sort_by_key(|&i| (rank(&self.templates[i].group), self.templates[i].group.clone()));
        order
    }

    pub fn chosen(&self) -> Option<&Template> {
        self.templates.get(self.template)
    }

    /// Picks a template by id, resetting its answers.
    #[cfg(test)]
    pub fn choose(&mut self, id: &str) -> bool {
        let Some(i) = self.templates.iter().position(|t| t.id == id) else { return false };
        self.template = i;
        self.answers = self.templates[i].default_answers(&self.name);
        true
    }

    /// The focusable rows of the current step, top to bottom.
    pub fn fields(&self) -> Vec<Field> {
        match self.step {
            Step::Template => self.template_order().into_iter().map(Field::Template).collect(),
            Step::Location => vec![Field::Name, Field::Parent, Field::GitInit, Field::GitCommit, Field::Register, Field::Continue],
            Step::Options => {
                let mut fields = Vec::new();
                if let Some(template) = self.chosen() {
                    for (i, ask) in template.asks.iter().enumerate().filter(|(i, _)| template.asks_now(*i, &self.answers)) {
                        match &ask.kind {
                            AskKind::Many { options, .. } => fields.extend((0..options.len()).map(|o| Field::ManyOption(i, o))),
                            _ => fields.push(Field::Ask(i)),
                        }
                    }
                }
                fields.push(Field::Continue);
                fields
            }
            Step::Review => vec![Field::Create],
            Step::Creating => Vec::new(),
        }
    }

    pub fn focused(&self) -> Option<Field> {
        self.fields().get(self.focus).copied()
    }

    fn focus_on(&mut self, field: Field) {
        if let Some(i) = self.fields().iter().position(|f| *f == field) {
            self.focus = i;
        }
    }

    fn is_text(&self, field: Field) -> bool {
        match field {
            Field::Name | Field::Parent => true,
            Field::Ask(i) => matches!(self.chosen().map(|t| &t.asks[i].kind), Some(AskKind::Text { .. })),
            _ => false,
        }
    }

    fn text_value(&self, field: Field) -> String {
        match field {
            Field::Name => self.name.clone(),
            Field::Parent => self.parent.clone(),
            Field::Ask(i) => match self.chosen().and_then(|t| self.answers.get(&t.asks[i].key)) {
                Some(Answer::Text(s)) => s.clone(),
                _ => String::new(),
            },
            _ => String::new(),
        }
    }

    fn set_text(&mut self, field: Field, value: String) {
        match field {
            Field::Name => {
                let old_names = self.chosen().map(|t| t.default_answers(&self.name));
                self.name = value;
                // Text answers still at the old name's default follow the
                // new name ("Label" defaults to the project name).
                if let (Some(old), Some(template)) = (old_names, self.chosen()) {
                    let new = template.default_answers(&self.name);
                    for (key, old_default) in old {
                        if self.answers.get(&key) == Some(&old_default) {
                            if let Some(new_default) = new.get(&key) {
                                self.answers.insert(key, new_default.clone());
                            }
                        }
                    }
                }
            }
            Field::Parent => {
                self.parent = value;
                self.refresh_location();
            }
            Field::Ask(i) => {
                if let Some(key) = self.chosen().map(|t| t.asks[i].key.clone()) {
                    self.answers.insert(key, Answer::Text(value));
                }
            }
            _ => {}
        }
    }

    /// Enter/Space/h/l on a choice or toggle row.
    fn change(&mut self, field: Field, forward: bool) {
        match field {
            Field::GitInit => {
                self.git_touched = true;
                self.git_init = !self.git_init;
                if !self.git_init {
                    self.git_commit = false;
                }
            }
            Field::GitCommit => {
                self.git_touched = true;
                self.git_commit = !self.git_commit;
                if self.git_commit {
                    self.git_init = true;
                }
            }
            Field::Register => self.register = !self.register,
            Field::Ask(i) => {
                let Some(ask) = self.chosen().map(|t| t.asks[i].clone()) else { return };
                match &ask.kind {
                    AskKind::Choice { choices, .. } => {
                        let current = match self.answers.get(&ask.key) {
                            Some(Answer::Text(s)) => choices.iter().position(|c| c == s).unwrap_or(0),
                            _ => 0,
                        };
                        let next = if forward { (current + 1) % choices.len() } else { (current + choices.len() - 1) % choices.len() };
                        self.answers.insert(ask.key.clone(), Answer::Text(choices[next].clone()));
                    }
                    AskKind::Toggle { .. } => {
                        let on = matches!(self.answers.get(&ask.key), Some(Answer::Bool(true)));
                        self.answers.insert(ask.key.clone(), Answer::Bool(!on));
                    }
                    _ => {}
                }
            }
            Field::ManyOption(i, o) => {
                let Some(ask) = self.chosen().map(|t| t.asks[i].clone()) else { return };
                if let AskKind::Many { options, .. } = &ask.kind {
                    let mut chosen = match self.answers.get(&ask.key) {
                        Some(Answer::Many(items)) => items.clone(),
                        _ => Vec::new(),
                    };
                    let option = &options[o];
                    if chosen.contains(option) {
                        chosen.retain(|c| c != option);
                    } else {
                        // Kept in the template's own order, so commands
                        // read the same whatever order you ticked them.
                        chosen.push(option.clone());
                        chosen.sort_by_key(|c| options.iter().position(|x| x == c));
                    }
                    self.answers.insert(ask.key.clone(), Answer::Many(chosen));
                }
            }
            _ => {}
        }
    }

    fn start_editing(&mut self, field: Field) {
        self.editing = Some(self.text_value(field));
    }

    /// Typed into the field being edited (a paste arrives as one string).
    pub fn type_text(&mut self, text: &str) {
        if let Some(editing) = &mut self.editing {
            editing.extend(text.chars().filter(|c| !c.is_control()));
        }
    }

    fn commit_editing(&mut self) {
        if let (Some(text), Some(field)) = (self.editing.take(), self.focused()) {
            self.set_text(field, text);
            self.problem = None;
        }
    }

    /// The folder the project will be created in.
    pub fn target(&self) -> PathBuf {
        Path::new(self.parent.trim()).join(&self.name)
    }

    /// Why Location can't go on, if it can't.
    pub fn location_problem(&self) -> Option<String> {
        let template = self.chosen()?;
        if let Err(e) = fenix_project::template::validate_name(&self.name, template.name_rule) {
            return Some(e);
        }
        let parent = Path::new(self.parent.trim());
        if self.parent.trim().is_empty() || !parent.is_absolute() {
            return Some("the parent folder must be a full path".to_string());
        }
        if parent.is_file() {
            return Some(format!("{} is a file", parent.display()));
        }
        let target = self.target();
        if target.is_file() {
            return Some(format!("{} is a file", target.display()));
        }
        let count = std::fs::read_dir(&target).map(|e| e.count()).unwrap_or(0);
        (count > 0).then(|| format!("{} already exists and isn't empty", target.display()))
    }

    fn go_to(&mut self, step: Step) {
        self.step = step;
        self.focus = 0;
        self.problem = None;
        self.editing = None;
    }

    /// The template's plan plus the wizard's own git steps.
    pub fn build_plan(&self) -> Result<Plan, String> {
        let template = self.chosen().ok_or("no template chosen")?;
        let mut plan = template.plan(&self.name, Path::new(self.parent.trim()), &self.answers)?;
        // Git comes last: after the template's commands and its late files.
        if self.git_init {
            plan.steps.push(CommandStep::new("git", &["init"]));
            if self.git_commit {
                plan.steps.push(CommandStep::new("git", &["add", "-A"]));
                plan.steps.push(CommandStep::new("git", &["commit", "-q", "-m", "Initial commit"]));
            }
        }
        Ok(plan)
    }

    /// Sets the folder to create the project in (the explorer's pick).
    pub fn set_parent(&mut self, parent: &Path) {
        self.set_text(Field::Parent, parent.display().to_string());
        self.problem = None;
    }

    /// Re-reads what the parent folder is inside: a repository turns the
    /// git rows off (unless you've set them yourself).
    fn refresh_location(&mut self) {
        self.repository = fenix_project::vcs::repository_root(Path::new(self.parent.trim()));
        if !self.git_touched {
            self.git_init = self.repository.is_none();
            self.git_commit = self.repository.is_none();
        }
    }

    /// The language workspace the new project would join (`cargo init`
    /// and `uv init` add themselves to one), for the review to say so.
    pub fn joins_workspace(&self) -> Option<PathBuf> {
        let kind = self.chosen()?.kind;
        if !matches!(kind, fenix_project::ProjectKind::Rust | fenix_project::ProjectKind::Python) {
            return None;
        }
        fenix_project::workspace::workspace_above(Path::new(self.parent.trim()), kind)
    }

    fn advance(&mut self) -> Action {
        match self.step {
            Step::Template => {
                let name = self.name.clone();
                if let Some(template) = self.chosen() {
                    self.answers = template.default_answers(&name);
                }
                self.go_to(Step::Location);
                if self.name.is_empty() {
                    self.start_editing(Field::Name);
                }
            }
            Step::Location => {
                if let Some(problem) = self.location_problem() {
                    self.problem = Some(problem);
                    return Action::None;
                }
                if self.chosen().is_some_and(|t| t.asks.is_empty()) {
                    return self.enter_review();
                }
                self.go_to(Step::Options);
            }
            Step::Options => return self.enter_review(),
            Step::Review => {
                // The disk may have changed since Location was checked.
                if let Some(problem) = self.location_problem() {
                    self.problem = Some(problem);
                    return Action::None;
                }
                let Some(plan) = self.plan.clone() else { return Action::None };
                let files = |n: usize, what: &str| format!("write {n} file{}{what}", if n == 1 { "" } else { "s" });
                let step = |label: String, work: RunWork| RunStep { label, work, status: Status::Pending, output: Vec::new() };
                let command = |s: &CommandStep| step(s.display(), RunWork::Command(s.clone()));
                let mut steps = vec![step(files(plan.files.len(), ""), RunWork::WriteFiles)];
                let (template, git) = plan.steps.split_at(self.template_steps.min(plan.steps.len()));
                steps.extend(template.iter().map(command));
                if !plan.after_files.is_empty() {
                    steps.push(step(files(plan.after_files.len(), " the commands left for last"), RunWork::WriteAfterFiles));
                }
                steps.extend(git.iter().map(command));
                self.run = Some(Run { steps, cancel_requested: false });
                self.go_to(Step::Creating);
                return Action::Create;
            }
            Step::Creating => {}
        }
        Action::None
    }

    /// `:project-new [template] [name] [key=value ...]`: goes as far as
    /// the arguments take it. A template alone lands on Location with
    /// the name being typed; a name too goes straight to Review with
    /// every other answer at its default (or as given), and Esc walks
    /// back to change them.
    pub fn preset(&mut self, args: &str) -> Result<(), String> {
        let mut words = args.split_whitespace();
        let Some(id) = words.next() else { return Ok(()) };
        let Some(i) = self.templates.iter().position(|t| t.id == id) else {
            let ids: Vec<&str> = self.templates.iter().map(|t| t.id.as_str()).collect();
            return Err(format!("no template called {id} -- try {}", ids.join(", ")));
        };
        self.template = i;
        let name = words.next().unwrap_or_default().to_string();
        self.name = name.clone();
        self.answers = self.templates[i].default_answers(&name);
        for pair in words {
            let Some((key, value)) = pair.split_once('=') else { return Err(format!("{pair}: options are key=value")) };
            let Some(ask) = self.templates[i].asks.iter().find(|a| a.key == key) else {
                let keys: Vec<&str> = self.templates[i].asks.iter().map(|a| a.key.as_str()).collect();
                return Err(format!("{id} has no option {key} -- it has {}", keys.join(", ")));
            };
            let answer = match &ask.kind {
                AskKind::Text { .. } => Answer::Text(value.to_string()),
                AskKind::Choice { choices, .. } if choices.iter().any(|c| c == value) => Answer::Text(value.to_string()),
                AskKind::Choice { choices, .. } => return Err(format!("{key} is one of {}", choices.join(", "))),
                AskKind::Toggle { .. } => match value {
                    "true" | "yes" | "on" | "1" => Answer::Bool(true),
                    "false" | "no" | "off" | "0" => Answer::Bool(false),
                    _ => return Err(format!("{key} is true or false")),
                },
                AskKind::Many { options, .. } => {
                    let items: Vec<String> = value.split(',').filter(|v| !v.is_empty()).map(str::to_string).collect();
                    if let Some(bad) = items.iter().find(|v| !options.contains(v)) {
                        return Err(format!("{bad} isn't one of {key}'s options ({})", options.join(", ")));
                    }
                    Answer::Many(items)
                }
            };
            self.answers.insert(key.to_string(), answer);
        }
        self.go_to(Step::Location);
        if name.is_empty() {
            self.editing = Some(String::new());
            return Ok(());
        }
        if let Some(problem) = self.location_problem() {
            self.problem = Some(problem);
            return Ok(());
        }
        self.enter_review();
        Ok(())
    }

    fn enter_review(&mut self) -> Action {
        match self.build_plan() {
            Ok(plan) => {
                self.template_steps = self.chosen().and_then(|t| t.plan(&self.name, Path::new(self.parent.trim()), &self.answers).ok()).map_or(0, |p| p.steps.len());
                self.plan = Some(plan);
                self.go_to(Step::Review);
            }
            Err(e) => self.problem = Some(e),
        }
        Action::None
    }

    fn back(&mut self) -> Action {
        match self.step {
            Step::Template => return Action::Close,
            Step::Location => self.go_to(Step::Template),
            Step::Options => self.go_to(Step::Location),
            Step::Review => {
                let has_options = self.chosen().is_some_and(|t| !t.asks.is_empty());
                self.go_to(if has_options { Step::Options } else { Step::Location });
            }
            Step::Creating => {}
        }
        if self.step == Step::Template {
            let template = self.template;
            self.focus_on(Field::Template(template));
        }
        Action::None
    }

    pub fn key(&mut self, key: Key) -> Action {
        if self.editing.is_some() {
            match key {
                Key::Escape => self.editing = None,
                Key::Enter | Key::Tab => {
                    self.commit_editing();
                    if key == Key::Tab {
                        self.move_focus(1);
                    } else if self.focused() == Some(Field::Name) {
                        // Naming is the one thing Location needs; Enter
                        // after it goes straight on to what's next.
                        self.move_focus(1);
                    }
                }
                Key::Backspace => {
                    if let Some(editing) = &mut self.editing {
                        editing.pop();
                    }
                }
                Key::Char(c) => self.type_text(&c.to_string()),
                Key::Space => self.type_text(" "),
                _ => {}
            }
            return Action::None;
        }
        if self.step == Step::Creating {
            return self.creating_key(key);
        }
        let field = self.focused();
        match key {
            Key::Char('q') => return Action::Close,
            Key::Char('b') if self.step == Step::Location => {
                self.focus_on(Field::Parent);
                return Action::BrowseParent;
            }
            Key::Escape => return self.back(),
            Key::Down | Key::Char('j') | Key::Tab => self.move_focus(1),
            Key::Up | Key::Char('k') | Key::BackTab => self.move_focus(-1),
            Key::Char('g') => self.focus = 0,
            Key::Char('G') => self.focus = self.fields().len().saturating_sub(1),
            Key::Left | Key::Char('h') => {
                if let Some(field) = field {
                    self.change_if_choice(field, false);
                }
            }
            Key::Right | Key::Char('l') => {
                if let Some(field) = field {
                    self.change_if_choice(field, true);
                }
            }
            Key::Char('i') | Key::Char('a') | Key::Char('c') => {
                if let Some(field) = field.filter(|f| self.is_text(*f)) {
                    self.start_editing(field);
                    if key == Key::Char('c') {
                        self.editing = Some(String::new());
                    }
                }
            }
            Key::Space => {
                if let Some(field) = field {
                    self.change(field, true);
                }
            }
            Key::Enter => match field {
                Some(Field::Template(i)) => {
                    self.template = i;
                    return self.advance();
                }
                Some(Field::Continue) | Some(Field::Create) => return self.advance(),
                Some(field) if self.is_text(field) => self.start_editing(field),
                Some(field) => self.change(field, true),
                None => {}
            },
            _ => {}
        }
        if self.step == Step::Template {
            if let Some(Field::Template(i)) = self.focused() {
                self.template = i;
            }
        }
        Action::None
    }

    /// Whether Space means something on the focused row -- a toggle, a
    /// list option or a choice. Elsewhere it stays the leader.
    pub fn claims_space(&self) -> bool {
        match self.focused() {
            Some(Field::GitInit | Field::GitCommit | Field::Register | Field::ManyOption(..)) => true,
            Some(Field::Ask(i)) => matches!(self.chosen().map(|t| &t.asks[i].kind), Some(AskKind::Toggle { .. } | AskKind::Choice { .. })),
            _ => false,
        }
    }

    fn change_if_choice(&mut self, field: Field, forward: bool) {
        let is_choice = matches!(field, Field::Ask(i) if matches!(self.chosen().map(|t| &t.asks[i].kind), Some(AskKind::Choice { .. })));
        if is_choice {
            self.change(field, forward);
        }
    }

    fn move_focus(&mut self, delta: isize) {
        let count = self.fields().len();
        if count > 0 {
            self.focus = (self.focus as isize + delta).clamp(0, count as isize - 1) as usize;
        }
    }

    fn creating_key(&mut self, key: Key) -> Action {
        let Some(run) = &mut self.run else { return Action::None };
        if run.failed().is_some() {
            return match key {
                Key::Char('r') => Action::Retry,
                Key::Char('s') => Action::Skip,
                Key::Char('o') => Action::Finish,
                Key::Char('q') | Key::Escape => Action::Close,
                _ => Action::None,
            };
        }
        if run.finished() {
            return match key {
                Key::Enter | Key::Char('o') => Action::Finish,
                Key::Char('q') | Key::Escape => Action::Close,
                _ => Action::None,
            };
        }
        if key == Key::CtrlC {
            run.cancel_requested = true;
        }
        Action::None
    }
}

// ------------------------------------------------------------------
// Layout
// ------------------------------------------------------------------

/// The page for `wizard` in a pane `cols` cells wide.
pub fn layout(wizard: &Wizard, cols: usize) -> Page {
    let (left, width) = frame(cols, 104);
    let mut g = Grid::new();

    // Header: the title, then the four steps, done ones ticked -- or,
    // where that doesn't fit, just which step this is.
    let mut x = g.put(1, left, "New project", Role::Title) + 4;
    let steps = [(Step::Template, "Template"), (Step::Location, "Location"), (Step::Options, "Options"), (Step::Review, "Review")];
    let current = steps.iter().position(|(s, _)| *s == wizard.step).unwrap_or(steps.len());
    let full_len: usize = steps.iter().map(|(_, label)| label.len() + 2).sum::<usize>() + 5 * (steps.len() - 1);
    let compact = x + full_len > left + width;
    if compact {
        let (n, label) = steps.get(current).map(|(_, l)| (current + 1, *l)).unwrap_or((steps.len(), "Creating"));
        x = g.put(1, x.saturating_sub(2), &format!("{n}/{}", steps.len()), Role::Accent) + 1;
        g.put(1, x, label, Role::Title);
    }
    for (i, (_, label)) in steps.iter().enumerate().filter(|_| !compact) {
        if i > 0 {
            x = g.put(1, x, "  ›  ", Role::Muted);
        }
        let (mark, role) = match i.cmp(&current) {
            std::cmp::Ordering::Less => ("✓".to_string(), Role::Good),
            std::cmp::Ordering::Equal => ((i + 1).to_string(), Role::Accent),
            std::cmp::Ordering::Greater => ((i + 1).to_string(), Role::Muted),
        };
        x = g.put(1, x, &mark, role) + 1;
        x = g.put(1, x, label, if i == current { Role::Title } else { Role::Muted });
    }
    g.rule(2, left..left + width);
    let top = 4;

    let keys: &[(&str, &str)] = match wizard.step {
        Step::Template => layout_templates(wizard, &mut g, left, width, top),
        Step::Location => layout_location(wizard, &mut g, left, width, top),
        Step::Options => layout_options(wizard, &mut g, left, width, top),
        Step::Review => layout_review(wizard, &mut g, left, width, top),
        Step::Creating => layout_creating(wizard, &mut g, left, width, top),
    };

    let keys: &[(&str, &str)] = if wizard.editing.is_some() { &[("Enter", "done"), ("Tab", "next field"), ("Esc", "undo")] } else { keys };
    g.keys(left, width, keys);
    g.finish()
}

fn layout_templates(wizard: &Wizard, g: &mut Grid, left: usize, width: usize, top: usize) -> &'static [(&'static str, &'static str)] {
    let two_columns = width >= 80;
    let list_width = if two_columns { width * 11 / 20 } else { width };
    let mut y = top;
    let mut group = None;
    let focused = wizard.focused();
    for i in wizard.template_order() {
        let template = &wizard.templates[i];
        if group != Some(template.group.as_str()) {
            if group.is_some() {
                y += 1;
            }
            g.heading(y, left, list_width - 2, &template.group);
            group = Some(template.group.as_str());
            y += 1;
        }
        let tag = template.kind.tag();
        g.put(y, left + 2, tag, Role::Kind(template.kind));
        let program = fenix_project::template::programs_needed(template).into_iter().next().unwrap_or_else(|| "built in".to_string());
        let right = list_width.saturating_sub(program.chars().count() + 2);
        g.put(y, left + 7, &fit(&template.name, right.saturating_sub(8)), if focused == Some(Field::Template(i)) { Role::Title } else { Role::Text });
        g.put(y, left + right, &program, Role::Muted);
        if focused == Some(Field::Template(i)) {
            g.focus(y, left..left + list_width - 2);
        }
        y += 1;
    }
    for error in &wizard.errors {
        y += 1;
        g.put(y, left + 2, &fit(&format!("! {error}"), list_width - 4), Role::Bad);
    }

    // The chosen template, described -- beside the list, or under it.
    if let Some(template) = wizard.chosen() {
        let (x, mut y, w) = if two_columns { (left + list_width + 2, top, width - list_width - 2) } else { (left, y + 2, width) };
        let end = g.put(y, x, &template.name, Role::Title);
        g.put(y, end + 2, template.kind.tag(), Role::Kind(template.kind));
        y += 1;
        for line in wrap(&template.description, w) {
            g.put(y, x, &line, Role::Muted);
            y += 1;
        }
        let programs = fenix_project::template::programs_needed(template);
        if !programs.is_empty() {
            y += 1;
            g.heading(y, x, w, "Needs");
            y += 1;
            for program in programs {
                let found = wizard.found.get(&program).copied().unwrap_or(false);
                g.put(y, x, "●", if found { Role::Good } else { Role::Bad });
                let end = g.put(y, x + 2, &program, Role::Text);
                g.put(y, end + 2, if found { "found" } else { "not found on PATH" }, Role::Muted);
                y += 1;
            }
        }
        // What it writes, from a plan with a sample name -- the files it
        // ships; what its commands add isn't known until they run.
        let sample = if template.name_rule == fenix_project::template::NameRule::Sketch { "Example" } else { "example" };
        if let Ok(plan) = template.plan(sample, Path::new("."), &template.default_answers(sample)) {
            y += 1;
            g.heading(y, x, w, "Creates");
            y += 1;
            for file in plan.files.iter().take(6) {
                g.put(y, x, &fit(&file.path.replace(sample, "<name>"), w), Role::Text);
                y += 1;
            }
            if plan.files.len() > 6 {
                g.put(y, x, &format!("and {} more", plan.files.len() - 6), Role::Muted);
                y += 1;
            }
            if !plan.steps.is_empty() {
                let n = plan.steps.len();
                g.put(y, x, &format!("then {n} command{}", if n == 1 { "" } else { "s" }), Role::Muted);
                y += 1;
            }
        }
        y += 1;
        g.heading(y, x, w, "Source");
        g.put(y + 1, x, &fit(&template.origin.describe(), w), Role::Muted);
    }
    &[("Enter", "choose"), ("j k", "move"), ("q", "close")]
}

/// One form row: the label, then `value` at the value column.
fn form_row(g: &mut Grid, wizard: &Wizard, y: usize, left: usize, width: usize, field: Field, label: &str) -> usize {
    let focused = wizard.focused() == Some(field);
    if focused {
        g.focus(y, left..left + width);
    }
    g.put(y, left + 2, label, Role::Muted);
    left + 18
}

#[allow(clippy::too_many_arguments)]
fn text_field(g: &mut Grid, wizard: &Wizard, y: usize, left: usize, width: usize, field: Field, label: &str, hint: Option<&str>) {
    let x = form_row(g, wizard, y, left, width, field, label);
    let editing = wizard.focused() == Some(field) && wizard.editing.is_some();
    let value = if editing { format!("{}▏", wizard.editing.as_deref().unwrap_or_default()) } else { wizard.text_value(field) };
    let shown = if value.is_empty() { "—".to_string() } else { value };
    let room = width.saturating_sub(x - left + 2);
    let end = g.put(y, x, &fit(&shown, room), if editing { Role::Title } else { Role::Text });
    if editing {
        g.panels.push((y, x.saturating_sub(1)..end + 1));
    } else if let Some(hint) = hint {
        if end + 4 < left + width {
            g.put(y, end + 3, &fit(hint, left + width - end - 3), Role::Muted);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn toggle_field(g: &mut Grid, wizard: &Wizard, y: usize, left: usize, width: usize, field: Field, label: &str, on: bool, text: &str) {
    let x = form_row(g, wizard, y, left, width, field, label);
    let end = g.put(y, x, if on { "[x]" } else { "[ ]" }, if on { Role::Good } else { Role::Muted });
    g.put(y, end + 1, &fit(text, (left + width).saturating_sub(end + 1)), Role::Text);
}

fn continue_row(g: &mut Grid, wizard: &Wizard, y: usize, left: usize, width: usize, label: &str) {
    let field = if wizard.step == Step::Review { Field::Create } else { Field::Continue };
    if wizard.focused() == Some(field) {
        g.focus(y, left..left + width);
    }
    let end = g.put(y, left + 2, label, Role::Title);
    g.put(y, end + 1, "›", Role::Accent);
}

fn problem_row(g: &mut Grid, wizard: &Wizard, y: usize, left: usize, width: usize) -> usize {
    match &wizard.problem {
        Some(problem) => {
            let mut y = y;
            for line in wrap(problem, width - 4) {
                g.put(y, left + 2, &line, Role::Bad);
                y += 1;
            }
            y
        }
        None => y,
    }
}

fn layout_location(wizard: &Wizard, g: &mut Grid, left: usize, width: usize, top: usize) -> &'static [(&'static str, &'static str)] {
    let mut y = top;
    if let Some(template) = wizard.chosen() {
        let end = g.put(y, left + 2, template.kind.tag(), Role::Kind(template.kind));
        g.put(y, end + 2, &template.name, Role::Title);
        y += 2;
    }
    g.heading(y, left, width, "Where");
    y += 1;
    let name_hint = match wizard.chosen().map(|t| t.name_rule) {
        Some(fenix_project::template::NameRule::Sketch) => "letters, digits, _ - . -- it names the .ino too",
        _ => "the folder's name",
    };
    text_field(g, wizard, y, left, width, Field::Name, "Name", Some(name_hint));
    text_field(g, wizard, y + 1, left, width, Field::Parent, "In", Some("b browse"));
    let result = wizard.target();
    let ok = !wizard.name.is_empty() && wizard.location_problem().is_none();
    g.put(y + 2, left + 2, "Creates", Role::Muted);
    g.put(y + 2, left + 18, "●", if ok { Role::Good } else { Role::Warn });
    g.put(y + 2, left + 20, &fit(&result.display().to_string(), width.saturating_sub(22)), Role::Muted);
    y += 4;
    g.heading(y, left, width, "After creating");
    y += 1;
    let git_text = match &wizard.repository {
        Some(repo) => format!("git init -- it's inside the {} repository already", repo.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
        None => "git init".to_string(),
    };
    toggle_field(g, wizard, y, left, width, Field::GitInit, "Git", wizard.git_init, &git_text);
    toggle_field(g, wizard, y + 1, left, width, Field::GitCommit, "", wizard.git_commit, "first commit");
    toggle_field(g, wizard, y + 2, left, width, Field::Register, "Projects", wizard.register, "add to SPC p p and Home");
    y += 4;
    continue_row(g, wizard, y, left, width, if wizard.chosen().is_some_and(|t| t.asks.is_empty()) { "Review" } else { "Options" });
    problem_row(g, wizard, y + 2, left, width);
    &[("Enter", "edit / next"), ("b", "browse"), ("Space", "toggle"), ("j k", "move"), ("Esc", "back"), ("q", "close")]
}

fn layout_options(wizard: &Wizard, g: &mut Grid, left: usize, width: usize, top: usize) -> &'static [(&'static str, &'static str)] {
    let Some(template) = wizard.chosen() else { return &[] };
    let mut y = top;
    let end = g.put(y, left + 2, template.kind.tag(), Role::Kind(template.kind));
    g.put(y, end + 2, &wizard.name, Role::Title);
    y += 2;
    g.heading(y, left, width, &template.name);
    y += 1;
    // A question that doesn't apply to the answers so far isn't shown.
    for (i, ask) in template.asks.iter().enumerate().filter(|(i, _)| template.asks_now(*i, &wizard.answers)) {
        match &ask.kind {
            AskKind::Text { .. } => text_field(g, wizard, y, left, width, Field::Ask(i), &ask.label, ask.hint.as_deref()),
            AskKind::Choice { choices, .. } => {
                let x = form_row(g, wizard, y, left, width, Field::Ask(i), &ask.label);
                let value = match wizard.answers.get(&ask.key) {
                    Some(Answer::Text(s)) => s.clone(),
                    _ => String::new(),
                };
                let mut end = g.put(y, x, "‹ ", Role::Accent);
                end = g.put(y, end, &value, Role::Title);
                end = g.put(y, end, " ›", Role::Accent);
                let position = choices.iter().position(|c| *c == value).map(|p| p + 1).unwrap_or(0);
                end = g.put(y, end + 2, &format!("{position}/{}", choices.len()), Role::Muted);
                if let Some(hint) = &ask.hint {
                    if end + 4 < left + width {
                        g.put(y, end + 3, &fit(hint, left + width - end - 3), Role::Muted);
                    }
                }
            }
            AskKind::Toggle { .. } => {
                let on = matches!(wizard.answers.get(&ask.key), Some(Answer::Bool(true)));
                toggle_field(g, wizard, y, left, width, Field::Ask(i), &ask.label, on, ask.hint.as_deref().unwrap_or(""));
            }
            AskKind::Many { options, .. } => {
                let chosen = match wizard.answers.get(&ask.key) {
                    Some(Answer::Many(items)) => items.clone(),
                    _ => Vec::new(),
                };
                for (o, option) in options.iter().enumerate() {
                    let label = if o == 0 { ask.label.as_str() } else { "" };
                    toggle_field(g, wizard, y, left, width, Field::ManyOption(i, o), label, chosen.contains(option), option);
                    y += 1;
                }
                y -= 1;
            }
        }
        y += 1;
    }
    y += 1;
    continue_row(g, wizard, y, left, width, "Review");
    problem_row(g, wizard, y + 2, left, width);
    &[("Enter", "edit / next"), ("h l", "choose"), ("Space", "toggle"), ("Esc", "back"), ("q", "close")]
}

fn layout_review(wizard: &Wizard, g: &mut Grid, left: usize, width: usize, top: usize) -> &'static [(&'static str, &'static str)] {
    let Some(plan) = &wizard.plan else { return &[] };
    let mut y = top;
    let end = g.put(y, left + 2, plan.kind.tag(), Role::Kind(plan.kind));
    g.put(y, end + 2, &fit(&plan.dir.display().to_string(), width.saturating_sub(8)), Role::Title);
    y += 2;
    g.heading(y, left, width, &format!("Files · {} new", plan.files.len() + plan.after_files.len()));
    y += 1;
    for file in &plan.files {
        g.put(y, left + 2, "+", Role::Good);
        g.put(y, left + 4, &fit(&file.path, width - 6), Role::Text);
        y += 1;
    }
    for file in &plan.after_files {
        g.put(y, left + 2, "+", Role::Good);
        let end = g.put(y, left + 4, &fit(&file.path, width.saturating_sub(30)), Role::Text);
        g.put(y, end + 2, "after the commands", Role::Muted);
        y += 1;
    }
    if !plan.steps.is_empty() {
        y += 1;
        g.heading(y, left, width, "Commands · in order, no shell");
        y += 1;
        for (i, step) in plan.steps.iter().enumerate() {
            g.put(y, left + 2, &(i + 1).to_string(), Role::Muted);
            g.put(y, left + 5, &fit(&step.display(), width - 7), Role::Text);
            y += 1;
        }
        g.put(y, left + 5, "files a command makes aren't listed above", Role::Muted);
        y += 1;
    }
    y += 1;
    g.heading(y, left, width, "Then");
    y += 1;
    let mut then = Vec::new();
    if wizard.register {
        then.push("add it to your projects".to_string());
    }
    if let Some(workspace) = wizard.joins_workspace() {
        let tool = if plan.kind == fenix_project::ProjectKind::Rust { "cargo" } else { "uv" };
        then.push(format!("{tool} adds it to the workspace at {}", workspace.display()));
    }
    for hook in &plan.hooks {
        match hook {
            Hook::MibRoot { path, label } if path == "." => then.push(format!("register it as MIB root \"{label}\"")),
            Hook::MibRoot { path, label } => then.push(format!("register {path}/ as MIB root \"{label}\"")),
        }
    }
    then.push("open it".to_string());
    for line in then {
        g.put(y, left + 2, "·", Role::Muted);
        g.put(y, left + 4, &fit(&line, width - 6), Role::Text);
        y += 1;
    }
    y += 1;
    continue_row(g, wizard, y, left, width, "Create");
    problem_row(g, wizard, y + 2, left, width);
    &[("Enter", "create"), ("Esc", "back"), ("q", "close")]
}

fn layout_creating(wizard: &Wizard, g: &mut Grid, left: usize, width: usize, top: usize) -> &'static [(&'static str, &'static str)] {
    let Some(run) = &wizard.run else { return &[] };
    let mut y = top;
    let done = run.steps.iter().filter(|s| matches!(s.status, Status::Done(_) | Status::Skipped)).count();
    let end = g.put(y, left + 2, &format!("Creating {}", wizard.name), Role::Title);
    g.put(y, end + 3, &format!("{done} / {}", run.steps.len()), Role::Muted);
    y += 2;
    for step in &run.steps {
        let (mark, role) = match &step.status {
            Status::Pending => ("○", Role::Muted),
            Status::Running => ("●", Role::Accent),
            Status::Done(_) => ("●", Role::Good),
            Status::Failed(_) => ("●", Role::Bad),
            Status::Skipped => ("○", Role::Warn),
        };
        g.put(y, left + 2, mark, role);
        let right = match &step.status {
            Status::Done(d) => format!("{:.1}s", d.as_secs_f32()),
            Status::Skipped => "skipped".to_string(),
            Status::Running => "running".to_string(),
            _ => String::new(),
        };
        let room = width.saturating_sub(right.chars().count() + 8);
        let label_role = if step.status == Status::Pending { Role::Muted } else { Role::Text };
        g.put(y, left + 4, &fit(&step.label, room), label_role);
        if !right.is_empty() {
            g.put(y, left + width - right.chars().count(), &right, Role::Muted);
        }
        if matches!(step.status, Status::Running | Status::Failed(_)) {
            g.focus(y, left..left + width);
        }
        y += 1;
        if let Status::Failed(message) = &step.status {
            g.put(y, left + 6, &fit(message, width - 8), Role::Bad);
            y += 1;
        }
        if matches!(step.status, Status::Running | Status::Failed(_)) {
            for line in &step.output {
                g.put(y, left + 6, &fit(line, width - 8), Role::Muted);
                y += 1;
            }
        }
    }
    y += 1;
    if run.failed().is_some() {
        g.put(y, left + 2, "Stopped. What's been done stays; nothing is deleted.", Role::Warn);
        return &[("r", "retry"), ("s", "skip"), ("o", "open what's there"), ("q", "close")];
    }
    if run.finished() {
        let text = if run.cancel_requested { "Cancelled. What's been done stays." } else { "Done." };
        g.put(y, left + 2, text, if run.cancel_requested { Role::Warn } else { Role::Good });
        g.focus(y, left..left + width);
        return &[("Enter", "open it"), ("q", "close")];
    }
    &[("C-c", "stop after this step")]
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_project::template::builtin_templates;

    fn wizard(parent: &Path) -> Wizard {
        let found = BTreeMap::from([("uv".to_string(), true), ("git".to_string(), true)]);
        Wizard::new(builtin_templates(), Vec::new(), found, parent)
    }

    fn type_str(w: &mut Wizard, text: &str) {
        for c in text.chars() {
            w.key(Key::Char(c));
        }
    }

    #[test]
    fn templates_are_listed_by_group_with_python_first() {
        let w = wizard(Path::new("/tmp"));
        let groups: Vec<&str> = w.template_order().iter().map(|&i| w.templates[i].group.as_str()).collect();
        assert_eq!(groups.first(), Some(&"Python"));
        let mut seen: Vec<&str> = groups.clone();
        seen.dedup();
        assert_eq!(seen, GROUPS, "each group once, in order");
        assert_eq!(groups.last(), Some(&"Start from"));
        let page = layout(&w, 120);
        assert!(page.text.contains("PYTHON") && page.text.contains("SYSTEMS") && page.text.contains("EMBEDDED") && page.text.contains("MISSION"));
        assert!(page.text.contains("Python · uv"));
        assert!(page.text.contains("found"), "the focused template's needs are looked up:\n{}", page.text);
    }

    #[test]
    fn choosing_a_template_goes_to_location_ready_to_type_the_name() {
        let mut w = wizard(Path::new("/tmp"));
        assert_eq!(w.key(Key::Enter), Action::None);
        assert_eq!(w.step, Step::Location);
        assert!(w.editing.is_some(), "the name field is being typed into");
        type_str(&mut w, "orbit-tools");
        w.key(Key::Enter);
        assert_eq!(w.name, "orbit-tools");
        assert_eq!(w.focused(), Some(Field::Parent), "Enter after the name moves on");
    }

    #[test]
    fn location_refuses_a_bad_name_or_a_folder_that_has_things_in_it() {
        let dir = std::env::temp_dir().join(format!("fenix-wizard-loc-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("taken")).unwrap();
        std::fs::write(dir.join("taken/file"), "").unwrap();
        let mut w = wizard(&dir);
        w.key(Key::Enter);
        type_str(&mut w, "taken");
        w.key(Key::Enter);
        w.key(Key::Char('G'));
        w.key(Key::Enter);
        assert_eq!(w.step, Step::Location);
        assert!(w.problem.as_deref().unwrap().contains("isn't empty"));
        let page = layout(&w, 100);
        let words: Vec<&str> = page.text.split_whitespace().collect();
        assert!(words.windows(2).any(|w| w == ["isn't", "empty"]), "the problem is shown:
{}", page.text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn options_cycle_choices_toggle_lists_and_reach_a_review_with_the_answers() {
        let dir = std::env::temp_dir().join(format!("fenix-wizard-opt-{}", std::process::id()));
        let mut w = wizard(&dir);
        assert!(w.choose("python-uv"));
        w.focus_on(Field::Template(w.template));
        w.key(Key::Enter);
        type_str(&mut w, "orbit-tools");
        w.key(Key::Enter);
        w.key(Key::Char('G'));
        w.key(Key::Enter);
        assert_eq!(w.step, Step::Options);
        // Layout is the first field: app -> lib.
        w.key(Key::Char('l'));
        assert_eq!(w.answers["layout"], Answer::Text("lib".into()));
        // Untick ruff (the second dev option).
        let ruff = w.fields().iter().position(|f| *f == Field::ManyOption(3, 1)).unwrap();
        w.focus = ruff;
        w.key(Key::Space);
        assert_eq!(w.answers["dev"], Answer::Many(vec!["pytest".into()]));
        w.key(Key::Char('G'));
        w.key(Key::Enter);
        assert_eq!(w.step, Step::Review);
        let page = layout(&w, 110);
        assert!(page.text.contains("uv init --lib --package"), "{}", page.text);
        assert!(page.text.contains("uv add --dev pytest"));
        assert!(page.text.contains("git commit -q -m \"Initial commit\""));
        assert!(page.text.contains("+ tests/test_smoke.py"));
    }

    #[test]
    fn preset_goes_as_far_as_its_arguments_take_it() {
        let dir = std::env::temp_dir().join(format!("fenix-wizard-preset-{}", std::process::id()));
        let mut w = wizard(&dir);
        w.preset("python-uv orbit python=3.13 dev=ruff pyright=yes").unwrap();
        assert_eq!(w.step, Step::Review);
        let steps: Vec<String> = w.plan.as_ref().unwrap().steps.iter().map(|s| s.display()).collect();
        assert!(steps[0].contains("--python 3.13") && steps.iter().any(|s| s == "uv add --dev ruff") && steps.iter().any(|s| s == "uv tool install pyright"), "{steps:?}");

        let mut w = wizard(&dir);
        w.preset("arduino-sketch").unwrap();
        assert_eq!((w.step, w.editing.is_some()), (Step::Location, true), "no name: it's asked for");

        for (bad, why) in [("cobol", "no template called cobol"), ("python-uv x python=2.7", "python is one of"), ("python-uv x colour=red", "no option colour"), ("python-uv x dev=pip", "isn't one of dev's options")] {
            let error = wizard(&dir).preset(bad).unwrap_err();
            assert!(error.contains(why), "{bad}: {error}");
        }
    }

    #[test]
    fn inside_a_repository_git_init_defaults_off_and_workspaces_are_named() {
        let repo = std::env::temp_dir().join(format!("fenix-wizard-repo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("crates")).unwrap();
        std::fs::write(repo.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        let mut w = wizard(&repo.join("crates"));
        assert!(w.repository.is_some());
        assert!(!w.git_init && !w.git_commit, "the repository already covers it");
        assert!(w.choose("rust-cargo"));
        assert_eq!(w.joins_workspace().as_deref(), Some(repo.as_path()));
        w.set_parent(&std::env::temp_dir());
        assert!(w.git_init, "outside it again: back on");
        w.focus_on(Field::GitInit);
        w.step = Step::Location;
        w.key(Key::Space);
        w.set_parent(&repo);
        assert!(!w.git_init, "still off: you turned it off yourself");
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn late_files_are_written_after_the_template_but_before_git() {
        let mut w = wizard(&std::env::temp_dir().join("nowhere-fenix"));
        assert!(w.choose("python-uv"));
        w.name = "orbit".into();
        w.step = Step::Location;
        w.focus = w.fields().len() - 1;
        w.key(Key::Enter); // Location -> Options
        w.focus = w.fields().len() - 1;
        w.key(Key::Enter); // Options -> Review
        assert_eq!(w.step, Step::Review);
        w.key(Key::Enter);
        let labels: Vec<String> = w.run.as_ref().unwrap().steps.iter().map(|s| s.label.clone()).collect();
        let late = labels.iter().position(|l| l.contains("left for last")).expect("a late-files step");
        let first_git = labels.iter().position(|l| l.starts_with("git ")).unwrap();
        let last_uv = labels.iter().rposition(|l| l.starts_with("uv ")).unwrap();
        assert!(last_uv < late && late < first_git, "{labels:?}");
    }

    #[test]
    fn b_on_location_asks_to_browse_for_the_folder() {
        let mut w = wizard(Path::new("/tmp"));
        w.key(Key::Enter);
        w.key(Key::Escape); // leave the name field
        assert_eq!(w.key(Key::Char('b')), Action::BrowseParent);
        assert_eq!(w.focused(), Some(Field::Parent));
        w.set_parent(&std::env::temp_dir());
        assert_eq!(w.parent, std::env::temp_dir().display().to_string());
    }

    #[test]
    fn escape_steps_back_and_closes_from_the_first_step() {
        let mut w = wizard(Path::new("/tmp"));
        w.key(Key::Enter);
        w.key(Key::Escape); // ends the name edit
        assert_eq!(w.key(Key::Escape), Action::None);
        assert_eq!(w.step, Step::Template);
        assert_eq!(w.key(Key::Escape), Action::Close);
    }

    #[test]
    fn a_label_that_defaults_to_the_name_follows_the_name() {
        let mut w = wizard(Path::new("/tmp"));
        assert!(w.choose("scos-mib"));
        w.set_text(Field::Name, "mission-c".into());
        assert_eq!(w.answers["label"], Answer::Text("mission-c".into()));
        w.answers.insert("label".into(), Answer::Text("Mission C".into()));
        w.set_text(Field::Name, "mission-d".into());
        assert_eq!(w.answers["label"], Answer::Text("Mission C".into()), "a label you typed is kept");
    }

    #[test]
    fn review_starts_a_run_whose_log_reports_progress_and_failure() {
        let dir = std::env::temp_dir().join(format!("fenix-wizard-run-{}", std::process::id()));
        let mut w = wizard(&dir);
        assert!(w.choose("empty"));
        w.name = "fresh".into();
        w.step = Step::Location;
        w.focus = w.fields().len() - 1;
        w.key(Key::Enter);
        assert_eq!(w.step, Step::Review, "no questions: straight to review");
        assert_eq!(w.key(Key::Enter), Action::Create);
        let run = w.run.as_mut().unwrap();
        assert_eq!(run.steps.len(), 4, "write, git init, add, commit");
        assert_eq!(run.next_pending(), Some(0));
        run.steps[0].status = Status::Done(Duration::from_millis(3));
        run.steps[1].status = Status::Failed("exit code 1".into());
        run.push_output(1, "fatal: nope".into());
        assert_eq!(run.next_pending(), None, "a failure stops the run");
        let page = layout(&w, 100);
        assert!(page.text.contains("exit code 1") && page.text.contains("fatal: nope"));
        assert_eq!(w.key(Key::Char('s')), Action::Skip);
        assert_eq!(w.key(Key::Char('r')), Action::Retry);
    }

    #[test]
    fn space_is_claimed_only_where_it_toggles_something() {
        let mut w = wizard(Path::new("/tmp"));
        assert!(!w.claims_space(), "a template row: Space is the leader");
        w.key(Key::Enter);
        w.key(Key::Escape);
        assert!(!w.claims_space(), "the name field");
        w.focus_on(Field::Register);
        assert!(w.claims_space());
        w.key(Key::Space);
        assert!(!w.register);
    }

    #[test]
    fn every_step_lays_out_within_a_narrow_pane() {
        let mut w = wizard(Path::new("/tmp"));
        for _ in 0..3 {
            let page = layout(&w, 40);
            assert!(page.text.lines().all(|l| l.chars().count() <= 40), "{}", page.text);
            assert!(page.focus.is_some());
            w.key(Key::Enter);
            type_str(&mut w, "x");
            w.key(Key::Enter);
        }
    }
}
