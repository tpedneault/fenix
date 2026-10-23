//! Features: the in-process draft of the extension host API (Phase 0
//! of `docs/EXTENSIONS_PLAN.md`).
//!
//! A feature is everything one optional part of Fenix owns -- its state,
//! commands, keys, events and its share of the UI -- behind one trait,
//! instead of fields and `match` arms spread through `App`. `App` keeps
//! the instances in `Features` and calls each hook at the one place the
//! feature used to be wired in by hand, so behaviour doesn't change.
//!
//! Two halves:
//!
//! - `FeatureSpec` is static: the commands and leader keys a feature
//!   contributes. The command registry and the leader trie are built
//!   without an `App`, so these can't live on the instance.
//! - `Feature` is the instance: its hooks run with the feature taken out
//!   of `App` for the call (`App::with_feature`), so a hook gets both
//!   `&mut self` and `&mut App` without the two borrows meeting.
//!
//! Only the hooks a moved feature needs are here; each feature that
//! moves adds what it is missing. That list is what the WIT host API is
//! designed from.

use std::any::Any;

#[cfg(feature = "agenda")]
use super::agenda_host::AgendaHost;
#[cfg(feature = "mission")]
use super::ccsds_host::CcsdsHost;
#[cfg(feature = "diagrams")]
use super::diagram_host::DiagramsHost;
#[cfg(feature = "docker")]
use super::docker_host::DockerHost;
#[cfg(feature = "embedded")]
use super::embedded_host::EmbeddedHost;
#[cfg(feature = "jira")]
use super::jira_host::JiraHost;
#[cfg(feature = "mission")]
use super::mib_host::MibHost;
#[cfg(feature = "notebook")]
use super::notebook_host::NotebookHost;
use super::pdf_host::PdfHost;
use super::review_host::ReviewHost;
#[cfg(feature = "tcl")]
use super::tcl_host::TclHost;
use super::vnc_host::VncHost;
use super::*;
use crate::popup::PopupId;

/// A command a feature contributes. `run` gets the whole `App`, like
/// every other `fn(&mut App)` command handler.
pub(crate) struct FeatureCommand {
    pub id: &'static str,
    pub description: &'static str,
    /// May run from the control channel (`fenix/runCommand`).
    pub external: bool,
    pub run: fn(&mut App),
}

