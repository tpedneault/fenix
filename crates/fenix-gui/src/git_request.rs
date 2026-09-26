//! The new pull request page (`SPC g P`, or `o` after a push): a form
//! prefilled from the branch -- the title from its one commit or its
//! name, the description from the repository's own template (else from
//! the commits), the base from `[git] base_branch`, you as the assignee,
//! a Jira key named in the branch linked -- and under it the commits it
//! brings and what's worth knowing before opening it. Nothing is sent
//! until `C-c C-c`; a branch that isn't pushed is pushed first.

use std::path::{Path, PathBuf};

use fenix_forge::{MergeRequest, NewRequest};

use crate::git_status::count;
use crate::page::{fit, frame, wrap, Grid, Key, Page, Role};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Base,
    Title,
    Draft,
    Assignees,
    Reviewers,
    Labels,
    /// Which of the repository's templates the description starts from
    /// -- there only when it has more than one.
    Template,
    Description,
}

const FIELDS: [Field; 8] = [Field::Base, Field::Title, Field::Draft, Field::Assignees, Field::Reviewers, Field::Labels, Field::Template, Field::Description];

impl Field {
    fn label(self) -> &'static str {
        match self {
            Field::Base => "From",
            Field::Title => "Title",
            Field::Draft => "Draft",
            Field::Assignees => "Assignee",
            Field::Reviewers => "Reviewers",
            Field::Labels => "Labels",
            Field::Template => "Template",
            Field::Description => "Description",
        }
    }
}

/// One of the repository's description templates: its name (the file's,
/// without `.md`) and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub name: String,
    pub text: String,
}

/// The `.md` files in `dir`, as templates, `Default` first and the rest
/// by name.
fn templates_in(dir: &Path) -> Vec<Template> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut found: Vec<Template> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("md")))
        .filter_map(|p| {
            let name = p.file_stem()?.to_string_lossy().into_owned();
            let text = std::fs::read_to_string(&p).ok()?;
            Some(Template { name, text })
        })
        .collect();
    found.sort_by_key(|t| (!t.name.eq_ignore_ascii_case("default"), t.name.to_lowercase()));
    found
}

/// A file in `dir` named `name` whatever its case -- GitHub takes
/// `PULL_REQUEST_TEMPLATE.md` and `pull_request_template.md` alike.
fn file_named(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.filter_map(Result::ok).map(|e| e.path()).find(|p| p.is_file() && p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name)))
}

/// Folders in `dir` named `name` whatever its case.
fn dir_named(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.filter_map(Result::ok).map(|e| e.path()).find(|p| p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name)))
}

fn github_templates(root: &Path) -> Vec<Template> {
    let github = dir_named(root, ".github");
    // Several, to choose from.
    if let Some(dir) = github.as_deref().and_then(|g| dir_named(g, "PULL_REQUEST_TEMPLATE")) {
        let found = templates_in(&dir);
        if !found.is_empty() {
            return found;
        }
    }
    // Or the one, where GitHub looks for it.
    let places = [github.clone(), Some(root.to_path_buf()), dir_named(root, "docs")];
    places
        .iter()
        .flatten()
        .find_map(|dir| file_named(dir, "pull_request_template.md"))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| vec![Template { name: "pull_request_template".to_string(), text }])
        .unwrap_or_default()
}

fn gitlab_templates(root: &Path) -> Vec<Template> {
    dir_named(root, ".gitlab").and_then(|g| dir_named(&g, "merge_request_templates")).map(|d| templates_in(&d)).unwrap_or_default()
}

/// The repository's own description templates, where `forge` keeps them
/// (`.gitlab/merge_request_templates/`, or GitHub's
/// `pull_request_template.md` and `.github/PULL_REQUEST_TEMPLATE/`) --
/// the other forge's places too, when the repository has none of its
/// own forge's.
pub fn templates(root: &Path, forge: &str) -> Vec<Template> {
    let (first, second): (fn(&Path) -> Vec<Template>, fn(&Path) -> Vec<Template>) =
        if forge == "GitLab" { (gitlab_templates, github_templates) } else { (github_templates, gitlab_templates) };
    let found = first(root);
    if found.is_empty() { second(root) } else { found }
}

/// How the branch stands against its remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pushed {
    UpToDate(String),
    /// Pushed, with this many commits since.
    Ahead(usize),
    NotYet,
}

