//! `SPC p h`, the project doctor's page: `fenix_project::doctor`'s checks
//! for one project, grouped by section, each with its fix. `f` runs the
//! fix under the cursor (a command's output shows beneath its row), `F`
//! runs every fix that's safe to run unasked, `r` checks again, `Enter`
//! on a check about a file opens it there.

use std::path::PathBuf;

use crate::page::{fit, frame, Grid, Key, Page, Role};
use fenix_project::doctor::{worst, Check, Health, Section};
use fenix_project::ProjectKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    Recheck,
    /// Run the fix of check `n`.
    Fix(usize),
    /// Run every safe fix, in order.
    FixAllSafe,
    Open(PathBuf, usize),
    CopyReport,
}

/// A fix being run, and what it's said so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fixing {
    pub check: usize,
    pub output: Vec<String>,
    /// `None` while running, then how it ended.
    pub result: Option<Result<(), String>>,
}

pub struct DoctorPage {
    pub root: PathBuf,
    pub name: String,
    pub kind: ProjectKind,
    /// `None` while a check is running.
    pub checks: Option<Vec<Check>>,
    pub focus: usize,
    pub fixing: Option<Fixing>,
    /// Safe fixes still to run after the current one (`F`).
    pub queue: Vec<usize>,
}

const OUTPUT_LINES: usize = 5;

impl DoctorPage {
    pub fn new(root: PathBuf, kind: ProjectKind) -> Self {
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
        DoctorPage { root, name, kind, checks: None, focus: 0, fixing: None, queue: Vec::new() }
    }

    /// Checks are in; keeps the cursor on the same row where it can.
    pub fn set_checks(&mut self, checks: Vec<Check>) {
        self.focus = self.focus.min(checks.len().saturating_sub(1));
        self.checks = Some(checks);
    }

    pub fn busy(&self) -> bool {
        self.checks.is_none() || self.fixing.as_ref().is_some_and(|f| f.result.is_none())
    }

    pub fn push_output(&mut self, line: String) {
        if let Some(fixing) = &mut self.fixing {
            fixing.output.push(line);
            if fixing.output.len() > OUTPUT_LINES {
                fixing.output.remove(0);
            }
        }
    }

    /// Indices of the checks with a safe fix.
    pub fn safe_fixes(&self) -> Vec<usize> {
        self.checks.iter().flatten().enumerate().filter(|(_, c)| c.fix.as_ref().is_some_and(|f| f.safe)).map(|(i, _)| i).collect()
    }

    /// Checks in display order: grouped by section, sections in a fixed
    /// order, checks as the doctor listed them within each.
    pub fn order(&self) -> Vec<usize> {
        let rank = |s: Section| [Section::Toolchain, Section::Hardware, Section::Database, Section::Project].iter().position(|x| *x == s).unwrap_or(9);
        let mut order: Vec<usize> = (0..self.checks.as_ref().map_or(0, Vec::len)).collect();
        if let Some(checks) = &self.checks {
            order.sort_by_key(|&i| rank(checks[i].section));
        }
        order
    }

    fn focused(&self) -> Option<usize> {
        self.order().get(self.focus).copied()
    }

    pub fn key(&mut self, key: Key) -> Action {
        let count = self.order().len();
        match key {
            Key::Char('q') | Key::Escape => Action::Close,
            Key::Char('j') | Key::Down | Key::Tab => {
                self.focus = (self.focus + 1).min(count.saturating_sub(1));
                Action::None
            }
            Key::Char('k') | Key::Up | Key::BackTab => {
                self.focus = self.focus.saturating_sub(1);
                Action::None
            }
            Key::Char('g') => {
                self.focus = 0;
                Action::None
            }
            Key::Char('G') => {
                self.focus = count.saturating_sub(1);
                Action::None
            }
            _ if self.busy() => Action::None,
            Key::Char('r') => Action::Recheck,
            Key::Char('y') => Action::CopyReport,
            Key::Char('F') if !self.safe_fixes().is_empty() => Action::FixAllSafe,
            Key::Char('f') => match self.focused() {
                Some(i) if self.check(i).and_then(|c| c.fix.as_ref()).is_some() => Action::Fix(i),
                _ => Action::None,
            },
            Key::Enter => match self.focused().and_then(|i| self.check(i).map(|c| (i, c))) {
                Some((_, check)) if check.location.is_some() => {
                    let (path, line) = check.location.clone().unwrap();
                    Action::Open(path, line)
                }
                Some((i, check)) if check.fix.is_some() => Action::Fix(i),
                _ => Action::None,
            },
            _ => Action::None,
        }
    }

