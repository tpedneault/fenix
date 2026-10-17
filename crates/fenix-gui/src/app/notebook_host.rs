//! The host half of the notebook (`notebook_page`, `SPC n`): the
//! `fenix_notebook::Notebook` opened lazily from `notebook.folder` (or
//! the data folder), the page's rows, what its keys ask for, new notes
//! from a template and a name, journal days, and autosave -- a notebook
//! file is written a second after you stop typing, keeping the version
//! before each editing session in its history, so there's no `:w` to
//! forget for a file you never chose a place for.

use super::pages::PageModel;
use super::*;
use crate::notebook_page::{Action as NbAction, NotebookPage, Row as NbRow};
use crate::page::ImageKey;
use crate::reading;
use fenix_notebook::{Kind as NbKind, Notebook};

/// How long typing has to pause before a notebook file is written.
const AUTOSAVE_AFTER: Duration = Duration::from_millis(1000);

/// What a notebook picker's choice does.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NotebookPick {
    /// Open an entry.
    Open(String),
    /// A note from this template text.
    Template { name: String, body: String },
    /// Type `[[name]]` at the cursor.
    InsertLink(String),
    /// A search hit: an entry, at a line.
    Hit { id: String, line: Option<usize> },
    /// The notebook page, with this filter.
    Filter(String),
    /// Export `what` this way.
    Export { what: super::notebook_export::ExportWhat, how: super::notebook_export::ExportHow },
    /// A notebook diagram into the buffer: its block, or an SVG and a link.
    InsertDiagram { id: String, svg: bool },
}

/// What the notebook offers other features.
static SERVICES: [super::feature::Service; 1] = [
    // `{text}` -> asks the new diagram's name, then keeps it in the
    // notebook and opens it (the diagrams feature's new-diagram flow).
    super::feature::Service {
        name: "notebook.new_diagram",
        call: super::feature::ServiceCall::Mut(|app, payload| {
            #[derive(serde::Deserialize)]
            struct NewDiagram {
                text: String,
            }
            let call: NewDiagram = serde_json::from_str(payload).map_err(|e| format!("a notebook call: {e}"))?;
            app.notebook_name_new_diagram(call.text).map(|()| String::new())
        }),
    },
];

/// What the notebook's modeline prompt is typing.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum NotebookPromptKind {
    /// The new note's name, then it's made from `body`.
    NoteName { template: String, body: String },
    /// The new diagram's name, then it's made from `text`.
    DiagramName { text: String },
    /// A line to capture, where it goes, whether it's a task, and the
    /// link back to where it was captured from.
    Capture { to: super::notebook_capture::CaptureTo, task: bool, link: Option<String> },
    /// A notebook search.
    Search,
    /// Where a file export goes.
    ExportPath { what: super::notebook_export::ExportWhat, how: super::notebook_export::ExportHow },
    /// The name a ```mermaid block is saved to the notebook under.
    SaveBlock { text: String },
    /// A file to copy into the notebook.
    Import,
}

#[derive(Debug, Clone)]
pub(super) struct NotebookPrompt {
    pub(super) kind: NotebookPromptKind,
    pub(super) input: String,
}

/// The notebook's side of `App`.
#[derive(Default)]
pub(super) struct NotebookState {
    pub(super) book: Option<Notebook>,
    /// Why it couldn't be opened, once said.
    pub(super) error: Option<String>,
    /// Per notebook buffer: its edit count when it last changed, and when.
    pub(super) edits: HashMap<BufferId, (u64, Instant)>,
    pub(super) prompt: Option<NotebookPrompt>,
    /// The pane the backlinks sidebar's note is in.
    pub(super) sidebar_origin: Option<fenix_window::WindowId>,
    /// The notebook is being read off the UI thread (for Home).
    pub(super) opening: bool,
}

/// The notebook's commands, its `SPC n` group and `SPC i D`, and its
/// entries in the Markdown `SPC m` menu (the reading view's own are core).
pub(crate) static SPEC: super::feature::FeatureSpec = {
    use super::feature::FeatureCommand as C;
    use super::feature::LeaderEntry::{Bind, Group};
    super::feature::FeatureSpec {
        id: "fenix.notebook",
        commands: &[
            C { id: "notebook.open", description: "The notebook: every note, journal day and diagram", external: true, run: |app| app.open_notebook_page() },
            C { id: "notebook.new_note", description: "A new note, from a template", external: true, run: |app| app.notebook_new_note() },
            C { id: "notebook.new_diagram", description: "A new Mermaid diagram", external: true, run: |app| app.notebook_new_diagram() },
            C { id: "notebook.find", description: "Find a note or diagram by name", external: true, run: |app| app.notebook_find() },
            C { id: "notebook.recent", description: "Reopen the note or diagram you touched last", external: true, run: |app| app.notebook_reopen() },
            C { id: "notebook.journal", description: "Today's journal day", external: true, run: |app| app.notebook_today() },
            C { id: "notebook.capture", description: "Capture a line into the journal or the Inbox", external: false, run: |app| app.notebook_capture() },
            C { id: "notebook.search", description: "Search every note and diagram", external: true, run: |app| app.notebook_search() },
            C { id: "notebook.tags", description: "Every tag in the notebook", external: true, run: |app| app.notebook_tags() },
            C { id: "notebook.project", description: "The notes about this project", external: true, run: |app| app.notebook_project_notes() },
            C { id: "notebook.to_agenda", description: "Send the checkbox under the cursor to the agenda", external: false, run: |app| app.notebook_to_agenda() },
            C { id: "notebook.journal_agenda", description: "Refresh a journal day's agenda list", external: false, run: |app| app.journal_refresh_agenda() },
            C { id: "notebook.journal_commits", description: "Add today's commits to a journal day", external: false, run: |app| app.journal_commits() },
            C { id: "notebook.export", description: "Export this diagram or note", external: false, run: |app| app.diagram_export_here() },
            C { id: "notebook.save_block", description: "Save the ```mermaid block here to the notebook", external: false, run: |app| app.notebook_save_block() },
            C { id: "notebook.import", description: "Copy a .md or .mmd file into the notebook", external: false, run: |app| app.notebook_import() },
            C { id: "insert.diagram", description: "Insert a notebook diagram: its block, or an SVG and a link", external: false, run: |app| app.notebook_insert_diagram() },
            C { id: "notebook.backlinks", description: "What links to this note, beside it", external: false, run: |app| app.notebook_backlinks() },
            C { id: "notebook.insert_link", description: "Insert a link to a note", external: false, run: |app| app.notebook_insert_link() },
            C { id: "notebook.copy_link", description: "Copy a link to this line, for a note", external: false, run: |app| app.notebook_copy_link() },
            C { id: "notebook.paste_image", description: "Paste the clipboard's picture into the note", external: false, run: |app| app.notebook_paste_image() },
        ],
        leader: &[
            Bind(&['i', 'D'], "a notebook diagram", "insert.diagram"),
            Group(&['n'], "notebook"),
            Bind(&['n', 'n'], "notebook page", "notebook.open"),
            Bind(&['n', 'N'], "new note", "notebook.new_note"),
            Bind(&['n', 'd'], "new diagram", "notebook.new_diagram"),
            Bind(&['n', 'f'], "find by name", "notebook.find"),
            Bind(&['n', 'r'], "reopen the last one", "notebook.recent"),
            Bind(&['n', 'j'], "today's journal", "notebook.journal"),
            Bind(&['n', 'b'], "links here (sidebar)", "notebook.backlinks"),
            Bind(&['n', 'c'], "capture a line", "notebook.capture"),
            Bind(&['n', '/'], "search the notebook", "notebook.search"),
            Bind(&['n', '#'], "tags", "notebook.tags"),
            Bind(&['n', 'p'], "this project's notes", "notebook.project"),
            Bind(&['n', 't'], "checkbox to the agenda", "notebook.to_agenda"),
            Bind(&['n', 'e'], "export", "notebook.export"),
            Bind(&['n', 's'], "save the ```mermaid block", "notebook.save_block"),
            Bind(&['n', 'i'], "import a file", "notebook.import"),
            Bind(&['n', 'l'], "insert a link", "notebook.insert_link"),
            Bind(&['n', 'y'], "copy a link to this line", "notebook.copy_link"),
            Bind(&['n', 'v'], "paste a picture", "notebook.paste_image"),
        ],
        local_menu: &[
            Bind(&['b'], "links here", "notebook.backlinks"),
            Bind(&['a'], "journal: refresh the agenda list", "notebook.journal_agenda"),
            Bind(&['g'], "journal: add today's commits", "notebook.journal_commits"),
        ],
        // The Markdown menu is the reading view's (core); the notebook adds to it.
        local_context: None,
        language_server: None,
        tasks: None,
        hover_fallback: None,
        completion_at_cursor: None,
        completion_pool_at: None,
        hover_first: None,
        picker_confirm: Some(|app, pick, _marked, query| {
            if let Some(pick) = pick.downcast_ref::<NotebookPick>() {
                app.notebook_picked(pick.clone(), query.to_string());
            }
        }),
        picker_cancel: None,
        modeline_clock: None,
        // What a key on the notebook page or the links sidebar asked for.
        page_action: Some(|app, id, action| {
            let action = match action.downcast::<NbAction>() {
                Ok(action) => return app.notebook_action(id, *action),
                Err(action) => action,
            };
            if let Ok(action) = action.downcast::<crate::backlinks_page::Action>() {
                app.backlinks_action(id, *action);
            }
        }),
        // The notebook page is laid out with the reading layout, which
        // borrows the app (its pictures, embeds and links): it's laid out
        // here, and handed over whole.
        page_context: Some(|app, id, need| match need {
            super::feature::PageNeed::Layout { cols, due: true } => Box::new(app.notebook_page_layout(id, cols)),
            _ => Box::new(()),
        }),
        page_title: None,
        page_after_layout: None,
        on_event: None,
        settings: &[],
        services: &SERVICES,
    }
};