/// What's worth knowing before opening it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Standing {
    pub pushed: Pushed,
    /// Whether it merges into the base cleanly; `None` when git couldn't
    /// say.
    pub merges_cleanly: Option<bool>,
    /// Commits on the base since the branch left it.
    pub base_moved: usize,
    /// `fixup!`/`squash!` commits not folded in yet.
    pub fixups: usize,
}

/// One of the branch's commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub short: String,
    pub subject: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestAction {
    None,
    Close,
    /// Write the description in the compose buffer.
    EditDescription,
    /// Open it: push first when `push`, then ask `reviewers`.
    Open { request: NewRequest, reviewers: Vec<String>, push: bool },
    Browser(String),
    /// Review the one already open.
    Review(u64),
}

pub struct RequestPage {
    pub root: PathBuf,
    pub forge: String,
    pub project: String,
    pub branch: String,
    pub base: String,
    pub title: String,
    pub description: String,
    pub draft: bool,
    pub reviewers: String,
    /// Who it's assigned to, with commas between; you, once the forge
    /// says who you are.
    pub assignees: String,
    pub labels: String,
    /// The repository's description templates, and which one's in use.
    pub templates: Vec<Template>,
    pub template: usize,
    /// The description as it was last filled in, to tell an edited one
    /// from an untouched one.
    filled: String,
    /// Changing the template asks first when the description was edited.
    armed_template: bool,
    /// Newest first.
    pub commits: Vec<Commit>,
    /// Files, lines added, lines removed.
    pub stat: (usize, usize, usize),
    pub standing: Standing,
    /// The request already open for this branch, once the forge has
    /// been asked; `Some(None)` when there's none.
    pub existing: Option<Option<MergeRequest>>,
    pub field: Field,
    /// The field being typed in, and its text so far.
    pub editing: Option<String>,
    pub message: Option<(String, bool)>,
    pub busy: bool,
    armed: bool,
    /// The Jira key named in the branch, for the description.
    jira: Option<String>,
}

