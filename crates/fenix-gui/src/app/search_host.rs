//! The host half of the Search page (`search_page.rs`): opening it
//! beside the file you're in, running each search off the UI thread
//! (`fenix_project::search`) and streaming what it finds back, showing a
//! match beside the page as the selection moves, the quickfix list
//! (`SPC p n`/`N`), each project's history, and a replace -- checked
//! against the files before it writes, and undone as one (`SPC s u`).

use super::pages::PageEvent;
use super::*;
use crate::search_page::{Action as SearchAction, Field, Input, ScopeKind, SearchPage, Status};
use fenix_project::search::{self as engine, FileResult, Options, Pattern, Scope, Summary};
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Each project's searches, in the state folder.
pub(super) const HISTORY_FILE: &str = "search.json";
/// How long typing pauses before it searches.
const DEBOUNCE: Duration = Duration::from_millis(150);
/// How many searches each project keeps.
const HISTORY: usize = 50;
/// What a search streams back at once: this many files, or this often.
const BATCH_FILES: usize = 200;
const BATCH_EVERY: Duration = Duration::from_millis(60);

/// How a search is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SearchOpen {
    /// `SPC /`, `SPC p s`: the project.
    Project,
    /// `SPC s p`: the project, Replace open.
    Replace,
    /// `SPC s s`: this file, or the selection.
    InFile,
    /// `SPC s r`: this file, or the selection, Replace open.
    ThisFile,
    /// `SPC e g`: the folder the explorer is in.
    Folder(PathBuf),
    /// `SPC s *`: the word under the cursor, in the project.
    Word,
    /// `SPC s .`: the last search, as it was left.
    Last,
}

/// The files a replace changed, to put back (`SPC s u`).
struct UndoFile {
    path: PathBuf,
    before: String,
    after: String,
    /// An open buffer changed in memory, and its edit count after.
    buffer: Option<(BufferId, u64)>,
}

#[derive(Default)]
pub(super) struct SearchHost {
    /// The page whose typing is waiting to search, and when.
    pub(super) due: Option<(BufferId, Instant)>,
    cancel: Option<Arc<AtomicBool>>,
    generation: u64,
    started: Option<Instant>,
    /// Each project's searches, newest first (read when first needed).
    history: Option<HashMap<PathBuf, Vec<String>>>,
    /// The page as it was closed, for `SPC s .`.
    last: Option<SearchPage>,
    undo: Option<Vec<UndoFile>>,
    /// The page the quickfix list came from.
    quickfix_page: Option<BufferId>,
}

impl App {
    fn search_page_id(&self) -> Option<BufferId> {
        self.find_page(|m| matches!(m, PageModel::Search(_)))
    }

    fn search_page_mut(&mut self, id: BufferId) -> Option<&mut SearchPage> {
        match self.pages.get_mut(&id).map(|s| &mut s.model) {
            Some(PageModel::Search(p)) => Some(p),
            _ => None,
        }
    }

    pub(super) fn search_page(&self, id: BufferId) -> Option<&SearchPage> {
        match self.pages.get(&id).map(|s| &s.model) {
            Some(PageModel::Search(p)) => Some(p),
            _ => None,
        }
    }

