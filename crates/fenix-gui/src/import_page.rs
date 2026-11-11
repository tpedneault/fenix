//! Bring your settings over (`settings.import`, `SPC h i`, and the
//! welcome's first step): the VS Code, Vim and Neovim configs found on
//! this machine, then what one would bring -- the settings and keys to
//! write, yours that differ (kept), and what Fenix has nothing for --
//! and nothing is written until `y`. `App` (`app/import_host.rs`) reads
//! and writes; `fenix_config::import` does the mapping.

use fenix_config::import::{Applied, Fate, Plan, Source};

use crate::page::{fit, frame, home_relative, wrap, Grid, Key, Page, Role};

pub enum Stage {
    /// Which config to read.
    Sources,
    /// What it would bring.
    Preview(Box<Plan>),
    /// What was written.
    Done(Applied),
}

pub struct ImportPage {
    pub sources: Vec<Source>,
    pub stage: Stage,
    /// The focused row: a source, or a line of the preview.
    pub focus: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    /// Read the source at this index.
    Read(usize),
    /// Write the preview: with `replace`, yours that differ too.
    Write { replace: bool },
}

impl ImportPage {
    pub fn new(sources: Vec<Source>) -> Self {
        ImportPage { sources, stage: Stage::Sources, focus: 0 }
    }

    /// How many rows the focus moves over.
    fn rows(&self) -> usize {
        match &self.stage {
            Stage::Sources => self.sources.len(),
            Stage::Preview(plan) => plan.settings.len() + plan.keys.len() + plan.unmapped.len(),
            Stage::Done(_) => 0,
        }
    }

    pub fn key(&mut self, key: Key) -> Action {
        let last = self.rows().saturating_sub(1);
        match (&self.stage, key) {
            (_, Key::Char('q') | Key::Escape) if !matches!(self.stage, Stage::Preview(_)) => return Action::Close,
            (Stage::Done(_), Key::Enter) => return Action::Close,
            (Stage::Preview(_), Key::Char('q') | Key::Escape | Key::Char('b') | Key::Backspace) => {
                self.stage = Stage::Sources;
                self.focus = 0;
                // Straight from a single source, back is out.
                if self.sources.len() <= 1 && key != Key::Char('b') && key != Key::Backspace {
                    return Action::Close;
                }
            }
            (_, Key::Char('j') | Key::Down) => self.focus = (self.focus + 1).min(last),
            (_, Key::Char('k') | Key::Up) => self.focus = self.focus.saturating_sub(1),
            (_, Key::Char('g')) => self.focus = 0,
            (_, Key::Char('G')) => self.focus = last,
            (Stage::Sources, Key::Enter) if !self.sources.is_empty() => return Action::Read(self.focus),
            (Stage::Sources, Key::Char(c @ '1'..='9')) => {
                let i = c as usize - '1' as usize;
                if i < self.sources.len() {
                    return Action::Read(i);
                }
            }
            (Stage::Preview(plan), Key::Char('y')) if plan.writes(false) != (0, 0) => return Action::Write { replace: false },
            (Stage::Preview(plan), Key::Char('R')) if plan.conflicts() => return Action::Write { replace: true },
            _ => {}
        }
        Action::None
    }

    /// The preview of what `plan` would bring, on the page.
    pub fn preview(&mut self, plan: Plan) {
        self.stage = Stage::Preview(Box::new(plan));
        self.focus = 0;
    }

    pub fn done(&mut self, applied: Applied) {
        self.stage = Stage::Done(applied);
        self.focus = 0;
    }
}

/// `value` as `settings.toml` reads it.
fn shown(value: &fenix_config::Value) -> String {
    match value {
        fenix_config::Value::Text(t) => format!("\"{t}\""),
        fenix_config::Value::Bool(b) => b.to_string(),
        fenix_config::Value::List(items) => format!("[{}]", items.iter().map(|i| format!("\"{i}\"")).collect::<Vec<_>>().join(", ")),
        v => v.show(),
    }
}