/// The notebook's state. Its code is still `App` methods (its pages are
/// core `PageModel`s), so it reaches this by type (`Features::
/// notebook_state`); every entry point is a spec function, which runs
/// with it in place, and it has no instance hooks that run code.
#[derive(Default)]
pub(crate) struct NotebookHost {
    pub(super) state: NotebookState,
}

impl super::feature::Feature for NotebookHost {
    fn spec(&self) -> &'static super::feature::FeatureSpec {
        &SPEC
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl super::feature::Features {
    pub(super) fn notebook_state(&self) -> &NotebookState {
        &self.get::<NotebookHost>().expect("the notebook has no instance hooks that run code, so it's always in place").state
    }

    pub(super) fn notebook_state_mut(&mut self) -> &mut NotebookState {
        &mut self.get_mut::<NotebookHost>().expect("the notebook has no instance hooks that run code, so it's always in place").state
    }
}

/// The notebook page (`SPC n n`).
impl super::feature::FeaturePage for NotebookPage {
    fn feature(&self) -> &'static str {
        SPEC.id
    }

    fn title(&self) -> String {
        "*notebook*".to_string()
    }

    fn typing(&self) -> bool {
        NotebookPage::typing(self)
    }

    fn paste(&mut self, text: &str) {
        NotebookPage::paste(self, text);
    }

    /// Laid out already, by `SPEC.page_context`.
    fn layout(&mut self, ctx: Box<dyn std::any::Any>, _cols: usize) -> crate::page::Page {
        ctx.downcast::<Option<crate::page::Page>>().ok().and_then(|page| *page).unwrap_or_default()
    }

    fn key(&mut self, key: crate::page::Key, _ctx: Box<dyn std::any::Any>) -> Box<dyn std::any::Any> {
        Box::new(NotebookPage::key(self, key))
    }
}

/// The links sidebar (`SPC n b`).
impl super::feature::FeaturePage for crate::backlinks_page::BacklinksPage {
    fn feature(&self) -> &'static str {
        SPEC.id
    }

    fn title(&self) -> String {
        format!("*links: {}*", self.name)
    }

    fn layout(&mut self, _ctx: Box<dyn std::any::Any>, cols: usize) -> crate::page::Page {
        crate::backlinks_page::layout(self, cols)
    }

    fn key(&mut self, key: crate::page::Key, _ctx: Box<dyn std::any::Any>) -> Box<dyn std::any::Any> {
        Box::new(crate::backlinks_page::BacklinksPage::key(self, key))
    }
}

/// A notebook picker over `candidates`, titled `title`.
pub(super) fn notebook_picker(title: &'static str, candidates: Vec<fenix_picker::Candidate<NotebookPick>>) -> ActivePicker {
    let candidates = candidates.into_iter().map(|c| super::feature::FeaturePick::candidate(c.label, c.payload)).collect();
    ActivePicker::Feature { feature: SPEC.id, badge: title, state: fenix_picker::PickerState::new(candidates) }
}

/// A capture name from `fenix-syntax` as one the page roles can hold.
pub(super) fn static_capture(name: &str) -> &'static str {
    match name.split('.').next().unwrap_or(name) {
        "keyword" | "repeat" | "conditional" | "storageclass" | "include" | "tag" => "keyword",
        "string" | "escape" => "string",
        "comment" | "spell" => "comment",
        "function" | "method" => "function",
        "type" | "namespace" | "module" => "type",
        "number" | "float" => "number",
        "constant" | "boolean" => "constant",
        "variable" | "property" | "label" | "field" | "parameter" => "variable",
        "operator" => "operator",
        "punctuation" => "punctuation",
        "attribute" | "constructor" | "preproc" | "embedded" => "attribute",
        _ => "text",
    }
}

/// The grammar a fenced block's info word names.
pub(super) fn language_for_fence(lang: &str) -> Option<fenix_syntax::LanguageId> {
    let ext = match lang {
        "rust" => "rs",
        "python" | "py3" => "py",
        "bash" | "shell" | "sh" | "zsh" | "console" => "sh",
        "javascript" | "node" => "js",
        "typescript" => "ts",
        "c++" | "cxx" | "cc" => "cpp",
        "yml" => "yaml",
        "csharp" | "c#" => "cs",
        "golang" => "go",
        "markdown" => "md",
        "arduino" => "ino",
        other => other,
    };
    fenix_syntax::detect_language(ext)
}