    /// Opens the Search page as `how` asks, beside the file you're in.
    pub(crate) fn search_open(&mut self, how: SearchOpen) {
        let root = self.project_root.clone().unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let file = self.open().buffer.path().map(Path::to_path_buf).filter(|_| self.open().kind == BufferKind::Text);
        // A buffer of no file -- a scratch, a lesson's practice -- isn't
        // on disk for the page to search: its lines are picked instead.
        if how == SearchOpen::InFile && file.is_none() {
            self.picker_search_buffer();
            return;
        }
        // The selection, as lines, when this comes from Visual mode.
        let selection = if self.vim.mode() == Mode::Visual && file.is_some() {
            let cursor = self.cursor();
            let ob = self.open();
            let lines = self.vim.visual_selection_range(&ob.buffer, &cursor).map(|(r, _)| {
                let lo = ob.buffer.line_col(&Cursor { char_idx: r.start, sticky_col: 0 }).0;
                let hi = ob.buffer.line_col(&Cursor { char_idx: r.end.saturating_sub(1).max(r.start), sticky_col: 0 }).0;
                (lo + 1, hi + 1)
            });
            self.vim.exit_visual_mode(&cursor);
            lines
        } else {
            None
        };
        let word = if how == SearchOpen::Word { self.word_at_cursor() } else { None };
        if how == SearchOpen::Word && word.is_none() {
            self.set_error("no word under the cursor");
            return;
        }
        let folder = match &how {
            SearchOpen::Folder(dir) => Some(dir.clone()),
            _ => None,
        };
        let existing = self.search_page_id();
        let mut page = match (&how, existing) {
            (_, Some(id)) => self.search_page(id).cloned().unwrap_or_default(),
            (SearchOpen::Last, None) => match self.search.last.clone() {
                Some(last) => last,
                None => {
                    self.set_error("no search yet -- SPC / starts one");
                    return;
                }
            },
            // A new page keeps the toggles the last one had, else starts
            // as the settings say.
            (_, None) => match self.search.last.clone() {
                Some(last) => SearchPage { options: last.options, preserve_case: last.preserve_case, globs: last.globs, following: last.following, ..Default::default() },
                None => {
                    let options = Options {
                        match_case: self.config.search_smart_case == Some(false),
                        regex: self.config.search_regex == Some(true),
                        context: self.config.search_context.unwrap_or(0),
                        ..Options::default()
                    };
                    let globs = self.config.search_exclude.iter().map(|g| if g.starts_with('!') { g.clone() } else { format!("!{g}") }).collect::<Vec<_>>().join(" ");
                    SearchPage { options, globs: Input::new(&globs), following: true, ..Default::default() }
                }
            },
        };
        if how != SearchOpen::Last {
            page.root = root.clone();
            page.folder = folder.clone();
            page.file = file.clone();
            page.selection = selection;
            let mut scopes = vec![ScopeKind::Project];
            if page.folder.is_some() {
                scopes.push(ScopeKind::Folder);
            }
            scopes.push(ScopeKind::OpenFiles);
            if page.file.is_some() {
                scopes.push(ScopeKind::ThisFile);
            }
            if page.selection.is_some() {
                scopes.push(ScopeKind::Selection);
            }
            scopes.push(ScopeKind::Changed);
            page.scopes = scopes;
            page.scope = Some(match &how {
                SearchOpen::Folder(_) => ScopeKind::Folder,
                SearchOpen::InFile | SearchOpen::ThisFile if page.selection.is_some() => ScopeKind::Selection,
                SearchOpen::InFile | SearchOpen::ThisFile => ScopeKind::ThisFile,
                _ => ScopeKind::Project,
            });
            if matches!(how, SearchOpen::Replace | SearchOpen::ThisFile) {
                page.replace_open = true;
            }
        }
        page.history = self.search_history(&page.root);
        page.message = None;
        page.confirm = false;
        let run_now = match &how {
            SearchOpen::Word => {
                page.query = Input::new(word.as_deref().unwrap_or_default());
                page.options.whole_word = true;
                page.options.regex = false;
                page.editing = None;
                true
            }
            SearchOpen::Last => {
                page.editing = None;
                !page.query.text.is_empty()
            }
            // A replace with a query already typed goes to the Replace field.
            SearchOpen::Replace | SearchOpen::ThisFile if !page.query.text.is_empty() => {
                page.edit(Field::Replace);
                true
            }
            _ => {
                page.edit(Field::Query);
                !page.query.text.is_empty()
            }
        };
        let id = match existing {
            Some(id) => {
                if let Some(p) = self.search_page_mut(id) {
                    *p = page;
                }
                self.search_show(id);
                id
            }
            None => {
                let id = self.add_page(PageModel::Search(Box::new(page)));
                self.search_show(id);
                id
            }
        };
        if run_now {
            self.search_run(id);
        }
        self.restale_page(id);
        self.wake_caret();
    }

