//! The import page's host (`settings.import`): finds the other editors'
//! configs, reads the one picked into a plan -- its chords checked
//! against the keymap, those Fenix already has so marked -- and, on
//! `y`, writes it and puts it in use. The page is `import_page.rs`, the
//! mapping `fenix_config::import`.

use fenix_config::import::{self, Catalog, KeyChange, Plan, Source};

use super::pages::PageModel;
use super::{App, BufferId};
use crate::import_page::{Action, ImportPage};
use crate::keymap::{self, ChordContext, Scheme};

impl App {
    /// `settings.import` (`SPC h i`): the configs found, to pick from.
    pub(crate) fn open_import_page(&mut self) {
        let sources = if cfg!(test) { Vec::new() } else { import::find_sources(&import::Places::current()) };
        self.open_import_page_with(sources);
    }

    pub(crate) fn open_import_page_with(&mut self, sources: Vec<Source>) {
        let one = (sources.len() == 1).then(|| sources[0].clone());
        let id = match self.find_page(|m| matches!(m, PageModel::Import(_))) {
            Some(id) => {
                if let Some(state) = self.pages.get_mut(&id) {
                    state.model = PageModel::Import(Box::new(ImportPage::new(sources)));
                    state.stale = true;
                }
                self.show_page(id);
                id
            }
            None => self.open_page(PageModel::Import(Box::new(ImportPage::new(sources)))),
        };
        // One found: straight to what it would bring.
        if let Some(source) = one {
            let plan = self.import_plan(&source);
            self.with_import_page(id, |p| p.preview(plan));
        }
    }

