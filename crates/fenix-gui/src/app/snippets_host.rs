//! The host half of the snippets page (`snippets_page`): the three
//! layers read into rows, and what the page asks for -- a new snippet's
//! file made and opened, one copied into yours or the project's, one
//! deleted to the Recycle Bin, one tried in the buffer you came from --
//! plus `SPC i n`, which starts a snippet from a Visual selection, and
//! the check a `.snippet` file gets when it's saved.

use super::pages::PageModel;
use super::*;
use crate::snippets_page::{Action as SnippetsAction, Item, SnippetsPage};
use fenix_snippets::{Catalog, Context};

impl App {
    fn snippets_page(&mut self, id: BufferId) -> Option<&mut SnippetsPage> {
        match self.pages.get_mut(&id).map(|s| {
            s.stale = true;
            &mut s.model
        }) {
            Some(PageModel::Snippets(p)) => Some(p),
            _ => None,
        }
    }

    /// The focused project's snippets folder, `.fenix/snippets`.
    fn project_snippets_dir(&self) -> Option<PathBuf> {
        self.project_root.as_ref().map(|r| r.join(".fenix").join("snippets"))
    }

    /// Every layer: the built-in snippets (unless they're turned off),
    /// yours, and the focused project's.
    pub(super) fn snippet_layers(&self, project: Option<&Path>) -> Catalog {
        Catalog::layers(self.config.snippets_builtin.unwrap_or(true), Some(&self.snippets_dir), project)
    }