/// A leader-key entry, as the keys after `SPC`.
pub(crate) enum LeaderEntry {
    /// A which-key group label.
    Group(&'static [char], &'static str),
    /// A binding: keys, which-key label, command id.
    Bind(&'static [char], &'static str, &'static str),
}

/// What a feature contributes without an instance.
pub(crate) struct FeatureSpec {
    /// The id it will keep as an extension (`fenix.docker`).
    pub id: &'static str,
    /// What the Extensions page calls it ("PDF reader").
    pub name: &'static str,
    /// What it's for, in a line.
    pub about: &'static str,
    pub commands: &'static [FeatureCommand],
    pub leader: &'static [LeaderEntry],
    /// Its `SPC m` menu, as the keys after `SPC m`, shown in the buffers
    /// `local_context` claims.
    pub local_menu: &'static [LeaderEntry],
    /// Whether its `SPC m` menu applies to the focused buffer, and under
    /// which context.
    pub local_context: Option<fn(&App) -> Option<crate::keymap::LocalContext>>,
    /// The language server for `language` files under `root`, when this
    /// feature knows better than the defaults. `Some(Err)` says why it
    /// can't start one.
    pub language_server: Option<LanguageServerFn>,
    /// Tasks it offers in the task picker for the project at `root`.
    pub tasks: Option<fn(&App, &Path) -> Vec<fenix_tasks::TaskDef>>,
    /// What `K` shows when no language server answers for the focused
    /// buffer.
    pub hover_fallback: Option<fn(&App) -> Option<String>>,
    /// A completion that takes over at the cursor before any other
    /// (Tcl's `-option` after a command that takes them).
    pub completion_at_cursor: Option<fn(&mut App) -> Option<CompletionAt>>,
    /// The only valid candidates for a word starting at char offset
    /// `at`, replacing the general pool (an ensemble's subcommands).
    pub completion_pool_at:
        Option<fn(&App, usize) -> Vec<fenix_picker::Candidate<completion::Item>>>,
    /// What `K` shows for the word under the cursor, ahead of any
    /// language server -- after the instances' own `Feature::hover`.
    pub hover_first: Option<fn(&mut App) -> Option<String>>,
    /// A row of this feature's picker was chosen, with the rows marked
    /// (`Tab`) and what was typed -- instead of `Feature::picker_confirm`.
    pub picker_confirm: Option<fn(&mut App, &dyn Any, &[FeaturePick], &str)>,
    /// Its picker was dismissed -- instead of `Feature::picker_cancel`.
    pub picker_cancel: Option<fn(&mut App)>,
    /// The modeline's clock text, in place of the wall clock (the
    /// mission clock).
    pub modeline_clock: Option<fn(&mut App) -> Option<String>>,
    /// Carries out what a key on one of its pages (`FeaturePage::key`)
    /// asked for, on page `BufferId`.
    pub page_action: Option<fn(&mut App, BufferId, Box<dyn Any>)>,
    /// What page `BufferId` reads besides itself to be laid out or take a
    /// key (a MIB page's set, a note laid out with its pictures), worked
    /// out before core borrows the page and handed to
    /// `FeaturePage::layout` or `FeaturePage::key`.
    pub page_context: Option<fn(&mut App, BufferId, PageNeed) -> Box<dyn Any>>,
    /// Page `BufferId`'s title, when it depends on more than the page (a
    /// definition's name in the loaded MIB) -- else `FeaturePage::title`.
    pub page_title: Option<fn(&App, BufferId) -> Option<String>>,
    /// After page `BufferId` was laid out: what its layout asks of the
    /// world (a diagram's drawing started, its problem on its file).
    pub page_after_layout: Option<fn(&mut App, BufferId)>,
    /// What one of its background threads sent (`FenixUserEvent::
    /// feature`), handled with the feature in place -- instead of
    /// `Feature::on_event`, for features whose code is still `App`
    /// methods.
    pub on_event: Option<fn(&mut App, Box<dyn Any + Send>)>,
    /// Its settings, registered with the schema at startup
    /// (`register_settings`): loaded, checked, saved, on the settings page
    /// and in the docs like core's, and read by key (`Config::value`).
    pub settings: &'static [fenix_config::Setting],
    /// Named calls it offers other features (`App::call_service`).
    pub services: &'static [Service],
    /// What it plugs into contribution points other features define (the
    /// agenda's `agenda_sync`), as `[[contributes.<point>]]` will.
    pub contributes: &'static [Contribution],
    /// Its documents a session keeps (`session.json`): their buffers and
    /// files, reopened with `session_reopen`.
    pub session_documents: Option<fn(&App) -> Vec<(BufferId, PathBuf)>>,
    /// A document of its, from a session: its file opened again.
    pub session_reopen: Option<fn(&mut App, &Path) -> BufferId>,
    /// Its sections of Home: everywhere (`None`), or a project's Home
    /// (`Some(root)`).
    pub home: Option<fn(&mut App, Option<&Path>) -> Vec<crate::dashboard::HomeSection>>,
    /// Enter on one of its Home rows (`HomeEntry::Feature`), by the
    /// row's key.
    pub home_activate: Option<fn(&mut App, &str)>,
    /// The project doctor's checks for the kinds of project it knows
    /// (`fenix_project::doctor::diagnose_with`).
    pub doctor: Option<fenix_project::doctor::FeatureCheck>,
    /// The files it opens itself rather than as text (`*.pdf`).
    pub opener: Option<FileOpener>,
    /// `gd`, `K` or `gf`, ahead of the language server and the core:
    /// `true` when it took it (a note's link followed, a diagram block
    /// shown).
    pub lsp_first: Option<fn(&mut App, fenix_vim::LspRequestKind) -> bool>,
    /// A key while one of its prompts is open, ahead of the core's
    /// prompts: `true` when it took it.
    pub prompt_key: Option<fn(&mut App, KeyPress) -> bool>,
    /// Does what's due now (a note written a moment after typing stops)
    /// and says when the event loop should wake for it next.
    pub wake: Option<fn(&mut App, Instant) -> Option<Instant>>,
    /// What `[[links]]` in Markdown point to, for the reading view.
    pub links: Option<&'static LinkResolver>,
    /// What it starts once the editor is up (the agenda's sync).
    pub on_start: Option<fn(&mut App)>,
    /// Ahead of every key: `true` when it took this one for itself (the
    /// agenda asking about a gap with the clock running).
    pub before_key: Option<fn(&mut App) -> bool>,
    /// What it does on the housekeeping tick, in place (the agenda
    /// reading its file again when something else changed it).
    pub on_tick: Option<fn(&mut App)>,
}

/// How a feature resolves the reading view's `[[links]]` (the notebook's
/// notes): what an embed shows, whether a link goes anywhere, following
/// one, and what a page with no file of its own is relative to.
pub(crate) struct LinkResolver {
    pub embed: fn(&App, &fenix_notebook::Link, Option<&Path>) -> crate::reading::Embedded,
    pub exists: fn(&App, &fenix_notebook::Link, Option<&Path>) -> bool,
    pub follow: fn(&mut App, fenix_notebook::Link, Option<PathBuf>),
    pub root: fn(&App) -> Option<PathBuf>,
}

/// A feature's claim on a kind of file: which it opens, and how.
pub(crate) struct FileOpener {
    pub claims: fn(&Path) -> bool,
    /// Opens it; `true` for the pane's preview tab (a jump).
    pub open: fn(&mut App, &Path, bool),
}

/// One thing a feature plugs into another's contribution point: the
/// point's name, and a value of the type that feature defines for it
/// (`agenda_host::AgendaSync` for `agenda_sync`), found with
/// `App::contributions`.
pub(crate) struct Contribution {
    pub point: &'static str,
    pub value: &'static (dyn Any + Send + Sync),
}

/// A named call one feature offers others (`diagrams.svg`), so they don't
/// reach into its code: a JSON payload in, text out, as an extension's
/// `call-service` will be. Runs with its feature in place.
pub(crate) struct Service {
    pub name: &'static str,
    pub call: ServiceCall,
}

/// What a service does to the editor.
#[derive(Clone, Copy)]
pub(crate) enum ServiceCall {
    /// Only reads it, so it can be asked while the editor is lent out
    /// (laying out a page): `App::query_service`.
    Read(fn(&App, &str) -> Result<String, String>),
    /// Changes it (the clipboard, a buffer): `App::call_service` only.
    Mut(fn(&mut App, &str) -> Result<String, String>),
}

/// A completion in progress: where its prefix starts, the prefix, and
/// the candidates to filter with it.
pub(crate) type CompletionAt = (
    usize,
    String,
    Vec<fenix_picker::Candidate<completion::Item>>,
);

/// See `FeatureSpec::language_server`.
pub(crate) type LanguageServerFn = fn(
    &App,
    &Path,
    fenix_syntax::LanguageId,
) -> Option<Result<fenix_project::tools::CommandSpec, String>>;

// The functions above live on the spec, not the instance, and run with
// the feature in place: a feature's instance is out of `App` while one
// of its own hooks runs, so anything a hook sets off that asks the
// features (a file opened, a language server restarted) would skip it.
// A feature whose code reaches its state by type (`Features::get`) --
// CCSDS, whose pages are still core -- uses spec functions for every
// entry point, so its state is always in place when that code runs.

/// Something that happened that features may want to follow up on. One
/// that arrives while a feature is out (inside one of its own hooks) is
/// kept and delivered when the feature is put back, so a feature hears
/// about what its own hooks caused.
#[derive(Debug, Clone)]
pub(crate) enum Notice {
    /// The focused buffer changed.
    FocusChanged,
    /// A task in the project at `root` finished.
    TaskFinished { root: PathBuf, success: bool },
    /// A terminal buffer was closed.
    TerminalClosed(BufferId),
    /// The periodic housekeeping tick (session checkpoint, disk probe).
    Tick,
    /// One of your settings changed (`App::apply_setting`), by key.
    SettingChanged(String),
}

/// Work a feature hands the disk probe (`Feature::disk_probe`): run on the
/// probe's thread, its answer back to `Feature::disk_probed`.
pub(crate) type DiskProbeJob = Box<dyn FnOnce() -> Box<dyn Any + Send> + Send>;

/// A page a feature owns (`PageModel::Feature`). Core keeps the buffer,
/// lays the page out when it changes or its pane does, routes keys to it
/// and draws it; the page itself only turns keys into actions, which its
/// feature's `FeatureSpec::page_action` carries out with the feature in
/// place. The same contract an extension's page will have.
pub(crate) trait FeaturePage: Any {
    /// The feature that owns it.
    fn feature(&self) -> &'static str;
    /// What it's called in its tab, the modeline and `SPC b b`.
    fn title(&self) -> String;
    /// Whether a text field on it has the keyboard -- then every key is
    /// the page's, Space and Ctrl-V included.
    fn typing(&self) -> bool {
        false
    }
    /// Whether it's laid out to its pane's height (`App::page_rows`), so
    /// laid out again when that changes.
    fn fills_pane(&self) -> bool {
        false
    }
    /// Whether Space means something on the focused row.
    fn claims_space(&self) -> bool {
        false
    }
    /// Text pasted (Ctrl-V) or typed while `typing`.
    fn paste(&mut self, _text: &str) {}
    /// Ctrl keys it takes besides the ones every page gets (`CtrlO`,
    /// `CtrlI`, `CtrlEnter`) -- otherwise they're Vim's.
    fn claims_ctrl(&self, _key: crate::page::Key) -> bool {
        false
    }
    /// Whether what `page_context` found this frame changes the page, so
    /// it's laid out again although nothing else did (a drawing came in).
    fn changed(&self, _ctx: &dyn Any) -> bool {
        false
    }
    /// The page, laid out for a pane `cols` cells wide; `ctx` is what its
    /// feature's `page_context` worked out for it (`()` without one), and
    /// the page can keep what it brought (a finished drawing).
    fn layout(&mut self, ctx: Box<dyn Any>, cols: usize) -> crate::page::Page;
    /// Outlines over the page's first picture, as fractions of it, and
    /// whether each is strong (a diagram's selected node and the source
    /// cursor's).
    fn image_marks(&self) -> Vec<(usize, (f32, f32, f32, f32), bool)> {
        Vec::new()
    }
    /// Whether it shows pictures that are drawn off the UI thread (a
    /// note's diagrams), so it's laid out again when one arrives.
    fn shows_pictures(&self) -> bool {
        false
    }
    /// A key; what it asks for goes to the feature's `page_action`.
    fn key(&mut self, key: crate::page::Key, ctx: Box<dyn Any>) -> Box<dyn Any>;
    /// It's being closed: stop whatever it started (a stream's reader).
    fn close(&self) {}
}

/// Why core asks a feature for its page's context (`FeatureSpec::
/// page_context`): every frame the page shows, for its layout -- `due`
/// when it's laid out again anyway (it changed, or its pane did), else
/// only `FeaturePage::changed` can ask for it -- or for a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageNeed {
    Layout { cols: usize, due: bool },
    Key,
}