    fn with_import_page(&mut self, id: BufferId, change: impl FnOnce(&mut ImportPage)) {
        if let Some(state) = self.pages.get_mut(&id) {
            if let PageModel::Import(page) = &mut state.model {
                change(page);
            }
            state.stale = true;
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// The themes there are, for matching a theme by name.
    fn import_catalog(&self) -> Catalog {
        Catalog { themes: crate::theme::all().iter().map(|t| (t.name.to_string(), theme_is_light(t))).collect() }
    }

    /// What `source` would bring, beside what you have.
    pub(crate) fn import_plan(&self, source: &Source) -> Plan {
        let mut proposal = import::read(source, &self.import_catalog());
        self.check_import_keys(&mut proposal);
        self.plan_import(proposal)
    }

    /// Keeps the chords whose keys parse and whose commands Fenix has.
    pub(crate) fn check_import_keys(&self, proposal: &mut import::Proposal) {
        let registry = self.commands();
        proposal.retain_keys(|k| {
            if keymap::parse_key(&k.keys).is_none() {
                return Err(format!("{} isn't a key Fenix knows", k.keys));
            }
            if registry.get(&k.command).is_none() {
                return Err(format!("{} isn't one of this Fenix's commands", k.command));
            }
            if keymap::RESERVED_CHORDS.iter().any(|r| keymap::parse_key(r) == keymap::parse_key(&k.keys)) && k.scheme == "standard" {
                return Err(format!("{} is Fenix's own (the leader, the palette, or the way out of a terminal)", k.keys));
            }
            Ok(())
        });
    }

    /// `proposal` beside your settings and keybindings, the chords Fenix
    /// already binds so marked.
    pub(crate) fn plan_import(&self, proposal: import::Proposal) -> Plan {
        let yours = keymap::user_keymap_path().and_then(|p| std::fs::read_to_string(p).ok());
        let mut plan = import::plan(proposal, &self.config, yours.as_deref());
        plan.mark_same_keys(fenix_binds_so);
        plan
    }

    pub(super) fn import_action(&mut self, id: BufferId, action: Action) {
        match action {
            Action::None => {}
            Action::Close => self.close_page(id),
            Action::Read(i) => {
                let Some(source) = self.import_page(id).and_then(|p| p.sources.get(i).cloned()) else { return };
                let plan = self.import_plan(&source);
                self.with_import_page(id, |p| p.preview(plan));
            }
            Action::Write { replace } => {
                let Some(plan) = self.import_page(id).and_then(|p| match &p.stage {
                    crate::import_page::Stage::Preview(plan) => Some((**plan).clone()),
                    _ => None,
                }) else {
                    return;
                };
                let applied = self.write_import(&plan, replace);
                self.with_import_page(id, |p| p.done(applied));
            }
        }
    }

    /// Writes `plan` and puts it in use: each setting applied, the keys
    /// read again.
    pub(crate) fn write_import(&mut self, plan: &Plan, replace: bool) -> import::Applied {
        let path = keymap::user_keymap_path().unwrap_or_else(|| self.config.path().with_file_name("keybindings.toml"));
        let applied = import::apply(plan, replace, &mut self.config, &path);
        for (change, _) in &plan.settings {
            self.apply_setting(change.key);
        }
        if applied.keys > 0 {
            self.reload_keybindings_if(&path);
        }
        self.refresh_settings_pages();
        // The welcome, if it's open (it offers this), shows what's in
        // use now, and every page says the keys picked.
        let (keys, theme) = (self.key_scheme().name(), self.theme.name.to_string());
        self.with_welcome_page(|p| {
            p.keys = keys;
            p.theme = theme;
        });
        for state in self.pages.values_mut() {
            state.stale = true;
        }
        applied
    }

    fn import_page(&self, id: BufferId) -> Option<&ImportPage> {
        match &self.pages.get(&id)?.model {
            PageModel::Import(p) => Some(p),
            _ => None,
        }
    }
}

/// Whether Fenix's keymap binds `k`'s keys to its command already, where
/// it would apply.
fn fenix_binds_so(k: &KeyChange) -> bool {
    let Some(key) = keymap::parse_key(&k.keys) else { return false };
    let scheme = if k.scheme == "vim" { Scheme::Vim } else { Scheme::Standard };
    let text = k.when != Some("terminal");
    let at = ChordContext { scheme, text, typing: k.when == Some("typing"), terminal: k.when == Some("terminal"), page: false };
    keymap::chord_map().lookup(key, &at) == Some(k.command.as_str())
}

/// A theme with a light background.
fn theme_is_light(theme: &crate::theme::Theme) -> bool {
    let [r, g, b, _] = theme.bg;
    0.2126 * r + 0.7152 * g + 0.0722 * b > 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vscode(settings: &str, keybindings: &str) -> import::Proposal {
        let app = App::with_file(None);
        import::read_vscode(Some(settings), Some(keybindings), false, &app.import_catalog())
    }

    #[test]
    fn every_command_the_importer_names_is_one_fenix_has() {
        let registry = crate::commands::CommandRegistry::with_builtins();
        let missing: Vec<&str> = import::VSCODE_COMMANDS.iter().chain(import::VIM_COMMANDS).chain(import::VIM_KEY_COMMANDS).map(|(_, c)| *c).filter(|c| registry.get(c).is_none()).collect();
        assert!(missing.is_empty(), "{missing:?}");
    }

    #[test]
    fn the_themes_vs_code_ships_with_are_fenix_s_and_light_is_told_from_dark() {
        let app = App::with_file(None);
        let catalog = app.import_catalog();
        assert!(catalog.themes.iter().any(|(n, l)| n == "Visual Studio Light" && *l));
        assert!(catalog.themes.iter().any(|(n, l)| n == "Orbit Dark" && !*l));
        let p = vscode(r#"{ "workbench.colorTheme": "Default Light Modern" }"#, "[]");
        assert_eq!(p.settings.iter().find(|s| s.key == "editor.theme").map(|s| s.value.clone()), Some(fenix_config::Value::Text("Visual Studio Light".into())));
    }

    #[test]
    fn a_chord_fenix_has_already_is_marked_and_one_it_cant_bind_is_said() {
        let app = App::with_file(None);
        let mut p = vscode("{}", r#"[
            { "key": "ctrl+p", "command": "workbench.action.quickOpen" },
            { "key": "ctrl+shift+p", "command": "workbench.action.quickOpen" },
            { "key": "ctrl+alt+d", "command": "editor.action.copyLinesDownAction" }
        ]"#);
        app.check_import_keys(&mut p);
        assert!(p.unmapped.iter().any(|u| u.from.starts_with("ctrl+shift+p") && u.why.contains("Fenix's own")), "{:?}", p.unmapped);
        let plan = app.plan_import(p);
        let fate = |keys: &str| plan.keys.iter().find(|(k, _)| k.keys == keys).map(|(_, f)| f.clone());
        assert_eq!(fate("C-p"), Some(import::Fate::Same), "Fenix's own Ctrl+P");
        assert_eq!(fate("C-M-d"), Some(import::Fate::New));
    }

    #[test]
    fn writing_puts_the_settings_in_use_and_the_page_says_what_was_written() {
        let mut app = App::with_file(None);
        let p = vscode(r#"{ "editor.lineNumbers": "relative", "editor.tabSize": 3 }"#, "[]");
        let plan = app.plan_import(p);
        let applied = app.write_import(&plan, false);
        assert!(applied.failed.is_empty(), "{:?}", applied.failed);
        assert_eq!(app.line_number_mode, super::super::LineNumberMode::Relative, "in use at once");
        assert_eq!(app.config.indent_width, Some(3));
        assert!(app.standard_keys(), "VS Code's keys");
    }

    #[test]
    fn importing_from_the_welcome_shows_there_what_s_in_use() {
        let mut app = App::with_file(None);
        app.open_welcome_page();
        let plan = app.plan_import(vscode(r#"{ "workbench.colorTheme": "Default Light Modern" }"#, "[]"));
        app.write_import(&plan, false);
        let id = app.find_page(|m| matches!(m, PageModel::Welcome(_))).unwrap();
        match &app.pages[&id].model {
            PageModel::Welcome(p) => {
                assert_eq!(p.keys, "standard");
                assert_eq!(p.theme, "Visual Studio Light");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn the_page_opens_on_the_sources_and_goes_to_the_preview() {
        let mut app = App::with_file(None);
        let dir = std::env::temp_dir().join(format!("fenix-import-host-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let vimrc = dir.join(".vimrc");
        std::fs::write(&vimrc, "set relativenumber\n").unwrap();
        let nvim = dir.join("init.lua");
        std::fs::write(&nvim, "vim.opt.number = true\n").unwrap();
        let sources = vec![
            Source { name: "Vim".into(), editor: import::Editor::Vim, files: vec![vimrc], vim_extension: false },
            Source { name: "Neovim".into(), editor: import::Editor::Vim, files: vec![nvim], vim_extension: false },
        ];
        app.open_import_page_with(sources);
        let id = app.find_page(|m| matches!(m, PageModel::Import(_))).expect("open");
        assert!(matches!(app.import_page(id).unwrap().stage, crate::import_page::Stage::Sources));
        app.import_action(id, Action::Read(0));
        match &app.import_page(id).unwrap().stage {
            crate::import_page::Stage::Preview(plan) => assert_eq!(plan.settings[0].0.key, "editor.line_numbers"),
            _ => panic!("the preview"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