/// A code block's colours, per line: (columns, capture).
pub(super) fn highlight_block(lang: &str, text: &str) -> Vec<Vec<(std::ops::Range<usize>, &'static str)>> {
    let Some(language) = language_for_fence(lang) else { return Vec::new() };
    let state = fenix_syntax::SyntaxState::new(language, text);
    let spans = state.highlights_in_range(text, 0..text.len());
    let mut starts = vec![0usize];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    let mut out: Vec<Vec<(std::ops::Range<usize>, &'static str)>> = vec![Vec::new(); starts.len()];
    for (range, name) in spans {
        let line = match starts.binary_search(&range.start) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let line_start = starts[line];
        let line_end = starts.get(line + 1).map(|e| e - 1).unwrap_or(text.len());
        let end = range.end.min(line_end);
        if end <= range.start {
            continue;
        }
        let a = text[line_start..range.start].chars().count();
        let b = a + text[range.start..end].chars().count();
        out[line].push((a..b, static_capture(name)));
    }
    out
}

impl App {
    /// The notebook's folder: the setting, else Fenix's data folder.
    pub(super) fn notebook_root(&self) -> PathBuf {
        if let Some(folder) = &self.config.notebook_folder {
            return folder.clone();
        }
        fenix_storage::paths::notebook_dir().unwrap_or_else(|| env::temp_dir().join("fenix-notebook"))
    }

    /// The notebook, opened the first time it's needed.
    pub(super) fn notebook(&mut self) -> Option<&mut Notebook> {
        if self.features.notebook_state().book.is_none() {
            let root = self.notebook_root();
            match Notebook::open(&root) {
                Ok(mut nb) => {
                    nb.history_limit = self.config.notebook_history.unwrap_or(50);
                    self.features.notebook_state_mut().book = Some(nb);
                    self.features.notebook_state_mut().error = None;
                }
                Err(e) => {
                    let why = format!("couldn't open the notebook at {}: {e}", root.display());
                    self.set_error(why.clone());
                    self.features.notebook_state_mut().error = Some(why);
                    return None;
                }
            }
        }
        self.features.notebook_state_mut().book.as_mut()
    }

    /// Reads the notebook off the UI thread, if its folder exists and it
    /// isn't open: Home lists it without the launch waiting on it.
    pub(super) fn notebook_open_in_background(&mut self) {
        if self.features.notebook_state().book.is_some() || self.features.notebook_state().opening {
            return;
        }
        // Tests never read the notebook of the Fenix you use.
        if cfg!(test) && self.config.notebook_folder.is_none() {
            return;
        }
        let root = self.notebook_root();
        if !root.is_dir() {
            return;
        }
        self.features.notebook_state_mut().opening = true;
        self.page_spawn(move |send| {
            if let Ok(nb) = Notebook::open(&root) {
                send(super::pages::PageEvent::NotebookOpened(Box::new(nb)));
            }
        });
    }

    /// The notebook, read in the background, arrived.
    pub(super) fn apply_notebook_opened(&mut self, mut nb: Notebook) {
        self.features.notebook_state_mut().opening = false;
        if self.features.notebook_state().book.is_none() {
            nb.history_limit = self.config.notebook_history.unwrap_or(50);
            self.features.notebook_state_mut().book = Some(nb);
            self.refresh_home_data(false);
        }
    }

    /// The project a note made now is about: the focused project's name.
    pub(super) fn notebook_project(&self) -> Option<String> {
        self.project_root.as_ref().and_then(|r| r.file_name()).map(|n| n.to_string_lossy().to_string())
    }

    /// The rows the notebook page shows.
    fn notebook_rows(&mut self) -> Vec<NbRow> {
        let project_files = self.config.notebook_project_files.unwrap_or(false);
        let project_root = self.project_root.clone();
        if self.notebook().is_none() {
            return Vec::new();
        }
        let nb = self.features.notebook_state().book.as_ref().expect("opened above");
        let mut rows: Vec<NbRow> = nb
            .entries()
            .iter()
            .map(|e| NbRow {
                id: e.id.clone(),
                kind: e.kind,
                name: e.display_name(),
                tags: e.tags.clone(),
                about: e.about.clone(),
                pinned: e.pinned,
                modified: e.modified,
                date: e.date(),
                text: e.text.clone(),
                open_tasks: e.open_tasks().len(),
                versions: nb.history(&e.id).len(),
                exports: e.exports.iter().map(|x| x.path.display().to_string()).collect(),
                problem: None,
                project_file: None,
            })
            .collect();
        for row in rows.iter_mut().filter(|r| r.kind == NbKind::Diagram) {
            row.problem = self.diagram_problem_of(&row.text);
        }
        if let (true, Some(root)) = (project_files, project_root) {
            let mut files = Vec::new();
            collect_docs(&root, &mut files, 0);
            for path in files {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let kind = if path.extension().is_some_and(|e| e == "md") { NbKind::Note } else { NbKind::Diagram };
                rows.push(NbRow {
                    id: format!("file:{}", path.display()),
                    kind,
                    name: path.strip_prefix(&root).unwrap_or(&path).display().to_string().replace('\\', "/"),
                    tags: if kind == NbKind::Note { fenix_notebook::meta::tags(&text) } else { Vec::new() },
                    about: None,
                    pinned: false,
                    modified: std::fs::metadata(&path).and_then(|m| m.modified()).ok(),
                    date: None,
                    open_tasks: fenix_notebook::meta::tasks(&text).iter().filter(|t| !t.done).count(),
                    text,
                    versions: 0,
                    exports: Vec::new(),
                    problem: None,
                    project_file: Some(path),
                });
            }
        }
        rows
    }

    fn notebook_page_mut(&mut self, id: BufferId) -> Option<&mut NotebookPage> {
        self.feature_page_mut::<NotebookPage>(id)
    }

    /// Re-reads what every open notebook page shows.
    pub(super) fn refresh_notebook_pages(&mut self) {
        let Some(id) = self.find_page(|m| super::feature::page_as::<NotebookPage>(m).is_some()) else { return };
        let rows = self.notebook_rows();
        let (tags, searches) = match self.features.notebook_state().book.as_ref() {
            Some(nb) => (nb.tags().into_iter().map(|(t, _)| t).collect(), nb.searches().to_vec()),
            None => (Vec::new(), Vec::new()),
        };
        if let Some(page) = self.notebook_page_mut(id) {
            page.refresh(rows, tags, searches);
        }
    }

    /// `SPC n n`: the notebook page.
    pub(crate) fn open_notebook_page(&mut self) {
        if self.notebook().is_none() {
            return;
        }
        if let Some(nb) = self.features.notebook_state_mut().book.as_mut() {
            let _ = nb.rescan();
        }
        match self.find_page(|m| super::feature::page_as::<NotebookPage>(m).is_some()) {
            Some(id) => {
                self.show_page(id);
                self.refresh_notebook_pages();
            }
            None => {
                let rows = self.notebook_rows();
                let nb = self.features.notebook_state().book.as_ref().expect("opened above");
                let tags = nb.tags().into_iter().map(|(t, _)| t).collect();
                let searches = nb.searches().to_vec();
                let folder = nb.root().display().to_string();
                self.open_page(PageModel::Feature(Box::new(NotebookPage::new(rows, tags, searches, folder))));
            }
        }
    }

    /// The picture a page shows for a diagram's source: the diagrams
    /// feature's (`diagrams.picture`); none to show without it.
    fn diagram_picture(&self, source: &str) -> u64 {
        self.query_service("diagrams.picture", &serde_json::json!({ "source": source })).ok().and_then(|key| key.parse().ok()).unwrap_or(0)
    }

    /// What's wrong with a diagram, as the diagrams feature drew it
    /// (`diagrams.problem`).
    fn diagram_problem_of(&self, source: &str) -> Option<String> {
        self.query_service("diagrams.problem", &serde_json::json!({ "source": source })).ok().filter(|p| !p.is_empty())
    }

    /// What the reading layout needs, borrowing only what it reads.
    pub(super) fn with_reading_ctx<R>(&self, base: Option<&Path>, f: impl FnOnce(&reading::Ctx) -> R) -> R {
        let nb = self.features.notebook_state().book.as_ref();
        let sizes = &self.page_images.image_sizes;
        let (cw, lh) = match &self.text {
            Some(t) => (t.char_width(), t.line_height()),
            None => (text::CHAR_WIDTH, text::LINE_HEIGHT),
        };
        let base_dir = base.and_then(Path::parent).map(Path::to_path_buf).or_else(|| nb.map(|n| n.root().to_path_buf()));
        let base_file = base.map(Path::to_path_buf);
        let highlight = |lang: &str, text: &str| highlight_block(lang, text);
        let image_size = |key: &ImageKey| sizes.get(key).copied();
        let diagram_key = |source: &str| ImageKey::Diagram(self.diagram_picture(source));
        let diagram_error = |source: &str| self.diagram_problem_of(source);
        let image_path = |src: &str| {
            if src.contains("://") {
                return None;
            }
            let decoded = src.replace("%20", " ");
            let p = PathBuf::from(&decoded);
            let path = if p.is_absolute() { p } else { base_dir.clone().unwrap_or_default().join(p) };
            path.exists().then_some(path)
        };
        let embed = |link: &fenix_notebook::Link| match nb {
            Some(nb) => match nb.resolve(link, base_file.as_deref()) {
                fenix_notebook::Target::Entry { id, .. } => match nb.get(&id) {
                    Some(e) if e.kind == NbKind::Diagram => reading::Embedded::Diagram { name: e.name.clone(), source: e.text.clone() },
                    Some(e) => reading::Embedded::Note { name: e.display_name(), text: e.text.clone() },
                    None => reading::Embedded::Missing(link.target.clone()),
                },
                _ => reading::Embedded::Missing(link.target.clone()),
            },
            None => reading::Embedded::Missing(link.target.clone()),
        };
        let link_exists = |link: &fenix_notebook::Link| match nb {
            Some(nb) => !matches!(nb.resolve(link, base_file.as_deref()), fenix_notebook::Target::Missing(_)),
            None => true,
        };
        let ctx = reading::Ctx {
            highlight: &highlight,
            image_size: &image_size,
            diagram_key: &diagram_key,
            diagram_error: &diagram_error,
            image_path: &image_path,
            embed: &embed,
            link_exists: &link_exists,
            cell: (cw, lh),
        };
        f(&ctx)
    }

    /// The notebook page's layout for a pane `cols` wide.
    pub(super) fn notebook_page_layout(&self, id: BufferId, cols: usize) -> Option<crate::page::Page> {
        let p = self.feature_page::<NotebookPage>(id)?;
        // The selected entry's links and pictures are relative to its file.
        let base = p.selected().and_then(|r| r.project_file.clone().or_else(|| self.features.notebook_state().book.as_ref().and_then(|nb| nb.get(&r.id)).map(|e| e.path.clone())));
        Some(self.with_reading_ctx(base.as_deref(), |ctx| crate::notebook_page::layout(p, cols, ctx)))
    }

    /// Opens an entry for editing in the focused pane.
    pub(super) fn open_notebook_entry(&mut self, id: &str) {
        let Some(path) = self.features.notebook_state().book.as_ref().and_then(|nb| nb.get(id)).map(|e| e.path.clone()) else {
            self.set_error("that entry isn't in the notebook any more");
            return;
        };
        self.open_file_from_picker(&path);
        if path.extension().is_some_and(|e| e == "mmd" || e == "mermaid") {
            self.open_diagram_preview(self.focused_buffer_id());
        }
    }

    /// Puts the cursor at (line, col) in the focused buffer.
    pub(super) fn notebook_place_cursor(&mut self, line: usize, col: usize) {
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        let line = line.min(buffer.visual_line_count().saturating_sub(1));
        let start = buffer.line_start_char(line);
        cursor.char_idx = start + col.min(buffer.line_len(line));
        let (_, sticky) = buffer.line_col(cursor);
        cursor.sticky_col = sticky;
    }

    // ------------------------------------------------------------------
    // New notes
    // ------------------------------------------------------------------

    /// `SPC n N`: pick a template, then name the note.
    pub(crate) fn notebook_new_note(&mut self) {
        if self.notebook().is_none() {
            return;
        }
        let mut candidates: Vec<fenix_picker::Candidate<NotebookPick>> = fenix_notebook::templates::BUILT_IN
            .iter()
            .map(|t| fenix_picker::Candidate::new(format!("{:<18} {}", t.name, t.about), NotebookPick::Template { name: t.name.to_string(), body: t.body.to_string() }))
            .collect();
        // Yours: Markdown snippets whose trigger starts with `note-`.
        let catalog = self.snippet_layers(self.project_root.as_ref().map(|r| r.join(".fenix").join("snippets")).as_deref());
        for s in &catalog.snippets {
            if s.trigger.starts_with("note-") && s.scopes.iter().any(|x| x == "markdown" || x == "md") {
                let body = s.template.render(&Default::default(), &fenix_snippets::Context::current(None), "").text;
                candidates.push(fenix_picker::Candidate::new(format!("{:<18} yours ({})", s.name, s.trigger), NotebookPick::Template { name: s.name.clone(), body }));
            }
        }
        self.completion = None;
        self.enter_picker(super::notebook_host::notebook_picker("TEMPLATE", candidates));
    }

    /// A notebook picker's choice.
    pub(super) fn notebook_picked(&mut self, choice: NotebookPick, _query: String) {
        match choice {
            NotebookPick::Open(id) => self.open_notebook_entry(&id),
            NotebookPick::Template { name, body } => {
                self.features.notebook_state_mut().prompt = Some(NotebookPrompt { kind: NotebookPromptKind::NoteName { template: name, body }, input: String::new() });
                self.wake_caret();
            }
            NotebookPick::InsertLink(name) => self.notebook_insert_text(&format!("[[{name}]]")),
            NotebookPick::Hit { id, line } => self.notebook_open_at(&id, line),
            NotebookPick::Filter(filter) => self.notebook_page_filtered(&filter),
            NotebookPick::Export { what, how } => self.export_picked(what, how),
            NotebookPick::InsertDiagram { id, svg } => self.insert_diagram_picked(&id, svg),
        }
    }

    pub(super) fn notebook_prompt_text(&self) -> Option<String> {
        let p = self.features.notebook_state().prompt.as_ref()?;
        Some(match &p.kind {
            NotebookPromptKind::NoteName { template, .. } => format!("New note ({template}) -- name: {}▏", p.input),
            NotebookPromptKind::DiagramName { .. } => format!("New diagram -- name: {}▏", p.input),
            NotebookPromptKind::Capture { to, task, link } => {
                let what = if *task { "task" } else { "note" };
                let link = link.as_deref().map(|l| format!(" · links {l} (C-l drops)")).unwrap_or_default();
                format!("Capture a {what} → {} (Tab changes, C-t task){link}: {}▏", to.label(), p.input)
            }
            NotebookPromptKind::Search => format!("Search the notebook (words, tag:, type:, project:, is:todo): {}▏", p.input),
            NotebookPromptKind::ExportPath { .. } => format!("Export to: {}▏", p.input),
            NotebookPromptKind::SaveBlock { .. } => format!("Save the diagram to the notebook as: {}▏", p.input),
            NotebookPromptKind::Import => format!("Import (a .md or .mmd file): {}▏", p.input),
        })
    }

    pub(super) fn notebook_prompt_key(&mut self, keypress: KeyPress) {
        if keypress == KeyPress::char('v').with_ctrl() {
            if let (Some(text), true) = (self.clipboard_text(), self.features.notebook_state().prompt.is_some()) {
                let text: String = text.chars().filter(|c| !c.is_control()).collect();
                if let Some(p) = &mut self.features.notebook_state_mut().prompt {
                    p.input.push_str(&text);
                }
            }
            self.wake_caret();
            return;
        }
        if self.notebook_capture_key(keypress) {
            self.wake_caret();
            return;
        }
        let Some(prompt) = &mut self.features.notebook_state_mut().prompt else { return };
        match keypress.code {
            KeyCode::Named(FenixNamedKey::Escape) => self.features.notebook_state_mut().prompt = None,
            KeyCode::Named(FenixNamedKey::Enter) => {
                let NotebookPrompt { kind, input } = self.features.notebook_state_mut().prompt.take().unwrap();
                self.notebook_prompt_done(kind, input);
            }
            KeyCode::Named(FenixNamedKey::Backspace) => {
                prompt.input.pop();
            }
            KeyCode::Char(c) if !keypress.mods.ctrl => prompt.input.push(c),
            _ => {}
        }
        self.wake_caret();
    }

    fn notebook_prompt_done(&mut self, kind: NotebookPromptKind, input: String) {
        let name = input.trim().to_string();
        match kind {
            NotebookPromptKind::NoteName { body, template } => {
                if name.is_empty() {
                    self.features.notebook_state_mut().prompt = Some(NotebookPrompt { kind: NotebookPromptKind::NoteName { template, body }, input });
                    self.set_error("a note needs a name");
                    return;
                }
                self.notebook_create_note(&name, &body);
            }
            NotebookPromptKind::DiagramName { text } => {
                if name.is_empty() {
                    self.features.notebook_state_mut().prompt = Some(NotebookPrompt { kind: NotebookPromptKind::DiagramName { text }, input });
                    self.set_error("a diagram needs a name");
                    return;
                }
                self.notebook_create_diagram(&name, &text);
            }
            NotebookPromptKind::Capture { to, task, link } => self.notebook_capture_done(to, task, link, &input),
            NotebookPromptKind::Search => {
                if !name.is_empty() {
                    self.notebook_search_results(&name);
                }
            }
            NotebookPromptKind::ExportPath { what, how } => {
                if !name.is_empty() {
                    self.export_to_path(what, how, &name);
                }
            }
            NotebookPromptKind::SaveBlock { text } => {
                if !name.is_empty() {
                    self.notebook_create_diagram(&name, &text);
                }
            }
            NotebookPromptKind::Import => {
                if !name.is_empty() {
                    self.notebook_import_path(&name);
                }
            }
        }
    }

    /// Makes a note called `name` from template `body` and opens it.
    pub(super) fn notebook_create_note(&mut self, name: &str, body: &str) {
        let project = self.notebook_project();
        let Some(nb) = self.notebook() else { return };
        if let Some(existing) = nb.by_name(name) {
            let id = existing.id.clone();
            self.set_message(format!("{name} is in the notebook already -- opened it"));
            self.open_notebook_entry(&id);
            return;
        }
        let fill = fenix_notebook::templates::Fill { title: name.to_string(), project, now: None };
        let (text, (line, col)) = fenix_notebook::templates::render(body, &fill);
        match nb.create(NbKind::Note, name, &text) {
            Ok(id) => {
                self.open_notebook_entry(&id);
                self.notebook_place_cursor(line, col);
                self.refresh_notebook_pages();
                self.set_message(format!("{name} -- in your notebook; it saves itself as you type"));
            }
            Err(e) => self.set_error(format!("couldn't make the note: {e}")),
        }
    }

    // ------------------------------------------------------------------
    // Journal days
    // ------------------------------------------------------------------

    /// The text a new journal day starts with: the agenda's tasks due by
    /// then, and the last day's open checkboxes.
    pub(super) fn journal_template(&self, date: chrono::NaiveDate) -> String {
        let mut fill = fenix_notebook::templates::DayFill::default();
        for t in &self.features.agenda().store.tasks {
            if t.status == fenix_agenda::Status::Done || t.archived {
                continue;
            }
            if let Some(due) = t.due {
                if due <= date {
                    let key = t.jira_key().map(|k| format!("{k} ")).unwrap_or_default();
                    let when = if due == date { "due today".to_string() } else { format!("overdue since {}", due.format("%b %-d")) };
                    fill.agenda.push(format!("{key}{} · {when}", t.title));
                }
            }
        }
        if let Some(nb) = self.features.notebook_state().book.as_ref() {
            let before = nb.entries().iter().filter(|e| e.date().is_some_and(|d| d < date)).max_by_key(|e| e.date());
            if let Some(prev) = before {
                fill.carried = prev.open_tasks().into_iter().map(|t| t.text).filter(|t| !t.is_empty() && !fill.agenda.iter().any(|a| a.contains(t.as_str()))).collect();
            }
        }
        fenix_notebook::templates::journal(date, &fill)
    }

    /// Opens (making it if need be) the journal day `date`.
    pub(crate) fn open_journal_day(&mut self, date: chrono::NaiveDate) {
        if self.notebook().is_none() {
            return;
        }
        let template = self.journal_template(date);
        let Some(nb) = self.notebook() else { return };
        match nb.ensure_day(date, || template) {
            Ok((id, made)) => {
                self.open_notebook_entry(&id);
                let lines = self.open().buffer.line_count();
                self.notebook_place_cursor(lines.saturating_sub(1), 0);
                self.refresh_notebook_pages();
                if made {
                    self.set_message(format!("{} -- a new day in your journal", date.format("%A %-d %B")));
                }
            }
            Err(e) => self.set_error(format!("couldn't open the journal: {e}")),
        }
    }

    // ------------------------------------------------------------------
    // The page's actions
    // ------------------------------------------------------------------

    pub(super) fn notebook_action(&mut self, page_id: BufferId, action: NbAction) {
        let note = |app: &mut App, text: String, bad: bool| {
            if let Some(p) = app.notebook_page_mut(page_id) {
                p.note = Some((text, bad));
            }
        };
        match action {
            NbAction::None => {}
            NbAction::Close => self.close_page(page_id),
            NbAction::Edit(id) => self.open_notebook_entry(&id),
            NbAction::Read(id) => {
                self.open_notebook_entry(&id);
                self.open_reading_view(true);
            }
            NbAction::OpenFile(path) => self.open_file_from_picker(&path),
            NbAction::NewNote => self.notebook_new_note(),
            NbAction::NewDiagram => self.notebook_new_diagram(),
            NbAction::Import => self.notebook_import(),
            NbAction::Day(date) => self.open_journal_day(date),
            NbAction::PinSearch(query) => {
                if let Some(nb) = self.notebook() {
                    match nb.toggle_search(&query) {
                        Ok(true) => note(self, format!("pinned /{query} as a chip"), false),
                        Ok(false) => note(self, format!("unpinned /{query}"), false),
                        Err(e) => note(self, format!("couldn't save it: {e}"), true),
                    }
                }
                self.refresh_notebook_pages();
            }
            NbAction::Rename(id, name) => match self.notebook_rename(&id, &name) {
                Ok(n) => note(self, format!("renamed to {name}{}", if n > 0 { format!(" -- and {n} link{} to it", if n == 1 { "" } else { "s" }) } else { String::new() }), false),
                Err(e) => note(self, e, true),
            },
            NbAction::SetTags(id, tags) => {
                let text = self.features.notebook_state().book.as_ref().and_then(|nb| nb.get(&id)).map(|e| e.text.clone()).unwrap_or_default();
                let value = (!tags.is_empty()).then(|| fenix_notebook::meta::yaml_list(&tags));
                let new_text = fenix_notebook::meta::set_front_matter(&text, "tags", value.as_deref());
                self.notebook_write_entry(&id, &new_text);
                note(self, if tags.is_empty() { "tags cleared".into() } else { format!("tagged {}", tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" ")) }, false);
            }
            NbAction::Pin(id, pinned) => {
                if let Some(nb) = self.notebook() {
                    let _ = nb.set_pinned(&id, pinned);
                }
                self.refresh_notebook_pages();
                note(self, if pinned { "pinned".into() } else { "unpinned".into() }, false);
            }
            NbAction::Duplicate(id) => {
                self.notebook_flush(&id);
                let Some(nb) = self.notebook() else { return };
                let Some(e) = nb.get(&id).cloned() else { return };
                let name = nb.free_name(&format!("{} copy", e.name));
                match nb.create(if e.kind == NbKind::Day { NbKind::Note } else { e.kind }, &name, &e.text) {
                    Ok(new) => {
                        self.refresh_notebook_pages();
                        if let Some(p) = self.notebook_page_mut(page_id) {
                            p.select(&new);
                        }
                        note(self, format!("made {name}"), false);
                    }
                    Err(e) => note(self, format!("couldn't copy it: {e}"), true),
                }
            }
            NbAction::Delete(id) => {
                self.notebook_close_buffers_of(&id);
                let name = self.features.notebook_state().book.as_ref().and_then(|nb| nb.get(&id)).map(|e| e.display_name()).unwrap_or_default();
                let result = self.notebook().map(|nb| nb.delete(&id));
                match result {
                    Some(Ok(())) => note(self, format!("{name} is in the Recycle Bin"), false),
                    Some(Err(e)) => note(self, format!("couldn't delete it: {e}"), true),
                    None => {}
                }
                self.refresh_notebook_pages();
            }
            NbAction::History(id) => {
                let versions: Vec<(String, PathBuf, String)> = self
                    .features
                    .notebook_state()
                    .book
                    .as_ref()
                    .map(|nb| nb.history(&id))
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(when, path)| {
                        let first = std::fs::read_to_string(&path).ok().and_then(|t| t.lines().find(|l| !l.trim().is_empty() && l.trim() != "---").map(str::to_string)).unwrap_or_default();
                        (when, path, first)
                    })
                    .collect();
                if let Some(p) = self.notebook_page_mut(page_id) {
                    p.show_history(id, versions);
                }
            }
            NbAction::Restore(id, version) => {
                self.notebook_flush(&id);
                let restored = self.notebook().map(|nb| nb.restore(&id, &version));
                match restored {
                    Some(Ok(text)) => {
                        self.notebook_reload_open(&id, &text);
                        note(self, "put back -- the version it replaced is kept too".into(), false);
                    }
                    Some(Err(e)) => note(self, format!("couldn't put it back: {e}"), true),
                    None => {}
                }
                self.refresh_notebook_pages();
            }
            NbAction::ToProject(id) => {
                let Some(root) = self.project_root.clone() else {
                    note(self, "no project open -- open a file in one first".into(), true);
                    return;
                };
                self.notebook_flush(&id);
                let kind = self.features.notebook_state().book.as_ref().and_then(|nb| nb.get(&id)).map(|e| e.kind);
                let dir = if kind == Some(NbKind::Diagram) { root.join("docs").join("diagrams") } else { root.join("docs") };
                self.notebook_close_buffers_of(&id);
                let moved = self.notebook().map(|nb| nb.move_out(&id, &dir));
                match moved {
                    Some(Ok(path)) => {
                        self.refresh_notebook_pages();
                        note(self, format!("moved to {} -- it's the project's now", path.display()), false);
                    }
                    Some(Err(e)) => note(self, format!("couldn't move it: {e}"), true),
                    None => {}
                }
            }
            NbAction::Export(id) => self.notebook_export(&id),
            NbAction::Theme(id) => self.notebook_pick_theme(&id),
            NbAction::View(id) => self.open_entry_viewer(&id),
        }
    }

    /// Renames an entry: its file, its open buffer, and the links to it.
    /// How many other entries' links changed.
    fn notebook_rename(&mut self, id: &str, name: &str) -> Result<usize, String> {
        self.notebook_flush(id);
        let old_path = self.features.notebook_state().book.as_ref().and_then(|nb| nb.get(id)).map(|e| e.path.clone()).ok_or("no such entry")?;
        let buffer = self.buffers.id_for_path(&old_path);
        let changes = self.notebook().ok_or("no notebook")?.rename(id, name).map_err(|e| e.to_string())?;
        let new_path = self.features.notebook_state().book.as_ref().and_then(|nb| nb.get(id)).map(|e| e.path.clone()).unwrap_or(old_path.clone());
        if let (Some(b), true) = (buffer, new_path != old_path) {
            if let Some(ob) = self.buffers.get_mut(b) {
                let _ = ob.buffer.save_as(&new_path);
            }
            self.buffers.path_changed(b, Some(&old_path));
            self.note_disk_state(b);
        }
        let n = changes.len();
        for (eid, text) in changes {
            self.notebook_write_entry(&eid, &text);
        }
        self.refresh_notebook_pages();
        Ok(n)
    }

    /// Writes `text` into an entry: into its buffer when it's open (and
    /// saves), else to its file.
    pub(super) fn notebook_write_entry(&mut self, id: &str, text: &str) {
        let Some(path) = self.features.notebook_state().book.as_ref().and_then(|nb| nb.get(id)).map(|e| e.path.clone()) else { return };
        match self.buffers.id_for_path(&path) {
            Some(b) => {
                if let Some(ob) = self.buffers.get_mut(b) {
                    if ob.buffer.text() != text {
                        let end = ob.buffer.len_chars();
                        let mut scratch = Cursor::at_start();
                        ob.buffer.replace_range(&mut scratch, 0, end, text);
                    }
                }
                self.notebook_save_buffer(b);
            }
            None => {
                if let Some(nb) = self.notebook() {
                    if let Err(e) = nb.write_text(id, text) {
                        self.set_error(format!("couldn't write {}: {e}", path.display()));
                    }
                }
            }
        }
        self.refresh_notebook_pages();
    }

    /// Puts `text` into an open buffer of the entry, as the file now is.
    fn notebook_reload_open(&mut self, id: &str, text: &str) {
        let Some(path) = self.features.notebook_state().book.as_ref().and_then(|nb| nb.get(id)).map(|e| e.path.clone()) else { return };
        if let Some(b) = self.buffers.id_for_path(&path) {
            if let Some(ob) = self.buffers.get_mut(b) {
                let end = ob.buffer.len_chars();
                let mut scratch = Cursor::at_start();
                ob.buffer.replace_range(&mut scratch, 0, end, text);
                ob.buffer.mark_saved();
            }
            self.note_disk_state(b);
        }
    }

    /// Writes an entry's open buffer now, if it has unsaved typing.
    pub(super) fn notebook_flush(&mut self, id: &str) {
        let Some(path) = self.features.notebook_state().book.as_ref().and_then(|nb| nb.get(id)).map(|e| e.path.clone()) else { return };
        if let Some(b) = self.buffers.id_for_path(&path) {
            if self.buffers.get(b).is_some_and(|ob| ob.buffer.is_dirty()) {
                self.notebook_save_buffer(b);
            }
        }
    }

    /// Closes the buffers showing an entry (before it moves or goes).
    fn notebook_close_buffers_of(&mut self, id: &str) {
        let Some(path) = self.features.notebook_state().book.as_ref().and_then(|nb| nb.get(id)).map(|e| e.path.clone()) else { return };
        if let Some(b) = self.buffers.id_for_path(&path) {
            if self.buffers.get(b).is_some_and(|ob| ob.buffer.is_dirty()) {
                self.notebook_save_buffer(b);
            }
            self.buffers.close(b);
            let fallback = self.buffers.mru().first().copied().unwrap_or_else(|| self.buffers.open_scratch());
            self.repoint_panes_showing(b, fallback);
            self.features.notebook_state_mut().edits.remove(&b);
        }
    }

    // ------------------------------------------------------------------
    // Autosave
    // ------------------------------------------------------------------

    /// Writes buffer `id` (a notebook file), keeping the version before
    /// this editing session in the entry's history.
    pub(super) fn notebook_save_buffer(&mut self, id: BufferId) {
        let Some(path) = self.buffers.get(id).and_then(|ob| ob.buffer.path()).map(Path::to_path_buf) else { return };
        let entry = self.features.notebook_state().book.as_ref().and_then(|nb| nb.by_path(&path)).map(|e| e.id.clone());
        if let (Some(eid), Some(nb)) = (&entry, self.features.notebook_state_mut().book.as_mut()) {
            if let Err(e) = nb.snapshot(eid) {
                eprintln!("fenix: couldn't keep a version of {}: {e}", path.display());
            }
        }
        let Some(ob) = self.buffers.get_mut(id) else { return };
        match ob.buffer.save() {
            Ok(()) => {
                let text = ob.buffer.text();
                self.note_disk_state(id);
                self.discard_recovery_for(id);
                self.refresh_gutter_hunks(id);
                if let (Some(eid), Some(nb)) = (&entry, self.features.notebook_state_mut().book.as_mut()) {
                    nb.refresh_text(eid, &text);
                }
                self.features.notebook_state_mut().edits.remove(&id);
            }
            Err(e) => self.set_error(format!("couldn't save {}: {e}", path.display())),
        }
    }

    /// Autosave: writes each notebook buffer a second after its last
    /// change. When to look again, if anything is waiting.
    pub(super) fn notebook_autosave(&mut self, now: Instant) -> Option<Instant> {
        let state = self.features.notebook_state_mut();
        let nb = state.book.as_ref()?;
        let root = nb.root().to_path_buf();
        let mut due = Vec::new();
        let mut next: Option<Instant> = None;
        for id in self.buffers.ids_sorted_by_path() {
            let Some(ob) = self.buffers.get(id) else { continue };
            if !ob.buffer.is_dirty() || !ob.kind.tracks_unsaved_changes() {
                continue;
            }
            let Some(path) = ob.buffer.path() else { continue };
            if !path.starts_with(&root) && !nb.contains(path) {
                continue;
            }
            let count = ob.buffer.edit_count();
            let slot = state.edits.entry(id).or_insert((count, now));
            if slot.0 != count {
                *slot = (count, now);
            }
            let at = slot.1 + AUTOSAVE_AFTER;
            if now >= at {
                due.push(id);
            } else {
                next = Some(next.map_or(at, |n: Instant| n.min(at)));
            }
        }
        let saved = !due.is_empty();
        for id in due {
            self.notebook_save_buffer(id);
        }
        if saved {
            self.refresh_notebook_pages();
            self.refresh_backlinks();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        next
    }

    // ------------------------------------------------------------------
    // Later phases fill these in.
    // ------------------------------------------------------------------


    /// `SPC n d`.
    pub(crate) fn notebook_new_diagram(&mut self) {
        if self.notebook().is_none() {
            return;
        }
        if let Err(e) = self.call_service("diagrams.new", &serde_json::json!({})) {
            self.set_error(format!("no new diagram: {e}"));
        }
    }

    /// `t` on a diagram on the notebook page: the diagrams feature picks
    /// its theme.
    fn notebook_pick_theme(&mut self, id: &str) {
        let Some(path) = self.features.notebook_state().book.as_ref().and_then(|nb| nb.get(id)).map(|e| e.path.clone()) else { return };
        if let Err(e) = self.call_service("diagrams.pick_theme", &serde_json::json!({ "path": path })) {
            self.set_error(format!("no theme: {e}"));
        }
    }

    /// `notebook.new_diagram`: a diagram holding `text`, once its name
    /// is typed.
    fn notebook_name_new_diagram(&mut self, text: String) -> Result<(), String> {
        self.notebook().ok_or("no notebook")?;
        self.features.notebook_state_mut().prompt = Some(NotebookPrompt { kind: NotebookPromptKind::DiagramName { text }, input: String::new() });
        self.wake_caret();
        Ok(())
    }

    /// Makes a diagram called `name` holding `text`, opens it and its
    /// preview.
    pub(super) fn notebook_create_diagram(&mut self, name: &str, text: &str) {
        let Some(nb) = self.notebook() else { return };
        if let Some(existing) = nb.by_name(name) {
            let id = existing.id.clone();
            self.set_message(format!("{name} is in the notebook already -- opened it"));
            self.open_notebook_entry(&id);
            return;
        }
        match nb.create(NbKind::Diagram, name, text) {
            Ok(id) => {
                self.open_notebook_entry(&id);
                // The cursor on the blank line under the header.
                let line = text.lines().position(|l| l.trim().is_empty()).unwrap_or(1);
                self.notebook_place_cursor(line, 4);
                self.refresh_notebook_pages();
                self.set_message(format!("{name} -- draws as you type; saves itself"));
            }
            Err(e) => self.set_error(format!("couldn't make the diagram: {e}")),
        }
    }


    /// `SPC n f`: every entry by name.
    pub(crate) fn notebook_find(&mut self) {
        let Some(nb) = self.notebook() else { return };
        let _ = nb.rescan();
        let candidates = nb
            .recent()
            .into_iter()
            .map(|e| {
                let tags: String = e.tags.iter().take(3).map(|t| format!(" #{t}")).collect();
                fenix_picker::Candidate::new(format!("{}  {}{tags}", e.kind.tag(), e.display_name()), NotebookPick::Open(e.id.clone()))
            })
            .collect();
        self.completion = None;
        self.enter_picker(super::notebook_host::notebook_picker("NOTEBOOK", candidates));
    }

    /// `SPC n r`: the entry you touched last.
    pub(crate) fn notebook_reopen(&mut self) {
        let Some(nb) = self.notebook() else { return };
        let last = nb.entries().iter().filter(|e| e.kind != NbKind::Day).max_by_key(|e| e.modified).map(|e| e.id.clone());
        match last {
            Some(id) => self.open_notebook_entry(&id),
            None => self.set_message("the notebook is empty -- SPC n N makes a note"),
        }
    }

    /// `SPC n j`: today's journal day.
    pub(crate) fn notebook_today(&mut self) {
        self.open_journal_day(chrono::Local::now().date_naive());
    }
}