/// `model` as feature page `T`, if it is one -- for `find_page`.
pub(super) fn page_as<T: FeaturePage>(model: &super::pages::PageModel) -> Option<&T> {
    match model {
        super::pages::PageModel::Feature(p) => (&**p as &dyn Any).downcast_ref(),
        _ => None,
    }
}

/// The same, to change it -- for a feature walking its pages.
pub(super) fn page_as_mut<T: FeaturePage>(model: &mut super::pages::PageModel) -> Option<&mut T> {
    match model {
        super::pages::PageModel::Feature(p) => (&mut **p as &mut dyn Any).downcast_mut(),
        _ => None,
    }
}

impl App {
    /// What feature page `id` reads besides itself, from its feature's
    /// `page_context` -- `()` for any other page.
    pub(super) fn feature_page_context(&mut self, id: BufferId, need: PageNeed) -> Box<dyn Any> {
        let feature = match self.pages.get(&id).map(|s| &s.model) {
            Some(super::pages::PageModel::Feature(p)) => p.feature(),
            _ => return Box::new(()),
        };
        let context = self
            .features
            .specs()
            .find(|spec| spec.id == feature)
            .and_then(|spec| spec.page_context);
        match context {
            Some(context) => context(self, id, need),
            None => Box::new(()),
        }
    }