/// A Jira key named in the branch -- `feature/FNX-58-status-page` --
/// matched case-insensitively against the project's own key when it has
/// one, else only in capitals, so `fix-2` isn't taken for one.
pub fn jira_key(branch: &str, project: Option<&str>) -> Option<String> {
    let chars: Vec<char> = branch.chars().collect();
    for start in 0..chars.len() {
        if start > 0 && chars[start - 1].is_ascii_alphanumeric() {
            continue;
        }
        let letters = chars[start..].iter().take_while(|c| c.is_ascii_alphanumeric()).count();
        let key: String = chars[start..start + letters].iter().collect();
        if letters < 2 || !key.starts_with(|c: char| c.is_ascii_alphabetic()) || chars.get(start + letters) != Some(&'-') {
            continue;
        }
        let digits = chars[start + letters + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 || chars.get(start + letters + 1 + digits).is_some_and(|c| c.is_ascii_alphanumeric()) {
            continue;
        }
        let known = match project {
            Some(p) => key.eq_ignore_ascii_case(p),
            None => key.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
        };
        if known {
            let number: String = chars[start + letters + 1..start + letters + 1 + digits].iter().collect();
            return Some(format!("{}-{number}", key.to_ascii_uppercase()));
        }
    }
    None
}

fn is_fixup(subject: &str) -> bool {
    subject.starts_with("fixup! ") || subject.starts_with("squash! ") || subject.starts_with("amend! ")
}

/// The title: the one commit's subject, else the branch's name made into
/// a sentence -- `feature/FNX-58-status-page` is "Status page".
pub fn title_for(branch: &str, commits: &[Commit], key: Option<&str>) -> String {
    let real: Vec<&Commit> = commits.iter().filter(|c| !is_fixup(&c.subject)).collect();
    if let [only] = real[..] {
        return only.subject.clone();
    }
    let mut name = branch.rsplit('/').next().unwrap_or(branch).to_string();
    if let Some(key) = key {
        let lower = name.to_ascii_lowercase();
        if let Some(at) = lower.find(&key.to_ascii_lowercase()) {
            name.replace_range(at..at + key.len(), "");
        }
    }
    let words: Vec<&str> = name.split(['-', '_', ' ']).filter(|w| !w.is_empty()).collect();
    let mut title = words.join(" ");
    if let Some(first) = title.get(..1) {
        title = first.to_uppercase() + &title[1..];
    }
    if title.is_empty() {
        branch.to_string()
    } else {
        title
    }
}

/// The description: the repository's template when it has one, else the
/// one commit's body, else a summary of the commits, oldest first --
/// with the Jira key linked at the end.
pub fn description_for(commits: &[Commit], key: Option<&str>, template: Option<&Template>) -> String {
    let real: Vec<&Commit> = commits.iter().rev().filter(|c| !is_fixup(&c.subject)).collect();
    let mut text = match (template, &real[..]) {
        (Some(t), _) => t.text.trim_end().to_string(),
        (None, []) => String::new(),
        (None, [only]) => only.body.trim().to_string(),
        (None, _) => format!("## Summary\n\n{}", real.iter().map(|c| format!("- {}", c.subject)).collect::<Vec<_>>().join("\n")),
    };
    if let Some(key) = key {
        if !text.contains(key) {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(&format!("Refs {key}"));
        }
    }
    text
}

use fenix_config::names;

impl RequestPage {
    #[allow(clippy::too_many_arguments)]
    pub fn new(root: PathBuf, forge: String, project: String, branch: String, base: String, commits: Vec<Commit>, stat: (usize, usize, usize), standing: Standing, jira: Option<String>) -> Self {
        let title = title_for(&branch, &commits, jira.as_deref());
        let templates = templates(&root, &forge);
        let description = description_for(&commits, jira.as_deref(), templates.first());
        let filled = description.clone();
        RequestPage {
            root,
            forge,
            project,
            branch,
            base,
            title,
            description,
            draft: false,
            reviewers: String::new(),
            assignees: String::new(),
            labels: String::new(),
            templates,
            template: 0,
            filled,
            armed_template: false,
            jira,
            commits,
            stat,
            standing,
            existing: None,
            field: Field::Title,
            editing: None,
            message: None,
            busy: false,
            armed: false,
        }
    }

    fn word(&self) -> &'static str {
        if self.forge == "GitLab" {
            "merge request"
        } else {
            "pull request"
        }
    }

    fn text_of(&mut self, field: Field) -> Option<&mut String> {
        match field {
            Field::Base => Some(&mut self.base),
            Field::Title => Some(&mut self.title),
            Field::Assignees => Some(&mut self.assignees),
            Field::Reviewers => Some(&mut self.reviewers),
            Field::Labels => Some(&mut self.labels),
            Field::Draft | Field::Template | Field::Description => None,
        }
    }

    /// The fields the form shows: Template only when there's a choice.
    fn fields(&self) -> Vec<Field> {
        FIELDS.iter().copied().filter(|f| *f != Field::Template || self.templates.len() > 1).collect()
    }

    fn step(&mut self, by: isize) {
        let fields = self.fields();
        let at = fields.iter().position(|f| *f == self.field).unwrap_or(0) as isize;
        self.field = fields[(at + by).clamp(0, fields.len() as isize - 1) as usize];
    }

    /// The template set on the project in the forge's own settings: the
    /// first choice, and the description too unless that was edited.
    pub fn add_forge_template(&mut self, text: String) {
        if self.templates.iter().any(|t| t.text.trim_end() == text.trim_end()) {
            return;
        }
        self.templates.insert(0, Template { name: "project default".to_string(), text });
        if self.description == self.filled {
            self.template = 0;
            self.description = description_for(&self.commits, self.jira.as_deref(), self.templates.first());
            self.filled = self.description.clone();
        } else {
            // Still pointing at the one it was on.
            self.template += 1;
            self.message = Some((format!("{} has a default description template -- it's under Template; yours is kept", self.forge), false));
        }
    }

    /// You, as the assignee -- unless one's been typed already.
    pub fn assign_to_me(&mut self, me: &str) {
        if self.assignees.trim().is_empty() && !me.is_empty() && !(self.field == Field::Assignees && self.editing.is_some()) {
            self.assignees = me.to_string();
        }
    }

    /// The next (or previous) template. The description follows it, but
    /// one that was edited is only replaced when asked twice.
    fn cycle_template(&mut self, by: isize) {
        let n = self.templates.len();
        if n < 2 {
            return;
        }
        if self.description != self.filled && !self.armed_template {
            self.armed_template = true;
            self.message = Some(("the description was edited -- again to replace it with the other template".to_string(), true));
            return;
        }
        self.armed_template = false;
        self.template = (self.template as isize + by).rem_euclid(n as isize) as usize;
        self.description = description_for(&self.commits, self.jira.as_deref(), self.templates.get(self.template));
        self.filled = self.description.clone();
        self.message = Some((format!("the description is the {} template now", self.templates[self.template].name), false));
    }

    /// Text pasted while typing in a field.
    pub fn paste(&mut self, text: &str) {
        if let Some(editing) = &mut self.editing {
            editing.extend(text.chars().filter(|c| !c.is_control()));
        }
    }

    fn finish_editing(&mut self) {
        if let Some(text) = self.editing.take() {
            let field = self.field;
            if let Some(slot) = self.text_of(field) {
                *slot = text.trim().to_string();
            }
        }
    }

    pub fn set_description(&mut self, text: String) {
        self.description = text.trim_end().to_string();
    }

    /// Why it can't be opened as it stands.
    fn problem(&self) -> Option<String> {
        if let Some(Some(existing)) = &self.existing {
            return Some(format!("{} is already open for {} -- M reviews it, o opens it in the browser", existing.reference(), self.branch));
        }
        if self.title.trim().is_empty() {
            return Some("it needs a title".to_string());
        }
        if self.base.trim().is_empty() || self.base == self.branch {
            return Some("it needs a base branch other than this one".to_string());
        }
        if self.commits.is_empty() {
            return Some(format!("{} has no commits that {} doesn't -- nothing to review", self.branch, self.base));
        }
        None
    }

    pub fn key(&mut self, key: Key) -> RequestAction {
        if self.editing.is_some() {
            let editing = self.editing.as_mut().unwrap();
            match key {
                Key::Escape => self.editing = None,
                Key::Backspace => {
                    editing.pop();
                }
                Key::Char(c) => editing.push(c),
                Key::Space => editing.push(' '),
                Key::Enter => self.finish_editing(),
                Key::Tab => {
                    self.finish_editing();
                    self.step(1);
                }
                _ => {}
            }
            return RequestAction::None;
        }
        if key != Key::CtrlC {
            self.armed = false;
        }
        let template_key = self.field == Field::Template && matches!(key, Key::Char('h' | 'l') | Key::Left | Key::Right | Key::Enter | Key::Space);
        if !template_key {
            self.armed_template = false;
        }
        if template_key {
            self.cycle_template(if matches!(key, Key::Char('h') | Key::Left) { -1 } else { 1 });
            return RequestAction::None;
        }
        match key {
            Key::Down | Key::Char('j') | Key::Tab => self.step(1),
            Key::Up | Key::Char('k') | Key::BackTab => self.step(-1),
            Key::Char('d') => self.draft = !self.draft,
            Key::Char('e') => return RequestAction::EditDescription,
            Key::Enter | Key::Space | Key::Char('i') | Key::Char('c') => match self.field {
                Field::Draft => self.draft = !self.draft,
                Field::Description => return RequestAction::EditDescription,
                field => {
                    // `c` starts afresh, the others where it was.
                    let text = if key == Key::Char('c') { String::new() } else { self.text_of(field).cloned().unwrap_or_default() };
                    self.editing = Some(text);
                }
            },
            Key::Char('o') => {
                if let Some(Some(existing)) = &self.existing {
                    return RequestAction::Browser(existing.web_url.clone());
                }
            }
            Key::Char('M') => {
                if let Some(Some(existing)) = &self.existing {
                    return RequestAction::Review(existing.number);
                }
            }
            Key::Char('q') | Key::Escape => return RequestAction::Close,
            Key::CtrlC => {
                if self.busy {
                    return RequestAction::None;
                }
                if let Some(problem) = self.problem() {
                    self.message = Some((problem, true));
                    return RequestAction::None;
                }
                if !self.armed {
                    self.armed = true;
                    return RequestAction::None;
                }
                self.armed = false;
                let request = NewRequest {
                    source_branch: self.branch.clone(),
                    target_branch: self.base.trim().to_string(),
                    title: self.title.trim().to_string(),
                    description: self.description.clone(),
                    draft: self.draft,
                    labels: names(&self.labels),
                    assignees: names(&self.assignees),
                };
                let push = !matches!(self.standing.pushed, Pushed::UpToDate(_));
                return RequestAction::Open { request, reviewers: names(&self.reviewers), push };
            }
            _ => {}
        }
        RequestAction::None
    }
}