    /// The word under the cursor: letters, digits and `_` around it.
    fn word_at_cursor(&self) -> Option<String> {
        let ob = self.open();
        let (line, col) = ob.buffer.line_col(&self.cursor());
        let start = ob.buffer.line_start_char(line);
        let chars: Vec<char> = ob.buffer.text_range(start, start + ob.buffer.line_len(line)).chars().collect();
        let word = |c: char| c.is_alphanumeric() || c == '_';
        let at = col.min(chars.len());
        if !chars.get(at).is_some_and(|&c| word(c)) {
            return None;
        }
        let from = (0..at).rev().take_while(|&i| word(chars[i])).last().unwrap_or(at);
        let to = (at..chars.len()).take_while(|&i| word(chars[i])).last().map_or(at, |i| i + 1);
        Some(chars[from..to].iter().collect())
    }

    /// The page shown and focused: where it is, else beside the file.
    fn search_show(&mut self, id: BufferId) {
        if let Some(pane) = self.pane_showing(id) {
            self.windows_mut().focus(pane);
        } else if self.windows().window_count() > 1 {
            let here = self.focused_pane_id();
            let other = self.windows().windows().into_iter().find(|&p| p != here).unwrap_or(here);
            self.windows_mut().focus(other);
            self.show_page(id);
        } else {
            self.split_vertical();
            self.show_page(id);
        }
        self.sidebar_focused = false;
        self.main_view = MainView::Editor;
    }

    /// A key with Alt, or a paste, while the page is focused: the
    /// toggles. Whether it was one.
    pub(super) fn search_page_special_key(&mut self, id: BufferId, keypress: KeyPress) -> bool {
        if keypress == KeyPress::char('v').with_ctrl() {
            let Some(text) = self.clipboard_text() else { return true };
            if let Some(action) = self.search_page_mut(id).map(|p| p.paste(&text)) {
                self.search_action(id, action);
            }
            return true;
        }
        if !keypress.mods.alt || keypress.mods.ctrl {
            return false;
        }
        let KeyCode::Char(c) = keypress.code else { return false };
        let Some(action) = self.search_page_mut(id).map(|p| p.toggle(c.to_ascii_lowercase())) else { return false };
        self.search_action(id, action);
        self.restale_page(id);
        self.wake_caret();
        true
    }

    /// What a key on the page asked for.
    pub(super) fn search_action(&mut self, id: BufferId, action: SearchAction) {
        match action {
            SearchAction::None => {}
            SearchAction::Close => self.search_close(id),
            SearchAction::Search => {
                self.search.due = None;
                self.search_remember(id);
                self.search_run(id);
            }
            SearchAction::SearchSoon => self.search.due = Some((id, Instant::now() + DEBOUNCE)),
            SearchAction::NextScope => {
                if let Some(p) = self.search_page_mut(id) {
                    p.next_scope();
                }
                self.search_run(id);
            }
            SearchAction::Open { path, line, col } => {
                self.search_remember(id);
                self.search_go_to(id, &path, line, col, true);
            }
            SearchAction::Show { path, line, col } => {
                if self.search_page(id).is_some_and(|p| p.following) {
                    self.search_go_to(id, &path, line, col, false);
                }
            }
            SearchAction::Follow(on) => {
                if on {
                    if let Some((path, line, col)) = self.search_page(id).and_then(SearchPage::selected) {
                        self.search_go_to(id, &path, line, col, false);
                    }
                }
            }
            SearchAction::Apply => self.search_apply(id),
        }
        self.restale_page(id);
    }

    /// Once a frame: a search waiting on typing that's paused runs.
    pub(super) fn search_fire_due(&mut self, now: Instant) -> Option<Instant> {
        let (id, at) = self.search.due?;
        if now < at {
            return Some(at);
        }
        self.search.due = None;
        self.search_run(id);
        self.restale_page(id);
        self.wake_caret();
        None
    }

