//! Links between notes: `[[` completes a note's name, `gd`/`gf` on a
//! link follow it (making the note when it doesn't exist yet), `SPC n b`
//! shows what links here beside the note, `SPC n l` inserts a link
//! through a picker, `SPC n y` copies a link to the line of code you're
//! on, and `SPC n v` pastes a picture from the clipboard next to the note.

use super::notebook_host::NotebookPick;
use super::pages::PageModel;
use super::*;
use crate::backlinks_page::{Action as BlAction, BacklinksPage, Item as BlItem};
use fenix_notebook::{Kind as NbKind, Link, Target};

impl App {
    /// Whether the focused buffer is Markdown.
    pub(super) fn in_markdown(&self) -> bool {
        self.focused_language() == Some(fenix_syntax::LanguageId::Markdown)
    }

    /// The `[[link]]` (or `[text](file.md)`) the cursor is on.
    pub(super) fn notebook_link_under_cursor(&self) -> Option<Link> {
        if !self.in_markdown() {
            return None;
        }
        let (line, col) = self.open().buffer.line_col(&self.cursor());
        let text = self.open().buffer.line(line).to_string();
        let text = text.trim_end_matches(['\n', '\r']);
        fenix_notebook::meta::links_in_line(text, line).into_iter().find(|l| l.cols.contains(&col))
    }

    /// `gd`/`gf` on a link: follows it. Whether there was one.
    pub(super) fn notebook_follow_link_under_cursor(&mut self) -> bool {
        let Some(link) = self.notebook_link_under_cursor() else { return false };
        let from = self.open().buffer.path().map(Path::to_path_buf);
        let jump_from = JumpEntry { buffer: self.focused_buffer_id(), char_idx: self.cursor().char_idx };
        self.notebook_follow(link, from);
        self.record_jump(jump_from);
        true
    }