    pub fn check(&self, i: usize) -> Option<&Check> {
        self.checks.as_ref()?.get(i)
    }

    /// The checks as plain text, for the clipboard.
    pub fn report(&self) -> String {
        let mut out = format!("{} ({}) -- {}\n", self.name, self.kind.label(), self.root.display());
        for i in self.order() {
            let Some(c) = self.check(i) else { continue };
            let mark = match c.health {
                Health::Ok => "ok  ",
                Health::Info => "info",
                Health::Warn => "warn",
                Health::Bad => "BAD ",
            };
            out.push_str(&format!("{mark} {:<9} {:<24} {}", c.section.title(), c.label, c.detail));
            if let Some(fix) = &c.fix {
                out.push_str(&format!("   [fix: {}]", fix.label));
            }
            out.push('\n');
        }
        out
    }
}

fn health_role(health: Health) -> Role {
    match health {
        Health::Ok => Role::Good,
        Health::Info => Role::Muted,
        Health::Warn => Role::Warn,
        Health::Bad => Role::Bad,
    }
}

pub fn layout(doctor: &DoctorPage, cols: usize) -> Page {
    let (left, width) = frame(cols, 100);
    let mut g = Grid::new();
    let end = g.put(1, left, doctor.kind.tag(), Role::Kind(doctor.kind));
    let end = g.put(1, end + 2, &doctor.name, Role::Title);
    g.put(1, end + 2, "· doctor", Role::Muted);
    let status = match &doctor.checks {
        None => "checking…".to_string(),
        Some(checks) => {
            let problems = checks.iter().filter(|c| c.health >= Health::Warn).count();
            match problems {
                0 => "all good".to_string(),
                n => format!("{n} problem{}", if n == 1 { "" } else { "s" }),
            }
        }
    };
    let role = match &doctor.checks {
        Some(checks) => health_role(worst(checks).max(Health::Ok)),
        None => Role::Muted,
    };
    let role = if role == Role::Muted && doctor.checks.is_some() { Role::Good } else { role };
    g.put(1, (left + width).saturating_sub(status.chars().count()), &status, role);
    g.rule(2, left..left + width);

    let mut y = 4;
    if let Some(checks) = &doctor.checks {
        let label_width = 26.min(width / 3);
        let mut section = None;
        for (row, i) in doctor.order().into_iter().enumerate() {
            let c = &checks[i];
            if section != Some(c.section) {
                if section.is_some() {
                    y += 1;
                }
                g.heading(y, left, width, c.section.title());
                section = Some(c.section);
                y += 1;
            }
            if row == doctor.focus {
                g.focus(y, left..left + width);
            }
            g.put(y, left + 2, "●", health_role(c.health));
            g.put(y, left + 4, &fit(&c.label, label_width), Role::Text);
            let fix_text = c.fix.as_ref().map(|f| format!("f  {}", f.label)).unwrap_or_default();
            let detail_x = left + 5 + label_width;
            let room = (left + width).saturating_sub(detail_x + fix_text.chars().count() + 2);
            g.put(y, detail_x, &fit(&c.detail, room), Role::Muted);
            if !fix_text.is_empty() && fix_text.chars().count() + detail_x < left + width {
                let x = left + width - fix_text.chars().count();
                g.put(y, x, "f", Role::Accent);
                g.put(y, x + 3, &c.fix.as_ref().unwrap().label, Role::Text);
            }
            y += 1;
            if let Some(fixing) = doctor.fixing.as_ref().filter(|f| f.check == i) {
                let (mark, role) = match &fixing.result {
                    None => ("running", Role::Accent),
                    Some(Ok(())) => ("done -- checking again", Role::Good),
                    Some(Err(_)) => ("failed", Role::Bad),
                };
                g.put(y, left + 6, mark, role);
                if let Some(Err(e)) = &fixing.result {
                    g.put(y, left + 6 + mark.len() + 2, &fit(e, width.saturating_sub(mark.len() + 10)), Role::Bad);
                }
                y += 1;
                for line in &fixing.output {
                    g.put(y, left + 6, &fit(line, width.saturating_sub(8)), Role::Muted);
                    y += 1;
                }
            }
        }
        let safe = doctor.safe_fixes().len();
        if safe > 0 {
            y += 1;
            let text = format!("{safe} fix{} safe to run unasked -- ", if safe == 1 { " is" } else { "es are" });
            let end = g.put(y, left + 2, &fit(&text, width.saturating_sub(16)), Role::Muted);
            let end = g.put(y, end, "F", Role::Accent);
            g.put(y, end + 1, "runs them", Role::Muted);
        }
    } else {
        g.put(y, left + 2, "Looking at the toolchain…", Role::Muted);
    }
    g.keys(left, width, &[("f", "fix"), ("F", "fix all safe"), ("Enter", "open / fix"), ("r", "recheck"), ("y", "copy report"), ("q", "close")]);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_project::doctor::{diagnose, Probe};
    use std::path::Path;

    struct NoTools;
    impl Probe for NoTools {
        fn locate(&self, _: &str) -> Option<PathBuf> {
            None
        }
        fn run(&self, _: &Path, _: &[&str], _: &Path) -> Option<(bool, String)> {
            None
        }
        fn mib_registered(&self, _: &Path) -> bool {
            false
        }
    }

    fn page(name: &str) -> (DoctorPage, PathBuf) {
        let dir = std::env::temp_dir().join(format!("fenix-doctor-page-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".fenix")).unwrap();
        std::fs::write(dir.join("uv.lock"), "").unwrap();
        std::fs::write(dir.join(".fenix/tools.json"), "{\"typo\":1}").unwrap();
        let mut page = DoctorPage::new(dir.clone(), ProjectKind::Python);
        page.set_checks(diagnose(&dir, ProjectKind::Python, &NoTools, false));
        (page, dir)
    }

    #[test]
    fn checks_are_grouped_by_section_with_their_fixes() {
        let (doctor, dir) = page("grouped");
        let text = layout(&doctor, 100).text;
        assert!(text.contains("TOOLCHAIN") && text.contains("PROJECT"), "{text}");
        assert!(text.contains("uv sync"), "the venv's fix is offered:\n{text}");
        assert!(text.contains("problems"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn f_fixes_enter_opens_and_f_runs_only_safe_fixes() {
        let (mut doctor, dir) = page("keys");
        let order = doctor.order();
        let venv = order.iter().position(|&i| doctor.check(i).unwrap().label == ".venv").unwrap();
        doctor.focus = venv;
        assert_eq!(doctor.key(Key::Char('f')), Action::Fix(order[venv]));
        let tools = order.iter().position(|&i| doctor.check(i).unwrap().label == "tools.json").unwrap();
        doctor.focus = tools;
        assert!(matches!(doctor.key(Key::Enter), Action::Open(p, 1) if p.ends_with("tools.json")));
        let safe = doctor.safe_fixes();
        assert!(safe.iter().all(|&i| doctor.check(i).unwrap().fix.as_ref().unwrap().safe));
        assert!(!safe.iter().any(|&i| doctor.check(i).unwrap().label == "pyright-langserver"), "installing isn't safe");
        assert_eq!(doctor.key(Key::Char('F')), Action::FixAllSafe);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_running_fix_shows_its_output_and_blocks_other_fixes() {
        let (mut doctor, dir) = page("running");
        let i = doctor.safe_fixes()[0];
        doctor.fixing = Some(Fixing { check: i, output: Vec::new(), result: None });
        for n in 0..8 {
            doctor.push_output(format!("line {n}"));
        }
        assert_eq!(doctor.fixing.as_ref().unwrap().output.len(), OUTPUT_LINES);
        assert_eq!(doctor.key(Key::Char('F')), Action::None, "busy");
        let text = layout(&doctor, 100).text;
        assert!(text.contains("running") && text.contains("line 7") && !text.contains("line 2"));
        assert!(doctor.report().contains("BAD "));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn while_checking_it_says_so() {
        let doctor = DoctorPage::new(PathBuf::from("/x/demo"), ProjectKind::Rust);
        let page = layout(&doctor, 60);
        assert!(page.text.contains("checking…"));
        assert!(page.text.lines().all(|l| l.chars().count() <= 60));
    }
}