pub fn layout(page: &ImportPage, cols: usize) -> Page {
    let (left, width) = frame(cols, 100);
    let mut g = Grid::new();
    g.put(1, left, &fit("Bring your settings over", width), Role::Title);
    g.rule(2, left..left + width);
    let mut line = 4;
    let para = |g: &mut Grid, line: &mut usize, text: &str, role: Role| {
        for l in wrap(text, width) {
            g.put(*line, left, &l, role);
            *line += 1;
        }
    };
    match &page.stage {
        Stage::Sources => {
            para(&mut g, &mut line, "Fenix reads another editor's settings and keys and shows what it would write, here, first. Nothing is written until you say so, and a setting you've already set stays yours.", Role::Text);
            line += 1;
            if page.sources.is_empty() {
                para(&mut g, &mut line, "No VS Code (or VSCodium, Cursor, Windsurf), Neovim or Vim settings were found where they're kept on this machine.", Role::Muted);
            }
            for (i, source) in page.sources.iter().enumerate() {
                let start = line;
                let x = g.chip(line, left + 1, &format!(" {} ", i + 1), Role::Accent, crate::page::Chip::Key);
                let extra = if source.vim_extension { "  with VSCodeVim" } else { "" };
                g.put(line, x + 1, &fit(&format!("{}{extra}", source.name), width.saturating_sub(x + 1 - left)), Role::Title);
                line += 1;
                for file in &source.files {
                    g.put(line, x + 1, &crate::page::fit_path(&home_relative(file), width.saturating_sub(x + 1 - left)), Role::Muted);
                    line += 1;
                }
                if i == page.focus {
                    g.focus(start, left..left + width);
                }
                line += 1;
            }
            para(&mut g, &mut line, "Vim and Neovim configs are read, never run: plain options (set number, vim.opt.tabstop = 4), the colour scheme and maps of one key to one command come over, and the rest is listed.", Role::Muted);
            g.keys(left, width, &[("1-9", "read"), ("↵", "read"), ("q", "close")]);
        }
        Stage::Preview(plan) => {
            let (n_settings, n_keys) = plan.writes(false);
            let same = plan.settings.iter().filter(|(_, f)| *f == Fate::Same).count() + plan.keys.iter().filter(|(_, f)| *f == Fate::Same).count();
            let mut summary = format!("From {}: {n_settings} settings and {n_keys} keys to write", plan.source);
            if same > 0 {
                summary.push_str(&format!(", {same} already as you have them"));
            }
            summary.push_str(&format!(", {} not brought over.", plan.unmapped.len()));
            para(&mut g, &mut line, &summary, Role::Text);
            line += 1;
            let mut row = 0;
            let section = |g: &mut Grid, line: &mut usize, title: &str| {
                g.put(*line, left, &fit(title, width), Role::Accent);
                *line += 1;
            };
            let mut item = |g: &mut Grid, line: &mut usize, head: &str, role: Role, sub: Option<String>| {
                let start = *line;
                g.put(*line, left + 2, &fit(head, width.saturating_sub(2)), role);
                *line += 1;
                if let Some(sub) = sub {
                    for l in wrap(&sub, width.saturating_sub(4)) {
                        g.put(*line, left + 4, &l, Role::Muted);
                        *line += 1;
                    }
                }
                if row == page.focus {
                    g.focus(start, left..left + width);
                }
                row += 1;
            };
            if !plan.settings.is_empty() {
                section(&mut g, &mut line, "Settings (settings.toml)");
                for (change, fate) in &plan.settings {
                    let (mark, role) = match fate {
                        Fate::New => ("+", Role::Good),
                        Fate::Same => ("=", Role::Muted),
                        Fate::Yours(_) => ("~", Role::Warn),
                    };
                    let mut sub = format!("from {}", change.from);
                    if let Some(note) = &change.note {
                        sub.push_str(&format!(" -- {note}"));
                    }
                    match fate {
                        Fate::Same => sub.push_str(" -- you have it so already"),
                        Fate::Yours(v) => sub.push_str(&format!(" -- yours is {v}, and stays unless you replace it")),
                        Fate::New => {}
                    }
                    item(&mut g, &mut line, &format!("{mark} {} = {}", change.key, shown(&change.value)), role, Some(sub));
                }
                line += 1;
            }
            if !plan.keys.is_empty() {
                section(&mut g, &mut line, "Keys (keybindings.toml)");
                for (change, fate) in &plan.keys {
                    let (mark, role) = match fate {
                        Fate::New => ("+", Role::Good),
                        Fate::Same => ("=", Role::Muted),
                        Fate::Yours(_) => ("~", Role::Warn),
                    };
                    let mut sub = format!("{} keys{}, from {}", change.scheme, change.when.map(|w| format!(", {w}")).unwrap_or_default(), change.from);
                    match fate {
                        Fate::Same => sub.push_str(" -- Fenix has it so already"),
                        Fate::Yours(v) => sub.push_str(&format!(" -- yours runs {v}, and stays unless you replace it")),
                        Fate::New => {}
                    }
                    item(&mut g, &mut line, &format!("{mark} {} → {}", change.keys, change.command), role, Some(sub));
                }
                line += 1;
            }
            if !plan.unmapped.is_empty() {
                section(&mut g, &mut line, "Not brought over");
                for u in &plan.unmapped {
                    item(&mut g, &mut line, &format!("· {}", u.from), Role::Text, Some(u.why.clone()));
                }
            }
            let mut keys: Vec<(&str, &str)> = Vec::new();
            if n_settings + n_keys > 0 {
                keys.push(("y", "write the + ones"));
            }
            if plan.conflicts() {
                keys.push(("R", "write, replacing yours (~)"));
            }
            keys.extend([("j k", "move"), ("b", "back"), ("q", "cancel")]);
            g.keys(left, width, &keys);
        }
        Stage::Done(applied) => {
            let mut text = format!("Wrote {} settings to settings.toml and {} keys to keybindings.toml.", applied.settings, applied.keys);
            if applied.settings + applied.keys == 0 {
                text = "Nothing was written.".to_string();
            }
            para(&mut g, &mut line, &text, Role::Good);
            for failure in &applied.failed {
                para(&mut g, &mut line, failure, Role::Bad);
            }
            line += 1;
            para(&mut g, &mut line, "They're in use now. SPC , has every setting, SPC h e opens your keybindings, and SPC h i brings this back.", Role::Muted);
            g.keys(left, width, &[("↵", "done")]);
        }
    }
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_config::import::{Catalog, Editor};

    fn sources() -> Vec<Source> {
        vec![
            Source { name: "VS Code".into(), editor: Editor::VsCode, files: vec!["/home/me/.config/Code/User/settings.json".into(), "/home/me/.config/Code/User/keybindings.json".into()], vim_extension: false },
            Source { name: "Neovim".into(), editor: Editor::Vim, files: vec!["/home/me/.config/nvim/init.lua".into()], vim_extension: false },
        ]
    }

    fn plan() -> Plan {
        let catalog = Catalog { themes: vec![("One Dark".into(), false)] };
        let proposal = fenix_config::import::read_vscode(
            Some(r#"{ "editor.tabSize": 2, "workbench.colorTheme": "One Dark Pro", "editor.minimap.enabled": false }"#),
            Some(r#"[{ "key": "ctrl+shift+d", "command": "editor.action.copyLinesDownAction" }]"#),
            false,
            &catalog,
        );
        let config = fenix_config::Config::empty(std::env::temp_dir().join("fenix-import-page.toml"), std::env::temp_dir());
        fenix_config::import::plan(proposal, &config, None)
    }

    #[test]
    fn a_source_is_picked_by_its_number_or_enter() {
        let mut p = ImportPage::new(sources());
        assert_eq!(p.key(Key::Char('2')), Action::Read(1));
        assert_eq!(p.key(Key::Char('7')), Action::None, "no seventh");
        p.key(Key::Down);
        assert_eq!(p.key(Key::Enter), Action::Read(1));
        assert_eq!(p.key(Key::Char('q')), Action::Close);
    }

    #[test]
    fn the_preview_writes_only_on_y_and_back_goes_to_the_sources() {
        let mut p = ImportPage::new(sources());
        p.preview(plan());
        let laid = layout(&p, 100);
        assert!(laid.text.contains("+ editor.tab_width = 2"), "{}", laid.text);
        assert!(laid.text.contains("+ C-S-d → edit.copy_lines_down"), "{}", laid.text);
        assert!(laid.text.contains("Not brought over"), "{}", laid.text);
        assert!(laid.text.contains("editor.minimap.enabled"), "{}", laid.text);
        assert_eq!(p.key(Key::Char('R')), Action::None, "nothing of yours to replace");
        assert_eq!(p.key(Key::Enter), Action::None, "Enter doesn't write");
        assert_eq!(p.key(Key::Char('y')), Action::Write { replace: false });
        assert_eq!(p.key(Key::Char('b')), Action::None);
        assert!(matches!(p.stage, Stage::Sources));
    }

    #[test]
    fn it_keeps_inside_a_narrow_pane() {
        let mut p = ImportPage::new(sources());
        for cols in [36, 40, 60, 80, 120] {
            let over = crate::page::overflow(&layout(&p, cols), cols);
            assert!(over.is_empty(), "sources at {cols}: {over:#?}");
        }
        p.preview(plan());
        for cols in [36, 40, 60, 80, 120] {
            let over = crate::page::overflow(&layout(&p, cols), cols);
            assert!(over.is_empty(), "preview at {cols}: {over:#?}");
        }
        p.done(Applied { settings: 3, keys: 1, failed: vec!["editor.theme: no".into()] });
        let over = crate::page::overflow(&layout(&p, 40), 40);
        assert!(over.is_empty(), "{over:#?}");
        let mut empty = ImportPage::new(Vec::new());
        assert_eq!(empty.key(Key::Enter), Action::None);
        assert!(layout(&empty, 60).text.contains("were found"));
    }
}