    /// Searches again with what the page has now, off the UI thread; the
    /// search before it is cancelled.
    pub(super) fn search_run(&mut self, id: BufferId) {
        if let Some(cancel) = self.search.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        let Some(page) = self.search_page(id) else { return };
        let options = page.search_options();
        let query = page.query.text.clone();
        let pattern = if query.is_empty() { Err(String::new()) } else { Pattern::new(&query, &options) };
        let pattern = match pattern {
            Ok(p) => p,
            Err(why) => {
                if let Some(p) = self.search_page_mut(id) {
                    if why.is_empty() {
                        p.error = None;
                        p.begin(None);
                    } else {
                        // The last good results stay; the field says why.
                        p.error = Some(why);
                    }
                }
                return;
            }
        };
        let (root, scope_kind) = (page.root.clone(), page.scope());
        let selection = page.selection;
        let scope = match scope_kind {
            ScopeKind::Project => Some(Scope::Folders(vec![root.clone()])),
            ScopeKind::Folder => Some(Scope::Folders(page.folder.clone().into_iter().collect())),
            ScopeKind::ThisFile | ScopeKind::Selection => Some(Scope::Files(page.file.clone().into_iter().collect())),
            ScopeKind::OpenFiles => Some(Scope::Files(
                self.buffers.ids_sorted_by_path().into_iter().filter_map(|b| self.buffers.get(b)).filter(|ob| ob.kind == BufferKind::Text).filter_map(|ob| ob.buffer.path().map(Path::to_path_buf)).collect(),
            )),
            // Asked of Git off the UI thread.
            ScopeKind::Changed => None,
        };
        // Open buffers are searched as they are, unsaved edits and all.
        let unsaved: HashMap<PathBuf, String> = self
            .buffers
            .ids_sorted_by_path()
            .into_iter()
            .filter_map(|b| self.buffers.get(b))
            .filter(|ob| ob.kind == BufferKind::Text && ob.buffer.is_dirty())
            .filter_map(|ob| ob.buffer.path().map(|p| (p.to_path_buf(), ob.buffer.text())))
            .collect();
        let max = self.config.search_max_results.unwrap_or(10_000);
        if let Some(p) = self.search_page_mut(id) {
            p.error = None;
            p.begin(Some(pattern.clone()));
        }
        self.search.generation += 1;
        self.search.started = Some(Instant::now());
        let generation = self.search.generation;
        let cancel = Arc::new(AtomicBool::new(false));
        self.search.cancel = Some(cancel.clone());
        self.page_spawn(move |send| {
            let scope = scope.unwrap_or_else(|| {
                let changed = fenix_explorer::status_for_dir(&root).into_iter().filter(|(_, s)| *s != fenix_explorer::GitStatus::Ignored).map(|(p, _)| p).collect::<Vec<_>>();
                let mut changed = changed;
                changed.sort();
                Scope::Files(changed)
            });
            let mut batch: Vec<FileResult> = Vec::new();
            let mut flushed = Instant::now();
            let result = engine::search(&scope, &pattern, &options, &unsaved, max, &cancel, |mut file| {
                if let Some((lo, hi)) = selection {
                    file.lines.retain(|l| (lo..=hi).contains(&l.number));
                    if file.match_count() == 0 {
                        return ControlFlow::Continue(());
                    }
                }
                batch.push(file);
                if batch.len() >= BATCH_FILES || flushed.elapsed() >= BATCH_EVERY {
                    send(PageEvent::SearchFound { buffer: id, generation, files: std::mem::take(&mut batch) });
                    flushed = Instant::now();
                }
                ControlFlow::Continue(())
            });
            if !batch.is_empty() {
                send(PageEvent::SearchFound { buffer: id, generation, files: batch });
            }
            send(PageEvent::SearchDone { buffer: id, generation, result });
        });
    }

    /// Files the search under way found.
    pub(super) fn apply_search_found(&mut self, id: BufferId, generation: u64, files: Vec<FileResult>) {
        if generation != self.search.generation {
            return;
        }
        if let Some(p) = self.search_page_mut(id) {
            p.push(files, 0);
        }
        self.restale_page(id);
        self.wake_caret();
    }