    /// Goes where `link` points.
    pub(super) fn notebook_follow(&mut self, link: Link, from: Option<PathBuf>) {
        if self.notebook().is_none() {
            return;
        }
        let target = self.notebook.book.as_ref().map(|nb| nb.resolve(&link, from.as_deref())).unwrap_or(Target::Missing(link.target.clone()));
        match target {
            Target::Entry { id, heading } => {
                self.open_notebook_entry(&id);
                if let Some(h) = heading {
                    self.notebook_goto_heading(&h);
                }
            }
            Target::Missing(name) if !link.markdown && !name.is_empty() => {
                let body = fenix_notebook::templates::BUILT_IN[0].body;
                self.notebook_create_note(&name, body);
            }
            Target::Missing(name) => self.set_error(format!("nothing called {name}")),
            Target::File { path, fragment } => {
                if !path.exists() {
                    self.set_error(format!("no file {}", path.display()));
                    return;
                }
                if Self::looks_like_pdf(&path) {
                    self.open_pdf_path_as(&path, true);
                    if let Some(page) = fragment.as_deref().and_then(|f| f.strip_prefix("page=")).and_then(|p| p.parse::<u32>().ok()) {
                        self.pdf_goto_page(page);
                    }
                } else {
                    self.open_file_as(&path, true);
                    if let Some(line) = fragment.as_deref().and_then(|f| f.strip_prefix('L')).and_then(|l| l.parse::<usize>().ok()) {
                        self.notebook_place_cursor(line.saturating_sub(1), 0);
                    } else if let Some(h) = fragment {
                        self.notebook_goto_heading(&h);
                    }
                }
            }
            Target::ProjectFile { project, path, line } => {
                let root = self
                    .known_projects
                    .roots()
                    .iter()
                    .chain(self.project_root.iter())
                    .find(|r| r.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(&project)))
                    .cloned();
                let Some(root) = root else {
                    self.set_error(format!("no project called {project} -- SPC p a adds it"));
                    return;
                };
                let file = root.join(&path);
                if !file.exists() {
                    self.set_error(format!("no {path} in {project}"));
                    return;
                }
                self.open_file_as(&file, true);
                if let Some(line) = line {
                    self.notebook_place_cursor(line.saturating_sub(1), 0);
                }
            }
            Target::Url(url) => {
                if let Err(e) = fenix_fs::open_url(&url) {
                    self.set_error(format!("couldn't open {url}: {e}"));
                }
            }
            Target::Mib(name) => self.notebook_open_mib(&name),
            Target::Task(key) => {
                let found = self.agenda_store.tasks.iter().find(|t| t.jira_key() == Some(key.as_str())).map(|t| t.title.clone());
                self.open_agenda_page(None);
                match found {
                    Some(title) => self.set_message(format!("{key}: {title}")),
                    None => self.set_message(format!("{key} isn't in the agenda -- SPC a i imports it")),
                }
            }
        }
    }

    /// Opens a MIB definition's page by name, searching every kind.
    fn notebook_open_mib(&mut self, name: &str) {
        let (key, _) = self.mib_key_here();
        let Some(set) = self.mib_set(&key) else {
            self.set_error("no MIB here -- mib.roots names the folders");
            return;
        };
        let found = (0..set.roots().len()).find_map(|root| fenix_mib::Kind::ALL.iter().find_map(|&k| set.find(k, root, name)));
        match found {
            Some(def) => self.open_mib_def(key, def),
            None => self.set_error(format!("{name} isn't in the MIB")),
        }
    }

    /// Puts the cursor on heading `title` of the focused buffer.
    pub(super) fn notebook_goto_heading(&mut self, title: &str) {
        let text = self.open().buffer.text();
        let want = title.trim().to_lowercase();
        let want_slug = fenix_notebook::slug(title);
        if let Some(h) = fenix_notebook::meta::headings(&text).into_iter().find(|h| h.title.to_lowercase() == want || fenix_notebook::slug(&h.title) == want_slug) {
            self.notebook_place_cursor(h.line, 0);
        }
    }

    /// Completion after `[[`: every note and diagram, by name.
    pub(super) fn notebook_link_completion(&mut self) -> Option<(usize, String, Vec<fenix_picker::Candidate<crate::completion::Item>>)> {
        if !self.in_markdown() {
            return None;
        }
        let cursor = self.cursor();
        let (line, col) = self.open().buffer.line_col(&cursor);
        let text: String = self.open().buffer.line(line).to_string();
        let before: String = text.chars().take(col).collect();
        let open = before.rfind("[[")?;
        let typed = &before[open + 2..];
        if typed.contains(']') || typed.contains('|') {
            return None;
        }
        let start = cursor.char_idx - typed.chars().count();
        let after: String = text.chars().skip(col).collect();
        let closed = after.starts_with("]]");
        let nb = self.notebook()?;
        let mut out = Vec::new();
        for e in nb.recent() {
            let name = e.display_name();
            let target = if e.kind == NbKind::Day { e.name.clone() } else { e.name.clone() };
            let insert = if closed { target.clone() } else { format!("{target}]]") };
            let mut item = crate::completion::Item::text(insert, crate::completion::Source::Note);
            item.label = name.clone();
            item.detail = e.kind.tag().to_string();
            out.push(fenix_picker::Candidate::new(name, item));
        }
        Some((start, typed.to_string(), out))
    }

    /// `SPC n l`: a link to a note, inserted at the cursor.
    pub(crate) fn notebook_insert_link(&mut self) {
        if !self.open().kind.tracks_unsaved_changes() {
            self.set_error("links go into a document");
            return;
        }
        let Some(nb) = self.notebook() else { return };
        let candidates = nb
            .recent()
            .into_iter()
            .map(|e| fenix_picker::Candidate::new(format!("{}  {}", e.kind.tag(), e.display_name()), NotebookPick::InsertLink(e.name.clone())))
            .collect();
        self.completion = None;
        self.enter_picker(ActivePicker::Notebook { title: "LINK", picker: fenix_picker::PickerState::new(candidates) });
    }

    /// Types `text` at the focused buffer's cursor.
    pub(super) fn notebook_insert_text(&mut self, text: &str) {
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        buffer.insert_str(cursor, text);
    }

    /// `SPC n y`: a link to the line you're on -- `[[project:path:line]]`
    /// in a project, the file's full path otherwise -- to the clipboard.
    pub(crate) fn notebook_copy_link(&mut self) {
        let Some(path) = self.open().buffer.path().map(Path::to_path_buf) else {
            self.set_error("this buffer isn't a file");
            return;
        };
        let (line, _) = self.open().buffer.line_col(&self.cursor());
        let link = self.notebook_link_to(&path, Some(line + 1));
        if let Some(clipboard) = &mut self.clipboard {
            let _ = clipboard.set_text(link.clone());
        }
        self.vim.set_register(link.clone(), false);
        self.set_message(format!("copied {link}"));
    }

    /// A link to `path` (at `line`, 1-based): a notebook entry by name, a
    /// project file as `project:path:line`, anything else by full path.
    pub(super) fn notebook_link_to(&mut self, path: &Path, line: Option<usize>) -> String {
        if let Some(e) = self.notebook.book.as_ref().and_then(|nb| nb.by_path(path)) {
            return format!("[[{}]]", e.name);
        }
        let root = self.project_root.clone().filter(|r| path.starts_with(r));
        match root {
            Some(root) => {
                let project = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let rel = path.strip_prefix(&root).unwrap_or(path).display().to_string().replace('\\', "/");
                match line {
                    Some(l) => format!("[[{project}:{rel}:{l}]]"),
                    None => format!("[[{project}:{rel}]]"),
                }
            }
            None => {
                let p = path.display().to_string().replace('\\', "/").replace(' ', "%20");
                match line {
                    Some(l) => format!("[{}:{l}]({p}#L{l})", path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
                    None => format!("[{}]({p})", path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
                }
            }
        }
    }

    /// `SPC n v`: the clipboard's picture, saved beside the note (in its
    /// attachments folder) and linked at the cursor.
    pub(crate) fn notebook_paste_image(&mut self) {
        let Some(path) = self.open().buffer.path().map(Path::to_path_buf) else {
            self.set_error("pictures go into a saved note");
            return;
        };
        let image = match self.clipboard.as_mut().map(|c| c.get_image()) {
            Some(Ok(image)) => image,
            _ => {
                self.set_error("there's no picture on the clipboard");
                return;
            }
        };
        let entry = self.notebook().and_then(|nb| nb.by_path(&path).map(|e| e.id.clone()));
        let dir = match entry.and_then(|id| self.notebook.book.as_ref().and_then(|nb| nb.attachments_dir(&id))) {
            Some(dir) => dir,
            None => path.parent().map(|p| p.join(format!("{}_files", path.file_stem().unwrap_or_default().to_string_lossy()))).unwrap_or_default(),
        };
        let name = format!("paste-{}.png", chrono::Local::now().format("%m%d-%H%M%S"));
        let file = dir.join(&name);
        let encoded = (|| -> Result<(), String> {
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let buf = image::RgbaImage::from_raw(image.width as u32, image.height as u32, image.bytes.into_owned()).ok_or("the picture's size doesn't match its pixels")?;
            buf.save_with_format(&file, image::ImageFormat::Png).map_err(|e| e.to_string())
        })();
        if let Err(e) = encoded {
            self.set_error(format!("couldn't save the picture: {e}"));
            return;
        }
        let rel = relative_path(path.parent().unwrap_or(Path::new(".")), &file);
        self.notebook_insert_text(&format!("![]({})", rel.replace(' ', "%20")));
        self.set_message(format!("pasted {}", file.display()));
    }

    // ------------------------------------------------------------------
    // The backlinks sidebar
    // ------------------------------------------------------------------

    /// `SPC n b`: what links to the focused note, beside it.
    pub(crate) fn notebook_backlinks(&mut self) {
        let Some(path) = self.open().buffer.path().map(Path::to_path_buf) else {
            self.set_error("SPC n b shows a notebook note's links");
            return;
        };
        if self.notebook().is_none() {
            return;
        }
        let Some(entry) = self.notebook.book.as_ref().and_then(|nb| nb.by_path(&path)).cloned() else {
            self.set_error("this file isn't in the notebook -- SPC n i imports it");
            return;
        };
        let origin = self.focused_pane_id();
        // One sidebar: it moves to this note.
        let existing = self.find_page(|m| matches!(m, PageModel::Backlinks(_)));
        let page_id = match existing {
            Some(id) => {
                if let Some(PageModel::Backlinks(p)) = self.pages.get_mut(&id).map(|s| &mut s.model) {
                    **p = BacklinksPage::new(entry.id.clone(), entry.display_name());
                }
                id
            }
            None => self.open_page_beside(PageModel::Backlinks(Box::new(BacklinksPage::new(entry.id.clone(), entry.display_name()))), 0.3),
        };
        self.notebook.sidebar_origin = Some(origin);
        self.refresh_backlinks();
        if existing.is_some() {
            self.show_page(page_id);
        }
    }

    /// Opens `model` in a new pane to the right, `share` of the width.
    pub(super) fn open_page_beside(&mut self, model: PageModel, share: f32) -> BufferId {
        self.split_vertical();
        let id = self.open_page(model);
        let pane = self.focused_pane_id();
        self.workspaces.active_pane_tabs_mut().insert(pane, vec![id]);
        // The split starts even; this side (the right) gets `share`.
        self.windows_mut().set_split_near(pane, 1.0 - share);
        id
    }

    /// Fills the sidebar's lists for its note, as the notebook now is.
    pub(super) fn refresh_backlinks(&mut self) {
        let Some(page_id) = self.find_page(|m| matches!(m, PageModel::Backlinks(_))) else { return };
        let Some(PageModel::Backlinks(p)) = self.pages.get(&page_id).map(|s| &s.model) else { return };
        let id = p.id.clone();
        let Some(nb) = self.notebook.book.as_ref() else { return };
        let Some(entry) = nb.get(&id).cloned() else { return };
        let row = |nb: &fenix_notebook::Notebook, b: fenix_notebook::Backlink| {
            let e = nb.get(&b.from);
            BlItem { id: b.from.clone(), name: e.map(|e| e.display_name()).unwrap_or_default(), kind: e.map(|e| e.kind.tag()).unwrap_or(""), line: b.line, context: b.context, exists: true }
        };
        let here: Vec<BlItem> = nb.backlinks(&id).into_iter().map(|b| row(nb, b)).collect();
        let mentions: Vec<BlItem> = nb.unlinked_mentions(&id).into_iter().map(|b| row(nb, b)).collect();
        let outline: Vec<BlItem> = fenix_notebook::meta::headings(&entry.text)
            .into_iter()
            .map(|h| BlItem { id: String::new(), name: h.title, kind: ["", "", "  ", "    ", "      ", "        ", "          "][h.level.min(6)], line: h.line, context: String::new(), exists: true })
            .collect();
        let out: Vec<BlItem> = entry
            .links
            .iter()
            .map(|l| {
                let target = nb.resolve(l, Some(&entry.path));
                let (kind, exists) = match &target {
                    Target::Entry { id, .. } => (nb.get(id).map(|e| e.kind.tag()).unwrap_or("NOTE"), true),
                    Target::Missing(_) => ("NEW", false),
                    Target::File { .. } | Target::ProjectFile { .. } => ("FILE", true),
                    Target::Url(_) => ("URL", true),
                    Target::Mib(_) => ("MIB", true),
                    Target::Task(_) => ("TASK", true),
                };
                BlItem { id: String::new(), name: l.label(), kind, line: l.line, context: String::new(), exists }
            })
            .collect();
        if let Some(PageModel::Backlinks(p)) = self.pages.get_mut(&page_id).map(|s| {
            s.stale = true;
            &mut s.model
        }) {
            p.here = here;
            p.mentions = mentions;
            p.outline = outline;
            p.out = out;
            p.clamp();
        }
    }

    /// Focuses the pane the sidebar's note is in (or any other one).
    fn focus_sidebar_origin(&mut self, page_id: BufferId) {
        let origin = self.notebook.sidebar_origin.filter(|p| self.windows().windows().contains(p));
        let target = origin.or_else(|| self.windows().windows().into_iter().find(|&p| self.windows().content(p) != Some(&page_id)));
        if let Some(p) = target {
            self.windows_mut().focus(p);
        }
    }

    pub(super) fn backlinks_action(&mut self, page_id: BufferId, action: BlAction) {
        let note_id = match self.pages.get(&page_id).map(|s| &s.model) {
            Some(PageModel::Backlinks(p)) => p.id.clone(),
            _ => return,
        };
        match action {
            BlAction::None => {}
            BlAction::Close => {
                self.close_page(page_id);
                let pane = self.windows().windows().into_iter().find(|&p| self.windows().content(p) == Some(&page_id));
                if let Some(p) = pane {
                    self.windows_mut().focus(p);
                    self.close_window();
                }
            }
            BlAction::Open { id, line } => {
                self.focus_sidebar_origin(page_id);
                self.open_notebook_entry(&id);
                self.notebook_place_cursor(line, 0);
            }
            BlAction::Line(line) => {
                self.focus_sidebar_origin(page_id);
                self.open_notebook_entry(&note_id);
                self.notebook_place_cursor(line, 0);
            }
            BlAction::Follow { name: _, line } => {
                let link = self.notebook.book.as_ref().and_then(|nb| nb.get(&note_id)).and_then(|e| e.links.iter().find(|l| l.line == line).cloned());
                let from = self.notebook.book.as_ref().and_then(|nb| nb.get(&note_id)).map(|e| e.path.clone());
                self.focus_sidebar_origin(page_id);
                if let Some(link) = link {
                    self.notebook_follow(link, from);
                }
            }
            BlAction::Link { id, line } => {
                let Some(nb) = self.notebook.book.as_ref() else { return };
                let (Some(target), Some(entry)) = (nb.get(&note_id).cloned(), nb.get(&id).cloned()) else { return };
                let mut lines: Vec<String> = entry.text.lines().map(str::to_string).collect();
                if let Some(l) = lines.get_mut(line) {
                    let lower = l.to_lowercase();
                    if let Some(at) = lower.find(&target.name.to_lowercase()) {
                        let end = at + target.name.len();
                        let shown = &l[at..end];
                        let link = if shown == target.name { format!("[[{}]]", target.name) } else { format!("[[{}|{shown}]]", target.name) };
                        l.replace_range(at..end, &link);
                    }
                }
                let mut text = lines.join("\n");
                if entry.text.ends_with('\n') {
                    text.push('\n');
                }
                self.notebook_write_entry(&id, &text);
                self.refresh_backlinks();
                self.set_message(format!("linked {} in {}", target.name, entry.display_name()));
            }
        }
    }
}

/// `to`, relative to folder `from`, with `/`.
pub(super) fn relative_path(from: &Path, to: &Path) -> String {
    let from: Vec<_> = from.components().collect();
    let to_c: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to_c).take_while(|(a, b)| a == b).count();
    if common == 0 {
        return to.display().to_string().replace('\\', "/");
    }
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_string(), from.len() - common).collect();
    parts.extend(to_c[common..].iter().map(|c| c.as_os_str().to_string_lossy().to_string()));
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_file(None);
        app.config.notebook_folder = Some(dir.path().join("nb"));
        (dir, app)
    }

    #[test]
    fn relative_paths() {
        assert_eq!(relative_path(Path::new("/nb/notes"), Path::new("/nb/attachments/x/a.png")), "../attachments/x/a.png");
        assert_eq!(relative_path(Path::new("/nb/notes"), Path::new("/nb/notes/a.png")), "a.png");
    }

    #[test]
    fn gd_on_a_link_opens_the_note_or_makes_it() {
        let (_dir, mut app) = app();
        app.notebook_create_note("Target", "# Target\n\n## Decision\nhere\n");
        app.notebook_create_note("Source", "see [[Target#Decision]] and [[New one]]\n");
        app.notebook_place_cursor(0, 6);
        assert!(app.notebook_follow_link_under_cursor());
        assert!(app.open().buffer.path().unwrap().ends_with("target.md"));
        assert_eq!(app.open().buffer.line_col(&app.cursor()).0, 2);
        app.notebook_create_note("Source", "");
        app.notebook_place_cursor(0, 30);
        assert!(app.notebook_follow_link_under_cursor());
        assert!(app.open().buffer.path().unwrap().ends_with("new-one.md"));
        app.notebook_place_cursor(0, 0);
        assert!(!app.notebook_follow_link_under_cursor() || app.open().buffer.text().starts_with("# New one"));
    }

    #[test]
    fn double_brackets_complete_note_names() {
        let (_dir, mut app) = app();
        app.notebook_create_note("Uplink retries", "");
        app.notebook_create_note("Log", "");
        app.test_vim_key(KeyPress::char('i'));
        app.test_insert_str("see [[upl");
        let (start, prefix, items) = app.notebook_link_completion().expect("completes after [[");
        assert_eq!(prefix, "upl");
        assert_eq!(start, 6);
        let item = items.iter().find(|c| c.label == "Uplink retries").expect("the note");
        assert!(matches!(&item.payload.insertion, crate::completion::Insertion::Text(t) if t == "Uplink retries]]"));
    }

    #[test]
    fn the_sidebar_lists_links_here_and_mentions_and_links_one() {
        let (_dir, mut app) = app();
        app.notebook_create_note("Uplink", "# Uplink\n## Retries\n");
        app.notebook_create_note("Log", "see [[Uplink]]\n");
        app.notebook_create_note("Other", "the uplink is slow\n");
        let uplink = app.notebook.book.as_ref().unwrap().by_name("Uplink").unwrap().path.clone();
        app.open_file_from_picker(&uplink);
        app.notebook_backlinks();
        let page = app.focused_buffer_id();
        let Some(PageModel::Backlinks(p)) = app.pages.get(&page).map(|s| &s.model) else { panic!("the sidebar") };
        assert_eq!(p.here.len(), 1);
        assert_eq!(p.mentions.len(), 1);
        assert_eq!(p.outline.len(), 2);
        let other = p.mentions[0].id.clone();
        app.backlinks_action(page, BlAction::Link { id: other.clone(), line: 0 });
        assert_eq!(app.notebook.book.as_ref().unwrap().get(&other).unwrap().text, "the [[Uplink|uplink]] is slow\n");
    }
}