    /// Page `id` as feature page `T`, if it is one -- for the feature
    /// that owns it.
    pub(super) fn feature_page<T: FeaturePage>(&self, id: BufferId) -> Option<&T> {
        match self.pages.get(&id).map(|s| &s.model) {
            Some(super::pages::PageModel::Feature(p)) => (&**p as &dyn Any).downcast_ref(),
            _ => None,
        }
    }

    /// The same, to change it; it's laid out again.
    pub(super) fn feature_page_mut<T: FeaturePage>(&mut self, id: BufferId) -> Option<&mut T> {
        let state = self.pages.get_mut(&id)?;
        match &mut state.model {
            super::pages::PageModel::Feature(p) => {
                let page = (&mut **p as &mut dyn Any).downcast_mut::<T>()?;
                state.stale = true;
                Some(page)
            }
            _ => None,
        }
    }
}

/// One row's payload in a feature's picker (`ActivePicker::Feature`):
/// whatever the feature put there, handed back to its `picker_confirm`.
#[derive(Clone)]
pub(crate) struct FeaturePick(pub(crate) Rc<dyn Any>);

impl FeaturePick {
    pub(crate) fn candidate<T: Any>(
        label: impl Into<String>,
        payload: T,
    ) -> fenix_picker::Candidate<FeaturePick> {
        fenix_picker::Candidate::new(label, FeaturePick(Rc::new(payload)))
    }
}

/// A message from one of a feature's background threads, delivered to
/// its `on_event` on the UI thread.
pub struct FeatureEvent {
    pub feature: &'static str,
    /// What kind of event it is, for the hang log.
    pub label: &'static str,
    pub payload: Box<dyn Any + Send>,
}

impl std::fmt::Debug for FeatureEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeatureEvent")
            .field("feature", &self.feature)
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

/// What arrived for a feature while it was out.
enum Missed {
    Notice(Notice),
    Event(Box<dyn Any + Send>),
}

/// Text colour ranges for the visible rows of a buffer a feature draws.
pub(crate) type Highlights = Vec<(std::ops::Range<usize>, glyphon::Color)>;

/// A feature's live half. Every hook has a default that does nothing,
/// so a feature implements only what it uses.
pub(crate) trait Feature: Any {
    fn spec(&self) -> &'static FeatureSpec;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// A `FeatureEvent` addressed to this feature arrived.
    fn on_event(&mut self, _app: &mut App, _payload: Box<dyn Any + Send>) {}

    /// A key arrived, before anything handles it: drop transient UI
    /// (an informational menu) that any key dismisses.
    fn key_arrived(&mut self) {}
    /// A prompt of this feature's is waiting for exactly one key.
    /// `true` when it took the key.
    fn capture_key(&mut self, _app: &mut App, _key: KeyPress) -> bool {
        false
    }
    /// A key in one of this feature's panes, before Vim sees it. `true`
    /// when the feature handled it.
    fn pane_key(&mut self, _app: &mut App, _key: KeyPress) -> bool {
        false
    }
    /// Vim handled a key (a motion, a search) and the cursor may have
    /// moved.
    fn after_vim_key(&mut self, _app: &mut App) {}

    /// The focused pane belongs to this feature, so opening a file
    /// mustn't replace what it shows.
    fn holds_focused_pane(&self, _app: &App) -> bool {
        false
    }
    /// Jumplist navigation would desync this feature's panes.
    fn blocks_jumplist(&self) -> bool {
        false
    }
    /// The frame holding `panes` is closing.
    fn close_panes(&mut self, _app: &mut App, _panes: &[fenix_window::WindowId]) {}
    /// `SPC b k` on `buffer`. `true` when the feature closed it its own
    /// way and the generic close mustn't run.
    fn close_buffer(&mut self, _app: &mut App, _buffer: BufferId) -> bool {
        false
    }

    /// The modeline's prompt text while this feature asks something.
    fn prompt_text(&self) -> Option<String> {
        None
    }
    /// Extra modeline text after the file name (Arduino's board and
    /// port, the agenda's running clock).
    fn modeline_indicator(&self, _app: &App) -> Option<String> {
        None
    }
    /// What it's busy with, for the modeline's activity (the agenda's
    /// sync).
    fn busy(&self, _app: &App) -> Option<String> {
        None
    }
    /// Open work it knows for a project whose Jira key is `jira`, a line
    /// each, for the project hub.
    fn project_work(&self, _app: &App, _jira: &str) -> Vec<String> {
        Vec::new()
    }
    /// A `Notice`.
    fn notice(&mut self, _app: &mut App, _notice: &Notice) {}

    /// What `K` shows for the word under the cursor, ahead of any
    /// language server (a MIB name's card).
    fn hover(&mut self, _app: &mut App) -> Option<String> {
        None
    }
    /// `gd` on the word under the cursor, ahead of any language server.
    /// `true` when it went somewhere.
    fn goto_definition(&mut self, _app: &mut App) -> bool {
        false
    }
    /// Its problems in the file at `path`, shown with the language
    /// servers'.
    fn diagnostics(&self, _path: &Path) -> Vec<lsp_types::Diagnostic> {
        Vec::new()
    }