    /// The search under way finished: how it went, and its matches are
    /// the quickfix list.
    pub(super) fn apply_search_done(&mut self, id: BufferId, generation: u64, result: Result<Summary, String>) {
        if generation != self.search.generation {
            return;
        }
        self.search.cancel = None;
        let ms = self.search.started.map(|s| s.elapsed().as_millis()).unwrap_or(0);
        let Some(p) = self.search_page_mut(id) else { return };
        p.status = match result {
            Ok(summary) if summary.cancelled => return,
            Ok(summary) => Status::Done { files: summary.files_searched, ms, binary: summary.binary_skipped, truncated: summary.truncated },
            Err(why) => Status::Failed(why),
        };
        let entries: Vec<QuickfixEntry> = p
            .match_rows()
            .into_iter()
            .map(|(fi, li)| {
                let (file, line) = (&p.files[fi], &p.files[fi].lines[li]);
                let col = line.matches.first().map(|m| line.text[..m.start].chars().count() + 1).unwrap_or(1);
                QuickfixEntry::Grep(fenix_project::GrepMatch { path: file.path.clone(), line: line.number, col, text: line.text.clone() })
            })
            .collect();
        self.quickfix = entries;
        self.quickfix_index = None;
        self.search.quickfix_page = Some(id);
        self.restale_page(id);
        self.wake_caret();
    }

    /// `SPC p n`/`N` stepped to quickfix entry `index`: the page lights it.
    pub(super) fn search_quickfix_moved(&mut self, index: usize) {
        let Some(id) = self.search.quickfix_page else { return };
        let Some(p) = self.search_page_mut(id) else { return };
        if let Some(&at) = p.match_rows().get(index) {
            p.set_current(at);
            self.restale_page(id);
        }
    }

    /// Shows a match in the pane beside the page -- as the preview tab,
    /// the keyboard staying on the page unless `focus`.
    fn search_go_to(&mut self, id: BufferId, path: &Path, line: usize, col: usize, focus: bool) {
        let page_pane = self.pane_showing(id).unwrap_or_else(|| self.focused_pane_id());
        let other = self.windows().windows().into_iter().find(|&p| p != page_pane);
        match other {
            Some(pane) => {
                self.windows_mut().focus(pane);
            }
            None => {
                self.windows_mut().focus(page_pane);
                self.split_vertical();
            }
        }
        let from = JumpEntry { buffer: self.focused_buffer_id(), char_idx: self.cursor().char_idx };
        self.open_file_as_preview(path);
        self.jump_to_grep_match(&fenix_project::GrepMatch { path: path.to_path_buf(), line, col, text: String::new() });
        if focus {
            self.record_jump(from);
        } else if let Some(pane) = self.pane_showing(id) {
            self.windows_mut().focus(pane);
        }
        self.wake_caret();
    }

    /// `q`: the page closes; what it had stays for `SPC s .`.
    fn search_close(&mut self, id: BufferId) {
        if let Some(cancel) = self.search.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.search_remember(id);
        if let Some(page) = self.search_page(id) {
            let mut last = page.clone();
            last.editing = None;
            last.confirm = false;
            self.search.last = Some(last);
        }
        if self.search.quickfix_page == Some(id) {
            self.search.quickfix_page = None;
        }
        // Its pane goes with it when it has one of its own.
        if let Some(pane) = self.pane_showing(id).filter(|_| self.windows().window_count() > 1) {
            self.windows_mut().focus(pane);
            self.close_page(id);
            if self.windows().content(pane).is_some_and(|b| self.search_page(*b).is_none()) && self.pane_tab_order(pane).len() <= 1 {
                self.close_window();
            }
        } else {
            self.close_page(id);
        }
        self.wake_caret();
    }

    // -- History ----------------------------------------------------------

    fn search_history(&mut self, root: &Path) -> Vec<String> {
        if self.search.history.is_none() {
            let path = self.config.state_dir().join(HISTORY_FILE);
            self.search.history = Some(fenix_storage::state::read(&path, "projects").ok().flatten().unwrap_or_default());
        }
        self.search.history.as_ref().and_then(|h| h.get(root).cloned()).unwrap_or_default()
    }