    /// The page's rows, from `catalog`, previewed for the file at `path`.
    fn snippet_items(catalog: &Catalog, path: Option<&Path>) -> Vec<Item> {
        let context = Context::current(path);
        catalog
            .snippets
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let overridden = catalog.snippets[i + 1..].iter().any(|later| later.trigger == s.trigger && later.scopes.first() == s.scopes.first() && later.source > s.source);
                Item {
                    name: s.name.clone(),
                    trigger: s.trigger.clone(),
                    scopes: s.scopes.clone(),
                    source: s.source,
                    file: s.file.clone(),
                    text: s.text.clone(),
                    preview: s.template.render(&Default::default(), &context, "").text,
                    fields: s.template.field_count(),
                    overridden,
                }
            })
            .collect()
    }

    /// `SPC i S`: the snippets page, for the language of the buffer
    /// you're in; with `body`, its new-snippet form is open on it.
    pub(crate) fn open_snippets_page(&mut self, body: Option<String>) {
        let from = self.focused_buffer_id();
        if !self.is_page_buffer(from) {
            self.snippets_origin = Some((from, self.focused_pane_id()));
        }
        let scope = self.snippet_scope();
        let project = self.project_snippets_dir();
        let path = self.open().buffer.path().map(Path::to_path_buf);
        let catalog = self.snippet_layers(project.as_deref());
        let items = Self::snippet_items(&catalog, path.as_deref());
        let id = match self.find_page(|m| matches!(m, PageModel::Snippets(_))) {
            Some(id) => {
                self.show_page(id);
                if let Some(page) = self.snippets_page(id) {
                    page.scope = scope;
                    page.has_project = project.is_some();
                    page.refresh(items, catalog.errors.clone());
                }
                id
            }
            None => self.open_page(PageModel::Snippets(Box::new(SnippetsPage::new(items, catalog.errors.clone(), scope, project.is_some())))),
        };
        if let (Some(body), Some(page)) = (body, self.snippets_page(id)) {
            page.new_from(body);
        }
    }

    fn refresh_snippets_page(&mut self, id: BufferId) {
        let project = self.snippets_project.clone().or_else(|| self.project_snippets_dir());
        let path = self.snippets_origin.and_then(|(b, _)| self.buffers.get(b)).and_then(|ob| ob.buffer.path()).map(Path::to_path_buf);
        let catalog = self.snippet_layers(project.as_deref());
        let items = Self::snippet_items(&catalog, path.as_deref());
        if let Some(page) = self.snippets_page(id) {
            page.refresh(items, catalog.errors.clone());
        }
    }

    /// Writes `text` as a new snippet file under `dir`, in its language's
    /// folder; refuses to replace one that's there.
    fn write_snippet(dir: &Path, trigger: &str, scopes: &[String], text: &str) -> Result<PathBuf, String> {
        let folder = dir.join(fenix_snippets::folder_for(scopes));
        let file = folder.join(format!("{trigger}.snippet"));
        if file.exists() {
            return Err(format!("{} is there already -- Enter on it edits it", file.display()));
        }
        std::fs::create_dir_all(&folder).map_err(|e| format!("couldn't make {}: {e}", folder.display()))?;
        std::fs::write(&file, text).map_err(|e| format!("couldn't write {}: {e}", file.display()))?;
        Ok(file)
    }

    pub(super) fn snippets_action(&mut self, id: BufferId, action: SnippetsAction) {
        // The project the page was opened for, even while it's in front.
        if self.snippets_project.is_none() {
            self.snippets_project = self.project_snippets_dir();
        }
        let project_dir = self.snippets_project.clone();
        let note = |app: &mut App, text: String, bad: bool| {
            if let Some(page) = app.snippets_page(id) {
                page.note = Some((text, bad));
            }
        };
        match action {
            SnippetsAction::None => {}
            SnippetsAction::Close => {
                self.snippets_project = None;
                self.close_page(id);
            }
            SnippetsAction::Edit(file) => self.open_file_from_picker(&file),
            SnippetsAction::Create { trigger, name, scopes, body, project } => {
                let dir = if project { project_dir } else { Some(self.snippets_dir.clone()) };
                let Some(dir) = dir else { return note(self, "no project open".into(), true) };
                let body = if body.is_empty() { String::new() } else { fenix_snippets::escape_body(&body) };
                match Self::write_snippet(&dir, &trigger, &scopes, &fenix_snippets::file_text(&name, &trigger, &scopes, &body)) {
                    Ok(file) => {
                        self.refresh_snippets_page(id);
                        self.open_file_from_picker(&file);
                        self.set_message(format!("{} -- write what it inserts under # --; :w and it's live", file.display()));
                    }
                    Err(why) => note(self, why, true),
                }
            }
            SnippetsAction::Copy { item, project } => {
                let dir = if project { project_dir } else { Some(self.snippets_dir.clone()) };
                let Some(dir) = dir else { return note(self, "no project open".into(), true) };
                match Self::write_snippet(&dir, &item.trigger, &item.scopes, &item.text) {
                    Ok(file) => {
                        self.refresh_snippets_page(id);
                        let whose = if project { "the project's" } else { "yours" };
                        note(self, format!("copied to {whose}: {} -- it's the one used now", file.display()), false);
                    }
                    Err(why) => note(self, why, true),
                }
            }
            SnippetsAction::Delete(file) => {
                let outcome = fenix_fs::to_recycle_bin(std::slice::from_ref(&file));
                match outcome.first() {
                    Some(o) if o.error.is_none() => {
                        self.refresh_snippets_page(id);
                        note(self, format!("{} is in the Recycle Bin", file.display()), false);
                    }
                    Some(o) => note(self, format!("couldn't delete it: {}", o.error.clone().unwrap_or_default()), true),
                    None => {}
                }
            }
            SnippetsAction::Try(item) => {
                let Some((buffer, pane)) = self.snippets_origin else {
                    return note(self, "open the snippets from a file to try one in it".into(), true);
                };
                let Ok(snippet) = fenix_snippets::Snippet::parse(&item.text) else { return };
                if !self.windows().windows().contains(&pane) || self.buffers.get(buffer).is_none() {
                    return note(self, "the file you came from is closed".into(), true);
                }
                self.windows_mut().focus(pane);
                self.open_buffer_in_focused_pane(buffer);
                let cursor = self.cursor();
                self.vim.exit_visual_mode(&cursor);
                if self.vim.mode() != Mode::Insert {
                    let pane = self.focused_pane_id();
                    let id = self.focused_buffer_id();
                    if let (Some(ob), Some(state)) = (self.buffers.get_mut(id), self.workspaces.active_pane_states_mut().get_mut(&pane)) {
                        self.vim.handle_key(&mut ob.buffer, &mut state.cursor, KeyPress::char('i'));
                    }
                }
                let at = self.cursor().char_idx;
                self.expand_snippet_template(at, snippet.template);
            }
        }
    }

    /// `SPC i n`: a snippet from what's selected -- the snippets page's
    /// new-snippet form, with the selection as its body.
    pub(crate) fn snippet_from_selection(&mut self) {
        let cursor = self.cursor();
        let text: Vec<char> = self.open().buffer.text().chars().collect();
        // The selection while it's there; after it (the leader isn't a
        // Visual-mode key), the whole lines it covered.
        let range = match self.vim.visual_selection_range(&self.open().buffer, &cursor) {
            Some((range, _)) => Some(range),
            None => self.vim.last_visual_range().map(|(a, b)| {
                let start = text[..a.min(text.len())].iter().rposition(|c| *c == '\n').map(|i| i + 1).unwrap_or(0);
                let end = text[b.min(text.len())..].iter().position(|c| *c == '\n').map(|i| b + i + 1).unwrap_or(text.len());
                start..end
            }),
        };
        let Some(range) = range else {
            self.set_error("select what the snippet should insert first (v or V), then SPC i n");
            return;
        };
        let body: String = text[range.start.min(text.len())..range.end.min(text.len())].iter().collect();
        self.vim.exit_visual_mode(&cursor);
        self.open_snippets_page(Some(body));
    }

    /// What saving a `.snippet` file says: that it works, or why not.
    pub(super) fn snippet_save_note(path: &Path) -> Option<Result<String, String>> {
        if path.extension().is_none_or(|e| e != "snippet") {
            return None;
        }
        let text = std::fs::read_to_string(path).ok()?;
        Some(match fenix_snippets::Snippet::parse(&text) {
            Ok(s) => Ok(format!("snippet \"{}\" works -- {}; it's live now", s.trigger, match s.template.field_count() {
                0 => "no fields".to_string(),
                1 => "1 field".to_string(),
                n => format!("{n} fields"),
            })),
            Err(why) => Err(format!("this snippet won't be offered until it's fixed: {why}")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_snippets::Source;

    fn app_on(text: &str, file: &str) -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(file);
        std::fs::write(&path, text).unwrap();
        let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
        app.snippets_dir = dir.path().join("my-snippets");
        (dir, app)
    }

    fn page(app: &mut App) -> &mut SnippetsPage {
        let id = app.focused_buffer_id();
        app.snippets_page(id).expect("the snippets page")
    }

    #[test]
    fn a_selection_becomes_a_snippet_file_in_its_languages_folder() {
        let (dir, mut app) = app_on("puts \"hello $name\"\nexit\n", "script.tcl");
        app.test_vim_key(KeyPress::char('V'));
        app.test_vim_key(KeyPress::named(FenixNamedKey::Escape));
        app.snippet_from_selection();
        assert!(page(&mut app).typing(), "the form is open");
        for c in "hello".chars() {
            app.page_key(KeyPress::char(c));
        }
        for _ in 0..3 {
            app.page_key(KeyPress::named(FenixNamedKey::Enter));
        }
        let file = dir.path().join("my-snippets").join("tcl").join("hello.snippet");
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.starts_with("# name: hello\n# key: hello\n# scope: tcl\n# --\n"), "{text}");
        assert_eq!(app.open().buffer.path(), Some(file.as_path()), "opened to edit");
        let catalog = app.snippet_layers(None);
        let snippet = catalog.matching("hello", "tcl").expect("offered now");
        assert_eq!(snippet.template.render(&Default::default(), &Default::default(), "").text, "puts \"hello $name\"\n", "inserts what was selected");
    }

    #[test]
    fn a_built_in_snippet_copied_to_yours_takes_over_and_can_be_deleted() {
        let (dir, mut app) = app_on("x", "a.tcl");
        app.open_snippets_page(None);
        // The built-in Tcl `proc`.
        assert!(page(&mut app).select(|i| i.trigger == "proc" && i.source == Source::BuiltIn));
        assert!(app.page_key(KeyPress::char('c')));
        let copy = dir.path().join("my-snippets").join("tcl").join("proc.snippet");
        assert!(copy.is_file());
        let catalog = app.snippet_layers(None);
        assert_eq!(catalog.matching("proc", "tcl").unwrap().source, Source::User, "yours is the one used");
        assert!(page(&mut app).items.iter().any(|i| i.trigger == "proc" && i.overridden), "the built-in one says so");
    }

    #[test]
    fn saving_a_snippet_file_says_whether_it_works() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good.snippet");
        std::fs::write(&good, "# key: g\n# --\n${1:a} ${2:b}$0").unwrap();
        assert_eq!(App::snippet_save_note(&good), Some(Ok("snippet \"g\" works -- 2 fields; it's live now".into())));
        let bad = dir.path().join("bad.snippet");
        std::fs::write(&bad, "# --\nno key").unwrap();
        assert!(App::snippet_save_note(&bad).unwrap().unwrap_err().contains("missing # key:"));
        assert_eq!(App::snippet_save_note(&dir.path().join("x.rs")), None);
    }
}