pub fn layout(page: &RequestPage, cols: usize) -> Page {
    let (left, width) = frame(cols, 120);
    let mut g = Grid::new();
    let mut y = 1;
    let heading = format!("New {} · {}", page.word(), page.project);
    let x = g.put(y, left, &heading, Role::Title) + 2;
    g.put(y, x, &page.forge, Role::Muted);
    y += 1;
    if page.busy {
        g.put(y, left, &format!("… opening the {}", page.word()), Role::Accent);
        y += 1;
    } else if let Some((text, failed)) = &page.message {
        for line in wrap(text, width).into_iter().take(3) {
            g.put(y, left, &line, if *failed { Role::Bad } else { Role::Muted });
            y += 1;
        }
    }
    y += 1;

    let value_x = left + 13;
    let value_width = (left + width).saturating_sub(value_x);
    for field in page.fields() {
        let on = field == page.field;
        g.put(y, left, field.label(), if on { Role::Accent } else { Role::Muted });
        let editing = on && page.editing.is_some();
        match field {
            Field::Base => {
                let x = g.put(y, value_x, &page.branch, Role::Text) + 1;
                let x = g.put(y, x, "→", Role::Muted) + 1;
                let base = if editing { page.editing.as_deref().unwrap_or("") } else { &page.base };
                let end = g.put(y, x, &fit(base, (left + width).saturating_sub(x)), Role::Accent);
                if editing {
                    g.panels.push((y, x..end.max(x + 20).min(left + width)));
                }
            }
            Field::Draft => {
                let text = if page.draft { "[x] open as draft" } else { "[ ] ready for review" };
                g.put(y, value_x, text, if page.draft { Role::Warn } else { Role::Text });
            }
            Field::Description => {
                let from = match page.templates.get(page.template) {
                    Some(t) if page.templates.len() == 1 => format!("from the {} template · e edits it in a buffer", t.name),
                    _ => "e edits it in a buffer".to_string(),
                };
                g.put(y, value_x, &fit(&from, value_width), Role::Muted);
            }
            Field::Template => {
                let name = page.templates.get(page.template).map(|t| t.name.as_str()).unwrap_or("none");
                let x = g.put(y, value_x, &format!("‹ {name} ›"), Role::Text) + 2;
                g.put(y, x, &format!("{} of {} · h/l changes it", page.template + 1, page.templates.len()), Role::Muted);
            }
            _ => {
                let value = match field {
                    Field::Title => &page.title,
                    Field::Assignees => &page.assignees,
                    Field::Reviewers => &page.reviewers,
                    _ => &page.labels,
                };
                let shown = if editing { page.editing.as_deref().unwrap_or("") } else { value.as_str() };
                if editing {
                    g.put(y, value_x, &fit(shown, value_width), Role::Text);
                    g.panels.push((y, value_x..left + width));
                } else if shown.is_empty() {
                    let hint = match field {
                        Field::Title => "none yet",
                        Field::Assignees => "nobody -- usernames, with commas between",
                        Field::Reviewers => "none -- usernames, with commas between",
                        _ => "none",
                    };
                    g.put(y, value_x, hint, Role::Muted);
                } else {
                    g.put(y, value_x, &fit(shown, value_width), Role::Text);
                }
            }
        }
        if on {
            g.focus(y, left..left + width);
        }
        y += 1;
    }
    let lines: Vec<String> = page.description.lines().flat_map(|l| if l.is_empty() { vec![String::new()] } else { wrap(l, value_width) }).collect();
    if lines.is_empty() {
        g.put(y, value_x, "(empty)", Role::Muted);
        y += 1;
    }
    for line in lines.iter().take(10) {
        g.put(y, value_x, line, Role::Text);
        g.panels.push((y, value_x.saturating_sub(1)..left + width));
        y += 1;
    }
    if lines.len() > 10 {
        g.put(y, value_x, &format!("… {} more lines", lines.len() - 10), Role::Muted);
        y += 1;
    }
    y += 1;

    let (files, added, removed) = page.stat;
    g.heading(y, left, width, &format!("{} · +{added} −{removed} · {}", count(page.commits.len(), "commit"), count(files, "file")));
    y += 1;
    for c in page.commits.iter().take(12) {
        let x = g.put(y, left + 2, &c.short, Role::Accent) + 2;
        g.put(y, x, &fit(&c.subject, (left + width).saturating_sub(x)), if is_fixup(&c.subject) { Role::Warn } else { Role::Text });
        y += 1;
    }
    if page.commits.len() > 12 {
        g.put(y, left + 2, &format!("… {} more", page.commits.len() - 12), Role::Muted);
        y += 1;
    }
    y += 1;

    g.heading(y, left, width, "Before you open it");
    y += 1;
    let check = |g: &mut Grid, y: &mut usize, role: Role, text: String| {
        let mark = match role {
            Role::Good => "✓",
            Role::Bad => "✗",
            _ => "!",
        };
        let x = g.put(*y, left + 2, mark, role) + 1;
        g.put(*y, x, &fit(&text, (left + width).saturating_sub(x)), if role == Role::Good { Role::Muted } else { Role::Text });
        *y += 1;
    };
    match &page.existing {
        None => check(&mut g, &mut y, Role::Muted, format!("asking {} whether one is open already…", page.forge)),
        Some(Some(existing)) => check(&mut g, &mut y, Role::Bad, format!("{} is already open for this branch: {}", existing.reference(), existing.title)),
        Some(None) => {}
    }
    match &page.standing.pushed {
        Pushed::UpToDate(remote) => check(&mut g, &mut y, Role::Good, format!("pushed, up to date with {remote}")),
        Pushed::Ahead(n) => check(&mut g, &mut y, Role::Warn, format!("{} not pushed -- pushed first", count(*n, "commit"))),
        Pushed::NotYet => check(&mut g, &mut y, Role::Warn, "not pushed yet -- pushed first, setting the upstream".to_string()),
    }
    match page.standing.merges_cleanly {
        Some(true) => check(&mut g, &mut y, Role::Good, format!("no conflicts with {}", page.base)),
        Some(false) => check(&mut g, &mut y, Role::Bad, format!("conflicts with {} -- r on the Git page rebases onto it", page.base)),
        None => {}
    }
    if page.standing.base_moved > 0 {
        check(&mut g, &mut y, Role::Warn, format!("{} moved {} since you branched", page.base, count(page.standing.base_moved, "commit")));
    }
    match page.standing.fixups {
        0 => check(&mut g, &mut y, Role::Good, "no fixup! commits left".to_string()),
        n => check(&mut g, &mut y, Role::Warn, format!("{} not folded in -- r i on the Git page does it", count(n, "fixup! commit"))),
    }

    let open = format!("open the {}", page.word());
    let keys: Vec<(&str, &str)> = if page.editing.is_some() {
        vec![("Enter", "done"), ("Tab", "next field"), ("Esc", "put it back")]
    } else if page.armed {
        vec![("C-c", "again to open it"), ("any key", "back")]
    } else if matches!(page.existing, Some(Some(_))) {
        vec![("M", "review it"), ("o", "browser"), ("q", "close")]
    } else {
        vec![("C-c C-c", open.as_str()), ("Enter", "edit field"), ("e", "edit description"), ("j/k", "field"), ("d", "toggle draft"), ("q", "cancel")]
    };
    g.keys(left, width, &keys);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(short: &str, subject: &str, body: &str) -> Commit {
        Commit { short: short.into(), subject: subject.into(), body: body.into() }
    }

    fn standing() -> Standing {
        Standing { pushed: Pushed::UpToDate("origin".into()), merges_cleanly: Some(true), base_moved: 1, fixups: 0 }
    }

    fn page(commits: Vec<Commit>) -> RequestPage {
        RequestPage::new(PathBuf::from("/r"), "GitHub".into(), "tpedneault/fenix".into(), "feature/FNX-58-git-daily".into(), "master".into(), commits, (14, 612, 240), standing(), Some("FNX-58".into()))
    }

    #[test]
    fn a_key_is_found_in_the_branch_by_the_projects_key_or_in_capitals() {
        assert_eq!(jira_key("feature/FNX-58-status", None).as_deref(), Some("FNX-58"));
        assert_eq!(jira_key("feature/fnx-58-status", Some("FNX")).as_deref(), Some("FNX-58"));
        assert_eq!(jira_key("feature/fix-2-things", None), None);
        assert_eq!(jira_key("feature/ABC-12-x", Some("FNX")), None);
        assert_eq!(jira_key("FNX-58x", None), None);
        assert_eq!(jira_key("feature/git-daily", None), None);
    }

    #[test]
    fn one_commit_gives_the_title_and_description_and_more_give_a_summary() {
        let one = [commit("a1", "Git: the push plan", "Pushes set the upstream.\n")];
        assert_eq!(title_for("feature/x", &one, None), "Git: the push plan");
        assert_eq!(description_for(&one, Some("FNX-58"), None), "Pushes set the upstream.\n\nRefs FNX-58");
        let two = [commit("b2", "fixup! Git: a status page", ""), commit("b1", "Git: the push plan", ""), commit("a1", "Git: a status page", "")];
        assert_eq!(title_for("feature/FNX-58-git-daily", &two, Some("FNX-58")), "Git daily");
        assert_eq!(description_for(&two, None, None), "## Summary\n\n- Git: a status page\n- Git: the push plan");
        assert_eq!(title_for("feature/pus17", &[], None), "Pus17");
    }

    #[test]
    fn fields_are_typed_in_place_and_escape_puts_one_back() {
        let mut p = page(vec![commit("a1", "One", "")]);
        assert_eq!(p.field, Field::Title);
        p.key(Key::Char('j'));
        p.key(Key::Char('j'));
        p.key(Key::Char('j'));
        assert_eq!(p.field, Field::Reviewers);
        p.key(Key::Enter);
        for c in "alex, @sam".chars() {
            p.key(if c == ' ' { Key::Space } else { Key::Char(c) });
        }
        assert!(layout(&p, 100).text.contains("alex, @sam"));
        p.key(Key::Tab);
        assert_eq!((p.reviewers.as_str(), p.field), ("alex, @sam", Field::Labels));
        p.key(Key::Char('c'));
        p.paste("git");
        p.key(Key::Escape);
        assert_eq!(p.labels, "", "Esc put it back");
        p.key(Key::Char('k'));
        p.key(Key::Char('k'));
        p.key(Key::Char('k'));
        p.key(Key::Enter);
        assert_eq!(p.field, Field::Draft);
        assert!(p.draft && layout(&p, 100).text.contains("[x] open as draft"));
        assert_eq!(p.key(Key::Char('e')), RequestAction::EditDescription);
    }

    fn write(dir: &Path, path: &str, text: &str) {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn templates_are_found_where_each_forge_keeps_them() {
        let dir = tempfile::tempdir().unwrap();
        assert!(templates(dir.path(), "GitLab").is_empty());
        write(dir.path(), ".github/PULL_REQUEST_TEMPLATE.md", "## What\n");
        let found = templates(dir.path(), "GitHub");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "## What\n");
        assert_eq!(templates(dir.path(), "GitLab"), found, "the other forge's, when there's none of its own");

        write(dir.path(), ".gitlab/merge_request_templates/Bug.md", "## Bug\n");
        write(dir.path(), ".gitlab/merge_request_templates/Default.md", "## Change\n");
        write(dir.path(), ".gitlab/merge_request_templates/notes.txt", "not a template");
        let names: Vec<String> = templates(dir.path(), "GitLab").into_iter().map(|t| t.name).collect();
        assert_eq!(names, ["Default", "Bug"], "Default first, then by name");
    }

    #[test]
    fn the_description_is_the_repositorys_template_and_the_template_can_be_changed() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), ".gitlab/merge_request_templates/Default.md", "## What does this MR do?\n\n## How to test\n");
        write(dir.path(), ".gitlab/merge_request_templates/Bug.md", "## Bug\n");
        let mut p = RequestPage::new(dir.path().to_path_buf(), "GitLab".into(), "g/fenix".into(), "feature/FNX-58-x".into(), "master".into(), vec![commit("a1", "One", "Body.")], (1, 1, 0), standing(), Some("FNX-58".into()));
        assert_eq!(p.description, "## What does this MR do?\n\n## How to test\n\nRefs FNX-58");
        let text = layout(&p, 100).text;
        assert!(text.contains("‹ Default ›") && text.contains("1 of 2"), "{text}");

        while p.field != Field::Template {
            p.key(Key::Char('j'));
        }
        p.key(Key::Char('l'));
        assert!(p.description.starts_with("## Bug"), "untouched: follows the template");

        p.set_description("My own words".into());
        p.key(Key::Char('l'));
        assert_eq!(p.description, "My own words", "edited: asks first");
        p.key(Key::Char('l'));
        assert!(p.description.starts_with("## What does this MR do?"));
    }

    #[test]
    fn the_projects_default_template_on_the_forge_comes_first() {
        let mut p = page(vec![commit("a1", "One", "Body.")]);
        assert_eq!(p.description, "Body.\n\nRefs FNX-58");
        p.add_forge_template("## Why\n\n## How to test\n".into());
        assert_eq!(p.description, "## Why\n\n## How to test\n\nRefs FNX-58", "untouched: the template replaces it");
        assert!(layout(&p, 100).text.contains("from the project default template"));

        let mut edited = page(vec![commit("a1", "One", "Body.")]);
        edited.set_description("Mine".into());
        edited.add_forge_template("## Why\n".into());
        assert_eq!(edited.description, "Mine", "edited: kept");
        assert_eq!(edited.templates[0].name, "project default");
    }

    #[test]
    fn you_are_the_assignee_unless_someone_else_was_typed() {
        let mut p = page(vec![commit("a1", "One", "")]);
        p.assign_to_me("tpedneault");
        assert_eq!(p.assignees, "tpedneault");
        assert!(layout(&p, 100).text.contains("Assignee"));
        p.assignees = "alex".into();
        p.assign_to_me("tpedneault");
        assert_eq!(p.assignees, "alex");
        p.existing = Some(None);
        p.assignees = "alex, sam".into();
        p.key(Key::CtrlC);
        let RequestAction::Open { request, .. } = p.key(Key::CtrlC) else { panic!() };
        assert_eq!(request.assignees, ["alex", "sam"]);
    }

    #[test]
    fn c_c_twice_opens_it_with_the_reviewers_and_labels_split() {
        let mut p = page(vec![commit("a1", "One", "")]);
        p.reviewers = "alex @sam".into();
        p.labels = "git, ux".into();
        p.existing = Some(None);
        assert_eq!(p.key(Key::CtrlC), RequestAction::None);
        let RequestAction::Open { request, reviewers, push } = p.key(Key::CtrlC) else { panic!() };
        assert_eq!(reviewers, ["alex", "sam"]);
        assert_eq!(request.labels, ["git", "ux"]);
        assert_eq!((request.source_branch.as_str(), request.target_branch.as_str(), push), ("feature/FNX-58-git-daily", "master", false));
        p.standing.pushed = Pushed::NotYet;
        p.key(Key::CtrlC);
        assert!(matches!(p.key(Key::CtrlC), RequestAction::Open { push: true, .. }));
    }

    #[test]
    fn one_already_open_is_named_and_offered_instead() {
        let mut p = page(vec![commit("a1", "One", "")]);
        let existing = fenix_forge::MergeRequest {
            number: 22,
            title: "Git daily".into(),
            description: String::new(),
            state: fenix_forge::MrState::Open,
            draft: false,
            source_branch: p.branch.clone(),
            target_branch: "master".into(),
            author: "me".into(),
            web_url: "https://github.com/o/r/pull/22".into(),
            has_conflicts: false,
            sha: "h".into(),
            diff_refs: fenix_forge::DiffRefs::default(),
            comment_count: 0,
            pipeline: None,
            updated_at: String::new(),
        };
        p.existing = Some(Some(existing));
        assert_eq!(p.key(Key::CtrlC), RequestAction::None);
        assert!(p.message.as_ref().unwrap().0.contains("#22 is already open"));
        assert_eq!(p.key(Key::Char('M')), RequestAction::Review(22));
        let text = layout(&p, 100).text;
        assert!(text.contains("✗ #22 is already open for this branch"), "{text}");
    }

    #[test]
    fn the_page_shows_the_commits_and_what_to_know_before_opening_it() {
        let mut p = page(vec![commit("b2", "fixup! One", ""), commit("a1", "One", "")]);
        p.standing.fixups = 1;
        p.existing = Some(None);
        let text = layout(&p, 100).text;
        for want in [
            "New pull request · tpedneault/fenix",
            "feature/FNX-58-git-daily → master",
            "2 COMMITS · +612 −240 · 14 FILES",
            "✓ pushed, up to date with origin",
            "✓ no conflicts with master",
            "! master moved 1 commit since you branched",
            "! 1 fixup! commit not folded in",
            "Refs FNX-58",
        ] {
            assert!(text.contains(want), "{want}:\n{text}");
        }
    }
}