/// A project's `.md`/`.mmd` files, not too deep, skipping build output.
fn collect_docs(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 4 || out.len() > 400 {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || matches!(name.as_str(), "target" | "node_modules" | "build" | "dist" | "vendor") {
            continue;
        }
        match entry.file_type() {
            Ok(t) if t.is_dir() => collect_docs(&path, out, depth + 1),
            Ok(t) if t.is_file() => {
                if matches!(path.extension().and_then(|e| e.to_str()), Some("md" | "mmd" | "mermaid")) {
                    out.push(path);
                }
            }
            _ => {}
        }
    }
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

    fn page(app: &mut App) -> &mut NotebookPage {
        let id = app.focused_buffer_id();
        app.notebook_page_mut(id).expect("the notebook page")
    }

    #[test]
    fn a_new_note_opens_at_its_cursor_and_saves_itself() {
        let (dir, mut app) = app();
        app.notebook_create_note("Bench session", "# {{title}}\n\n{{cursor}}\n");
        let path = dir.path().join("nb").join("notes").join("bench-session.md");
        assert_eq!(app.open().buffer.path(), Some(path.as_path()));
        assert_eq!(app.cursor().char_idx, "# Bench session\n\n".chars().count());
        app.test_insert_str("retry is 3");
        let now = Instant::now();
        assert!(app.notebook_autosave(now).is_some(), "waits for typing to stop");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Bench session\n\n\n");
        assert!(app.notebook_autosave(now + AUTOSAVE_AFTER).is_none());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Bench session\n\nretry is 3\n");
        assert!(!app.open().buffer.is_dirty());
        // A note is found by name; making it again opens it.
        app.notebook_create_note("bench session", "");
        assert_eq!(app.features.notebook_state().book.as_ref().unwrap().entries().len(), 1);
    }

    #[test]
    fn the_page_lists_entries_and_renames_with_their_links() {
        let (_dir, mut app) = app();
        app.notebook_create_note("Uplink", "# Uplink\n");
        app.notebook_create_note("Log", "see [[Uplink]]\n");
        app.open_notebook_page();
        let id = app.focused_buffer_id();
        app.ensure_page_layout(id, app.focused_pane_id(), 140);
        let text = app.open().buffer.text();
        assert!(text.contains("Uplink") && text.contains("Log"), "{text}");
        let uplink = app.features.notebook_state().book.as_ref().unwrap().by_name("Uplink").unwrap().id.clone();
        assert!(page(&mut app).select(&uplink));
        app.notebook_action(id, NbAction::Rename(uplink, "TC uplink".into()));
        let nb = app.features.notebook_state().book.as_ref().unwrap();
        assert_eq!(nb.by_name("Log").unwrap().text, "see [[TC uplink]]\n");
        // The open buffer of Log has the new link too, and is saved.
        let log_path = nb.by_name("Log").unwrap().path.clone();
        let b = app.buffers.id_for_path(&log_path).unwrap();
        assert_eq!(app.buffers.get(b).unwrap().buffer.text(), "see [[TC uplink]]\n");
        assert_eq!(std::fs::read_to_string(log_path).unwrap(), "see [[TC uplink]]\n");
    }

    #[test]
    fn tags_are_written_to_front_matter_and_journal_days_are_made() {
        let (_dir, mut app) = app();
        app.notebook_create_note("N", "# N\n");
        app.open_notebook_page();
        let page_id = app.focused_buffer_id();
        let id = app.features.notebook_state().book.as_ref().unwrap().by_name("N").unwrap().id.clone();
        app.notebook_action(page_id, NbAction::SetTags(id.clone(), vec!["bench".into(), "tc".into()]));
        let nb = app.features.notebook_state().book.as_ref().unwrap();
        assert_eq!(nb.get(&id).unwrap().text, "---\ntags: [bench, tc]\n---\n# N\n");
        assert_eq!(nb.get(&id).unwrap().tags, vec!["bench", "tc"]);
        let day = chrono::NaiveDate::from_ymd_opt(2026, 9, 29).unwrap();
        app.open_journal_day(day);
        assert!(app.open().buffer.text().starts_with("# Tuesday 29 September 2026"));
        assert!(app.open().buffer.path().unwrap().ends_with("journal/2026/2026-09-29.md"));
    }

    #[test]
    fn code_blocks_are_highlighted_per_line() {
        let spans = highlight_block("rust", "fn main() {\n    let x = 1;\n}");
        assert!(spans[0].iter().any(|(r, n)| *r == (0..2) && *n == "keyword"));
        assert!(spans[1].iter().any(|(r, n)| *r == (4..7) && *n == "keyword"));
        assert!(highlight_block("nope", "x").is_empty());
    }

    // Through its spec, the way `App` reaches the notebook now.

    #[test]
    fn the_notebook_contributes_its_commands_and_keys_from_its_spec() {
        let registry = CommandRegistry::with_builtins();
        let notebook: Vec<(&str, bool)> = registry.iter().filter(|c| c.id.starts_with("notebook.") || c.id == "insert.diagram").map(|c| (c.id.as_ref(), c.external())).collect();
        // 21 from the spec, plus the reading view's two (core).
        assert_eq!(notebook.len(), 23, "{notebook:?}");
        for id in ["notebook.open", "notebook.search", "notebook.journal"] {
            assert!(notebook.contains(&(id, true)), "{id} stays external");
        }
        assert!(notebook.contains(&("notebook.preview", false)) && notebook.contains(&("notebook.read", false)), "the reading view's are still there");
        let keys = keymap::leader_keys();
        assert_eq!(keys.get("notebook.open").map(String::as_str), Some("SPC n n"));
        assert_eq!(keys.get("insert.diagram").map(String::as_str), Some("SPC i D"));
        let menu = keymap::local_trie(&[crate::keymap::LocalContext::Markdown]);
        assert!(matches!(menu.matcher().feed(KeyPress::char('p')), fenix_keymap::Step::Matched(&"notebook.preview")), "core's");
        assert!(matches!(menu.matcher().feed(KeyPress::char('b')), fenix_keymap::Step::Matched(&"notebook.backlinks")), "the notebook's");
    }

    #[test]
    fn a_notebook_pick_carries_its_query_through_the_spec() {
        let picker = notebook_picker("FOUND", vec![fenix_picker::Candidate::new("a note", NotebookPick::Filter("tag".to_string()))]);
        let ActivePicker::Feature { feature, badge, state } = picker else { panic!("a feature picker") };
        assert_eq!((feature, badge), ("fenix.notebook", "FOUND"));
        assert!(state.selected().is_some_and(|c| c.payload.0.downcast_ref::<NotebookPick>().is_some()));
        assert!(SPEC.picker_confirm.is_some());
    }

    #[test]
    fn the_notebook_page_is_laid_out_by_its_context_only_when_a_layout_is_due() {
        let (_dir, mut app) = app();
        app.notebook_create_note("Uplink", "# Uplink
retries
");
        app.open_notebook_page();
        let id = app.focused_buffer_id();
        assert!(app.feature_page::<NotebookPage>(id).is_some());
        assert_eq!(app.page_title(id), "*notebook*");
        let key_ctx = app.feature_page_context(id, super::super::feature::PageNeed::Key);
        assert!(key_ctx.downcast_ref::<()>().is_some(), "a key doesn't lay the page out");
        app.ensure_page_layout(id, app.focused_pane_id(), 100);
        assert!(app.open().buffer.text().contains("Uplink"), "laid out through `page_context`");
    }
}