    /// Candidates it adds to the focused buffer's general completion
    /// pool.
    fn completion_items(
        &mut self,
        _app: &mut App,
    ) -> Vec<fenix_picker::Candidate<completion::Item>> {
        Vec::new()
    }
    /// Candidates it adds at the end of that pool, after the buffer's
    /// words and the snippets (the MIB's names).
    fn completion_tail_items(
        &mut self,
        _app: &mut App,
    ) -> Vec<fenix_picker::Candidate<completion::Item>> {
        Vec::new()
    }
    /// The file its page `page` shows, so the project follows a page as
    /// it follows a buffer.
    fn page_file(&self, _app: &App, _page: BufferId) -> Option<PathBuf> {
        None
    }
    /// The project its page `page` was opened in, for a page with no
    /// file of its own.
    fn page_project(&self, _app: &App, _page: BufferId) -> Option<PathBuf> {
        None
    }
    /// What it wants looked at on disk, by the disk probe, off the UI
    /// thread (`stat` keeps away from a volume that isn't answering).
    fn disk_probe(&self, _app: &App) -> Option<DiskProbeJob> {
        None
    }
    /// What its `disk_probe` work found.
    fn disk_probed(&mut self, _app: &mut App, _found: Box<dyn Any + Send>) {}
    /// For a buffer in its language: the names a generic `function`
    /// capture may be colored as, when the grammar can't tell a call
    /// from any other word in command position.
    fn known_commands(
        &mut self,
        _app: &mut App,
        _buffer: BufferId,
    ) -> Option<std::collections::HashSet<String>> {
        None
    }

    /// The title text of this feature's open picker (`ActivePicker::
    /// Feature`), what it's choosing.
    fn picker_label(&self) -> Option<&str> {
        None
    }
    /// A row of this feature's picker was chosen.
    fn picker_confirm(&mut self, _app: &mut App, _pick: &dyn Any) {}
    /// This feature's picker was cancelled.
    fn picker_cancel(&mut self) {}

    /// Colours for `buffer`'s visible rows, when this feature draws it.
    /// Given the theme rather than `App`: the caller holds the buffer
    /// mutably while it asks.
    fn highlights(
        &self,
        _ob: &OpenBuffer,
        _buffer: BufferId,
        _first_row: usize,
        _rows: usize,
        _theme: &Theme,
    ) -> Option<Highlights> {
        None
    }
    /// A popup to show this frame.
    fn popup(
        &self,
        _app: &App,
        _window_width: f32,
        _modeline_top: f32,
    ) -> Option<(PopupId, fenix_window::Rect, RowSpans)> {
        None
    }
}

/// Every built-in feature's spec, in the order their settings are
/// registered: within a settings category, earlier first (Jira's before
/// the agenda's, as the page has always listed them). Only the ones this
/// build has (each is a Cargo feature).
pub(crate) static BUILTIN_SPECS: &[&FeatureSpec] = &[
    #[cfg(feature = "docker")]
    &super::DOCKER_SPEC,
    #[cfg(feature = "embedded")]
    &super::EMBEDDED_SPEC,
    #[cfg(feature = "tcl")]
    &super::TCL_SPEC,
    #[cfg(feature = "mission")]
    &super::MIB_SPEC,
    #[cfg(feature = "mission")]
    &super::CCSDS_SPEC,
    #[cfg(feature = "notebook")]
    &super::NOTEBOOK_SPEC,
    #[cfg(feature = "diagrams")]
    &super::DIAGRAMS_SPEC,
    #[cfg(feature = "jira")]
    &super::JIRA_SPEC,
    #[cfg(feature = "agenda")]
    &super::AGENDA_SPEC,
    &super::REVIEW_SPEC,
    &super::PDF_SPEC,
    &super::VNC_SPEC,
];

/// Registers the switched-on features' settings with the schema --
/// once, before the settings are first read (`main`, `App::with_file`).
/// A switched-off one's stay in the file untouched.
pub(crate) fn register_settings() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        for spec in BUILTIN_SPECS.iter().filter(|spec| enabled(spec.id)) {
            fenix_config::schema::register(spec.settings);
        }
    });
}

/// The built-in extensions switched off for this run: `extensions.
/// disabled`, read once from the settings file before anything else (it
/// decides which keys, commands and settings there are). Tests switch
/// nothing off this way; they build `Features::builtin_without`.
pub(crate) fn switched_off() -> &'static [String] {
    &startup().0
}

/// Whether this is a fresh install's first run: nothing of Fenix's yet --
/// no `settings.toml`, and no 1.x `config.ini` about to move into one.
/// It starts with the core alone, and the welcome.
pub(crate) fn first_run() -> bool {
    startup().1
}

/// What's switched off for this run, and whether it's a first run.
fn startup() -> &'static (Vec<String>, bool) {
    static STARTUP: std::sync::OnceLock<(Vec<String>, bool)> = std::sync::OnceLock::new();
    STARTUP.get_or_init(|| {
        if cfg!(test) {
            return (Vec::new(), false);
        }
        let Some(settings) = fenix_storage::paths::Roots::current().map(|roots| roots.settings_file()) else { return (Vec::new(), false) };
        let legacy = fenix_storage::paths::legacy_dir().is_some_and(|dir| dir.join("config.ini").exists());
        if !settings.exists() && !legacy {
            return (BUILTIN_SPECS.iter().map(|spec| spec.id.to_string()).collect(), true);
        }
        (fenix_config::disabled_extensions_in(&settings), false)
    })
}

/// Whether a switched-on feature opens `path` itself (`FeatureSpec::
/// opener`) -- for start-up, before there's an `App` to ask.
pub(crate) fn claims_file(path: &Path) -> bool {
    BUILTIN_SPECS.iter().filter(|spec| enabled(spec.id)).filter_map(|spec| spec.opener.as_ref()).any(|o| (o.claims)(path))
}