    /// The page's query goes to the front of its project's history.
    fn search_remember(&mut self, id: BufferId) {
        let Some((root, query)) = self.search_page(id).map(|p| (p.root.clone(), p.query.text.clone())) else { return };
        if query.trim().is_empty() {
            return;
        }
        let mut list = self.search_history(&root);
        list.retain(|q| q != &query);
        list.insert(0, query);
        list.truncate(HISTORY);
        if let Some(p) = self.search_page_mut(id) {
            p.history = list.clone();
        }
        let history = self.search.history.get_or_insert_with(HashMap::new);
        history.insert(root, list);
        let path = self.config.state_dir().join(HISTORY_FILE);
        if let Err(err) = fenix_storage::state::write(&path, "projects", history) {
            eprintln!("fenix: couldn't keep the search history: {err}");
        }
    }

    // -- Replace ----------------------------------------------------------

    /// `R`, `y`: every match still in is replaced. A file that changed
    /// since it was searched is left alone and searched again. Open
    /// files change in their buffers (unsaved); the rest are written.
    fn search_apply(&mut self, id: BufferId) {
        let Some(page) = self.search_page(id) else { return };
        let Some(pattern) = page.pattern.clone() else { return };
        let (replacement, preserve) = (page.replace.text.clone(), page.preserve_case);
        let files: Vec<_> = page.files.iter().filter(|f| f.included() > 0).cloned().collect();
        let mut undo = Vec::new();
        let (mut changed, mut count, mut stale, mut unsaved) = (0usize, 0usize, 0usize, 0usize);
        for file in &files {
            let open = self.buffers.id_for_path(&file.path).filter(|&b| self.buffers.get(b).is_some_and(|ob| ob.kind == BufferKind::Text));
            let text = match open.and_then(|b| self.buffers.get(b)) {
                Some(ob) => ob.buffer.text(),
                None => match std::fs::read_to_string(&file.path) {
                    Ok(text) => text,
                    Err(err) => {
                        eprintln!("fenix: replace couldn't read {}: {err}", file.path.display());
                        stale += 1;
                        continue;
                    }
                },
            };
            // Still as it was searched? Each matched line must be the same.
            let lines: Vec<&str> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
            let same = file.lines.iter().filter(|l| !l.matches.is_empty()).all(|l| lines.get(l.number - 1).is_some_and(|now| if l.cut.0 || l.cut.1 { now.contains(&l.text) } else { *now == l.text }));
            if !same {
                stale += 1;
                continue;
            }
            let skip = |at: engine::MatchAt| file.skipped.contains(&at);
            let (after, n) = engine::replace(&text, &pattern, &replacement, preserve, &skip);
            if n == 0 || after == text {
                continue;
            }
            match open {
                Some(b) => {
                    let Some(ob) = self.buffers.get_mut(b) else { continue };
                    let mut scratch = Cursor::at_start();
                    let end = ob.buffer.len_chars();
                    ob.buffer.replace_range(&mut scratch, 0, end, &after);
                    undo.push(UndoFile { path: file.path.clone(), before: text, after, buffer: Some((b, ob.buffer.edit_count())) });
                    unsaved += 1;
                }
                None => {
                    if let Err(err) = std::fs::write(&file.path, &after) {
                        eprintln!("fenix: replace couldn't write {}: {err}", file.path.display());
                        stale += 1;
                        continue;
                    }
                    undo.push(UndoFile { path: file.path.clone(), before: text, after, buffer: None });
                }
            }
            changed += 1;
            count += n;
        }
        if !undo.is_empty() {
            self.search.undo = Some(undo);
        }
        let mut message = format!("Replaced {count} in {changed} file{}", if changed == 1 { "" } else { "s" });
        if unsaved > 0 {
            message.push_str(&format!(" ({unsaved} open, not saved yet)"));
        }
        if stale > 0 {
            message.push_str(&format!(" · {stale} changed since the search, left alone"));
        }
        if changed > 0 {
            message.push_str(" · SPC s u undoes it");
        }
        self.search_run(id);
        if let Some(p) = self.search_page_mut(id) {
            p.message = Some(message.clone());
        }
        self.set_message(message);
    }

    /// Whether a replace can be undone.
    pub(super) fn search_can_undo(&self) -> bool {
        self.search.undo.is_some()
    }