/// Whether built-in extension `id` is on for this run.
pub(crate) fn enabled(id: &str) -> bool {
    !switched_off().iter().any(|off| off == id)
}

/// The commands of the built-in extensions switched off for this run (so
/// core's keymap leaves their keys out), as `on` decides.
pub(crate) fn commands_switched_off(on: &dyn Fn(&str) -> bool) -> Vec<&'static str> {
    BUILTIN_SPECS.iter().filter(|spec| !on(spec.id)).flat_map(|spec| spec.commands.iter().map(|c| c.id)).collect()
}

/// The built-in features. A slot is empty only while its feature is
/// out for a hook (`App::with_feature_at`).
pub(crate) struct Features {
    slots: Vec<Option<Box<dyn Feature>>>,
    /// Each slot's spec, there even while the feature is out.
    specs: Vec<&'static FeatureSpec>,
    /// Whether each is switched on (`extensions.disabled`). One that's
    /// off keeps its state, so code reaching it by type still finds it,
    /// but contributes nothing: it's left out of `specs()` and of every
    /// hook.
    on: Vec<bool>,
    /// Notices and events a feature missed while it was out, by slot.
    missed: Vec<(usize, Missed)>,
}

impl Features {
    pub(crate) fn builtin() -> Self {
        let slots: Vec<Option<Box<dyn Feature>>> = vec![
            #[cfg(feature = "docker")]
            Some(Box::new(DockerHost::default())),
            #[cfg(feature = "embedded")]
            Some(Box::new(EmbeddedHost::default())),
            #[cfg(feature = "tcl")]
            Some(Box::new(TclHost::default())),
            #[cfg(feature = "mission")]
            Some(Box::new(MibHost::default())),
            #[cfg(feature = "mission")]
            Some(Box::new(CcsdsHost::default())),
            #[cfg(feature = "notebook")]
            Some(Box::new(NotebookHost::default())),
            #[cfg(feature = "diagrams")]
            Some(Box::new(DiagramsHost::default())),
            #[cfg(feature = "agenda")]
            Some(Box::new(AgendaHost::default())),
            #[cfg(feature = "jira")]
            Some(Box::new(JiraHost::default())),
            Some(Box::new(ReviewHost::default())),
            Some(Box::new(PdfHost::default())),
            Some(Box::new(VncHost::default())),
        ];
        let specs: Vec<&'static FeatureSpec> = slots
            .iter()
            .map(|slot| slot.as_deref().expect("all in place").spec())
            .collect();
        let on = specs.iter().map(|spec| enabled(spec.id)).collect();
        Self {
            slots,
            specs,
            on,
            missed: Vec::new(),
        }
    }

    /// The built-in features with `off` switched off -- for tests, which
    /// can't change what this process switched off at startup.
    #[cfg(test)]
    pub(crate) fn builtin_without(off: &[&str]) -> Self {
        let mut features = Self::builtin();
        for (spec, on) in features.specs.iter().zip(features.on.iter_mut()) {
            *on = !off.contains(&spec.id);
        }
        features
    }

    /// The spec of every feature that's switched on, whether or not it's
    /// in place.
    pub(crate) fn specs(&self) -> impl Iterator<Item = &'static FeatureSpec> + '_ {
        self.specs.iter().zip(&self.on).filter(|(_, on)| **on).map(|(spec, _)| *spec)
    }

    /// Every built-in feature's spec and whether it's on (the Extensions
    /// page).
    pub(crate) fn all_specs(&self) -> impl Iterator<Item = (&'static FeatureSpec, bool)> + '_ {
        self.specs.iter().copied().zip(self.on.iter().copied())
    }

    /// Whether the feature with this id is switched on.
    pub(crate) fn is_on(&self, id: &str) -> bool {
        self.all_specs().any(|(spec, on)| on && spec.id == id)
    }

    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    /// Every feature that's in place and switched on.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &dyn Feature> {
        self.slots.iter().zip(&self.on).filter(|(_, on)| **on).filter_map(|(slot, _)| slot.as_deref())
    }

    /// Typed access, for the code that reaches a feature directly (its
    /// own, mostly). Found whether or not it's switched on; `None` while
    /// it's out.
    pub(crate) fn get<T: Feature>(&self) -> Option<&T> {
        self.slots.iter().filter_map(|slot| slot.as_deref()).find_map(|f| f.as_any().downcast_ref::<T>())
    }

    pub(crate) fn get_mut<T: Feature>(&mut self) -> Option<&mut T> {
        self.slots
            .iter_mut()
            .filter_map(|slot| slot.as_deref_mut())
            .find_map(|f| f.as_any_mut().downcast_mut::<T>())
    }

    fn position_of<T: Feature>(&self) -> Option<usize> {
        self.slots
            .iter()
            .position(|slot| slot.as_deref().is_some_and(|f| f.as_any().is::<T>()))
    }

    /// The feature whose id is `id`, if it's in place.
    pub(crate) fn by_id(&self, id: &str) -> Option<&dyn Feature> {
        self.iter().find(|f| f.spec().id == id)
    }

    /// Feature `id`'s slot, when it's switched on.
    fn position_of_id(&self, id: &str) -> Option<usize> {
        self.specs.iter().zip(&self.on).position(|(spec, on)| *on && spec.id == id)
    }
}

impl App {
    /// Runs `f` with feature `i` taken out of `App`, then puts it back.
    /// `None` when the slot is empty (the feature is already out, in a
    /// hook that came back into `App`).
    fn with_feature_at<R>(
        &mut self,
        i: usize,
        f: impl FnOnce(&mut dyn Feature, &mut App) -> R,
    ) -> Option<R> {
        let mut feature = self.features.slots.get_mut(i)?.take()?;
        let result = f(feature.as_mut(), self);
        self.put_back(i, feature);
        Some(result)
    }

    /// Puts feature `i` back, then delivers the notices and events it
    /// missed while it was out -- including any those cause, in order.
    fn put_back(&mut self, i: usize, feature: Box<dyn Feature>) {
        self.features.slots[i] = Some(feature);
        while let Some(at) = self.features.missed.iter().position(|(slot, _)| *slot == i) {
            let (_, missed) = self.features.missed.remove(at);
            let mut feature = self.features.slots[i].take().expect("just put back");
            match missed {
                Missed::Notice(notice) => feature.notice(self, &notice),
                Missed::Event(payload) => feature.on_event(self, payload),
            }
            self.features.slots[i] = Some(feature);
        }
    }

    /// Runs `f` with the `T` feature and the rest of `App`.
    pub(crate) fn with_feature<T: Feature, R>(
        &mut self,
        f: impl FnOnce(&mut T, &mut App) -> R,
    ) -> Option<R> {
        let i = self.features.position_of::<T>()?;
        self.with_feature_at(i, |feature, app| {
            let feature = feature
                .as_any_mut()
                .downcast_mut::<T>()
                .expect("the slot was found by its type");
            f(feature, app)
        })
    }

    /// Offers `f` to each feature in turn until one returns `true`.
    pub(crate) fn feature_claims(
        &mut self,
        mut f: impl FnMut(&mut dyn Feature, &mut App) -> bool,
    ) -> bool {
        (0..self.features.len()).any(|i| self.features.on[i] && self.with_feature_at(i, &mut f).unwrap_or(false))
    }

    /// Runs `f` on every feature.
    pub(crate) fn each_feature(&mut self, mut f: impl FnMut(&mut dyn Feature, &mut App)) {
        for i in 0..self.features.len() {
            if self.features.on[i] {
                self.with_feature_at(i, &mut f);
            }
        }
    }

    /// Tells every feature about `notice` -- now, or, for one that's out,
    /// as soon as it's back.
    pub(crate) fn notify_features(&mut self, notice: Notice) {
        for i in 0..self.features.len() {
            if !self.features.on[i] {
                continue;
            }
            if self.features.slots[i].is_some() {
                self.with_feature_at(i, |feature, app| feature.notice(app, &notice));
            } else {
                self.features
                    .missed
                    .push((i, Missed::Notice(notice.clone())));
            }
        }
    }

    /// Delivers a background event to the feature it's addressed to.
    /// One that arrives while the feature is out (with no event loop,
    /// work started from one of its own hooks reports back inline) waits
    /// until it's back.
    pub(crate) fn deliver_feature_event(&mut self, event: FeatureEvent) {
        let on_event = self
            .features
            .specs()
            .find(|spec| spec.id == event.feature)
            .and_then(|spec| spec.on_event);
        if let Some(on_event) = on_event {
            return on_event(self, event.payload);
        }
        let Some(i) = self.features.position_of_id(event.feature) else {
            return;
        };
        if self.features.slots[i].is_some() {
            self.with_feature_at(i, |feature, app| feature.on_event(app, event.payload));
        } else {
            self.features.missed.push((i, Missed::Event(event.payload)));
        }
    }

    /// How a switched-on feature opens `path`, when one claims it.
    pub(crate) fn opener_for(&self, path: &Path) -> Option<fn(&mut App, &Path, bool)> {
        self.features.specs().filter_map(|spec| spec.opener.as_ref()).find(|o| (o.claims)(path)).map(|o| o.open)
    }

    /// The features' doctor checks, for `doctor::diagnose_with` (they're
    /// plain functions, so a pass off the UI thread can take them along).
    pub(crate) fn doctor_checks(&self) -> Vec<fenix_project::doctor::FeatureCheck> {
        self.features.specs().filter_map(|spec| spec.doctor).collect()
    }

    /// Calls service `name` with `payload`, through the feature offering
    /// it. `Err` when none does (that feature is off) or the call fails.
    pub(crate) fn call_service(
        &mut self,
        name: &str,
        payload: &serde_json::Value,
    ) -> Result<String, String> {
        match self.service(name)? {
            ServiceCall::Read(call) => call(self, &payload.to_string()),
            ServiceCall::Mut(call) => call(self, &payload.to_string()),
        }
    }

    /// Calls service `name`, one that only reads the editor, from code
    /// that has it lent out. `Err` for one that changes it.
    pub(crate) fn query_service(
        &self,
        name: &str,
        payload: &serde_json::Value,
    ) -> Result<String, String> {
        match self.service(name)? {
            ServiceCall::Read(call) => call(self, &payload.to_string()),
            ServiceCall::Mut(_) => Err(format!("{name} changes the editor: call it, don't query it")),
        }
    }

    fn service(&self, name: &str) -> Result<ServiceCall, String> {
        self.features
            .specs()
            .flat_map(|spec| spec.services)
            .find(|service| service.name == name)
            .map(|service| service.call)
            .ok_or_else(|| format!("nothing offers {name}"))
    }

    /// What the features plug into contribution point `point`, in
    /// feature order. The feature defining the point downcasts each to
    /// its own type.
    pub(crate) fn contributions(
        &self,
        point: &str,
    ) -> impl Iterator<Item = &'static (dyn Any + Send + Sync)> + '_ {
        let point = point.to_string();
        self.features
            .specs()
            .flat_map(|spec| spec.contributes)
            .filter(move |c| c.point == point)
            .map(|c| c.value)
    }