    /// `SPC s u`: the last replace undone -- every file it changed that
    /// hasn't been edited since.
    pub(crate) fn search_undo(&mut self) {
        let Some(undo) = self.search.undo.take() else {
            self.set_error("no replace to undo");
            return;
        };
        let (mut restored, mut kept) = (0usize, 0usize);
        for file in undo {
            match file.buffer {
                Some((b, edits)) if self.buffers.get(b).is_some_and(|ob| ob.buffer.edit_count() == edits && ob.buffer.text() == file.after) => {
                    let Some(ob) = self.buffers.get_mut(b) else { continue };
                    let mut scratch = Cursor::at_start();
                    let end = ob.buffer.len_chars();
                    ob.buffer.replace_range(&mut scratch, 0, end, &file.before);
                    restored += 1;
                }
                Some(_) => kept += 1,
                None => match std::fs::read_to_string(&file.path) {
                    Ok(now) if now == file.after => match std::fs::write(&file.path, &file.before) {
                        Ok(()) => restored += 1,
                        Err(_) => kept += 1,
                    },
                    _ => kept += 1,
                },
            }
        }
        let mut message = format!("Put back {restored} file{}", if restored == 1 { "" } else { "s" });
        if kept > 0 {
            message.push_str(&format!(" · {kept} edited since, left as they are"));
        }
        if let Some(id) = self.search_page_id() {
            self.search_run(id);
            if let Some(p) = self.search_page_mut(id) {
                p.message = Some(message.clone());
            }
        }
        self.set_message(message);
        self.wake_caret();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::Key;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            use std::sync::atomic::AtomicU64;
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("fenix-gui-test-{name}-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
        fn write(&self, name: &str, contents: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, contents).unwrap();
            path
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn app_in(dir: &TempDir) -> App {
        let mut app = App::with_file(None);
        app.project_root = Some(dir.path().to_path_buf());
        app
    }

    fn type_into(app: &mut App, id: BufferId, text: &str) {
        for c in text.chars() {
            let action = app.search_page_mut(id).unwrap().key(Key::Char(c));
            app.search_action(id, action);
        }
    }

    fn press(app: &mut App, id: BufferId, key: Key) {
        let action = app.search_page_mut(id).unwrap().key(key);
        app.search_action(id, action);
    }

    #[test]
    fn typing_searches_after_a_pause_and_results_stream_in() {
        let dir = TempDir::new("search_host_typing");
        dir.write("a.rs", "fn helper() {}\nhelper();\n");
        dir.write("b.md", "no match here\n");
        let mut app = app_in(&dir);
        app.search_open(SearchOpen::Project);
        let id = app.search_page_id().expect("the page opened");
        assert!(app.search_page(id).unwrap().typing(), "the field has the keyboard");
        type_into(&mut app, id, "helper");
        assert!(app.search.due.is_some(), "waiting for a pause in the typing");
        app.search_fire_due(Instant::now() + DEBOUNCE * 2);
        let page = app.search_page(id).unwrap();
        assert_eq!(page.files.len(), 1);
        assert_eq!(page.total(), 2);
        assert!(matches!(page.status, Status::Done { .. }));
        assert_eq!(app.quickfix.len(), 2, "the matches are the quickfix list");
    }

    /// `SPC s s` searches the file you're in, on the page, without
    /// Replace; a buffer that isn't a file gets the line picker.
    #[test]
    fn spc_s_s_searches_this_file_and_a_buffer_of_no_file_picks_a_line() {
        let dir = TempDir::new("search_host_in_file");
        let a = dir.write("a.rs", "fn helper() {}\nhelper();\n");
        dir.write("b.rs", "helper();\n");
        let mut app = app_in(&dir);
        app.open_startup_file(&a);
        app.search_open(SearchOpen::InFile);
        let id = app.search_page_id().expect("the page opened");
        let page = app.search_page(id).unwrap();
        assert_eq!(page.scope, Some(ScopeKind::ThisFile));
        assert!(page.typing() && !page.replace_open);
        type_into(&mut app, id, "helper");
        app.search_fire_due(Instant::now() + DEBOUNCE * 2);
        let page = app.search_page(id).unwrap();
        assert_eq!((page.files.len(), page.total()), (1, 2), "a.rs only");

        let mut app = App::with_file(None);
        app.search_open(SearchOpen::InFile);
        assert!(app.search_page_id().is_none());
        assert!(matches!(app.active_picker, Some(ActivePicker::BufferSearch(_))));
    }

    #[test]
    fn a_replace_changes_what_is_in_and_one_undo_puts_it_back() {
        let dir = TempDir::new("search_host_replace");
        let a = dir.write("a.txt", "helper one\nhelper two\n");
        let b = dir.write("b.txt", "helper three\n");
        let mut app = app_in(&dir);
        app.search_open(SearchOpen::Replace);
        let id = app.search_page_id().unwrap();
        type_into(&mut app, id, "helper");
        press(&mut app, id, Key::Tab);
        type_into(&mut app, id, "assist");
        press(&mut app, id, Key::Enter);
        // Leave out the second line of a.txt. (Not following, so the
        // files stay closed and are written to the disk.)
        let p = app.search_page_mut(id).unwrap();
        p.following = false;
        p.cursor = p.rows().iter().position(|r| *r == crate::search_page::Row::Line(0, 1)).unwrap();
        press(&mut app, id, Key::Char('x'));
        press(&mut app, id, Key::Char('R'));
        assert!(app.search_page(id).unwrap().confirm);
        press(&mut app, id, Key::Char('y'));
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "assist one\nhelper two\n");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "assist three\n");
        assert!(app.search_page(id).unwrap().message.as_deref().unwrap().contains("Replaced 2 in 2 files"));
        app.search_undo();
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "helper one\nhelper two\n");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "helper three\n");
    }

    #[test]
    fn a_file_changed_since_the_search_is_left_alone() {
        let dir = TempDir::new("search_host_stale");
        let a = dir.write("a.txt", "helper\n");
        let mut app = app_in(&dir);
        app.search_open(SearchOpen::Replace);
        let id = app.search_page_id().unwrap();
        type_into(&mut app, id, "helper");
        press(&mut app, id, Key::Enter);
        std::fs::write(&a, "someone else's helper\n").unwrap();
        press(&mut app, id, Key::Char('R'));
        press(&mut app, id, Key::Char('y'));
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "someone else's helper\n");
        assert!(app.search_page(id).unwrap().message.as_deref().unwrap().contains("changed since the search"));
    }

    #[test]
    fn a_bad_regex_keeps_the_last_results_and_says_why() {
        let dir = TempDir::new("search_host_bad");
        dir.write("a.txt", "a(b\n");
        let mut app = app_in(&dir);
        app.search_open(SearchOpen::Project);
        let id = app.search_page_id().unwrap();
        type_into(&mut app, id, "a");
        press(&mut app, id, Key::Enter);
        assert_eq!(app.search_page(id).unwrap().total(), 1);
        app.search_page_special_key(id, KeyPress { code: KeyCode::Char('r'), mods: Mods { alt: true, ..Mods::default() } });
        press(&mut app, id, Key::Char('/'));
        type_into(&mut app, id, "(");
        press(&mut app, id, Key::Enter);
        let page = app.search_page(id).unwrap();
        assert!(page.error.as_deref().is_some_and(|e| !e.is_empty()), "{:?}", page.error);
        assert_eq!(page.total(), 1, "the last good results stay");
    }

    #[test]
    fn closing_keeps_it_for_the_last_search() {
        let dir = TempDir::new("search_host_last");
        dir.write("a.txt", "needle\n");
        let mut app = app_in(&dir);
        app.search_open(SearchOpen::Project);
        let id = app.search_page_id().unwrap();
        type_into(&mut app, id, "needle");
        press(&mut app, id, Key::Enter);
        press(&mut app, id, Key::Char('q'));
        assert!(app.search_page_id().is_none());
        app.search_open(SearchOpen::Last);
        let id = app.search_page_id().unwrap();
        assert_eq!(app.search_page(id).unwrap().query.text, "needle");
        assert_eq!(app.search_page(id).unwrap().total(), 1, "searched again");
    }
}