    /// Runs `f` with the feature whose id is `id`.
    /// The file the focused page shows, as the feature that owns it says.
    pub(crate) fn page_file(&self) -> Option<PathBuf> {
        let id = self.focused_buffer_id();
        self.features.iter().find_map(|f| f.page_file(self, id))
    }

    /// The project the focused page was opened in, as its feature says.
    pub(crate) fn page_project(&self) -> Option<PathBuf> {
        let id = self.focused_buffer_id();
        self.features.iter().find_map(|f| f.page_project(self, id))
    }

    pub(crate) fn with_feature_id<R>(
        &mut self,
        id: &str,
        f: impl FnOnce(&mut dyn Feature, &mut App) -> R,
    ) -> Option<R> {
        let i = self.features.position_of_id(id)?;
        self.with_feature_at(i, f)
    }
}

#[cfg(test)]
mod tests {
    /// docs/CONFIGURATION.md lists every setting, core's and the
    /// features', and this is what keeps it true. When it fails, paste
    /// `markdown_table()` over its table (run `cargo test -p fenix-gui
    /// print_the_settings_table -- --ignored --nocapture`).
    #[test]
    fn the_docs_list_every_setting_as_the_schema_declares_it() {
        super::register_settings();
        let docs = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/CONFIGURATION.md")).unwrap().replace('\r', "");
        assert!(docs.contains(&fenix_config::schema::markdown_table()), "docs/CONFIGURATION.md's settings table is out of date");
    }

    #[test]
    #[ignore]
    fn print_the_settings_table() {
        super::register_settings();
        print!("{}", fenix_config::schema::markdown_table());
    }

    #[test]
    fn every_service_name_is_offered_once_under_its_feature() {
        let mut seen = std::collections::HashSet::new();
        for spec in super::BUILTIN_SPECS {
            for service in spec.services {
                assert!(seen.insert(service.name), "{} offered twice", service.name);
                let short = spec.id.strip_prefix("fenix.").unwrap_or(spec.id);
                assert!(service.name.starts_with(&format!("{short}.")), "{} isn't named under {}", service.name, spec.id);
            }
        }
    }

    #[test]
    fn a_service_nobody_offers_is_an_error_not_a_panic() {
        let mut app = super::App::with_file(None);
        assert_eq!(app.call_service("nobody.here", &serde_json::json!({})), Err("nothing offers nobody.here".to_string()));
        assert_eq!(app.query_service("nobody.here", &serde_json::json!({})), Err("nothing offers nobody.here".to_string()));
    }

    #[test]
    fn a_service_that_changes_the_editor_cant_be_queried() {
        let app = super::App::with_file(None);
        let err = app.query_service("diagrams.copy_picture", &serde_json::json!({ "source": "flowchart TD\n    a --> b\n" })).unwrap_err();
        assert!(err.contains("changes the editor"), "{err}");
    }

    /// With the features' settings registered, every category on the
    /// settings page has something in it.
    /// A switched-off feature keeps its state (reached by type) but is
    /// left out of everything that goes by spec, and its events are
    /// dropped.
    #[test]
    fn a_switched_off_feature_contributes_nothing() {
        let mut app = super::App::with_file(None);
        app.features = super::Features::builtin_without(&["fenix.jira", "fenix.pdf"]);
        assert!(!app.features.specs().any(|s| s.id == "fenix.jira" || s.id == "fenix.pdf"));
        assert!(!app.features.is_on("fenix.jira") && app.features.is_on("fenix.agenda"));
        assert_eq!(app.features.all_specs().count(), super::BUILTIN_SPECS.len(), "the Extensions page still lists it");
        assert!(app.features.get::<super::super::jira_host::JiraHost>().is_some(), "its state is still there");
        assert!(app.features.iter().all(|f| f.spec().id != "fenix.jira"));
        assert!(app.agenda_sync().is_none(), "Jira's sync isn't plugged in");
        assert!(app.call_service("diagrams.svg", &serde_json::json!({ "source": "flowchart TD\n a --> b\n" })).is_ok(), "the others still answer");
        app.deliver_feature_event(super::FeatureEvent { feature: "fenix.jira", label: "test", payload: Box::new(()) });
    }

    /// A PDF is the PDF feature's to open -- unless it's switched off,
    /// when it's a file like any other.
    #[test]
    fn a_feature_opens_the_files_it_claims_while_it_is_on() {
        let mut app = super::App::with_file(None);
        let pdf = std::path::Path::new("spec.pdf");
        assert!(app.opener_for(pdf).is_some() && super::claims_file(pdf));
        assert!(app.opener_for(std::path::Path::new("notes.md")).is_none());
        app.features = super::Features::builtin_without(&["fenix.pdf"]);
        assert!(app.opener_for(pdf).is_none());
    }

    #[test]
    fn every_settings_category_has_a_setting() {
        super::register_settings();
        let all = fenix_config::settings();
        for c in fenix_config::Category::ALL {
            assert!(all.iter().any(|s| s.category == c), "{} is empty", c.label());
        }
    }

    #[test]
    fn every_feature_setting_is_registered_once_and_none_shadows_core() {
        super::register_settings();
        let all = fenix_config::settings();
        for spec in super::BUILTIN_SPECS {
            for s in spec.settings {
                assert_eq!(all.iter().filter(|t| t.key == s.key).count(), 1, "{}", s.key);
                assert!(std::ptr::eq(fenix_config::schema::setting(s.key).unwrap(), s), "{} is {}'s, not core's", s.key, spec.id);
            }
        }
    }
}
