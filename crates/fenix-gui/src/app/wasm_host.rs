//! The extensions that run code (WebAssembly), in Fenix: each loaded on a
//! worker thread of its own (`fenix_wasm::host`) when Fenix starts, if it
//! asks to be, or the first time one of its commands runs. What they say,
//! what they ask of Fenix and how their calls end come back as
//! `FenixUserEvent::Wasm`. Here they're answered: a picker, a prompt, a
//! page (`code_page`), a terminal, a file opened, another extension's or
//! Fenix's service. What they said is kept for their output page
//! (`SPC h l`).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

use fenix_wasm::host::{Call, Event, Level, Notice, Reply, Request, Source, Spec, Worker};

use super::pages::{PageEvent, PageModel};
use super::*;
use crate::code_page::CodePage;
use crate::installed::CodeExtension;

/// How long a command may run, in epoch ticks (`fenix_wasm::TICK`).
const COMMAND_TICKS: u64 = 500;
/// How long loading and activating one may take.
const ACTIVATE_TICKS: u64 = 1000;
/// How long laying out a page, or a key on one, may take.
const PAGE_TICKS: u64 = 100;
/// The most memory one may have.
const MEMORY: usize = 128 << 20;
/// The most lines its output page keeps.
const KEPT_LINES: usize = 2000;

/// The system's keychain, looked for once; none in tests.
pub(crate) fn system_keychain() -> &'static fenix_config::keychain::Keychain {
    static SYSTEM: std::sync::OnceLock<fenix_config::keychain::Keychain> = std::sync::OnceLock::new();
    SYSTEM.get_or_init(|| if cfg!(test) { fenix_config::keychain::Keychain::none() } else { fenix_config::keychain::Keychain::system() })
}

/// How many epoch ticks a command or an event of extension `manifest`'s
/// may take: its `limits.call_ms`, else 5 seconds.
pub(crate) fn call_ticks(manifest: &fenix_extensions::Manifest) -> u64 {
    (u64::from(manifest.limits.call_ms()) / fenix_wasm::TICK.as_millis() as u64).max(1)
}

/// Where a code extension is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Status {
    Starting,
    Ready,
    Stopped(String),
}

/// What a call in flight was for, so its end is dealt with.
#[derive(Debug, Clone)]
enum Pending {
    Command(String),
    /// A page laid out, for page buffer `buffer`.
    Render(BufferId),
    /// A service asked of it, as its call `n` (`WasmHost::serving` has
    /// who asked).
    Serve(u64),
    /// What page buffer `buffer` needs to open again, for the session.
    State(BufferId),
    /// A page put back from the session: closed if its extension can't.
    Restore(BufferId),
    /// A file it opens itself, told it's opened (`opened`): a page it opens
    /// or brings forward meanwhile is that file's.
    Opened(PathBuf),
    /// Nothing to do with its end but report a failure.
    Other,
}

/// Who a service call is answered to.
pub(crate) enum Answer {
    /// The extension that asked, as its call `id`.
    Ext { asker: String, id: u64 },
    /// Fenix itself: the control channel, waiting on it.
    Fenix(Box<dyn FnOnce(&mut App, Result<String, String>)>),
}

/// The PDF reader's services Fenix asks (`fenix.pdf`'s): a file opened at
/// a page or a heading -- `gf` on `x.pdf#page=38`, a standard's clause --
/// and a file searched, for the standards library.
pub(crate) const PDF_OPEN: &str = "pdf.open";
/// A file shown as hex, the CCSDS extension's: for a file that isn't
/// text.
pub(crate) const CCSDS_HEX: &str = "ccsds.hex";

/// How often `active` is told: the first key in this long.
const ACTIVE_EVERY: std::time::Duration = std::time::Duration::from_secs(30);
/// How long a buffer is still before its change is told (`buffer-changed`).
const CHANGED_AFTER: std::time::Duration = std::time::Duration::from_millis(300);
/// How long the lines a pane shows stay still before `view-changed`.
const VIEW_AFTER: std::time::Duration = std::time::Duration::from_millis(100);
/// How soon a second press on a page's cell is a double click.
const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(400);
/// How many files given to an extension are kept for it (`granted`).
const KEPT_GRANTS: usize = 64;

/// One of an extension's scratch buffers (`scratch-open`).
#[derive(Debug, Clone)]
struct Scratch {
    ext: String,
    id: u64,
    title: String,
}

/// The key pages draw extension `ext`'s image `id` by.
pub(crate) fn image_key(ext: &str, id: u32) -> crate::page::ImageKey {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (ext, id).hash(&mut h);
    crate::page::ImageKey::Code(h.finish())
}

/// A line one of them asked to have typed (`host.prompt`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodePrompt {
    pub ext: String,
    pub id: u64,
    pub label: String,
    pub input: String,
    /// What's typed shown as dots (`prompt-secret`).
    pub masked: bool,
}

/// A task of an extension's beside the task pane (`run-task-background`):
/// its output in a buffer of its own.
pub(crate) struct BackgroundTask {
    pub ext: String,
    pub id: u64,
    run: fenix_tasks::RunId,
    /// While it runs: dropped, it's stopped.
    runner: Option<fenix_tasks::TaskRunner>,
}

/// The code extensions and their workers.
#[derive(Default)]
pub(crate) struct WasmHost {
    engine: Option<(fenix_wasm::wasmtime::Engine, fenix_wasm::Ticker)>,
    pub(crate) installed: Vec<CodeExtension>,
    workers: HashMap<String, Worker>,
    pub(crate) status: HashMap<String, Status>,
    /// What each said, oldest first.
    pub(crate) output: HashMap<String, Vec<String>>,
    calls: HashMap<(String, u64), Pending>,
    /// Calls asked for while their extension starts, with what each is for.
    queued: HashMap<String, Vec<(Call, u64, Pending)>>,
    /// Each one's settings and secrets, as its worker reads them.
    settings: HashMap<String, Arc<RwLock<HashMap<String, String>>>>,
    secrets: HashMap<String, Arc<RwLock<HashMap<String, String>>>>,
    pub(crate) prompt: Option<CodePrompt>,
    /// Each one's modeline item (`status-set`).
    pub(crate) statuses: HashMap<String, String>,
    /// Service calls in flight, by the extension answering and Fenix's
    /// number for the call: who gets the answer.
    serving: HashMap<(String, u64), Answer>,
    next_serve: u64,
    /// Each one's section on Home (`home-set`).
    pub(crate) home: HashMap<String, fenix_wasm::host::HomeSection>,
    /// The editor theme's colours, as every worker reads them.
    theme: Option<Arc<RwLock<fenix_wasm::host::EditorTheme>>>,
    /// Where you were when each one's command ran (`editor-here`), for
    /// those with `editor = "read"`.
    heres: HashMap<String, Arc<RwLock<Option<fenix_wasm::host::Here>>>>,
    /// And where the cursor and the selection were (`editor-cursor`).
    cursors: HashMap<String, Arc<RwLock<Option<fenix_wasm::host::CursorInfo>>>>,
    /// When `active` was last told.
    pub(crate) told_active: Option<std::time::Instant>,
    /// Code pages drawn this frame, and those that were last frame.
    drawn: HashSet<BufferId>,
    shown: HashSet<BufferId>,
    /// Ids for pages put back from the session, or kept over a restart,
    /// out of the extensions' own range.
    restored: u64,
    /// The project each worker was started in: its `/project`.
    projects: HashMap<String, Option<PathBuf>>,
    /// What each was last told it's in (`focus-changed`).
    focused: HashMap<String, Option<String>>,
    /// Terminals they opened, by buffer: whose, and its id there.
    terminals: HashMap<BufferId, (String, u64)>,
    /// The task one of them ran (`run-task`): whose, its id, its project.
    task: Option<(String, u64, PathBuf)>,
    /// Their tasks beside the task pane (`run-task-background`), by their
    /// output's buffer.
    pub(crate) background: HashMap<BufferId, BackgroundTask>,
    /// Language servers they added to their projects (`language-server-add`),
    /// beside the others, by extension and project.
    pub(crate) added_servers: HashMap<(String, PathBuf), Vec<fenix_project::tools::AdditionalServer>>,
    /// The lines each pane showed of a buffer at the last frame: buffer,
    /// pane, first and last (`view-changed`).
    pub(crate) views_drawn: Vec<(BufferId, u64, u32, u32)>,
    /// Those, and since when they've been so, until they're told.
    views_seen: (Vec<(BufferId, u64, u32, u32)>, Option<std::time::Instant>),
    /// What each was told last.
    views_told: HashMap<String, Vec<fenix_wasm::host::TextView>>,
    /// What each said about a project (`language-server-set`,
    /// `doctor-set`, `tasks-set`), by extension and project.
    servers: HashMap<(String, PathBuf), (Vec<String>, fenix_project::tools::CommandSpec)>,
    /// And `debug-adapter-set`, `formatter-set`.
    adapters: HashMap<(String, PathBuf), (Vec<String>, fenix_project::tools::CommandSpec)>,
    formatters: HashMap<(String, PathBuf), (Vec<String>, fenix_project::tools::CommandSpec)>,
    /// The formatters whose output is kept when they fail
    /// (`formatter-set-ex`).
    keep_output: HashSet<(String, PathBuf)>,
    /// And `debug-adapter-connect`: an adapter over a socket, in place of
    /// one of `adapters`.
    adapter_sockets: HashMap<(String, PathBuf), (Vec<String>, crate::dap::Adapter)>,
    /// The settings each gave its project's servers, by server's name
    /// (`language-server-settings`).
    server_settings: HashMap<(String, PathBuf), HashMap<String, serde_json::Value>>,
    /// The launches each offers (`launches-set`), and its variables
    /// (`launch-variable-set`).
    launches: HashMap<(String, PathBuf), Vec<crate::dap::Template>>,
    launch_variables: HashMap<(String, PathBuf), std::collections::BTreeMap<String, Vec<fenix_wasm::host::PickerItem>>>,
    /// The fixes their problems came with (`diagnostics-set-ex`), by whose
    /// and the file: offered while the file is as it was.
    pub(crate) fixes: HashMap<(String, PathBuf), CodeFixes>,
    /// The keychain their secrets are kept in, in place of the system's
    /// (tests).
    pub(crate) keychain: Option<Arc<fenix_config::keychain::Keychain>>,
    /// The pane beside each one's scratch buffers and text open in
    /// (`scratch-open-beside`, `open-text-beside`), while it's there.
    beside: HashMap<String, fenix_window::WindowId>,
    checks: HashMap<(String, PathBuf), Vec<fenix_wasm::host::Check>>,
    tasks: HashMap<(String, PathBuf), Vec<fenix_tasks::TaskDef>>,
    /// Each one's tasks as it gave them, by name: where they run and
    /// their environment.
    task_specs: HashMap<(String, PathBuf), HashMap<String, fenix_project::tools::CommandSpec>>,
    /// Programs they found (`find-program`), by name: the doctor's probe
    /// and the wizard look there too.
    pub(crate) found: HashMap<String, PathBuf>,
    /// What they draw over the editor.
    pub(crate) layers: super::layers::Layers,
    /// What the editor events last told them: the focused file's problem
    /// counts, and when typing was.
    pub(crate) told_diagnostics: Option<(u32, u32)>,
    pub(crate) told_typing: Option<std::time::Instant>,
    /// The extension being installed from the registry, and those after
    /// it (a pack's, an upgrade's).
    pub(crate) installing: Option<String>,
    pub(crate) install_queue: Vec<String>,
    /// The surface that has the keyboard (`surface-options.capture`), by
    /// its page buffer.
    pub(crate) captured: Option<BufferId>,
    /// The surface a button went down on, and its pane: it hears the
    /// pointer until they're all let go, wherever it goes.
    dragging: Option<(BufferId, fenix_window::Rect)>,
    /// The surfaces the last frame drew, their panes' sizes and the
    /// display's scale (`code_surfaces_drawn`).
    pub(crate) sized: Vec<(BufferId, (u32, u32), f32)>,
    /// Surfaces whose state (`page-state`) is asked for and not back yet:
    /// a surface changes as it draws, so it's asked again then, once at a
    /// time.
    state_asked: HashSet<BufferId>,
    /// Buffers opened only because an extension asked (`open-file`) on a
    /// file it may not reach: what's in them doesn't reach the extensions.
    sealed: HashSet<BufferId>,
    /// Their scratch buffers, by buffer.
    scratches: HashMap<BufferId, Scratch>,
    /// Each buffer's edits as last counted, and since when it changed
    /// untold (`buffer-changed`).
    edits: HashMap<BufferId, (u64, Option<std::time::Instant>)>,
    /// The files given to each (`opened`, `explorer-selected`), the
    /// latest last: kept with its pages for the session, and given again
    /// when it starts again with them.
    pub(crate) granted: HashMap<String, Vec<PathBuf>>,
    /// What each was last told has the focus (`focus-moved`).
    focus_told: HashMap<String, fenix_wasm::host::Focus>,
    /// The theme's palette, as every worker reads it (`theme-palette`).
    palette: Option<Arc<RwLock<Vec<fenix_wasm::host::Rgb>>>>,
    /// Their decorations of buffers (`decorations-set`), by buffer and
    /// whose.
    pub(crate) decorations: HashMap<BufferId, Vec<(String, Vec<crate::decorations::Decoration>)>>,
    /// Their rules for decorating files (`decoration-rules-set`), by
    /// whose; and, by buffer, what was last applied: its edit count, and
    /// the rules' `rules_given` then.
    rules: HashMap<String, Vec<Rule>>,
    rules_given: u64,
    ruled: HashMap<BufferId, (u64, u64)>,
    /// The page buffer a mouse button went down on, and the cell it was
    /// last at: a drag goes on reaching it.
    pub(crate) page_drag: Option<(BufferId, fenix_wasm::host::MouseButton, (u32, u32))>,
    /// The last press on a page -- where, which button, when, how many in
    /// a row -- for double and triple clicks.
    pub(crate) last_press: Option<(BufferId, (u32, u32), fenix_wasm::host::MouseButton, std::time::Instant, u8)>,
    /// Sounds they played, in tests (none are, then).
    #[cfg(test)]
    pub(crate) played: Vec<Vec<u8>>,
    /// URLs it asked to open, in tests (no browser then).
    #[cfg(test)]
    pub(crate) opened: Vec<String>,
    /// Events from the workers when there's no event loop (tests).
    pub(crate) inbox: Arc<Mutex<Vec<(String, Event)>>>,
}

/// An extension's fixes for its problems in a file (`diagnostics-set-ex`).
pub(crate) struct CodeFixes {
    /// The file's buffer's edits when they came, if it was open: once it's
    /// been edited, they no longer fit.
    pub edits: Option<(BufferId, u64)>,
    /// Each fix: the problem's range, its title, and its edits, in
    /// language servers' terms.
    pub fixes: Vec<(lsp_types::Range, String, Vec<lsp_types::TextEdit>)>,
}

/// A rule for decorating files, read (`decoration-rules-set`).
struct Rule {
    files: Vec<String>,
    pattern: regex::Regex,
    group: usize,
    look: fenix_wasm::host::DecorationLook,
}

/// What a file's decorations by rule are kept under, among the
/// extensions' own.
fn rules_key(ext: &str) -> String {
    format!("{ext}:#rules")
}

/// The most text, in characters, rules are tried on.
const MAX_RULED: usize = 4 << 20;

/// `rules` tried on `buffer`'s lines: what they decorate.
fn by_rules(buffer: &fenix_core::Buffer, rules: &[&Rule]) -> Vec<fenix_wasm::host::Decoration> {
    let mut out = Vec::new();
    for line in 0..buffer.line_count() {
        let text = buffer.line(line).to_string();
        let text = text.trim_end_matches(['\n', '\r']);
        for rule in rules {
            for found in rule.pattern.captures_iter(text) {
                let Some(m) = found.get(rule.group).filter(|m| !m.is_empty()) else { continue };
                let column = |byte: usize| text[..byte].chars().count() as u32;
                out.push(fenix_wasm::host::Decoration { start: (line as u32, column(m.start())), end: (line as u32, column(m.end())), look: rule.look.clone(), id: None });
                if out.len() >= fenix_wasm::host::MAX_DECORATIONS {
                    return out;
                }
            }
        }
    }
    out
}

/// A program it gave, held to its permissions (`Checked`), as Fenix runs
/// one: where it said, else the project; with the environment it gave.
fn checked_spec(((program, args, cwd), env): (fenix_wasm::host::Checked, fenix_wasm::host::Env)) -> fenix_project::tools::CommandSpec {
    fenix_project::tools::CommandSpec { cwd, env: env.into_iter().collect(), ..fenix_project::tools::CommandSpec::new(program, args) }
}

/// Whose problems `diagnostics-set` gave are, among the language
/// servers': its own, beside any server it is (`<ext>:<server>`).
pub(crate) fn problems_key(ext: &str) -> String {
    format!("{ext}:#problems")
}

/// The same folder, however it's written.
fn same_dir(a: &Path, b: &Path) -> bool {
    fenix_lsp::normalize(refactor::identity(a)) == fenix_lsp::normalize(refactor::identity(b))
}

impl WasmHost {
    /// The language servers extensions added beside the others in the
    /// project at `root` (`language-server-add`).
    pub(crate) fn added_servers(&self, root: &Path) -> Vec<fenix_project::tools::AdditionalServer> {
        self.added_servers.iter().filter(|((_, r), _)| same_dir(r, root)).flat_map(|(_, list)| list.iter().cloned()).collect()
    }

    /// The language server an extension gave for `language` files in the
    /// project at `root`.
    pub(crate) fn language_server(&self, root: &Path, language: fenix_syntax::LanguageId) -> Option<fenix_project::tools::CommandSpec> {
        let name = crate::lsp::language_config_name(language);
        self.servers.iter().find(|((_, r), (languages, _))| same_dir(r, root) && languages.contains(&name)).map(|(_, (_, spec))| spec.clone())
    }

    /// The debug adapter an extension gave for `language` files in the
    /// project at `root`.
    pub(crate) fn debug_adapter(&self, root: &Path, language: fenix_syntax::LanguageId) -> Option<crate::dap::Adapter> {
        let name = crate::lsp::language_config_name(language);
        let stdio = self.adapters.iter().find(|((_, r), (languages, _))| same_dir(r, root) && languages.contains(&name)).map(|(_, (_, spec))| crate::dap::Adapter::stdio(spec.clone()));
        stdio.or_else(|| self.adapter_sockets.iter().find(|((_, r), (languages, _))| same_dir(r, root) && languages.contains(&name)).map(|(_, (_, adapter))| adapter.clone()))
    }

    /// The formatter an extension gave for `language` files in the project
    /// at `root`, and whether what it writes is kept when it fails.
    pub(crate) fn formatter(&self, root: &Path, language: fenix_syntax::LanguageId) -> Option<(fenix_project::tools::CommandSpec, bool)> {
        let name = crate::lsp::language_config_name(language);
        self.formatters.iter().find(|((_, r), (languages, _))| same_dir(r, root) && languages.contains(&name)).map(|(key, (_, spec))| (spec.clone(), self.keep_output.contains(key)))
    }

    /// The keychain secrets are kept in: the system's, none in tests.
    pub(crate) fn keychain(&self) -> &fenix_config::keychain::Keychain {
        match &self.keychain {
            Some(keychain) => keychain,
            None => system_keychain(),
        }
    }

    /// The launches the extensions offer as they run in the project at
    /// `root` (`launches-set`).
    pub(crate) fn launches(&self, root: &Path) -> Vec<crate::dap::Template> {
        let mut all: Vec<crate::dap::Template> = self.launches.iter().filter(|((_, r), _)| same_dir(r, root)).flat_map(|(_, t)| t.iter().cloned()).collect();
        all.sort_by(|a, b| a.name.cmp(&b.name));
        all
    }

    /// What `${name}` is filled in from in the project at `root`, when an
    /// extension says (`launch-variable-set`).
    pub(crate) fn launch_variable(&self, root: &Path, name: &str) -> Option<Vec<fenix_wasm::host::PickerItem>> {
        self.launch_variables.iter().filter(|((_, r), _)| same_dir(r, root)).find_map(|(_, vars)| vars.get(name).cloned())
    }

    /// The settings extensions gave the server `name` in the project at
    /// `root` (`language-server-settings`), laid over each other.
    pub(crate) fn server_settings(&self, root: &Path, name: &str) -> Option<serde_json::Value> {
        let mut given = self.server_settings.iter().filter(|((_, r), _)| same_dir(r, root)).filter_map(|((ext, _), by)| by.get(name).map(|v| (ext, v))).collect::<Vec<_>>();
        given.sort_by(|a, b| a.0.cmp(b.0));
        given.into_iter().map(|(_, v)| v.clone()).reduce(fenix_lsp::pull::merge)
    }

    /// Whether extension `ext` is for the project at `root`, by what its
    /// `[[contributes.contexts]]` say: one of their markers there, or a
    /// file of one of their languages at its top or in `src/`. One that
    /// names neither is for every project. The language packs all start
    /// with Fenix and say their tasks and checks for whatever project is
    /// open: a Go project's doctor listed Rust's, .NET's and Python's.
    pub(crate) fn is_for(&self, ext: &str, root: &Path) -> bool {
        let Some(extension) = self.installed.iter().find(|e| e.id == ext) else { return true };
        let contexts: Vec<_> = extension.manifest.contributes.contexts.iter().filter(|c| !c.markers.is_empty() || !c.languages.is_empty()).collect();
        if contexts.is_empty() || contexts.iter().any(|c| c.holds(root)) {
            return true;
        }
        let languages: Vec<&str> = contexts.iter().flat_map(|c| c.languages.iter().map(String::as_str)).collect();
        let speaks = |dir: &Path| {
            std::fs::read_dir(dir).into_iter().flatten().flatten().any(|entry| {
                let path = entry.path();
                path.is_file() && fenix_syntax::detect_language_from_path(&path).is_some_and(|l| languages.contains(&crate::lsp::language_config_name(l).as_str()))
            })
        };
        speaks(root) || speaks(&root.join("src"))
    }

    /// The tasks the extensions offer for the project at `root`.
    pub(crate) fn tasks(&self, root: &Path) -> Vec<fenix_tasks::TaskDef> {
        self.tasks.iter().filter(|((ext, r), _)| same_dir(r, root) && self.is_for(ext, root)).flat_map(|(_, t)| t.clone()).collect()
    }

    /// How an extension's task `name` runs in the project at `root`: its
    /// program, where, and its environment.
    pub(crate) fn task_spec(&self, root: &Path, name: &str) -> Option<fenix_project::tools::CommandSpec> {
        self.task_specs.iter().filter(|((_, r), _)| same_dir(r, root)).find_map(|(_, specs)| specs.get(name).cloned())
    }

    /// Their checks for the project doctor of `root`.
    pub(crate) fn doctor_checks(&self, root: &Path) -> Vec<fenix_project::doctor::Check> {
        use fenix_project::doctor::{Check, EditorFix, FixAction, Health, Section};
        use fenix_wasm::host::{Health as H, Section as S};
        // The servers, adapters and formatters they gave, each found or
        // not, as Fenix's own servers are.
        let given = [("language server", &self.servers), ("debug adapter", &self.adapters), ("formatter", &self.formatters)];
        let mut tools: Vec<Check> = given
            .iter()
            .flat_map(|(what, map)| map.iter().filter(|((ext, r), _)| same_dir(r, root) && self.is_for(ext, root)).map(move |((ext, _), (languages, spec))| (*what, ext, languages, spec)))
            .map(|(what, ext, languages, spec)| {
                let path = Path::new(&spec.executable);
                let label = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| spec.executable.clone());
                let found = if path.is_absolute() { path.is_file().then(|| path.to_path_buf()) } else { self.found.get(&label).cloned().or_else(|| fenix_project::doctor::which(&spec.executable)) };
                let detail = format!("{what} for {} · {ext}", languages.join(", "));
                match found {
                    Some(at) => Check::new(Section::Toolchain, label, Health::Ok, format!("{detail} · {}", at.display())),
                    None => Check::new(Section::Toolchain, label, Health::Warn, format!("{detail} · not found")),
                }
            })
            .collect();
        tools.sort_by(|a, b| a.label.cmp(&b.label).then(a.detail.cmp(&b.detail)));
        let said = self
            .checks
            .iter()
            .filter(|((ext, r), _)| same_dir(r, root) && self.is_for(ext, root))
            .flat_map(|(_, checks)| checks.iter())
            .map(|c| {
                let section = match c.section {
                    S::Toolchain => Section::Toolchain,
                    S::Hardware => Section::Hardware,
                    S::Database => Section::Database,
                    S::Project => Section::Project,
                };
                let health = match c.health {
                    H::Ok => Health::Ok,
                    H::Info => Health::Info,
                    H::Warn => Health::Warn,
                    H::Bad => Health::Bad,
                };
                let check = Check::new(section, c.label.clone(), health, c.detail.clone());
                match &c.fix {
                    Some((label, command)) => check.fix(label, FixAction::Editor(EditorFix::RunCommand(command.clone())), false),
                    None => check,
                }
            });
        tools.into_iter().chain(said).collect()
    }
}

/// A forge's answer to `forge.branch` (`fenix_extension_api::forge::Request`).
#[derive(serde::Deserialize)]
struct ForgeRequest {
    number: u64,
    reference: String,
    title: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    checks: Option<String>,
    #[serde(default)]
    approved_by: Vec<String>,
    #[serde(default)]
    unresolved: usize,
}

impl ForgeRequest {
    fn line(self) -> crate::git_status::RequestLine {
        crate::git_status::RequestLine { number: self.number, reference: self.reference, title: self.title, draft: self.draft, checks: self.checks, approved_by: self.approved_by, unresolved: self.unresolved }
    }
}

/// The editor theme's colours, as an extension reads them.
fn editor_theme(t: &crate::theme::Theme) -> fenix_wasm::host::EditorTheme {
    let rgb = |c: glyphon::Color| (c.r(), c.g(), c.b());
    let rgbf = |c: [f32; 4]| ((c[0] * 255.0).round() as u8, (c[1] * 255.0).round() as u8, (c[2] * 255.0).round() as u8);
    let background = rgbf(t.bg);
    fenix_wasm::host::EditorTheme {
        name: t.name.to_string(),
        background,
        text: rgb(t.fg),
        panel: rgbf(t.bg_modeline),
        accent: rgbf(t.caret),
        muted: rgb(t.gutter_fg),
        tint: rgbf(t.hl_line),
        warn: rgb(t.git_modified),
        dark: (u32::from(background.0) + u32::from(background.1) + u32::from(background.2)) < 384,
    }
}

/// A PNG's pixels, as RGBA.
fn decode_rgba(png: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let img = image::load_from_memory(png).map_err(|e| e.to_string())?.to_rgba8();
    let (w, h) = img.dimensions();
    Ok((w, h, img.into_raw()))
}

/// RGBA pixels as a PNG.
pub(crate) fn encode_png(width: u32, height: u32, rgba: Vec<u8>) -> Option<Vec<u8>> {
    let img = image::RgbaImage::from_raw(width, height, rgba)?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// `edits` made to `buffer` as one undo step, each at a place in the text
/// as it was before any of them: the last first, so the earlier places
/// stay where they were.
fn apply_text_edits(buffer: &mut fenix_core::Buffer, mut edits: Vec<fenix_wasm::host::TextEdit>) {
    let at = |buffer: &fenix_core::Buffer, (line, col): (u32, u32)| {
        let lines = buffer.line_count();
        if line as usize >= lines {
            return buffer.len_chars();
        }
        let start = buffer.line_start_char(line as usize);
        start + (col as usize).min(buffer.line_len(line as usize))
    };
    edits.sort_by_key(|e| std::cmp::Reverse(e.start));
    buffer.begin_undo_group();
    for edit in edits {
        let (start, end) = (at(buffer, edit.start), at(buffer, edit.end));
        let mut cursor = Cursor::at_start();
        buffer.replace_range(&mut cursor, start.min(end), end.max(start), &edit.text);
    }
    buffer.end_undo_group();
}

/// A setting's value as an extension reads it: text.
fn as_text(value: &fenix_config::Value) -> String {
    use fenix_config::Value;
    match value {
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Text(t) => t.clone(),
        Value::List(items) => items.join(","),
        // As JSON, `[["key", "value"], ...]`: a value may hold anything.
        Value::Map(pairs) => serde_json::to_string(pairs).unwrap_or_default(),
        // As JSON too, a row a list of its fields.
        Value::Records(rows) => serde_json::to_string(rows).unwrap_or_default(),
    }
}

/// A code page as the session keeps it: its buffer, extension, kind, title
/// and state, the file it shows, and what its extension was given.
pub(super) struct SavedPage {
    pub buffer: BufferId,
    pub ext: String,
    pub kind: String,
    pub title: String,
    pub state: String,
    pub file: Option<PathBuf>,
    pub given: Vec<PathBuf>,
}

/// Something that happened that an extension may want to follow up on.
#[derive(Debug, Clone)]
pub(crate) enum Happened {
    /// The focused buffer changed.
    FocusChanged,
    /// A task in the project at `root` finished.
    TaskFinished { root: PathBuf, success: bool },
    /// A terminal buffer was closed.
    TerminalClosed(BufferId),
}

impl App {
    /// At start-up: the code extensions there are, and those that start
    /// with Fenix, started.
    pub(super) fn code_extensions_at_startup(&mut self) {
        self.wasm.installed = crate::installed::code_extensions();
        // The packs' project markers: a project's top where Fenix's own
        // find none (a `dotnet new console` folder).
        let markers = self.wasm.installed.iter().flat_map(|e| &e.manifest.contributes.contexts).flat_map(|c| c.markers.iter().cloned()).collect();
        fenix_project::set_extension_markers(markers);
        let starting: Vec<String> = self.wasm.installed.iter().filter(|e| e.manifest.activation.on_startup).map(|e| e.id.clone()).collect();
        for id in starting {
            self.start_code_extension(&id);
        }
    }

    /// Extension `ext`'s settings and secrets, read from yours now: those
    /// it contributes, and Fenix's own under its name (`jira.base_url`).
    fn code_settings_now(&self, ext: &CodeExtension) -> (HashMap<String, String>, HashMap<String, String>) {
        let read = |key: &str| fenix_config::schema::setting(key).and_then(|s| s.get(&self.config)).or_else(|| self.config.value(key).cloned()).map(|v| as_text(&v));
        let mut settings: HashMap<String, String> = ext.manifest.contributes.settings.iter().map(|c| (c.key.clone(), read(&c.key).unwrap_or_else(|| c.default.clone()))).collect();
        // Its project's own, over yours, for the settings a project may set.
        if let Some(root) = self.wasm.projects.get(&ext.id).cloned().flatten().or_else(|| self.code_project()) {
            let project = fenix_config::ProjectSettings::load(&root);
            for c in ext.manifest.contributes.settings.iter().filter(|c| c.project) {
                if let Some(value) = project.get(&c.key).map(as_text) {
                    settings.insert(c.key.clone(), value);
                }
            }
        }
        // Its own name's, and those `permissions.settings` names (another
        // extension's, `ccsds.*`): the project's own over yours there too.
        let project = self.wasm.projects.get(&ext.id).cloned().flatten().or_else(|| self.code_project()).map(|root| fenix_config::ProjectSettings::load(&root));
        let prefixes: Vec<String> = std::iter::once(ext.id.rsplit('.').next().unwrap_or(&ext.id).to_string()).chain(ext.manifest.permissions.settings.iter().cloned()).map(|n| format!("{n}.")).collect();
        for s in fenix_config::schema::settings() {
            if prefixes.iter().any(|p| s.key.starts_with(p.as_str())) && !settings.contains_key(s.key) && !matches!(s.kind, fenix_config::schema::Kind::Secret(_)) {
                let theirs = project.as_ref().filter(|_| s.project).and_then(|p| p.get(s.key)).map(as_text);
                if let Some(value) = theirs.or_else(|| read(s.key)) {
                    settings.insert(s.key.to_string(), value);
                }
            }
        }
        // A folder's setting reads as where the folder is, set or not: an
        // extension names its files to Fenix and the others by it.
        let folders = self.code_folders(ext);
        for (name, path) in &ext.manifest.permissions.folders {
            let key = path.trim().strip_prefix("${setting:").and_then(|rest| rest.split_once('}')).map(|(key, _)| key);
            if let (Some(key), Some((_, dir))) = (key, folders.iter().find(|(n, _)| n == name)) {
                settings.insert(key.to_string(), dir.to_string_lossy().into_owned());
            }
        }
        // Its secrets: those it names, and the secret settings its patterns
        // match (`http.secret.*`). From the environment, else the
        // keychain, else the file.
        let allowed = &ext.manifest.permissions.secrets;
        let mut keys: Vec<String> = allowed.iter().filter(|k| !k.contains(['*', '?'])).cloned().collect();
        let patterned = fenix_config::schema::settings().into_iter().filter(|s| matches!(s.kind, fenix_config::schema::Kind::Secret(_)) && fenix_extensions::manifest::secret_allowed(allowed, s.key));
        keys.extend(patterned.map(|s| s.key.to_string()).filter(|k| !allowed.contains(k)));
        let keychain = self.wasm.keychain();
        let secrets = keys
            .into_iter()
            .filter_map(|key| {
                let value = std::env::var(crate::installed::secret_env(&key)).ok().filter(|v| !v.is_empty()).or_else(|| keychain.get(&key)).or_else(|| read(&key));
                value.map(|v| (key, v))
            })
            .collect();
        (settings, secrets)
    }

    /// Starts extension `id`'s worker, unless it's running already.
    pub(crate) fn start_code_extension(&mut self, id: &str) {
        if self.wasm.workers.contains_key(id) {
            return;
        }
        let Some(ext) = self.wasm.installed.iter().find(|e| e.id == id).cloned() else { return };
        if self.wasm.engine.is_none() {
            match fenix_wasm::engine(&fenix_wasm::Options::default()) {
                Ok(engine) => {
                    let ticker = fenix_wasm::Ticker::start(&engine);
                    self.wasm.engine = Some((engine, ticker));
                }
                Err(e) => return self.set_error(format!("extensions can't run code here: {e:#}")),
            }
        }
        let (settings, secrets) = self.code_settings_now(&ext);
        let settings = Arc::new(RwLock::new(settings));
        let secrets = Arc::new(RwLock::new(secrets));
        self.wasm.settings.insert(ext.id.clone(), settings.clone());
        self.wasm.secrets.insert(ext.id.clone(), secrets.clone());
        let here = self.wasm.heres.entry(ext.id.clone()).or_default().clone();
        let cursor = self.wasm.cursors.entry(ext.id.clone()).or_default().clone();
        let mut spec = Spec::new(&ext.id, ext.dir.join("extension.wasm"), &ext.dir);
        // Its data with yours, its compiled code with Fenix's caches -- a
        // machine's extension is in a folder only its administrator writes.
        if !cfg!(test) {
            if let Some(data) = fenix_storage::paths::data_file("extensions") {
                spec.data = data.join(&ext.id);
            }
            if let Some(cache) = fenix_storage::paths::cache_dir() {
                spec.cache = cache.join("extensions").join(&ext.id);
            }
        }
        spec.memory = MEMORY;
        spec.activate_ticks = ACTIVATE_TICKS;
        // Its own deadline, when its manifest gives one (`limits.call_ms`,
        //): what `code_call` gives a command or an event.
        spec.call_ticks = call_ticks(&ext.manifest);
        spec.permissions = ext.manifest.permissions.clone();
        // `/project` is fixed for as long as the worker runs; moving to
        // another project starts it again there (`code_focus_changed`).
        spec.project = self.code_project();
        spec.tools_dir = if cfg!(test) { None } else { fenix_storage::paths::tools_dir() };
        self.wasm.projects.insert(ext.id.clone(), spec.project.clone());
        spec.settings = settings;
        spec.own_settings = ext.manifest.contributes.settings.iter().map(|s| s.key.clone()).collect();
        // One it may read by pattern that's no setting of anyone's -- one it
        // kept (`secret-set`) -- from its variable, else the keychain.
        let keychain = self.wasm.keychain.clone();
        spec.secrets = Arc::new(move |key| {
            if let Some(value) = secrets.read().ok()?.get(key).cloned() {
                return Some(value);
            }
            let from_env = std::env::var(crate::installed::secret_env(key)).ok().filter(|v| !v.is_empty());
            from_env.or_else(|| match &keychain {
                Some(keychain) => keychain.get(key),
                None if cfg!(test) => None,
                None => system_keychain().get(key),
            })
        });
        spec.here = here;
        spec.cursor = cursor;
        spec.opens = ext.manifest.contributes.opens.clone();
        spec.theme = self.wasm.theme.get_or_insert_with(|| Arc::new(RwLock::new(editor_theme(self.theme)))).clone();
        spec.palette = self.wasm.palette.get_or_insert_with(|| Arc::new(RwLock::new(self.theme.palette()))).clone();
        spec.folders = self.code_folders(&ext);
        if ext.manifest.permissions.fonts {
            spec.fonts = fenix_wasm::host::font_dirs();
        }
        let ext_id = ext.id.clone();
        let sink: Box<dyn Fn(Event) + Send + Sync> = match self.event_proxy.clone() {
            Some(proxy) => {
                let proxy = Mutex::new(proxy);
                Box::new(move |event| {
                    if let Ok(proxy) = proxy.lock() {
                        let _ = proxy.send_event(FenixUserEvent::Wasm { ext: ext_id.clone(), event });
                    }
                })
            }
            None => {
                let inbox = self.wasm.inbox.clone();
                Box::new(move |event| inbox.lock().unwrap().push((ext_id.clone(), event)))
            }
        };
        let Some((engine, _)) = &self.wasm.engine else { return };
        let worker = Worker::start(engine, spec, sink);
        self.wasm.workers.insert(ext.id.clone(), worker);
        self.wasm.status.insert(ext.id.clone(), Status::Starting);
        self.note_output(&ext.id, format!("-- starting {} {}", ext.name, ext.manifest.extension.version));
    }

    /// The project an extension starting now works in: the focused one,
    /// else the workspace's, else the one Fenix was started in (a page
    /// restored with the session starts its extension before anything has
    /// focus).
    fn code_project(&self) -> Option<PathBuf> {
        self.project_root.clone().or_else(|| self.workspaces.active_workspace().project.clone()).or_else(|| std::env::current_dir().ok().as_deref().and_then(fenix_project::find_project_root))
    }

    /// Something that happened, told to the code extensions it concerns.
    pub(crate) fn tell_extensions(&mut self, happened: Happened) {
        use Happened as Told;
        match &happened {
            Told::FocusChanged => self.code_focus_changed(),
            Told::TaskFinished { root, success } => {
                self.editor_event("tasks", Notice::TaskEnded(*success));
                let ours = self.wasm.task.as_ref().is_some_and(|(_, _, r)| fenix_lsp::normalize(refactor::identity(r)) == *root || r == root);
                if ours {
                    if let Some((ext, id, _)) = self.wasm.task.take() {
                        self.code_deliver(&ext, Notice::TaskFinished((id, *success)));
                    }
                }
            }
            Told::TerminalClosed(buffer) => {
                if let Some((ext, id)) = self.wasm.terminals.remove(buffer) {
                    self.code_deliver(&ext, Notice::TerminalClosed(id));
                }
            }
        }
    }

    /// How an extension opens `path` itself, when one does
    /// (`contributes.opens`).
    pub(crate) fn opener_for(&self, path: &Path) -> Option<Box<dyn FnOnce(&mut App, &Path, bool)>> {
        let ext = self.code_opener(path)?;
        Some(Box::new(move |app: &mut App, path: &Path, _| app.open_with_extension(&ext, path)))
    }

    /// You moved: each running extension that reads the project is told
    /// where you are -- or, if it's a project other than its `/project`,
    /// started again there, its pages kept.
    fn code_focus_changed(&mut self) {
        // Only a file moves it: a page (its own, Home) belongs to no
        // project, and following those would start it again each time
        // you went between its page and your code.
        let file = self.open().buffer.path().map(Path::to_path_buf);
        let project = file.as_deref().and_then(fenix_project::find_project_root);
        let running: Vec<CodeExtension> = self.wasm.installed.iter().filter(|e| self.wasm.workers.contains_key(&e.id) && e.manifest.permissions.filesystem != fenix_extensions::manifest::Filesystem::None).cloned().collect();
        for ext in running {
            let theirs = self.wasm.projects.get(&ext.id).cloned().flatten();
            if project.is_some() && theirs != project {
                self.restart_code_extension(&ext.id);
                continue;
            }
            let guest = match (&file, &theirs) {
                (Some(file), Some(root)) => file.strip_prefix(root).ok().map(|rel| format!("/project/{}", rel.to_string_lossy().replace('\\', "/"))),
                _ => None,
            };
            if !matches!(self.wasm.status.get(&ext.id), Some(Status::Ready)) {
                continue;
            }
            if self.wasm.focused.get(&ext.id) != Some(&guest) {
                self.wasm.focused.insert(ext.id.clone(), guest.clone());
                self.code_deliver(&ext.id, Notice::FocusChanged(guest));
            }
        }
        self.code_focus_moved();
    }

    /// What has the focus, told to each running extension that asks
    /// (`contributes.events` `focus`) when it isn't what it was told.
    pub(crate) fn code_focus_moved(&mut self) {
        let asking: Vec<CodeExtension> = self.wasm.installed.iter().filter(|e| e.manifest.contributes.events.iter().any(|n| n == "focus") && self.code_ready(&e.id)).cloned().collect();
        for ext in asking {
            let focus = self.focus_for(&ext);
            if self.wasm.focus_told.get(&ext.id) != Some(&focus) {
                self.wasm.focus_told.insert(ext.id.clone(), focus.clone());
                self.code_call(&ext.id, Call::FocusMoved(focus), COMMAND_TICKS, Pending::Other);
            }
        }
    }

    /// An editor event's export of its own (`task-started`,
    /// `debug-changed`), called in each running extension that asks for
    /// the event (`contributes.events`). One built before it has no such
    /// export, and the call does nothing there.
    pub(crate) fn editor_call(&mut self, name: &str, call: Call) {
        let to: Vec<String> = self.wasm.installed.iter().filter(|e| e.manifest.contributes.events.iter().any(|n| n == name) && self.code_ready(&e.id)).map(|e| e.id.clone()).collect();
        for ext in to {
            self.code_call(&ext, call.clone(), COMMAND_TICKS, Pending::Other);
        }
    }

    /// File `path` saved, told by its path to each running extension that
    /// asks (`contributes.events` `saved`, `file-saved`): its full path with
    /// `editor = "read"`, else under its `/project` when it's there.
    pub(crate) fn code_file_saved(&mut self, path: &Path) {
        let asking: Vec<CodeExtension> = self.wasm.installed.iter().filter(|e| e.manifest.contributes.events.iter().any(|n| n == "saved") && self.code_ready(&e.id)).cloned().collect();
        for ext in asking {
            let told = if ext.manifest.permissions.editor.reads() {
                Some(fenix_lsp::normalize(path.to_path_buf()).to_string_lossy().into_owned())
            } else {
                let root = self.wasm.projects.get(&ext.id).cloned().flatten().filter(|_| ext.manifest.permissions.filesystem != fenix_extensions::manifest::Filesystem::None);
                root.and_then(|root| path.strip_prefix(&root).ok().map(|rel| format!("/project/{}", rel.to_string_lossy().replace('\\', "/"))))
            };
            if let Some(told) = told {
                self.code_call(&ext.id, Call::FileSaved(told), COMMAND_TICKS, Pending::Other);
            }
        }
    }

    /// What has the focus, as extension `ext` may know it: a file's full
    /// path with `editor = "read"`, else its path under its `/project`.
    fn focus_for(&self, ext: &CodeExtension) -> fenix_wasm::host::Focus {
        use fenix_wasm::host::{Focus, FocusedPage};
        // The explorer, a picker: nothing of the editor's; the terminal
        // panel, text that's no file.
        if self.main_view != MainView::Editor || self.sidebar_focused {
            return Focus::None;
        }
        if self.terminal_focused {
            return Focus::Text(None);
        }
        let focused = self.focused_buffer_id();
        if let Some(state) = self.pages.get(&focused) {
            let named = |kind: &str| Focus::Page(FocusedPage { kind: kind.to_string(), mine: None });
            return match &state.model {
                PageModel::Code(p) if p.ext == ext.id => Focus::Page(FocusedPage { kind: p.kind.clone(), mine: Some(p.page) }),
                PageModel::Code(p) => named(&format!("{}/{}", p.ext, p.kind)),
                PageModel::Wizard(_) => named("new-project"),
                PageModel::Hub(_) => named("projects"),
                PageModel::Doctor(_) => named("doctor"),
                PageModel::Git(_) => named("git"),
                PageModel::Log(_) => named("git-log"),
                PageModel::Compare(_) => named("git-compare"),
                PageModel::Rebase(_) => named("git-rebase"),
                PageModel::UserSettings(_) => named("settings"),
                PageModel::Snippets(_) => named("snippets"),
                PageModel::Reading(_) => named("reading"),
                PageModel::Extensions(_) => named("extensions"),
                PageModel::ExtensionOutput(_) => named("extensions-output"),
                PageModel::Tutorial(_) => named("tutorial"),
                PageModel::Search(_) => named("search"),
                PageModel::Agent(_) => named("agent"),
                PageModel::AgentReview(_) => named("agent-changes"),
                // A listing in a pane and its preview are the explorer's.
                PageModel::Explorer(..) | PageModel::Preview(_) => Focus::None,
            };
        }
        let Some(ob) = self.buffers.get(focused) else { return Focus::None };
        match ob.kind {
            BufferKind::Dashboard => return Focus::Page(FocusedPage { kind: "home".into(), mine: None }),
            BufferKind::Explorer => return Focus::None,
            _ => {}
        }
        if let Some(scratch) = self.wasm.scratches.get(&focused) {
            return Focus::Text((scratch.ext == ext.id).then_some(scratch.id));
        }
        match ob.buffer.path().filter(|_| ob.kind.tracks_unsaved_changes()) {
            Some(path) if ext.manifest.permissions.editor.reads() => Focus::File(Some(fenix_lsp::normalize(path.to_path_buf()).to_string_lossy().into_owned())),
            Some(path) => {
                let root = self.wasm.projects.get(&ext.id).cloned().flatten().filter(|_| ext.manifest.permissions.filesystem != fenix_extensions::manifest::Filesystem::None);
                Focus::File(root.and_then(|root| path.strip_prefix(&root).ok().map(|rel| format!("/project/{}", rel.to_string_lossy().replace('\\', "/")))))
            }
            None => Focus::Text(None),
        }
    }

    /// Extension `ext`'s scratch buffer `id`, while it's open.
    fn code_scratch(&self, ext: &str, id: u64) -> Option<BufferId> {
        self.wasm.scratches.iter().find(|(b, s)| s.ext == ext && s.id == id && self.buffers.get(**b).is_some()).map(|(b, _)| *b)
    }

    /// What buffer `buffer` is called, when it's an extension's scratch
    /// buffer.
    pub(crate) fn code_scratch_name(&self, buffer: BufferId) -> Option<String> {
        self.wasm.scratches.get(&buffer).map(|s| format!("*{}*", s.title))
    }

    /// The lines the panes show, once they've been still a moment, told to
    /// the extensions that ask (`contributes.events` `view`): of their
    /// scratch buffers, and with `editor = "read"` of files. When to look
    /// again, if something's waiting.
    pub(crate) fn code_views_changed(&mut self, now: std::time::Instant) -> Option<std::time::Instant> {
        use fenix_wasm::host::{TextBuffer, TextView};
        let asking: Vec<(String, bool)> = self.wasm.installed.iter().filter(|e| e.manifest.contributes.events.iter().any(|n| n == "view") && self.code_ready(&e.id)).map(|e| (e.id.clone(), e.manifest.permissions.editor.reads())).collect();
        if asking.is_empty() {
            self.wasm.views_seen = (Vec::new(), None);
            return None;
        }
        if self.wasm.views_seen.0 != self.wasm.views_drawn {
            self.wasm.views_seen = (self.wasm.views_drawn.clone(), Some(now));
        }
        let since = self.wasm.views_seen.1?;
        if now.saturating_duration_since(since) < VIEW_AFTER {
            return Some(since + VIEW_AFTER);
        }
        self.wasm.views_seen.1 = None;
        for (ext, reads) in asking {
            let views: Vec<TextView> = self
                .wasm
                .views_seen
                .0
                .iter()
                .filter_map(|&(buffer, pane, first, last)| {
                    let ob = self.buffers.get(buffer)?;
                    let buffer = match self.wasm.scratches.get(&buffer) {
                        Some(s) if s.ext == ext => TextBuffer::Scratch(s.id),
                        Some(_) => return None,
                        None if reads && ob.kind.tracks_unsaved_changes() && !self.wasm.sealed.contains(&buffer) => TextBuffer::File(fenix_lsp::normalize(ob.buffer.path()?.to_path_buf()).to_string_lossy().into_owned()),
                        None => return None,
                    };
                    Some(TextView { buffer, pane, first, last })
                })
                .collect();
            if self.wasm.views_told.get(&ext) == Some(&views) {
                continue;
            }
            self.wasm.views_told.insert(ext.clone(), views.clone());
            self.code_call(&ext, Call::ViewChanged(views), COMMAND_TICKS, Pending::Other);
        }
        None
    }

    /// Buffers whose text changed and has been still a moment, told: a
    /// scratch buffer's to its extension, a file's to those that ask
    /// (`contributes.events` `changed`); and scratch buffers you closed.
    /// When to look again, if something's waiting.
    pub(crate) fn code_buffers_changed(&mut self, now: std::time::Instant) -> Option<std::time::Instant> {
        use fenix_wasm::host::Changed;
        let closed: Vec<BufferId> = self.wasm.scratches.keys().filter(|b| self.buffers.get(**b).is_none()).copied().collect();
        for buffer in closed {
            if let Some(scratch) = self.wasm.scratches.remove(&buffer) {
                self.wasm.edits.remove(&buffer);
                if self.code_ready(&scratch.ext) {
                    self.code_call(&scratch.ext, Call::ScratchClosed(scratch.id), COMMAND_TICKS, Pending::Other);
                }
            }
        }
        let asking: Vec<String> = self.wasm.installed.iter().filter(|e| e.manifest.contributes.events.iter().any(|n| n == "changed") && e.manifest.permissions.editor.reads() && self.code_ready(&e.id)).map(|e| e.id.clone()).collect();
        let pages_for_files = self.pages.values().any(|s| matches!(&s.model, PageModel::Code(p) if p.file.is_some()));
        if asking.is_empty() && self.wasm.scratches.is_empty() && !pages_for_files {
            self.wasm.edits.clear();
            return None;
        }
        let (mut wake, mut told) = (None::<std::time::Instant>, Vec::new());
        for id in self.buffers.ids_sorted_by_path() {
            let Some(ob) = self.buffers.get(id) else { continue };
            let changed = match self.wasm.scratches.get(&id) {
                Some(s) => (s.ext.clone(), Changed::Scratch(s.id)),
                None => match ob.buffer.path().filter(|_| ob.kind.tracks_unsaved_changes() && (!asking.is_empty() || pages_for_files)) {
                    Some(path) => (String::new(), Changed::File(fenix_lsp::normalize(path.to_path_buf()).to_string_lossy().into_owned())),
                    None => continue,
                },
            };
            let count = ob.buffer.edit_count();
            let seen = self.wasm.edits.entry(id).or_insert((count, None));
            if seen.0 != count {
                *seen = (count, Some(now));
            }
            let Some(since) = seen.1 else { continue };
            if now.saturating_duration_since(since) < CHANGED_AFTER {
                wake = Some(wake.map_or(since + CHANGED_AFTER, |w| w.min(since + CHANGED_AFTER)));
                continue;
            }
            seen.1 = None;
            told.push((id, changed));
        }
        self.wasm.edits.retain(|b, _| self.buffers.get(*b).is_some());
        for (buffer, (owner, changed)) in told {
            if owner.is_empty() {
                for ext in &asking {
                    self.code_call(ext, Call::BufferChanged(changed.clone()), COMMAND_TICKS, Pending::Other);
                }
                // An extension's page for the file hears it too: the page
                // and the text stay in step.
                let Changed::File(path) = &changed else { continue };
                for ext in self.code_page_owners(Path::new(path)) {
                    if !asking.contains(&ext) && self.code_ready(&ext) {
                        self.code_call(&ext, Call::BufferChanged(changed.clone()), COMMAND_TICKS, Pending::Other);
                    }
                }
            } else {
                // Its own: never dirty, so never asked to be saved.
                if let Some(ob) = self.buffers.get_mut(buffer) {
                    ob.buffer.mark_saved();
                }
                if self.code_ready(&owner) {
                    self.code_call(&owner, Call::BufferChanged(changed), COMMAND_TICKS, Pending::Other);
                }
            }
        }
        wake
    }

    /// The extensions with a page showing file `path` (`opened`).
    fn code_page_owners(&self, path: &Path) -> Vec<String> {
        let full = fenix_lsp::normalize(std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
        let mut owners: Vec<String> = self.pages.values().filter_map(|s| match &s.model {
            PageModel::Code(p) if p.file.as_ref() == Some(&full) => Some(p.ext.clone()),
            _ => None,
        }).collect();
        owners.dedup();
        owners
    }

    /// `SPC f o`: from an extension's page to the text of the file it
    /// shows, or from a file's text to the page of the extension that opens
    /// it (`contributes.opens`).
    pub(crate) fn toggle_file_text(&mut self) {
        let focused = self.focused_buffer_id();
        if let Some(state) = self.pages.get(&focused) {
            match &state.model {
                PageModel::Code(p) => match p.file.clone() {
                    // Its buffer when one's open, however it's spelled --
                    // the one its extension edits (`buffer-edit`): one
                    // text, one undo history.
                    Some(file) => match self.buffer_on(&file) {
                        Some(buffer) => {
                            self.open_buffer_in_focused_pane_as(buffer, false);
                            self.refresh_project_root();
                        }
                        None => {
                            self.open_file_as_text(&file);
                        }
                    },
                    None => self.set_message("this page shows no file Fenix opened in it"),
                },
                _ => self.set_message("not an extension's page, nor a file"),
            }
            return;
        }
        let Some(path) = self.open().buffer.path().map(Path::to_path_buf) else { return self.set_message("not a file") };
        let full = fenix_lsp::normalize(std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()));
        if let Some(page) = self.find_page(|m| matches!(m, PageModel::Code(p) if p.file.as_ref() == Some(&full))) {
            return self.show_page(page);
        }
        match self.code_opener(&path) {
            Some(ext) => self.open_with_extension(&ext, &path),
            None => self.set_message(format!("no extension opens {}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())),
        }
    }

    /// Extension `ext` started again, in the project you're in now: its
    /// pages saved (`page-state`, as the session keeps them) and put back
    /// under new ids, the ones it can't say how to reopen closed. Its
    /// terminals stay open, as plain terminals; a task it ran is let go.
    pub(crate) fn restart_code_extension(&mut self, ext: &str) {
        let pages: Vec<(BufferId, String, Option<String>)> = self.pages.iter().filter_map(|(id, s)| match &s.model {
            PageModel::Code(p) if p.ext == ext => Some((*id, p.kind.clone(), p.state.clone())),
            _ => None,
        }).collect();
        self.note_output(ext, "-- the project changed: starting again there".into());
        self.wasm.workers.remove(ext);
        self.forget_code_lsp_sessions(ext);
        self.wasm.status.remove(ext);
        self.wasm.queued.remove(ext);
        self.wasm.calls.retain(|(e, _), _| e != ext);
        self.wasm.statuses.remove(ext);
        self.wasm.focused.remove(ext);
        self.wasm.focus_told.remove(ext);
        // Its scratch buffers stay, as yours; its decorations go, set
        // again as it starts.
        self.wasm.scratches.retain(|_, s| s.ext != ext);
        let by_rule = rules_key(ext);
        for sets in self.wasm.decorations.values_mut() {
            sets.retain(|(e, _)| e != ext && *e != by_rule);
        }
        if self.wasm.rules.remove(ext).is_some() {
            self.wasm.rules_given += 1;
        }
        self.wasm.layers.forget(ext);
        self.wasm.terminals.retain(|_, (e, _)| e != ext);
        self.wasm.views_told.remove(ext);
        // Its tasks beside the task pane stop; their output stays, yours.
        self.wasm.background.retain(|_, t| t.ext != ext);
        if self.wasm.task.as_ref().is_some_and(|(e, _, _)| e == ext) {
            self.wasm.task = None;
        }
        if self.wasm.prompt.as_ref().is_some_and(|p| p.ext == ext) {
            self.wasm.prompt = None;
        }
        self.start_code_extension(ext);
        // What it was given, given again: its pages may show it.
        for path in self.wasm.granted.get(ext).cloned().unwrap_or_default() {
            self.code_call(ext, Call::Grant(path), COMMAND_TICKS, Pending::Other);
        }
        for (buffer, kind, state) in pages {
            let Some(state) = state.filter(|s| !s.is_empty()) else {
                self.close_page_quietly(buffer);
                continue;
            };
            self.wasm.restored += 1;
            let page = (1u64 << 40) + self.wasm.restored;
            if let Some(s) = self.pages.get_mut(&buffer) {
                if let PageModel::Code(p) = &mut s.model {
                    p.page = page;
                    p.width = None;
                    p.asked = None;
                    p.stopped = None;
                }
                s.stale = true;
            }
            self.code_call(ext, Call::PageRestore { page, kind, state }, PAGE_TICKS, Pending::Restore(buffer));
        }
    }

    /// Where you are now, for extension `ext` to read (`editor-here`,
    /// `editor-cursor`) in whatever it's called for -- `buffer-changed`,
    /// `focus-moved`, a timer, a key on its page -- with `editor =
    /// "read"`.
    fn code_where_now(&mut self, ext: &str) {
        let (Some(here), Some(cursor)) = (self.wasm.heres.get(ext).cloned(), self.wasm.cursors.get(ext).cloned()) else { return };
        if !self.wasm.installed.iter().any(|e| e.id == ext && e.manifest.permissions.editor.reads()) {
            return;
        }
        let (now, at) = self.editor_where();
        *here.write().unwrap() = Some(now);
        *cursor.write().unwrap() = at;
    }

    /// Sends `what` to extension `ext`, starting it first if need be.
    fn code_call(&mut self, ext: &str, what: Call, ticks: u64, pending: Pending) {
        // A command's, an event's, a service's: the extension's own deadline
        // (`limits.call_ms`, its worker's `call_ticks`).
        let ticks = if ticks == COMMAND_TICKS { 0 } else { ticks };
        // Not for an answer to what its command asked -- a picker's, a
        // prompt's -- which goes on from where you were then.
        let answer = matches!(&what, Call::Deliver(Notice::Picked(_) | Notice::PickedMany(_) | Notice::Prompted(_) | Notice::Composed(_)));
        if !answer && !matches!(what, Call::RunCommand(_)) {
            self.code_where_now(ext);
        }
        match self.wasm.status.get(ext) {
            Some(Status::Stopped(why)) => {
                let why = why.clone();
                match pending {
                    Pending::Command(_) => self.set_error(format!("{why} -- SPC h l has what it said; it starts again with Fenix")),
                    Pending::Serve(call) => self.code_answered(ext, call, Err(why)),
                    _ => {}
                }
            }
            Some(Status::Ready) => {
                if let Some(worker) = self.wasm.workers.get_mut(ext) {
                    let call = worker.call(what, ticks);
                    self.wasm.calls.insert((ext.to_string(), call), pending);
                }
            }
            starting => {
                let start = starting.is_none();
                self.wasm.queued.entry(ext.to_string()).or_default().push((what, ticks, pending));
                if start {
                    self.start_code_extension(ext);
                }
            }
        }
    }

    /// Runs extension `ext`'s command `id`.
    pub(crate) fn run_extension_command(&mut self, ext: &str, id: &str) {
        // Where you are, as it was when you picked the command.
        let reads = self.wasm.installed.iter().any(|e| e.id == ext && e.manifest.permissions.editor.reads());
        if reads {
            let (here, cursor) = self.editor_where();
            if let Some(shared) = self.wasm.heres.get(ext) {
                *shared.write().unwrap() = Some(here);
            }
            if let Some(shared) = self.wasm.cursors.get(ext) {
                *shared.write().unwrap() = cursor;
            }
        }
        self.code_call(ext, Call::RunCommand(id.to_string()), COMMAND_TICKS, Pending::Command(id.to_string()));
    }

    /// Where you are: the file, the selection (or the cursor's line), its
    /// project; and where the cursor and the selection are in it
    /// (`editor-cursor`), none in what isn't a buffer of text.
    pub(crate) fn editor_where(&self) -> (fenix_wasm::host::Here, Option<fenix_wasm::host::CursorInfo>) {
        let ob = self.open();
        let path = ob.buffer.path().map(Path::to_path_buf);
        let cursor = self.cursor();
        // By the rope's lines, not the whole text: it's read as often as
        // the extension is called.
        let total = ob.buffer.len_chars();
        let line_of = |at: usize| ob.buffer.rope().char_to_line(at.min(total));
        let line_range = |at: usize| {
            let line = line_of(at);
            let start = ob.buffer.line_start_char(line);
            (start, start + ob.buffer.line_len(line))
        };
        let ((from, to), selected) = match self.vim.visual_selection_range(&ob.buffer, &cursor) {
            Some((range, _)) => ((range.start, range.end), true),
            None => match self.vim.last_visual_range() {
                // The selection just left for the leader: the cursor's still in it.
                Some((a, b)) if (a..=b).contains(&cursor.char_idx) => ((line_range(a).0, line_range(b).1), true),
                _ => (line_range(cursor.char_idx), false),
            },
        };
        let (from, to) = (from.min(total), to.min(total));
        let line = line_of(from) as u32 + 1;
        let place = |at: usize| {
            let at = at.min(total);
            (line_of(at) as u32, (at - line_range(at).0) as u32)
        };
        let is_text = !self.is_page_buffer(self.focused_buffer_id()) && ob.kind.tracks_unsaved_changes();
        let cursor_info = is_text.then(|| fenix_wasm::host::CursorInfo { at: place(cursor.char_idx), selection: selected.then(|| (place(from), place(to))), selected });
        let project = path.as_deref().and_then(fenix_project::find_project_root).or_else(|| self.project_root.clone()).map(|root| {
            let meta = fenix_project::meta::jira_key(&root).map(|k| ("jira".to_string(), k)).into_iter().collect();
            let plain = fenix_lsp::normalize(root.clone());
            fenix_wasm::host::ProjectInfo { name: root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), root: plain.to_string_lossy().into_owned(), meta }
        });
        let here = fenix_wasm::host::Here {
            language: path.as_deref().and_then(Path::extension).map(|e| e.to_string_lossy().into_owned()).unwrap_or_default(),
            path: path.map(|p| fenix_lsp::normalize(p).to_string_lossy().into_owned()),
            line,
            text: ob.buffer.text_range(from, to),
            project,
        };
        (here, cursor_info)
    }


    /// The language servers code extensions are for file `path`, in
    /// language `language` (Fenix's name, if it has one), in the project at
    /// `root`: each one's extension, its name, its languages, and whether
    /// it serves the file by pattern (`files`) rather than by language.
    pub(crate) fn code_language_servers_for(&self, path: &Path, language: Option<&str>, root: &Path) -> Vec<(String, String, Vec<String>, bool)> {
        self.wasm
            .installed
            .iter()
            .flat_map(|e| {
                e.manifest.contributes.language_servers.iter().filter(|s| s.serves(path, language, Some(root))).map(|s| {
                    let by_language = language.is_some_and(|l| s.languages.iter().any(|x| x == l));
                    (e.id.clone(), s.name.clone(), s.languages.clone(), !by_language)
                })
            })
            .collect()
    }

    /// The code extension that's the forge for the repository at `root`
    /// (`contributes.forges`), by the host of its `origin`, and what a forge
    /// call about it says (`forge::Repository`, with `number`).
    fn code_forge(&self, root: &Path, number: Option<u64>) -> Option<(String, String)> {
        let remote = fenix_git::remote_url(root, "origin")?;
        let host = fenix_extensions::manifest::url_host(&remote)?;
        let setting = |key: &str| fenix_config::schema::setting(key).and_then(|s| s.get(&self.config)).or_else(|| self.config.value(key).cloned()).map(|v| as_text(&v));
        let (best, ext) = self
            .wasm
            .installed
            .iter()
            .flat_map(|e| e.manifest.contributes.forges.iter().map(|f| (f.claims(&host, &setting), e.id.clone())))
            .max_by_key(|(claim, _)| *claim)?;
        if best == 0 {
            return None;
        }
        let root = fenix_lsp::normalize(root.to_path_buf()).to_string_lossy().into_owned();
        let payload = serde_json::json!({ "root": root, "remote": remote, "branch": fenix_git::oplog::current_branch(Path::new(&root)), "number": number });
        Some((ext, payload.to_string()))
    }

    /// Asks the code forge for the repository at `root` for `service`
    /// (`forge.branch`, `forge.open`, `forge.reviews`); `then` gets its
    /// answer. False when no code extension is its forge.
    pub(crate) fn code_forge_call(&mut self, root: &Path, service: &str, number: Option<u64>, then: Box<dyn FnOnce(&mut App, Result<String, String>)>) -> bool {
        let Some((ext, payload)) = self.code_forge(root, number) else { return false };
        // It may read the repository, and run git there.
        self.code_call(&ext, Call::Grant(fenix_lsp::normalize(root.to_path_buf())), COMMAND_TICKS, Pending::Other);
        self.code_serve(&ext, service, payload, Answer::Fenix(then));
        true
    }

    /// The Git status page `id` was read: its branch's request, from a code
    /// forge, for its header. False when no code extension is the forge.
    pub(crate) fn code_forge_branch(&mut self, id: BufferId) -> bool {
        let Some(root) = self.pages.get(&id).and_then(|s| match &s.model {
            PageModel::Git(g) => Some(g.root.clone()),
            _ => None,
        }) else {
            return false;
        };
        self.code_forge_call(
            &root,
            "forge.branch",
            None,
            Box::new(move |app, answer| {
                let line = match answer.map(|text| serde_json::from_str::<Option<ForgeRequest>>(&text).map_err(|e| e.to_string())) {
                    Ok(Ok(found)) => found.map(ForgeRequest::line),
                    // No line, as when a forge can't be asked (no token).
                    Ok(Err(_)) | Err(_) => return,
                };
                if let Some(state) = app.pages.get_mut(&id) {
                    if let PageModel::Git(g) = &mut state.model {
                        g.request = Some(line);
                    }
                    state.stale = true;
                }
            }),
        )
    }

    /// `o` on the Git status page, or the reviews: to the code forge, when
    /// there's one. False when not.
    pub(crate) fn code_forge_open(&mut self, root: &Path, service: &str, number: Option<u64>) -> bool {
        let service_name = service.to_string();
        self.code_forge_call(
            root,
            service,
            number,
            Box::new(move |app, answer| {
                if let Err(why) = answer {
                    app.set_error(format!("{service_name}: {why}"));
                }
            }),
        )
    }

    /// The code extension that opens `path` itself (`contributes.opens`).
    /// One whose `contributes.magic` says what such a file starts with
    /// opens it only when it does: else it's text.
    pub(crate) fn code_opener(&self, path: &Path) -> Option<String> {
        let head = |n: usize| -> Vec<u8> {
            use std::io::Read;
            let mut bytes = Vec::with_capacity(n);
            if let Ok(file) = std::fs::File::open(path) {
                let _ = file.take(n as u64).read_to_end(&mut bytes);
            }
            bytes
        };
        self.wasm.installed.iter().find(|e| e.manifest.contributes.opens_file(path, &head)).map(|e| e.id.clone())
    }

    /// `path` opened by extension `ext`: started if need be, and told
    /// (`opened`), with the path's to read.
    pub(crate) fn open_with_extension(&mut self, ext: &str, path: &Path) {
        let full = fenix_lsp::normalize(std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
        self.note_granted(ext, full.clone());
        self.code_call(ext, Call::Deliver(Notice::Opened(full.to_string_lossy().into_owned())), COMMAND_TICKS, Pending::Opened(full));
    }

    /// File `path` given to extension `ext`, kept for it (`granted`).
    fn note_granted(&mut self, ext: &str, path: PathBuf) {
        let kept = self.wasm.granted.entry(ext.to_string()).or_default();
        kept.retain(|p| *p != path);
        kept.push(path);
        if kept.len() > KEPT_GRANTS {
            kept.drain(..kept.len() - KEPT_GRANTS);
        }
    }

    /// The file extension `ext` is being told it opens (`opened`), while
    /// it is: a page it opens or brings forward then shows it.
    fn code_opening(&self, ext: &str) -> Option<PathBuf> {
        let (_, pending) = self.wasm.calls.iter().filter(|((e, _), p)| e == ext && matches!(p, Pending::Opened(_))).min_by_key(|((_, call), _)| *call)?;
        match pending {
            Pending::Opened(path) => Some(path.clone()),
            _ => None,
        }
    }

    /// `path` opened by the code extension that opens it, at `page`
    /// (counting from 1) or `heading` when it's asked for one and the
    /// extension can (`pdf.open`); else as `opened` opens it. False when
    /// no extension opens it.
    pub(crate) fn open_with_extension_at(&mut self, path: &Path, page: Option<u32>, heading: Option<&str>) -> bool {
        let Some(ext) = self.code_opener(path) else { return false };
        let serves = self.wasm.installed.iter().any(|e| e.id == ext && e.manifest.contributes.services.iter().any(|s| s == PDF_OPEN));
        if !serves || (page.is_none() && heading.is_none()) {
            self.open_with_extension(&ext, path);
            return true;
        }
        let full = fenix_lsp::normalize(std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
        // It may read the file, as one it's told it opens.
        self.note_granted(&ext, full.clone());
        self.code_call(&ext, Call::Grant(full.clone()), COMMAND_TICKS, Pending::Other);
        let payload = serde_json::json!({ "path": full, "page": page, "heading": heading }).to_string();
        self.code_serve(
            &ext,
            PDF_OPEN,
            payload,
            Answer::Fenix(Box::new(|app, answer| {
                if let Err(why) = answer {
                    app.set_error(why);
                }
            })),
        );
        true
    }

    /// Where extension `ext`'s language servers are sent their messages,
    /// while its worker runs.
    pub(crate) fn code_lsp_port(&self, ext: &str) -> Option<fenix_wasm::host::LspPort> {
        self.wasm.workers.get(ext).map(|w| w.lsp_port())
    }

    /// The buffer open on file `path`, however it's spelled.
    fn buffer_on(&self, path: &Path) -> Option<BufferId> {
        self.buffers.id_for_path(path).or_else(|| {
            let want = fenix_lsp::normalize(std::fs::canonicalize(path).unwrap_or(path.to_path_buf()));
            self.buffers.ids_sorted_by_path().into_iter().find(|id| {
                self.buffers.get(*id).and_then(|ob| ob.buffer.path()).is_some_and(|p| fenix_lsp::normalize(std::fs::canonicalize(p).unwrap_or(p.to_path_buf())) == want)
            })
        })
    }

    /// The buffer an extension's `buffer-*` call reaches on `path`: any
    /// open on it, or when `open_only` (a path it may not reach itself)
    /// one you opened -- not one sealed.
    fn code_buffer_on(&self, path: &Path, open_only: bool) -> Option<BufferId> {
        self.buffer_on(path).filter(|id| !(open_only && self.wasm.sealed.contains(id)))
    }

    /// The code extension, other than `but`, that answers service `name`.
    pub(crate) fn code_server(&self, name: &str, but: Option<&str>) -> Option<String> {
        self.wasm.installed.iter().find(|e| Some(e.id.as_str()) != but && e.manifest.contributes.services.iter().any(|s| s == name)).map(|e| e.id.clone())
    }

    /// Lets code extension `ext` read the file at `path`, as one Fenix gave
    /// it.
    pub(crate) fn code_grant(&mut self, ext: &str, path: PathBuf) {
        self.note_granted(ext, path.clone());
        self.code_call(ext, Call::Grant(path), COMMAND_TICKS, Pending::Other);
    }

    /// Asks code extension `server` for service `name`; `answer` gets
    /// what it says, now or later.
    pub(crate) fn code_serve(&mut self, server: &str, name: &str, payload: String, answer: Answer) {
        self.wasm.next_serve += 1;
        let call = self.wasm.next_serve;
        self.wasm.serving.insert((server.to_string(), call), answer);
        self.code_call(server, Call::Serve { call, name: name.to_string(), payload }, COMMAND_TICKS, Pending::Serve(call));
    }

    /// Service call `call` to `server` answered: to whoever asked.
    fn code_answered(&mut self, server: &str, call: u64, answer: Result<String, String>) {
        match self.wasm.serving.remove(&(server.to_string(), call)) {
            Some(Answer::Ext { asker, id }) => self.code_deliver(&asker, Notice::ServiceReply((id, answer))),
            Some(Answer::Fenix(then)) => then(self, answer),
            None => {}
        }
    }

    /// The editor theme changed: the extensions read the new one, and are
    /// told.
    pub(crate) fn code_theme_changed(&mut self) {
        if let Some(shared) = &self.wasm.theme {
            *shared.write().unwrap() = editor_theme(self.theme);
        }
        if let Some(shared) = &self.wasm.palette {
            *shared.write().unwrap() = self.theme.palette();
        }
        let running: Vec<String> = self.wasm.workers.keys().cloned().collect();
        for ext in running {
            self.code_deliver(&ext, Notice::ThemeChanged);
        }
    }

    /// A key was pressed: `active` told, the first in a while.
    pub(crate) fn code_active(&mut self) {
        if self.wasm.told_active.is_some_and(|at| at.elapsed() < ACTIVE_EVERY) {
            return;
        }
        self.wasm.told_active = Some(std::time::Instant::now());
        self.editor_event("active", Notice::Active);
    }

    /// Their sections on Home.
    pub(crate) fn code_home_sections(&self) -> Vec<crate::dashboard::HomeSection> {
        use crate::dashboard::{HomeColumn, HomeEntry, HomeRow, HomeSection, RowMark};
        use fenix_wasm::host::HomeMark;
        let mut exts: Vec<&String> = self.wasm.home.keys().collect();
        exts.sort();
        exts.into_iter()
            .map(|ext| {
                let s = &self.wasm.home[ext];
                let rows = s
                    .rows
                    .iter()
                    .map(|r| HomeRow {
                        name: r.name.clone(),
                        detail: r.detail.clone(),
                        mark: match &r.mark {
                            HomeMark::None => RowMark::None,
                            HomeMark::Live(t) => RowMark::Live(t.clone()),
                            HomeMark::Task(urgent) => RowMark::Task(*urgent),
                        },
                        entry: HomeEntry::Code { ext: ext.clone(), value: r.value.clone() },
                    })
                    .collect();
                let leak = |t: &str| -> &'static str { Box::leak(t.to_string().into_boxed_str()) };
                HomeSection { title: leak(&s.title), key: leak(&s.key), column: HomeColumn::Tasks, rank: 10, rows, max: Some(s.most.max(1) as usize) }
            })
            .collect()
    }

    /// Text pasted on code page `buffer` while it's typing.
    pub(super) fn code_page_paste(&mut self, buffer: BufferId, text: String) {
        let Some(state) = self.pages.get_mut(&buffer) else { return };
        let PageModel::Code(p) = &mut state.model else { return };
        let (ext, page) = (p.ext.clone(), p.page);
        p.width = None;
        state.stale = true;
        self.code_call(&ext, Call::Paste { page, text }, PAGE_TICKS, Pending::Other);
    }

    /// Page buffer `buffer`, if it's a code extension's, is going because
    /// you closed it: its extension is told (`page-closed`), and lets go of
    /// what it kept for it.
    pub(super) fn code_page_closed(&mut self, buffer: BufferId) {
        let Some(PageModel::Code(p)) = self.pages.get(&buffer).map(|s| &s.model) else { return };
        let (ext, page) = (p.ext.clone(), p.page);
        if p.stopped.is_none() && matches!(self.wasm.status.get(&ext), Some(Status::Ready)) {
            self.code_call(&ext, Call::PageClosed(page), PAGE_TICKS, Pending::Other);
        }
        self.wasm.state_asked.remove(&buffer);
    }

    /// Extension `ext`'s decorations of `buffer`, in place of those it set
    /// before; the others' first moved with the edits since they were
    /// last.
    fn set_code_decorations(&mut self, ext: &str, buffer: BufferId, list: &[fenix_wasm::host::Decoration]) {
        let Some(ob) = self.buffers.get_mut(buffer) else { return };
        ob.buffer.track_anchors();
        let edits = ob.buffer.take_anchor_edits();
        let sets = self.wasm.decorations.entry(buffer).or_default();
        for (_, set) in sets.iter_mut() {
            crate::decorations::shift(set, &edits);
        }
        sets.retain(|(e, _)| e != ext);
        let new: Vec<crate::decorations::Decoration> = list.iter().filter_map(|w| crate::decorations::from_wit(&ob.buffer, w).map(|d| crate::decorations::Decoration { id: w.id, ..d })).collect();
        if !new.is_empty() {
            sets.push((ext.to_string(), new));
        }
        if sets.is_empty() {
            self.wasm.decorations.remove(&buffer);
        }
        self.request_window_redraw();
    }

    /// Buffer `buffer` highlighted as `language` (`buffer-language-set`), or
    /// as its file's name says again.
    fn set_code_language(&mut self, ext: &str, buffer: BufferId, language: Option<String>) {
        let lang = match &language {
            Some(name) => match fenix_syntax::LanguageId::from_name(name) {
                Some(lang) => Some(lang),
                None => {
                    let names: Vec<String> = fenix_syntax::LanguageId::all().into_iter().map(|l| l.name()).collect();
                    return self.note_output(ext, format!("  warn  buffer-language-set: no language \"{name}\" here -- one of {}", names.join(", ")));
                }
            },
            None => None,
        };
        let Some(ob) = self.buffers.get_mut(buffer) else { return };
        let lang = lang.or_else(|| ob.buffer.path().and_then(fenix_syntax::detect_language_from_path));
        // Edits made before are in the text it's parsed from now.
        ob.buffer.drain_edits();
        ob.syntax = lang.map(|lang| fenix_syntax::SyntaxState::new(lang, &ob.buffer.text()));
        self.request_window_redraw();
    }

    /// Extension `ext`'s task `id`, checked, run beside the task pane: its
    /// output in a buffer of its own, its end told to it.
    #[allow(clippy::too_many_arguments)]
    fn start_background_task(&mut self, ext: &str, id: u64, name: String, program: String, args: Vec<String>, cwd: Option<PathBuf>, env: Vec<(String, String)>) {
        let Some(root) = cwd.or_else(|| self.wasm.projects.get(ext).cloned().flatten()).or_else(|| self.project_root.clone()) else {
            self.note_output(ext, format!("  warn  {name}: no project to run it in"));
            return self.code_deliver(ext, Notice::TaskFinished((id, false)));
        };
        let spec = fenix_project::tools::CommandSpec { env: env.into_iter().collect(), ..fenix_project::tools::CommandSpec::new(program.clone(), args.clone()) };
        let buffer = self.buffers.open_task_output(&format!("$ {program} {}\n", args.join(" ")));
        self.ref_view_names.insert(buffer, format!("*{name}*"));
        let run = fenix_tasks::RunId::next();
        self.wasm.background.insert(buffer, BackgroundTask { ext: ext.to_string(), id, run, runner: None });
        let command = match spec.command(&root) {
            Ok(command) => command,
            Err(e) => return self.background_task_event(buffer, run, fenix_tasks::TaskEvent::Finished(fenix_tasks::TaskOutcome::Failed(format!("couldn't start `{program}`: {e}")))),
        };
        if let Some(proxy) = self.event_proxy.clone() {
            match fenix_tasks::TaskRunner::spawn_command(command, move |event| proxy.send_event(super::FenixUserEvent::TaskEvent(buffer, run, event)).is_ok()) {
                Ok(runner) => {
                    if let Some(task) = self.wasm.background.get_mut(&buffer) {
                        task.runner = Some(runner);
                    }
                }
                Err(e) => self.background_task_event(buffer, run, fenix_tasks::TaskEvent::Finished(fenix_tasks::TaskOutcome::Failed(format!("couldn't start `{program}`: {e}")))),
            }
            return;
        }
        // No event loop (tests): to its end, here.
        let (tx, rx) = std::sync::mpsc::channel();
        match fenix_tasks::TaskRunner::spawn_command(command, move |event| tx.send(event).is_ok()) {
            Ok(runner) => {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                while let Ok(event) = rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) {
                    let finished = matches!(event, fenix_tasks::TaskEvent::Finished(_));
                    self.background_task_event(buffer, run, event);
                    if finished {
                        return;
                    }
                }
                runner.cancel();
                self.background_task_event(buffer, run, fenix_tasks::TaskEvent::Finished(fenix_tasks::TaskOutcome::Cancelled));
            }
            Err(e) => self.background_task_event(buffer, run, fenix_tasks::TaskEvent::Finished(fenix_tasks::TaskOutcome::Failed(format!("couldn't start `{program}`: {e}")))),
        }
    }

    /// What one of the extensions' background tasks did: its output, and
    /// its end, told to it.
    pub(super) fn background_task_event(&mut self, buffer: BufferId, run: fenix_tasks::RunId, event: fenix_tasks::TaskEvent) {
        if !self.wasm.background.get(&buffer).is_some_and(|t| t.run == run) {
            return;
        }
        match event {
            fenix_tasks::TaskEvent::Output(lines) => {
                for line in lines {
                    self.append_task_output_text(buffer, &line.text);
                }
            }
            fenix_tasks::TaskEvent::Finished(outcome) => {
                let success = match outcome {
                    fenix_tasks::TaskOutcome::Exited { success, .. } => {
                        self.append_task_output_text(buffer, if success { "-- done --" } else { "-- failed --" });
                        success
                    }
                    fenix_tasks::TaskOutcome::Cancelled => {
                        self.append_task_output_text(buffer, "-- task cancelled --");
                        false
                    }
                    fenix_tasks::TaskOutcome::Failed(error) => {
                        self.append_task_output_text(buffer, &format!("-- task error: {error} --"));
                        false
                    }
                };
                let Some(task) = self.wasm.background.get_mut(&buffer) else { return };
                task.runner = None;
                let (ext, id) = (task.ext.clone(), task.id);
                self.code_deliver(&ext, Notice::TaskFinished((id, success)));
            }
        }
    }

    /// Buffer `buffer` of extension `ext`'s, opened in its pane beside the
    /// focused one -- split off, `share` of it, when it has none here --
    /// the focus staying where it was.
    fn code_open_beside(&mut self, ext: &str, buffer: BufferId, share: f32) {
        let back = self.focused_pane_id();
        let kept = self.wasm.beside.get(ext).copied().filter(|pane| *pane != back && self.windows().windows().contains(pane));
        match kept {
            Some(pane) => {
                self.windows_mut().focus(pane);
                self.open_buffer_in_focused_pane(buffer);
            }
            None => {
                self.split_vertical();
                self.open_buffer_in_focused_pane(buffer);
                let pane = self.focused_pane_id();
                self.workspaces.active_pane_tabs_mut().insert(pane, vec![buffer]);
                // The split starts even; this side (the right) gets `share`.
                self.windows_mut().set_split_near(pane, 1.0 - share);
                self.wasm.beside.insert(ext.to_string(), pane);
            }
        }
        self.windows_mut().focus(back);
        self.wake_caret();
    }

    /// Extension `ext`'s problems in the file on `path` (`diagnostics-set`),
    /// in place of those it gave before, as a language server's are.
    fn set_code_problems(&mut self, ext: &str, path: &Path, problems: Vec<fenix_wasm::host::Problem>, fixes: Vec<Vec<fenix_wasm::host::QuickFix>>) {
        use fenix_wasm::host::Severity;
        let canonical = fenix_lsp::normalize(std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
        // Columns come in characters; Fenix keeps them as language servers
        // count them, in UTF-16 units.
        let buffer = self.code_buffer_on(path, false);
        let text = match buffer.and_then(|b| self.buffers.get(b)) {
            Some(ob) => Some(ob.buffer.text()),
            None => std::fs::read_to_string(path).ok(),
        };
        let lines: Vec<&str> = text.as_deref().map(|t| t.split('\n').collect()).unwrap_or_default();
        let position = |(line, column): (u32, u32)| {
            let character = match lines.get(line as usize) {
                Some(text) => text.chars().take(column as usize).map(char::len_utf16).sum::<usize>() as u32,
                None => column,
            };
            lsp_types::Position { line, character }
        };
        let range = |start, end| lsp_types::Range { start: position(start), end: position(end) };
        // Its fixes, each with its problem's range: offered while
        // the buffer is as it is now.
        let mut offered = Vec::new();
        for (problem, fixes) in problems.iter().zip(fixes) {
            for fix in fixes {
                let edits = fix.edits.into_iter().map(|e| lsp_types::TextEdit { range: range(e.start, e.end), new_text: e.text }).collect();
                offered.push((range(problem.start, problem.end), fix.title, edits));
            }
        }
        let key = (ext.to_string(), canonical.clone());
        if offered.is_empty() {
            self.wasm.fixes.remove(&key);
        } else {
            let edits = buffer.and_then(|b| self.buffers.get(b).map(|ob| (b, ob.buffer.edit_count())));
            self.wasm.fixes.insert(key, CodeFixes { edits, fixes: offered });
        }
        let diagnostics = problems
            .into_iter()
            .map(|p| lsp_types::Diagnostic {
                range: range(p.start, p.end),
                severity: Some(match p.severity {
                    Severity::Error => lsp_types::DiagnosticSeverity::ERROR,
                    Severity::Warning => lsp_types::DiagnosticSeverity::WARNING,
                    Severity::Info => lsp_types::DiagnosticSeverity::INFORMATION,
                    Severity::Hint => lsp_types::DiagnosticSeverity::HINT,
                }),
                code: p.code.map(lsp_types::NumberOrString::String),
                source: Some(p.source.unwrap_or_else(|| ext.to_string())),
                message: p.message,
                ..Default::default()
            })
            .collect();
        let root = self.wasm.projects.get(ext).cloned().flatten().unwrap_or_else(|| path.parent().map(Path::to_path_buf).unwrap_or_default());
        let key = super::LspKey { server: super::LspServer::Additional(problems_key(ext)), root };
        self.publish_diagnostics(canonical, key, ext, diagnostics);
        self.request_window_redraw();
    }

    /// The fixes extensions gave for problems on line `line` of the file
    /// at `canonical` (`diagnostics-set-ex`), as code actions: those whose
    /// file hasn't been edited since, each with whose it is.
    pub(crate) fn code_fixes_on(&self, canonical: &Path, line: u32) -> Vec<(String, lsp_types::CodeAction)> {
        let Some(uri) = fenix_lsp::path_to_uri(canonical) else { return Vec::new() };
        let mut out = Vec::new();
        for ((ext, path), given) in &self.wasm.fixes {
            if path != canonical {
                continue;
            }
            let still = match given.edits {
                Some((buffer, edits)) => self.buffers.get(buffer).is_some_and(|ob| ob.buffer.edit_count() == edits),
                None => self.buffer_id_for_lsp_path(canonical).is_none(),
            };
            if !still {
                continue;
            }
            for (range, title, edits) in given.fixes.iter().filter(|(r, _, _)| r.start.line <= line && line <= r.end.line) {
                let edit = lsp_types::WorkspaceEdit { changes: Some(std::collections::HashMap::from([(uri.clone(), edits.clone())])), ..Default::default() };
                let diagnostic = lsp_types::Diagnostic { range: *range, message: title.clone(), ..Default::default() };
                out.push((ext.clone(), lsp_types::CodeAction { title: title.clone(), kind: Some(lsp_types::CodeActionKind::QUICKFIX), diagnostics: Some(vec![diagnostic]), edit: Some(edit), ..Default::default() }));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Every open doctor's page on the project at `root`, checked again.
    fn doctor_recheck(&mut self, root: &Path) {
        let pages: Vec<BufferId> = self.pages.iter().filter(|(_, state)| matches!(&state.model, PageModel::Doctor(d) if same_dir(&d.root, root))).map(|(id, _)| *id).collect();
        for page in pages {
            self.doctor_check(page);
        }
    }

    /// The decorations moved with their buffers' edits, ahead of a frame;
    /// a closed buffer's gone.
    pub(crate) fn shift_code_decorations(&mut self) {
        let buffers = &mut self.buffers;
        self.wasm.decorations.retain(|id, sets| {
            let Some(ob) = buffers.get_mut(*id) else { return false };
            let edits = ob.buffer.take_anchor_edits();
            for (_, set) in sets.iter_mut() {
                crate::decorations::shift(set, &edits);
            }
            true
        });
    }

    /// The extensions' rules for decorating files, applied to each buffer on
    /// one of their files that changed since they last were, or that's new
    /// -- ahead of a frame, so a file is never drawn without them.
    pub(crate) fn apply_decoration_rules(&mut self) {
        if self.wasm.rules.is_empty() && self.wasm.ruled.is_empty() {
            return;
        }
        let given = self.wasm.rules_given;
        for id in self.buffers.ids_sorted_by_path() {
            let Some(ob) = self.buffers.get_mut(id) else { continue };
            let count = ob.buffer.edit_count();
            if self.wasm.ruled.get(&id) == Some(&(count, given)) {
                continue;
            }
            self.wasm.ruled.insert(id, (count, given));
            let path = ob.buffer.path().filter(|_| ob.kind.tracks_unsaved_changes() && !self.wasm.sealed.contains(&id)).map(Path::to_path_buf);
            let small = ob.buffer.len_chars() <= MAX_RULED;
            let sets = self.wasm.decorations.entry(id).or_default();
            for (ext, rules) in &self.wasm.rules {
                let key = rules_key(ext);
                sets.retain(|(e, _)| *e != key);
                let Some(path) = path.as_deref().filter(|_| small) else { continue };
                let mine: Vec<&Rule> = rules.iter().filter(|r| fenix_extensions::manifest::file_matches(&r.files, path, None)).collect();
                if mine.is_empty() {
                    continue;
                }
                ob.buffer.track_anchors();
                let new: Vec<crate::decorations::Decoration> = by_rules(&ob.buffer, &mine).iter().filter_map(|d| crate::decorations::from_wit(&ob.buffer, d)).collect();
                if !new.is_empty() {
                    sets.push((key, new));
                }
            }
            // Those of rules taken away.
            sets.retain(|(e, _)| !e.ends_with(":#rules") || self.wasm.rules.contains_key(e.trim_end_matches(":#rules")));
            if sets.is_empty() {
                self.wasm.decorations.remove(&id);
            }
        }
        let buffers = &self.buffers;
        self.wasm.ruled.retain(|id, _| buffers.get(*id).is_some());
    }

    /// A click at `pos` on a decoration an extension tagged
    /// (`decorations-set-tagged`), told to it (`decoration-clicked`): the
    /// click is its alone. False when there's none there.
    pub(crate) fn decoration_click(&mut self, pos: (f32, f32)) -> bool {
        use fenix_wasm::host::TextBuffer;
        let (cw, lh) = self.wasm.layers.geometry.cell;
        let Some(pane) = self.wasm.layers.geometry.panes.iter().find(|p| p.rect.contains_point(pos.0, pos.1)).cloned() else { return false };
        if cw <= 0.0 || lh <= 0.0 || pos.0 < pane.text_x || pos.1 < pane.text_y {
            return false;
        }
        let Some(window) = self.windows().windows().into_iter().find(|w| w.number() as u64 == pane.id) else { return false };
        let Some(&buffer) = self.windows().content(window) else { return false };
        let Some(sets) = self.wasm.decorations.get(&buffer).filter(|sets| sets.iter().any(|(_, set)| set.iter().any(|d| d.id.is_some()))) else { return false };
        let display = self.pane_display_lines(window, buffer);
        let base = display.iter().position(|&l| l >= pane.base_line).unwrap_or(0);
        let row = ((pos.1 - pane.text_y) / lh).floor() as usize;
        let col = ((pos.0 - pane.text_x) / cw).floor() as usize + pane.scroll_col;
        let Some(&line) = display.get(base + row) else { return false };
        let Some(ob) = self.buffers.get(buffer) else { return false };
        if line >= ob.buffer.line_count() {
            return false;
        }
        let start = ob.buffer.line_start_char(line);
        let len = ob.buffer.line_len(line);
        let here: Vec<&crate::decorations::Decoration> = sets.iter().flat_map(|(_, set)| set.iter()).filter(|d| d.start <= start + len && d.end >= start).collect();
        let cursor_line = ob.buffer.line_col(&self.pane_state(window).cursor).0;
        let layout = crate::decorations::layout_line(&ob.buffer.text_range(start, start + len), start, &here, &self.tab_stops_for(buffer), cursor_line == line);
        let Some(&(_, id)) = layout.clicks.iter().find(|(cols, _)| cols.contains(&col)) else { return false };
        // Whose it is: the extension that set a decoration with that id there.
        let Some(ext) = sets.iter().find(|(_, set)| set.iter().any(|d| d.id == Some(id) && d.start <= start + len && d.end >= start)).map(|(e, _)| e.clone()) else { return false };
        let target = match self.wasm.scratches.get(&buffer) {
            Some(s) if s.ext == ext => TextBuffer::Scratch(s.id),
            _ => match ob.buffer.path() {
                Some(path) => TextBuffer::File(fenix_lsp::normalize(path.to_path_buf()).to_string_lossy().into_owned()),
                None => return false,
            },
        };
        self.windows_mut().focus(window);
        if self.code_ready(&ext) {
            self.code_call(&ext, Call::DecorationClicked { buffer: target, id }, COMMAND_TICKS, Pending::Other);
        }
        true
    }

    /// Every extension's decorations of `buffer`.
    pub(crate) fn code_decorations(&self, buffer: BufferId) -> Vec<&crate::decorations::Decoration> {
        self.wasm.decorations.get(&buffer).map(|sets| sets.iter().flat_map(|(_, set)| set.iter()).collect()).unwrap_or_default()
    }

    /// Gives `notice` to extension `ext`, if it's running.
    pub(super) fn code_deliver(&mut self, ext: &str, notice: Notice) {
        if matches!(self.wasm.status.get(ext), Some(Status::Ready)) {
            self.code_call(ext, Call::Deliver(notice), COMMAND_TICKS, Pending::Other);
        }
    }

    /// Extension `ext`'s folders (`permissions.folders`), where your
    /// settings put them now; one that's not set isn't there.
    fn code_folders(&self, ext: &CodeExtension) -> Vec<(String, PathBuf)> {
        let project = self.wasm.projects.get(&ext.id).cloned().flatten().or_else(|| self.code_project());
        let mut many = Vec::new();
        let mut one: Vec<(String, PathBuf)> = ext
            .manifest
            .permissions
            .folders
            .iter()
            .filter_map(|(name, path)| {
                // `${setting:key}`, and after `|`, where it is when that's
                // not set: `data:<name>`, a folder in Fenix's data folder.
                let (path, fallback) = path.split_once('|').map_or((path.as_str(), None), |(p, f)| (p, Some(f.trim())));
                let set = match path.trim().strip_prefix("${setting:").and_then(|k| k.strip_suffix('}')) {
                    Some(key) => {
                        // The project's own, for a setting a project may
                        // set (`mib.roots`), else yours.
                        let theirs = project.as_deref().filter(|_| fenix_config::schema::setting(key).is_some_and(|s| s.project)).and_then(|root| fenix_config::ProjectSettings::load(root).get(key).cloned());
                        let value = theirs.or_else(|| fenix_config::schema::setting(key).and_then(|s| s.get(&self.config))).or_else(|| self.config.value(key).cloned());
                        // A map of folders, each at `/folders/<name>/<its
                        // name>`; one relative to the project is in it.
                        if let Some(fenix_config::Value::Map(entries)) = &value {
                            for (label, dir) in entries {
                                let dir = fenix_fs::expand(dir.trim());
                                let dir = match &project {
                                    Some(root) if dir.is_relative() => root.join(dir),
                                    _ => dir,
                                };
                                if !label.is_empty() && !label.contains(['/', '\\']) && label != ".." {
                                    many.push((format!("{name}/{label}"), dir));
                                }
                            }
                            return None;
                        }
                        value.map(|v| as_text(&v)).unwrap_or_default()
                    }
                    None => path.to_string(),
                };
                let dir = match (set.trim(), fallback.and_then(|f| f.strip_prefix("data:"))) {
                    ("", Some(sub)) => fenix_storage::paths::data_file(sub)?,
                    ("", None) => return None,
                    (set, _) => fenix_fs::expand(set),
                };
                // Made the first time, as Fenix 1.x made the notebook's:
                // a folder that isn't there can't be given to it.
                if !dir.is_dir() {
                    if let Err(e) = std::fs::create_dir_all(&dir) {
                        eprintln!("fenix: couldn't make {} for {}: {e}", dir.display(), ext.id);
                    }
                }
                Some((name.clone(), dir))
            })
            .collect();
        // A map's folders are its own: one that isn't there is left out,
        // not made.
        one.extend(many);
        one
    }

    /// Setting `key` changed: the extension it's one of reads it anew.
    pub(crate) fn code_setting_changed(&mut self, key: &str) {
        // A folder of its moved: it starts again, with it there.
        let moved: Vec<String> = self.wasm.installed.iter().filter(|e| e.manifest.permissions.folders.values().any(|p| p.contains(&format!("${{setting:{key}}}"))) && self.wasm.workers.contains_key(&e.id)).map(|e| e.id.clone()).collect();
        for ext in moved {
            self.restart_code_extension(&ext);
        }
        // Its owner, and those that may read it (`permissions.settings`).
        let namespace = key.split('.').next().unwrap_or(key);
        let readers: Vec<CodeExtension> = self
            .wasm
            .installed
            .iter()
            .filter(|e| e.manifest.contributes.settings.iter().any(|s| s.key == key) || fenix_extensions::manifest::secret_allowed(&e.manifest.permissions.secrets, key) || (key.contains('.') && e.manifest.permissions.settings.iter().any(|n| n == namespace)))
            .cloned()
            .collect();
        for ext in readers {
            let (settings, secrets) = self.code_settings_now(&ext);
            if let Some(shared) = self.wasm.settings.get(&ext.id) {
                *shared.write().unwrap() = settings;
            }
            if let Some(shared) = self.wasm.secrets.get(&ext.id) {
                *shared.write().unwrap() = secrets;
            }
            self.code_deliver(&ext.id, Notice::SettingChanged(key.to_string()));
        }
    }

    /// What a worker sent.
    pub(crate) fn apply_wasm_event(&mut self, ext: String, event: Event) {
        match event {
            Event::Ready => {
                self.wasm.status.insert(ext.clone(), Status::Ready);
                self.note_output(&ext, "-- ready".into());
                for (what, ticks, pending) in self.wasm.queued.remove(&ext).unwrap_or_default() {
                    self.code_call(&ext, what, ticks, pending);
                }
                // Where you are already, for one that reads the project.
                self.code_focus_changed();
            }
            Event::Said(source, text) => {
                let tag = match source {
                    Source::Log(Level::Debug) => "debug",
                    Source::Log(Level::Info) => "info",
                    Source::Log(Level::Warn) => "warn",
                    Source::Log(Level::Error) => "error",
                    Source::Stdout => "stdout",
                    Source::Stderr => "stderr",
                };
                for line in text.lines() {
                    self.note_output(&ext, format!("{tag:>6}  {line}"));
                }
            }
            Event::Asked(request) => self.answer_code_request(&ext, request),
            Event::Finished { call, result } => {
                let pending = self.wasm.calls.remove(&(ext.clone(), call)).unwrap_or(Pending::Other);
                self.code_call_finished(&ext, pending, result);
            }
            Event::Stopped(why) => {
                self.note_output(&ext, format!("-- {why}"));
                self.wasm.status.insert(ext.clone(), Status::Stopped(why.clone()));
                if self.wasm.prompt.as_ref().is_some_and(|p| p.ext == ext) {
                    self.wasm.prompt = None;
                }
                if let Some(queued) = self.wasm.queued.remove(&ext) {
                    if queued.iter().any(|(_, _, p)| matches!(p, Pending::Command(_))) {
                        self.set_error(format!("{why} -- SPC h l has what it said"));
                    }
                }
                self.mark_code_pages(&ext, |p| p.stopped = Some(why.clone()));
                self.wasm.focus_told.remove(&ext);
                self.wasm.layers.forget(&ext);
                self.forget_code_lsp_sessions(&ext);
            }
        }
        self.refresh_extension_output();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn code_call_finished(&mut self, ext: &str, pending: Pending, result: Result<Reply, String>) {
        match (pending, result) {
            (Pending::Render(buffer), Ok(Reply::Page(page))) => {
                let page = *page;
                let mut first = None;
                if let Some(state) = self.pages.get_mut(&buffer) {
                    if let PageModel::Code(p) = &mut state.model {
                        if p.state.is_none() {
                            first = Some(p.page);
                        }
                        p.shown = Some(page);
                        p.width = p.asked.take();
                    }
                    state.stale = true;
                }
                if let Some(page) = first {
                    self.code_call(ext, Call::PageState(page), PAGE_TICKS, Pending::State(buffer));
                }
            }
            (Pending::State(buffer), Ok(Reply::State(state))) => {
                self.wasm.state_asked.remove(&buffer);
                if let Some(PageModel::Code(p)) = self.pages.get_mut(&buffer).map(|s| &mut s.model) {
                    p.state = Some(state.unwrap_or_default());
                }
            }
            (Pending::State(buffer), Err(_)) => {
                self.wasm.state_asked.remove(&buffer);
            }
            (Pending::Restore(buffer), Ok(Reply::Restored(false)) | Err(_)) => {
                self.note_output(ext, "-- a page from the last session couldn't be opened again".into());
                self.close_page_quietly(buffer);
            }
            (Pending::Render(buffer), Err(_)) => {
                if let Some(PageModel::Code(p)) = self.pages.get_mut(&buffer).map(|s| &mut s.model) {
                    p.asked = None;
                }
            }
            (Pending::Serve(call), Ok(Reply::Served(Some(answer)))) => self.code_answered(ext, call, answer),
            // It answers later, through `service-answer`.
            (Pending::Serve(_), Ok(Reply::Served(None))) => {}
            (Pending::Serve(call), Err(why)) => self.code_answered(ext, call, Err(why)),
            (Pending::Command(command), Err(why)) => {
                self.note_output(ext, format!(" error  {command}: {why}"));
                self.set_error(format!("{command}: {why}"));
            }
            _ => {}
        }
    }

    /// What extension `ext` asked of Fenix, done.
    fn answer_code_request(&mut self, ext: &str, request: Request) {
        match request {
            Request::Notify(Level::Error | Level::Warn, text) => self.set_error(text),
            Request::Notify(_, text) => self.set_message(text),
            Request::RunCommand(id) => self.run_command_for_extension(&id),
            Request::OpenFile { path, line, sealed, text } => {
                // Sealed unless you had it open already.
                let sealed = sealed && self.buffer_on(&path).is_none();
                // Its buffer, from what was read as it was checked, so a
                // link put in its place since isn't what's opened.
                let as_text = self.code_opener(&path).is_none_or(|e| e == ext);
                if let Some(text) = text.filter(|_| !sealed && as_text && self.buffer_on(&path).is_none()) {
                    let id = self.buffers.open_path_with_text(&path, &text);
                    self.note_disk_state(id);
                }
                // From a page opened beside a pane (a sidebar, a preview):
                // in another pane, the page staying where it is.
                let focused = self.focused_buffer_id();
                if matches!(self.pages.get(&focused).map(|s| &s.model), Some(PageModel::Code(p)) if p.beside) {
                    let pane = self.focused_pane_id();
                    if let Some(other) = self.windows().windows().into_iter().find(|&w| w != pane) {
                        self.windows_mut().focus(other);
                    }
                }
                if sealed {
                    // As text, whatever opens it: an extension it went to
                    // would be given it.
                    if !self.open_file_as_text(&path) {
                        return;
                    }
                    if let Some(id) = self.buffer_on(&path) {
                        self.wasm.sealed.insert(id);
                    }
                } else if self.code_opener(&path).as_deref() == Some(ext) {
                    // One it opens itself: as text, not back to it.
                    if !self.open_file_as_text(&path) {
                        return;
                    }
                } else if let Some(page) = line.filter(|p| *p > 0 && self.code_opener(&path).is_some()) {
                    // A file an extension opens itself (a PDF) opens at the
                    // page given as its line.
                    self.open_with_extension_at(&path, Some(page), None);
                    return;
                } else {
                    self.open_file_from_picker(&path);
                }
                if let Some(line) = line {
                    let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
                    let line = (line as usize).saturating_sub(1).min(buffer.visual_line_count().saturating_sub(1));
                    cursor.char_idx = buffer.line_start_char(line);
                    cursor.sticky_col = 0;
                }
            }
            Request::Picker { id, title, items } => {
                let candidates = items.into_iter().map(|i| fenix_picker::Candidate::new(if i.detail.is_empty() { i.label } else { format!("{}  {}", i.label, i.detail) }, i.value)).collect();
                self.completion = None;
                let badge: &'static str = Box::leak(title.to_uppercase().into_boxed_str());
                self.enter_picker(ActivePicker::Code { ext: ext.to_string(), id, badge, many: false, state: fenix_picker::PickerState::new(candidates) });
            }
            Request::PickerMany { id, title, items } => {
                let candidates = items.into_iter().map(|i| fenix_picker::Candidate::new(if i.detail.is_empty() { i.label } else { format!("{}  {}", i.label, i.detail) }, i.value)).collect();
                self.completion = None;
                let badge: &'static str = Box::leak(title.to_uppercase().into_boxed_str());
                self.enter_picker(ActivePicker::Code { ext: ext.to_string(), id, badge, many: true, state: fenix_picker::PickerState::new(candidates) });
            }
            Request::Compose { id, title, verb, initial, allow_empty } => {
                let verb: &'static str = Box::leak(verb.into_boxed_str());
                self.open_code_compose(ext, id, title, verb, allow_empty, &initial);
            }
            Request::Home(section) => {
                match section {
                    Some(section) => self.wasm.home.insert(ext.to_string(), section),
                    None => self.wasm.home.remove(ext),
                };
                self.refresh_home_data(false);
            }
            Request::SettingSet { key, value } => self.code_setting_set(ext, &key, value),
            Request::SecretSet { key, value } => self.code_secret_set(ext, &key, value.map(|v| v.0)),
            Request::ImageLoad { id, bytes } => {
                let key = image_key(ext, id);
                self.page_images.pending.insert(key.clone());
                self.page_spawn(move |send| send(PageEvent::PageImage { key, result: super::reading_host::decode_picture_bytes(&bytes) }));
            }
            Request::ImageFree(id) => {
                let key = image_key(ext, id);
                self.page_images.textures.remove(&key);
                self.page_images.image_sizes.remove(&key);
            }
            Request::ServiceAnswer { call, answer } => self.code_answered(ext, call, answer),
            Request::Prompt { id, label, initial, masked } => {
                self.wasm.prompt = Some(CodePrompt { ext: ext.to_string(), id, label, input: initial, masked });
                self.wake_caret();
            }
            Request::PageOpen { page, kind, title } => {
                let mut model = CodePage::new(ext, page, &kind, &title);
                model.file = self.code_opening(ext);
                self.open_page(PageModel::Code(Box::new(model)));
            }
            Request::PageOpenBeside { page, kind, title, width } => {
                let back = self.focused_pane_id();
                let mut model = CodePage::new(ext, page, &kind, &title);
                model.beside = true;
                model.file = self.code_opening(ext);
                self.open_page_beside(PageModel::Code(Box::new(model)), width);
                self.windows_mut().focus(back);
            }
            Request::ClipboardImage(png) => match decode_rgba(&png) {
                Ok((width, height, rgba)) => {
                    if let Some(clipboard) = self.clipboard.as_mut() {
                        match clipboard.set_image(arboard::ImageData { width: width as usize, height: height as usize, bytes: rgba.into() }) {
                            Ok(()) => self.set_message("Copied the picture"),
                            Err(e) => self.set_error(format!("couldn't copy the picture: {e}")),
                        }
                    }
                }
                Err(e) => self.note_output(ext, format!("  warn  clipboard-set-image: {e}")),
            },
            Request::ClipboardImageRead(answer) => {
                let image = self.clipboard.as_mut().and_then(|c| c.get_image().ok());
                answer.send(image.and_then(|i| encode_png(i.width as u32, i.height as u32, i.bytes.into_owned())));
            }
            Request::BufferText { path, open_only, answer } => {
                let text = self.code_buffer_on(&path, open_only).and_then(|id| self.buffers.get(id)).map(|ob| ob.buffer.text());
                answer.send(text);
            }
            Request::BufferEdit { path, open_only, edits, text } => {
                let id = match self.code_buffer_on(&path, open_only) {
                    Some(id) => id,
                    None if open_only => return self.note_output(ext, format!("  warn  buffer-edit: {} isn't open, and it may not open it", path.display())),
                    // As it was read when it was checked.
                    None => match text {
                        Some(text) => self.buffers.open_path_with_text(&path, &text),
                        None => return self.note_output(ext, format!("  warn  buffer-edit: {} isn't there, or isn't text", path.display())),
                    },
                };
                if let Some(ob) = self.buffers.get_mut(id) {
                    apply_text_edits(&mut ob.buffer, edits);
                }
            }
            Request::BufferInsert(text) => {
                let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
                buffer.insert_str(cursor, &text);
            }
            Request::BufferSave { path, open_only } => {
                let saved = self.code_buffer_on(&path, open_only).and_then(|id| self.buffers.get_mut(id)).map(|ob| ob.buffer.save());
                if let Some(Err(e)) = saved {
                    self.set_error(format!("couldn't save {}: {e}", path.display()));
                }
            }
            Request::PageRefresh(page) => {
                if let Some(buffer) = self.code_page_buffer(ext, page) {
                    if let Some(state) = self.pages.get_mut(&buffer) {
                        if let PageModel::Code(p) = &mut state.model {
                            p.width = None;
                        }
                        state.stale = true;
                    }
                }
            }
            Request::PageClose(page) => {
                if let Some(buffer) = self.code_page_buffer(ext, page) {
                    self.close_page_quietly(buffer);
                }
            }
            Request::PageFocus(page) => {
                let Some(buffer) = self.code_page_buffer(ext, page) else { return };
                if let Some(file) = self.code_opening(ext) {
                    if let Some(PageModel::Code(p)) = self.pages.get_mut(&buffer).map(|s| &mut s.model) {
                        p.file = Some(file);
                    }
                }
                // A surface is drawn in one pane at a time: the others
                // showing it go on to their next tab.
                if self.code_surface(buffer).is_some() {
                    let here = self.focused_pane_id();
                    let others: Vec<fenix_window::WindowId> = self.windows().windows().into_iter().filter(|&p| p != here && self.windows().content(p) == Some(&buffer)).collect();
                    for pane in others {
                        self.close_pane_tab(pane, buffer);
                    }
                }
                self.show_page(buffer);
                self.code_surface_sync_capture();
                self.request_window_redraw();
            }
            Request::Terminal { id, program, args, cwd, title, below } => {
                let buffer = self.buffers.open_terminal();
                if let Some(cwd) = cwd.or_else(|| self.project_root.clone()) {
                    self.terminal_buffer_cwds.insert(buffer, cwd);
                }
                self.terminal_buffer_labels.insert(buffer, format!("*{title}*"));
                self.terminal_buffer_programs.insert(buffer, (program, args));
                self.wasm.terminals.insert(buffer, (ext.to_string(), id));
                if below {
                    let pane = self.windows_mut().split(SplitKind::Horizontal, buffer);
                    self.workspaces.active_pane_states_mut().insert(pane, PaneState::seeded_at(Cursor::at_start()));
                    self.windows_mut().resize_focused(0.35);
                    self.windows_mut().focus(pane);
                    self.pane_titles.insert(pane, title);
                } else {
                    self.open_buffer_in_focused_pane(buffer);
                }
                self.spawn_terminal_for(buffer);
                self.wake_caret();
            }
            Request::TerminalStop(id) => {
                if let Some(buffer) = self.code_terminal(ext, id) {
                    self.terminal_buffers.remove(&buffer);
                    if self.terminal_buffer_focused == Some(buffer) {
                        self.terminal_buffer_focused = None;
                    }
                }
            }
            Request::TerminalRestart { id, program, title } => {
                let Some(buffer) = self.code_terminal(ext, id) else { return };
                if let Some((program, args, cwd)) = program {
                    if let Some(cwd) = cwd {
                        self.terminal_buffer_cwds.insert(buffer, cwd);
                    }
                    self.terminal_buffer_programs.insert(buffer, (program, args));
                }
                if let Some(title) = title {
                    self.terminal_buffer_labels.insert(buffer, format!("*{title}*"));
                    if let Some(pane) = self.windows().windows().into_iter().find(|&p| self.windows().content(p) == Some(&buffer)) {
                        self.pane_titles.insert(pane, title);
                    }
                }
                self.terminal_buffers.remove(&buffer);
                self.spawn_terminal_for(buffer);
            }
            Request::LanguageServer { languages, program } => {
                let Some(root) = self.wasm.projects.get(ext).cloned().flatten() else { return self.note_output(ext, "-- language-server-set: it has no project".into()) };
                let key = (ext.to_string(), root.clone());
                match program {
                    // Where it said, with its environment.
                    Some(given) => {
                        self.wasm.servers.insert(key, (languages, checked_spec(given)));
                    }
                    None => {
                        self.wasm.servers.remove(&key);
                    }
                }
                self.restart_language_servers_in(&root);
            }
            Request::LanguageServerAdd { name, languages, program } => {
                let Some(root) = self.wasm.projects.get(ext).cloned().flatten() else { return self.note_output(ext, "-- language-server-add: it has no project".into()) };
                let full = super::code_server_name(ext, &name);
                let list = self.wasm.added_servers.entry((ext.to_string(), root.clone())).or_default();
                list.retain(|s| s.name != full);
                if let Some(((executable, args, cwd), env)) = program {
                    list.push(fenix_project::tools::AdditionalServer { name: full.clone(), languages, executable, args, cwd, env: env.into_iter().collect() });
                }
                // That one started again, as it's given now; the others
                // carry on.
                let at = fenix_lsp::normalize(refactor::identity(&root));
                let key = super::LspKey { server: super::LspServer::Additional(full), root: at.clone() };
                self.lsp_unavailable.remove(&key);
                if self.lsp_sessions.contains_key(&key) {
                    self.stop_lsp_session(&key);
                }
                self.lsp_additional.retain(|r, _| !same_dir(r, &at));
                self.sync_lsp_for_focused_buffer();
            }
            Request::AgentProvide { id, program } => self.agents_provided(ext, &id, program),
            Request::DebugAdapter { languages, program } => {
                let Some(root) = self.wasm.projects.get(ext).cloned().flatten() else { return self.note_output(ext, "-- debug-adapter-set: it has no project".into()) };
                let key = (ext.to_string(), root);
                self.wasm.adapter_sockets.remove(&key);
                match program {
                    Some(checked) => self.wasm.adapters.insert(key, (languages, checked_spec(checked))),
                    None => self.wasm.adapters.remove(&key),
                };
            }
            Request::DebugAdapterSocket { languages, program, port } => {
                let Some(root) = self.wasm.projects.get(ext).cloned().flatten() else { return self.note_output(ext, "-- debug-adapter-connect: it has no project".into()) };
                let key = (ext.to_string(), root);
                self.wasm.adapters.remove(&key);
                self.wasm.adapter_sockets.insert(key, (languages, crate::dap::Adapter { command: program.map(checked_spec), port: Some(port) }));
            }
            Request::Formatter { languages, program, keep_output } => {
                let Some(root) = self.wasm.projects.get(ext).cloned().flatten() else { return self.note_output(ext, "-- formatter-set: it has no project".into()) };
                let key = (ext.to_string(), root);
                match program {
                    Some(checked) => {
                        if keep_output {
                            self.wasm.keep_output.insert(key.clone());
                        } else {
                            self.wasm.keep_output.remove(&key);
                        }
                        self.wasm.formatters.insert(key, (languages, checked_spec((checked, Vec::new()))))
                    }
                    None => self.wasm.formatters.remove(&key),
                };
            }
            Request::Diagnostics { path, open_only, problems } => {
                if open_only && self.code_buffer_on(&path, true).is_none() {
                    return self.note_output(ext, format!("  warn  diagnostics-set: {} isn't open", path.display()));
                }
                let fixes = problems.iter().map(|p| p.fixes.clone()).collect();
                self.set_code_problems(ext, &path, problems, fixes);
            }
            Request::LanguageServerSettings { name, settings } => {
                let Some(root) = self.wasm.projects.get(ext).cloned().flatten() else { return self.note_output(ext, "-- language-server-settings: it has no project".into()) };
                // A server it added, or a language's: not another's own.
                let added = self.wasm.added_servers.get(&(ext.to_string(), root.clone())).is_some_and(|list| list.iter().any(|s| s.name == super::code_server_name(ext, &name)));
                if !added && fenix_syntax::LanguageId::from_name(&name).is_none() {
                    return self.note_output(ext, format!("  warn  language-server-settings: {name} is no server it added, nor a language"));
                }
                let mine = self.wasm.server_settings.entry((ext.to_string(), root.clone())).or_default();
                match settings {
                    Some(settings) => mine.insert(name.clone(), settings),
                    None => mine.remove(&name),
                };
                self.lsp_settings_changed(&root, Some(&name));
            }
            Request::Launches(launches) => {
                let Some(root) = self.wasm.projects.get(ext).cloned().flatten() else { return self.note_output(ext, "-- launches-set: it has no project".into()) };
                let mut templates = Vec::new();
                for launch in launches {
                    match crate::dap::Template::from_config(launch) {
                        Ok(t) => templates.push(t),
                        Err(e) => self.note_output(ext, format!("  warn  launches-set: {e}")),
                    }
                }
                self.wasm.launches.insert((ext.to_string(), root), templates);
            }
            Request::LaunchVariable { name, choices } => {
                let Some(root) = self.wasm.projects.get(ext).cloned().flatten() else { return self.note_output(ext, "-- launch-variable-set: it has no project".into()) };
                let mine = self.wasm.launch_variables.entry((ext.to_string(), root)).or_default();
                if choices.is_empty() {
                    mine.remove(&name);
                } else {
                    mine.insert(name, choices);
                }
            }
            Request::LanguageServerRestart => {
                if let Some(root) = self.wasm.projects.get(ext).cloned().flatten() {
                    self.restart_language_servers_in(&root);
                }
            }
            Request::SheetLoad { id, bytes, frame } => self.load_sheet(ext, id, &bytes, frame),
            Request::ModelLoad { id, model } => {
                self.wasm.layers.models.insert((ext.to_string(), id), model.0);
            }
            Request::LayerOpen { id, level } => {
                self.wasm.layers.layers.insert((ext.to_string(), id), super::layers::Layer { level, items: Vec::new() });
            }
            Request::LayerSet { layer, items } => {
                let most = 64;
                if items.len() > most {
                    self.note_output(ext, format!("  warn  layer-set: {} items, at most {most} are drawn", items.len()));
                }
                self.wasm.layers.set(ext, layer, items.into_iter().take(most).collect(), std::time::Instant::now());
                self.request_window_redraw();
            }
            Request::LayerClose(layer) => {
                self.wasm.layers.layers.remove(&(ext.to_string(), layer));
                self.request_window_redraw();
            }
            Request::Found { name, path } => {
                self.wasm.found.insert(name, path);
            }
            Request::NewProject(args) => self.project_new_with(&args),
            Request::Doctor(checks) => {
                if let Some(root) = self.wasm.projects.get(ext).cloned().flatten() {
                    self.project_health.remove(&root);
                    self.wasm.checks.insert((ext.to_string(), root.clone()), checks);
                    // A doctor's page open on it shows them: a fix it ran
                    // (an install) has done what it said.
                    self.doctor_recheck(&root);
                }
            }
            Request::Tasks(tasks) => {
                if let Some(root) = self.wasm.projects.get(ext).cloned().flatten() {
                    // Run as they were given: where, and with what
                    // environment (`TaskDef` has only the command).
                    let specs = tasks.iter().map(|(name, checked, env)| (name.clone(), checked_spec((checked.clone(), env.clone())))).collect();
                    self.wasm.task_specs.insert((ext.to_string(), root.clone()), specs);
                    let tasks = tasks.into_iter().map(|(name, (command, args, _), _)| fenix_tasks::TaskDef { name, command, args }).collect();
                    self.wasm.tasks.insert((ext.to_string(), root), tasks);
                }
            }
            Request::Task { id, name, program, args, cwd, env } => {
                let Some(root) = cwd.or_else(|| self.wasm.projects.get(ext).cloned().flatten()).or_else(|| self.project_root.clone()) else {
                    return self.set_error(format!("{name}: no project to run it in"));
                };
                // One task at a time: a new one ends the last.
                if let Some((before, before_id, _)) = self.wasm.task.take() {
                    self.code_deliver(&before, Notice::TaskFinished((before_id, false)));
                }
                self.wasm.task = Some((ext.to_string(), id, root.clone()));
                let spec = fenix_project::tools::CommandSpec { env: env.into_iter().collect(), ..fenix_project::tools::CommandSpec::new(program.clone(), args.clone()) };
                self.run_task_as(fenix_tasks::TaskDef { name, command: program, args }, root, Some(spec));
            }
            Request::OpenText { title, text, language, beside } => {
                let like: String = title.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
                let path = format!("{like}.{}", language.as_deref().unwrap_or("txt"));
                let id = self.buffers.open_text_view_like(&text, std::path::Path::new(&path));
                self.ref_view_names.insert(id, format!("*{title}*"));
                match beside {
                    Some(share) => self.code_open_beside(ext, id, share),
                    None => self.open_buffer_in_focused_pane(id),
                }
            }
            Request::OpenUrl(url) => {
                #[cfg(test)]
                self.wasm.opened.push(url.clone());
                #[cfg(not(test))]
                if let Err(e) = super::open_in_browser(&url) {
                    self.set_error(format!("{url}: {e}"));
                }
            }
            Request::Clipboard(text) => {
                if let Some(clipboard) = self.clipboard.as_mut() {
                    match clipboard.set_text(text) {
                        Ok(()) => self.set_message("Copied"),
                        Err(e) => self.set_error(format!("couldn't copy: {e}")),
                    }
                }
            }
            Request::Status(Some(text)) => {
                self.wasm.statuses.insert(ext.to_string(), text);
            }
            Request::Status(None) => {
                self.wasm.statuses.remove(ext);
            }
            Request::SurfaceOpen { page, kind, title, options, surface, replaces } => {
                // Opened as a page of its came back (`page-restore`): in
                // that page's place, its tab and pane.
                if let Some(buffer) = replaces.and_then(|old| self.code_page_buffer(ext, old)) {
                    if let Some(state) = self.pages.get_mut(&buffer) {
                        if let PageModel::Code(p) = &mut state.model {
                            p.page = page;
                            p.kind = kind;
                            p.title = title;
                            p.shown = None;
                            p.width = None;
                            p.asked = None;
                            p.surface = Some(crate::code_page::CodeSurface::new(surface, options));
                        }
                        state.stale = true;
                    }
                    self.code_surface_sync_capture();
                    self.request_window_redraw();
                    return;
                }
                let mut model = CodePage::new(ext, page, &kind, &title);
                model.surface = Some(crate::code_page::CodeSurface::new(surface, options));
                model.file = self.code_opening(ext);
                match options.beside {
                    Some(width) => {
                        let back = self.focused_pane_id();
                        self.open_page_beside(PageModel::Code(Box::new(model)), width);
                        self.windows_mut().focus(back);
                    }
                    None => {
                        self.open_page(PageModel::Code(Box::new(model)));
                        self.code_surface_sync_capture();
                    }
                }
                self.request_window_redraw();
            }
            // Its pixels, pointer or marks changed: drawn on the next frame.
            // What it needs to open again may have changed with them (a
            // page turned), so that's asked, once at a time; a surface
            // that has the keyboard (a remote desktop) changes all the
            // time, and is asked only as it opens.
            Request::SurfaceChanged(page) => {
                if let Some(buffer) = self.code_page_buffer(ext, page) {
                    if self.code_surface(buffer).is_some_and(|s| !s.options.capture) && self.wasm.state_asked.insert(buffer) {
                        self.code_call(ext, Call::PageState(page), PAGE_TICKS, Pending::State(buffer));
                    }
                }
                self.request_window_redraw();
            }
            Request::PageRetitle { page, title } => {
                if let Some(buffer) = self.code_page_buffer(ext, page) {
                    if let Some(state) = self.pages.get_mut(&buffer) {
                        if let PageModel::Code(p) = &mut state.model {
                            p.title = title.clone();
                        }
                        state.stale = true;
                    }
                    for pane in self.windows().windows().into_iter().filter(|&p| self.windows().content(p) == Some(&buffer)).collect::<Vec<_>>() {
                        if self.pane_titles.contains_key(&pane) {
                            self.pane_titles.insert(pane, title.clone());
                        }
                    }
                }
            }
            Request::ClipboardText(answer) => answer.send(self.clipboard_text()),
            Request::ExplorerSelected(answer) => {
                let selected = self.explorer_selected_path();
                answer.send(selected.map(|p| fenix_lsp::normalize(p).to_string_lossy().into_owned()));
            }
            Request::GitRefresh => self.git_refresh_all_views(),
            Request::ScratchOpen { id, title, text, language, beside } => {
                let like: String = title.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
                let buffer = self.buffers.open_text_view_like(&text, Path::new(&format!("{like}.{}", language.as_deref().unwrap_or("txt"))));
                if let Some(ob) = self.buffers.get(buffer) {
                    self.wasm.edits.insert(buffer, (ob.buffer.edit_count(), None));
                }
                self.wasm.scratches.insert(buffer, Scratch { ext: ext.to_string(), id, title });
                match beside {
                    Some(share) => self.code_open_beside(ext, buffer, share),
                    None => self.open_buffer_in_focused_pane(buffer),
                }
                self.refresh_project_root();
                self.wake_caret();
            }
            Request::ScratchText { id, answer } => answer.send(self.code_scratch(ext, id).and_then(|b| self.buffers.get(b)).map(|ob| ob.buffer.text())),
            Request::ScratchSet { id, text } => {
                let Some(buffer) = self.code_scratch(ext, id) else { return };
                if let Some(ob) = self.buffers.get_mut(buffer) {
                    apply_text_edits(&mut ob.buffer, vec![fenix_wasm::host::TextEdit { start: (0, 0), end: (u32::MAX, 0), text }]);
                    ob.buffer.mark_saved();
                    // Its own change: not told back.
                    self.wasm.edits.insert(buffer, (ob.buffer.edit_count(), None));
                }
            }
            Request::ScratchClose(id) => {
                if let Some(buffer) = self.code_scratch(ext, id) {
                    self.wasm.scratches.remove(&buffer);
                    self.wasm.edits.remove(&buffer);
                    self.close_page_quietly(buffer);
                }
            }
            Request::PlaySound(bytes) => {
                #[cfg(test)]
                self.wasm.played.push(bytes);
                #[cfg(not(test))]
                if let Err(e) = crate::sound::play_bytes(bytes) {
                    self.note_output(ext, format!("  warn  play-sound: {e}"));
                }
            }
            Request::Granted(path) => self.note_granted(ext, path),
            Request::FileDecorations { path, open_only, decorations } => match self.code_buffer_on(&path, open_only) {
                Some(buffer) => self.set_code_decorations(ext, buffer, &decorations),
                None => self.note_output(ext, format!("  warn  decorations-set: {} isn't open", path.display())),
            },
            Request::ScratchDecorations { id, decorations } => {
                if let Some(buffer) = self.code_scratch(ext, id) {
                    self.set_code_decorations(ext, buffer, &decorations);
                }
            }
            Request::BufferUndo { path, open_only, redo } => {
                let Some(buffer) = self.code_buffer_on(&path, open_only) else {
                    return self.note_output(ext, format!("  warn  buffer-{}: {} isn't open", if redo { "redo" } else { "undo" }, path.display()));
                };
                if let Some(ob) = self.buffers.get_mut(buffer) {
                    let mut cursor = Cursor::at_start();
                    if redo {
                        ob.buffer.redo(&mut cursor);
                    } else {
                        ob.buffer.undo(&mut cursor);
                    }
                }
            }
            Request::DecorationRules(rules) => {
                let rules: Vec<Rule> = rules
                    .into_iter()
                    .filter_map(|r| Some(Rule { pattern: regex::Regex::new(&r.pattern).ok()?, files: r.files, group: r.group as usize, look: r.look }))
                    .collect();
                if rules.is_empty() {
                    self.wasm.rules.remove(ext);
                } else {
                    self.wasm.rules.insert(ext.to_string(), rules);
                }
                self.wasm.rules_given += 1;
                self.request_window_redraw();
            }
            Request::FileLanguage { path, open_only, language } => match self.code_buffer_on(&path, open_only) {
                Some(buffer) => self.set_code_language(ext, buffer, language),
                None => self.note_output(ext, format!("  warn  buffer-language-set: {} isn't open", path.display())),
            },
            Request::ScratchLanguage { id, language } => {
                if let Some(buffer) = self.code_scratch(ext, id) {
                    self.set_code_language(ext, buffer, language);
                }
            }
            Request::BackgroundTask { id, name, program, args, cwd, env } => self.start_background_task(ext, id, name, program, args, cwd, env),
            Request::TaskCancel(id) => {
                if let Some(task) = self.wasm.background.values_mut().find(|t| t.ext == ext && t.id == id) {
                    if let Some(runner) = task.runner.take() {
                        runner.cancel();
                    }
                }
            }
            Request::TaskShow(id) => match self.wasm.background.iter().find(|(_, t)| t.ext == ext && t.id == id).map(|(b, _)| *b) {
                Some(buffer) => self.open_buffer_in_focused_pane(buffer),
                None => self.note_output(ext, format!("  warn  task-show: {id} isn't one of its tasks")),
            },
            Request::PageFile { page, path } => {
                let Some(buffer) = self.code_page_buffer(ext, page) else { return self.note_output(ext, format!("  warn  page-file: {page} isn't one of its pages")) };
                let full = path.map(|p| fenix_lsp::normalize(std::fs::canonicalize(&p).unwrap_or(p)));
                if let Some(PageModel::Code(p)) = self.pages.get_mut(&buffer).map(|s| &mut s.model) {
                    p.file = full;
                }
            }
            // Fenix's own.
            Request::Service { id, name, payload } if name == "agents.ask" => {
                let root = self.wasm.projects.get(ext).cloned().flatten();
                self.agents_ask(ext, id, &payload, root);
            }
            Request::Service { id, name, payload } => {
                // Another code extension's.
                match self.code_server(&name, Some(ext)) {
                    Some(server) => self.code_serve(&server, &name, payload, Answer::Ext { asker: ext.to_string(), id }),
                    None => self.code_deliver(ext, Notice::ServiceReply((id, Err(format!("nothing offers {name}"))))),
                }
            }
        }
    }

    /// Page buffer `buffer`'s surface, when it's one whose extension runs.
    pub(crate) fn code_surface(&self, buffer: BufferId) -> Option<&crate::code_page::CodeSurface> {
        match self.pages.get(&buffer).map(|s| &s.model) {
            Some(PageModel::Code(p)) if p.stopped.is_none() => p.surface.as_ref(),
            _ => None,
        }
    }

    fn code_surface_mut(&mut self, buffer: BufferId) -> Option<&mut crate::code_page::CodeSurface> {
        match self.pages.get_mut(&buffer).map(|s| &mut s.model) {
            Some(PageModel::Code(p)) if p.stopped.is_none() => p.surface.as_mut(),
            _ => None,
        }
    }

    /// The surface pane `pane` shows, by its page buffer.
    pub(crate) fn code_surface_in_pane(&self, pane: fenix_window::WindowId) -> Option<BufferId> {
        self.windows().content(pane).copied().filter(|b| self.code_surface(*b).is_some())
    }

    /// The boxes a surface tints over itself, on screen, for a pane at
    /// `rect`: each in its role's colour, cut to what's drawn.
    pub(crate) fn code_surface_marks(&self, buffer: BufferId, rect: fenix_window::Rect) -> Vec<((f32, f32, f32, f32), [f32; 4])> {
        let Some(surface) = self.code_surface(buffer) else { return Vec::new() };
        let Some(((x, y, w, h), (u0, v0, u1, v1))) = surface.placed((rect.x, rect.y, rect.w, rect.h)) else { return Vec::new() };
        let fb = surface.shared.lock();
        let (sw, sh) = (fb.size().0 as f32, fb.size().1 as f32);
        let (kx, ky) = (w / ((u1 - u0) * sw), h / ((v1 - v0) * sh));
        let theme = self.theme;
        let rgba = |c: glyphon::Color| [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0];
        fb.marks()
            .iter()
            .filter_map(|m| {
                let color = match &m.role {
                    fenix_wasm::host::Role::Good => rgba(theme.git_staged),
                    fenix_wasm::host::Role::Warn => rgba(theme.git_modified),
                    fenix_wasm::host::Role::Bad => rgba(theme.git_conflicted),
                    fenix_wasm::host::Role::Muted => rgba(theme.gutter_fg),
                    _ => theme.caret,
                };
                let x0 = (x + (m.x as f32 - u0 * sw) * kx).max(x);
                let y0 = (y + (m.y as f32 - v0 * sh) * ky).max(y);
                let x1 = (x + (m.x as f32 + m.width as f32 - u0 * sw) * kx).min(x + w);
                let y1 = (y + (m.y as f32 + m.height as f32 - v0 * sh) * ky).min(y + h);
                (x1 > x0 && y1 > y0).then_some(((x0, y0, x1 - x0, y1 - y0), [color[0], color[1], color[2], 0.35]))
            })
            .collect()
    }

    /// `input` for the surface in page buffer `buffer`.
    pub(crate) fn code_surface_input(&mut self, buffer: BufferId, input: fenix_wasm::host::SurfaceInput) {
        let Some(PageModel::Code(p)) = self.pages.get(&buffer).map(|s| &s.model) else { return };
        let (ext, surface) = (p.ext.clone(), p.page);
        self.code_call(&ext, Call::SurfaceInput { surface, input }, PAGE_TICKS, Pending::Other);
    }

    /// Who has the keyboard, as the focus says: a surface that captures it,
    /// focused, takes it; anything else gives it back.
    pub(crate) fn code_surface_sync_capture(&mut self) {
        let focused = self.focused_buffer_id();
        let wants = (self.main_view == MainView::Editor && self.code_surface(focused).is_some_and(|s| s.options.capture)).then_some(focused);
        if wants == self.wasm.captured {
            return;
        }
        if let Some(old) = self.wasm.captured.take() {
            self.code_surface_input(old, fenix_wasm::host::SurfaceInput::Capture(false));
        }
        if let Some(new) = wants {
            self.wasm.captured = Some(new);
            self.code_surface_input(new, fenix_wasm::host::SurfaceInput::Capture(true));
        }
        if let Some(window) = &self.window {
            window.set_cursor_visible(self.wasm.captured.is_none());
        }
    }

    /// The keyboard given back (Ctrl+\, or Fenix's window left).
    pub(crate) fn code_surface_release(&mut self) {
        if let Some(old) = self.wasm.captured.take() {
            self.code_surface_input(old, fenix_wasm::host::SurfaceInput::Capture(false));
            if let Some(window) = &self.window {
                window.set_cursor_visible(true);
            }
        }
    }

    /// A key, pressed or let go, while a surface has the keyboard: true
    /// when it was the surface's. Ctrl+\ gives the keyboard back.
    pub(crate) fn code_surface_captured_key(&mut self, keypress: Option<KeyPress>, caps_lock: bool, down: bool) -> bool {
        let Some(buffer) = self.wasm.captured else { return false };
        if self.focused_buffer_id() != buffer || self.code_surface(buffer).is_none() {
            self.code_surface_release();
            return false;
        }
        let key = if caps_lock {
            Some(fenix_wasm::host::KeyEvent { key: fenix_wasm::host::SurfaceKey::Named(fenix_wasm::host::NamedKey::CapsLock), down, mods: crate::code_page::modifiers(Mods::default()) })
        } else {
            match keypress {
                Some(k) if k == KeyPress::char('\\').with_ctrl() => {
                    if down {
                        self.code_surface_release();
                    }
                    return true;
                }
                Some(k) => crate::code_page::surface_key(k, down),
                None => None,
            }
        };
        if let Some(key) = key {
            self.code_surface_input(buffer, fenix_wasm::host::SurfaceInput::Key(key));
        }
        true
    }

    /// A key on a surface that doesn't capture the keyboard: a page's
    /// keys -- `q` closes it, Space, `:` and Ctrl+W (the windows) are
    /// Fenix's -- and the rest its.
    pub(super) fn code_surface_page_key(&mut self, buffer: BufferId, keypress: KeyPress) -> bool {
        if keypress == KeyPress::char('q') {
            self.close_page(buffer);
            return true;
        }
        if keypress == KeyPress::char(' ') || keypress == KeyPress::char(':') || keypress == KeyPress::char('w').with_ctrl() {
            return false;
        }
        if let Some(key) = crate::code_page::surface_key(keypress, true) {
            self.code_surface_input(buffer, fenix_wasm::host::SurfaceInput::Key(key));
        }
        true
    }

    /// The pointer moved (`button` none), or a button went down or up, at
    /// window pixel `pos`: to the surface under it, or the one a button
    /// went down on. Moves reach a surface while a button's held, or always
    /// while it has the keyboard.
    pub(crate) fn code_surface_pointer(&mut self, pos: (f32, f32), button: Option<(MouseButton, bool)>) {
        let target = self.wasm.dragging.or_else(|| {
            let (pane, rect) = self.pane_rect_at(pos)?;
            Some((self.code_surface_in_pane(pane)?, rect))
        });
        let Some((buffer, rect)) = target else { return };
        let captured = self.wasm.captured == Some(buffer);
        let mods = crate::code_page::modifiers(Mods { ctrl: self.modifiers.control_key(), alt: self.modifiers.alt_key(), super_: self.modifiers.super_key() });
        let Some(surface) = self.code_surface_mut(buffer) else {
            self.wasm.dragging = None;
            return;
        };
        let held_before = surface.buttons;
        if let Some((button, pressed)) = button {
            let bit = match button {
                MouseButton::Left => 1,
                MouseButton::Middle => 2,
                MouseButton::Right => 4,
                _ => return,
            };
            if pressed {
                surface.buttons |= bit;
            } else {
                surface.buttons &= !bit;
            }
        } else if held_before == 0 && !captured {
            return;
        }
        let Some(at) = surface.to_pixels((rect.x, rect.y, rect.w, rect.h), pos, held_before != 0) else { return };
        if button.is_none() && surface.pointer == Some(at) {
            return;
        }
        surface.pointer = Some(at);
        let buttons = surface.buttons;
        self.wasm.dragging = (buttons != 0).then_some((buffer, rect));
        self.code_surface_input(buffer, fenix_wasm::host::SurfaceInput::Pointer(fenix_wasm::host::PointerEvent { x: at.0, y: at.1, buttons, mods }));
        self.request_window_redraw();
    }

    /// The cell under `pos` of a code page that takes the mouse
    /// (`page-extra.mouse`) in pane `pane` at `rect`: its buffer, line,
    /// column in characters and cell.
    fn code_page_cell(&self, pane: fenix_window::WindowId, rect: fenix_window::Rect, pos: (f32, f32)) -> Option<(BufferId, u32, u32, u32)> {
        let &buffer = self.windows().content(pane)?;
        let state = self.pages.get(&buffer)?;
        let PageModel::Code(p) = &state.model else { return None };
        if !p.mouse() {
            return None;
        }
        let (cw, lh) = self.text.as_ref().map(|t| (t.char_width(), t.line_height())).unwrap_or((text::LINE_HEIGHT * 0.6, text::LINE_HEIGHT));
        let row = (pos.1 - rect.y - text::PAD_TOP) / lh + self.pane_state(pane).rendered_scroll;
        let x = (pos.0 - rect.x - text::PAD_LEFT) / cw;
        if row < 0.0 || x < 0.0 || pos.0 > rect.x + rect.w || pos.1 > rect.y + rect.h {
            return None;
        }
        let (line, cell) = (row.floor() as usize, x.floor() as usize);
        let column = crate::page::col_of(state.page.cells(line).as_deref(), cell);
        Some((buffer, line as u32, column as u32, cell as u32))
    }

    fn key_mods(&self) -> fenix_wasm::host::KeyMods {
        fenix_wasm::host::KeyMods { shift: self.modifiers.shift_key(), ctrl: self.modifiers.control_key(), alt: self.modifiers.alt_key(), meta: self.modifiers.super_key() }
    }

    /// A button pressed or let go (`button`), or the pointer moved, over a
    /// code page that takes the mouse: told to it. True when it was its.
    pub(crate) fn code_page_pointer(&mut self, pos: (f32, f32), button: Option<(MouseButton, bool)>) -> bool {
        use fenix_wasm::host::{MouseAction, MouseButton as B, MouseEvent, MousePress};
        let Some((pane, rect)) = self.pane_rect_at(pos) else { return false };
        let which = |b: MouseButton| match b {
            MouseButton::Left => Some(B::Left),
            MouseButton::Middle => Some(B::Middle),
            MouseButton::Right => Some(B::Right),
            _ => None,
        };
        let mods = self.key_mods();
        let event = |line, column, cell, action| MouseEvent { line, column, cell, action, mods };
        match button {
            Some((b, true)) => {
                let (Some((buffer, line, column, cell)), Some(b)) = (self.code_page_cell(pane, rect, pos), which(b)) else { return false };
                let now = std::time::Instant::now();
                let clicks = match self.wasm.last_press {
                    Some((pb, at, pbutton, when, n)) if pb == buffer && at == (line, cell) && pbutton == b && now.duration_since(when) < DOUBLE_CLICK => (n % 3) + 1,
                    _ => 1,
                };
                self.wasm.last_press = Some((buffer, (line, cell), b, now, clicks));
                self.wasm.page_drag = Some((buffer, b, (line, cell)));
                self.code_page_mouse(buffer, event(line, column, cell, MouseAction::Press(MousePress { button: b, clicks })));
                true
            }
            Some((b, false)) => {
                let Some((buffer, held, _)) = self.wasm.page_drag.take() else { return false };
                if which(b) != Some(held) {
                    self.wasm.page_drag = Some((buffer, held, (0, 0)));
                    return true;
                }
                let (line, column, cell) = self.code_page_cell(pane, rect, pos).filter(|c| c.0 == buffer).map(|c| (c.1, c.2, c.3)).unwrap_or((0, 0, 0));
                self.code_page_mouse(buffer, event(line, column, cell, MouseAction::Release(held)));
                true
            }
            None => {
                let Some((buffer, held, at)) = self.wasm.page_drag else { return false };
                let Some((b2, line, column, cell)) = self.code_page_cell(pane, rect, pos) else { return true };
                if b2 != buffer || at == (line, cell) {
                    return true;
                }
                self.wasm.page_drag = Some((buffer, held, (line, cell)));
                self.code_page_mouse(buffer, event(line, column, cell, MouseAction::Drag(held)));
                true
            }
        }
    }

    /// The wheel over pane `pane`: a code page's that takes the mouse.
    pub(crate) fn code_page_wheel(&mut self, pane: fenix_window::WindowId, pos: (f32, f32), lines: isize) -> bool {
        let Some((_, rect)) = self.pane_rect_at(pos) else { return false };
        let Some((buffer, line, column, cell)) = self.code_page_cell(pane, rect, pos) else { return false };
        let event = fenix_wasm::host::MouseEvent { line, column, cell, action: fenix_wasm::host::MouseAction::Wheel(lines as i32), mods: self.key_mods() };
        self.code_page_mouse(buffer, event);
        true
    }

    /// The wheel over pane `pane`: its surface's, if it shows one.
    pub(crate) fn code_surface_wheel(&mut self, pane: fenix_window::WindowId, pos: (f32, f32), lines: isize) -> bool {
        let Some(buffer) = self.code_surface_in_pane(pane) else { return false };
        let Some((_, rect)) = self.pane_rect_at(pos) else { return true };
        let mods = crate::code_page::modifiers(Mods { ctrl: self.modifiers.control_key(), alt: self.modifiers.alt_key(), super_: self.modifiers.super_key() });
        let Some(at) = self.code_surface(buffer).and_then(|s| s.to_pixels((rect.x, rect.y, rect.w, rect.h), pos, true)) else { return true };
        self.code_surface_input(buffer, fenix_wasm::host::SurfaceInput::Wheel(fenix_wasm::host::WheelEvent { x: at.0, y: at.1, lines: lines as i32, mods }));
        true
    }

    /// After a frame: each surface drawn, at its pane's size in pixels and
    /// the display's scale. A size held for a moment is told to its
    /// extension (`surface-resized`), so dragging a split doesn't flood it.
    pub(crate) fn code_surfaces_drawn(&mut self, sizes: Vec<(BufferId, (u32, u32), f32)>) {
        const SETTLE: std::time::Duration = std::time::Duration::from_millis(120);
        let now = std::time::Instant::now();
        let mut waiting = false;
        for (buffer, (width, height), scale) in sizes {
            self.wasm.drawn.insert(buffer);
            let Some(surface) = self.code_surface_mut(buffer) else { continue };
            let size = (width, height, scale);
            if surface.told == Some(size) {
                surface.seen = None;
                continue;
            }
            let since = match surface.seen {
                Some((seen, since)) if seen == size => since,
                _ => {
                    surface.seen = Some((size, now));
                    now
                }
            };
            // The first size at once; a change once it's settled.
            if surface.told.is_some() && now.duration_since(since) < SETTLE {
                waiting = true;
                continue;
            }
            surface.told = Some(size);
            surface.seen = None;
            let Some(PageModel::Code(p)) = self.pages.get(&buffer).map(|s| &s.model) else { continue };
            let (ext, page) = (p.ext.clone(), p.page);
            self.code_deliver(&ext, Notice::SurfaceResized(fenix_wasm::host::SurfaceSize { surface: page, width, height, scale }));
        }
        if waiting {
            self.request_window_redraw();
        }
    }

    /// A Fenix command an extension asked for: run now when it needs no
    /// event loop, else on the next key.
    pub(super) fn run_command_for_extension(&mut self, id: &str) {
        let commands = Rc::clone(&self.commands);
        match commands.run_without_event_loop(self, id) {
            Ok(()) => {}
            Err(crate::commands::ExternalError::Unknown) => self.set_error(format!("no command {id}")),
            Err(crate::commands::ExternalError::NotExternal) => self.queued_command = Some(id.to_string()),
        }
    }

    /// The buffer showing extension `ext`'s page `page`, if one does.
    /// The language servers running for the project at `root`, stopped
    /// and started again as its files need them.
    fn restart_language_servers_in(&mut self, root: &Path) {
        let root = fenix_lsp::normalize(refactor::identity(root));
        self.lsp_unavailable.retain(|key| key.root != root);
        let running: Vec<_> = self.lsp_sessions.keys().filter(|key| key.root == root).cloned().collect();
        for key in running {
            self.stop_lsp_session(&key);
        }
        self.sync_lsp_for_focused_buffer();
    }

    /// The buffer of extension `ext`'s terminal `id`, while it's open.
    fn code_terminal(&self, ext: &str, id: u64) -> Option<BufferId> {
        self.wasm.terminals.iter().find(|(b, (e, i))| e == ext && *i == id && self.buffers.get(**b).is_some()).map(|(b, _)| *b)
    }

    fn code_page_buffer(&self, ext: &str, page: u64) -> Option<BufferId> {
        self.find_page(|m| matches!(m, PageModel::Code(p) if p.ext == ext && p.page == page))
    }

    fn mark_code_pages(&mut self, ext: &str, change: impl Fn(&mut CodePage)) {
        for state in self.pages.values_mut() {
            if let PageModel::Code(p) = &mut state.model {
                if p.ext == ext {
                    change(p);
                    state.stale = true;
                }
            }
        }
    }

    /// Asks for code page `buffer` laid out `width` wide for a pane
    /// `height` lines tall, unless that's what's shown or asked for
    /// already. Called as it's drawn.
    pub(super) fn code_page_wants(&mut self, buffer: BufferId, width: u32, height: u32) {
        let Some(PageModel::Code(p)) = self.pages.get_mut(&buffer).map(|s| &mut s.model) else { return };
        if p.width == Some((width, height)) || p.asked.is_some() || p.stopped.is_some() {
            return;
        }
        p.asked = Some((width, height));
        let (ext, page) = (p.ext.clone(), p.page);
        self.code_call(&ext, Call::Render { page, width, height }, PAGE_TICKS, Pending::Render(buffer));
    }

    /// A key `page-key` doesn't take, on code page `buffer` that takes
    /// more: to its extension, then laid out again.
    pub(super) fn code_page_key_press(&mut self, buffer: BufferId, key: fenix_wasm::host::KeyPress) {
        let Some(state) = self.pages.get_mut(&buffer) else { return };
        let PageModel::Code(p) = &mut state.model else { return };
        let (ext, page) = (p.ext.clone(), p.page);
        p.width = None;
        state.stale = true;
        self.code_call(&ext, Call::KeyPress { page, key }, PAGE_TICKS, Pending::Other);
        self.code_call(&ext, Call::PageState(page), PAGE_TICKS, Pending::State(buffer));
    }

    /// The mouse on code page `buffer`, at a cell: to its extension, then
    /// laid out again.
    pub(super) fn code_page_mouse(&mut self, buffer: BufferId, event: fenix_wasm::host::MouseEvent) {
        let Some(state) = self.pages.get_mut(&buffer) else { return };
        let PageModel::Code(p) = &mut state.model else { return };
        let (ext, page) = (p.ext.clone(), p.page);
        p.width = None;
        state.stale = true;
        self.code_call(&ext, Call::Mouse { page, event }, PAGE_TICKS, Pending::Other);
    }

    /// A key on code page `buffer`: to its extension, then laid out again.
    pub(super) fn code_page_key(&mut self, buffer: BufferId, key: fenix_wasm::host::Key) {
        let Some(state) = self.pages.get_mut(&buffer) else { return };
        let PageModel::Code(p) = &mut state.model else { return };
        let (ext, page) = (p.ext.clone(), p.page);
        p.width = None;
        state.stale = true;
        self.code_call(&ext, Call::Key { page, key }, PAGE_TICKS, Pending::Other);
        self.code_call(&ext, Call::PageState(page), PAGE_TICKS, Pending::State(buffer));
    }

    /// Asks for code page `buffer` laid out, and notes it was drawn.
    pub(super) fn code_page_drawn(&mut self, buffer: BufferId, width: u32, height: u32) {
        self.wasm.drawn.insert(buffer);
        self.code_page_wants(buffer, width, height);
    }

    /// After a frame: the code pages that came into view or left every
    /// pane, told to their extensions.
    pub(super) fn code_pages_drawn(&mut self) {
        let drawn = std::mem::take(&mut self.wasm.drawn);
        let changed: Vec<(BufferId, bool)> = drawn.difference(&self.wasm.shown).map(|b| (*b, true)).chain(self.wasm.shown.difference(&drawn).map(|b| (*b, false))).collect();
        self.wasm.shown = drawn;
        for (buffer, shown) in changed {
            if let Some(PageModel::Code(p)) = self.pages.get(&buffer).map(|s| &s.model) {
                let (ext, page) = (p.ext.clone(), p.page);
                self.code_deliver(&ext, Notice::PageShown((page, shown)));
            }
        }
    }

    /// The code extensions' modeline items.
    pub(crate) fn code_statuses(&self) -> Vec<String> {
        let mut items: Vec<(&String, &String)> = self.wasm.statuses.iter().collect();
        items.sort();
        items.into_iter().map(|(_, text)| format!("   {text}")).collect()
    }

    /// The code pages to save with the session: their extension, kind,
    /// title and state, by buffer.
    /// And the file it shows, and what its extension was given.
    pub(super) fn code_pages_to_save(&self) -> Vec<SavedPage> {
        self.pages
            .iter()
            .filter_map(|(id, s)| match &s.model {
                // One with nothing to open it again from isn't kept.
                PageModel::Code(p) => p.state.clone().filter(|s| !s.is_empty()).map(|state| SavedPage {
                    buffer: *id,
                    ext: p.ext.clone(),
                    kind: p.kind.clone(),
                    title: p.title.clone(),
                    state,
                    file: p.file.clone(),
                    given: self.wasm.granted.get(&p.ext).cloned().unwrap_or_default(),
                }),
                _ => None,
            })
            .collect()
    }

    /// A code page from the last session, put back: its extension starts
    /// and opens it again from its state, or it's closed. `None` when that
    /// extension isn't installed and switched on any more.
    /// What it was given (`given`) is given again first -- the files its
    /// pages show -- and `file` is the file the page shows.
    pub(super) fn restore_code_page(&mut self, ext: &str, kind: &str, title: &str, state: &str, file: Option<PathBuf>, given: &[PathBuf]) -> Option<BufferId> {
        if self.wasm.installed.is_empty() {
            self.wasm.installed = crate::installed::code_extensions();
        }
        if !self.wasm.installed.iter().any(|e| e.id == ext) {
            return None;
        }
        for path in given.iter().filter(|p| p.exists()) {
            if !self.wasm.granted.get(ext).is_some_and(|kept| kept.contains(path)) {
                self.code_grant(ext, path.clone());
            }
        }
        self.wasm.restored += 1;
        let page = (1u64 << 40) + self.wasm.restored;
        let mut model = CodePage::new(ext, page, kind, title);
        model.state = Some(state.to_string());
        model.file = file;
        let buffer = self.add_page(PageModel::Code(Box::new(model)));
        self.code_call(ext, Call::PageRestore { page, kind: kind.to_string(), state: state.to_string() }, PAGE_TICKS, Pending::Restore(buffer));
        Some(buffer)
    }

    /// The prompt's modeline text, while an extension's prompt is open.
    pub(crate) fn code_prompt_text(&self) -> Option<String> {
        self.wasm.prompt.as_ref().map(|p| format!("{}: {}▏", p.label, if p.masked { "•".repeat(p.input.chars().count()) } else { p.input.clone() }))
    }

    /// A key for an extension's prompt: `false` when none is open.
    pub(crate) fn code_prompt_key(&mut self, keypress: KeyPress) -> bool {
        let Some(prompt) = &mut self.wasm.prompt else { return false };
        match keypress.code {
            KeyCode::Named(FenixNamedKey::Escape) => {
                let p = self.wasm.prompt.take().unwrap();
                self.code_deliver(&p.ext, Notice::Prompted((p.id, None)));
            }
            // `SPC a s`'s, for an agent's API key.
            KeyCode::Named(FenixNamedKey::Enter) if prompt.ext == super::agents::SECRET_PROMPT => {
                let p = self.wasm.prompt.take().unwrap();
                self.agents_secret_entered(&p.label, &p.input);
            }
            KeyCode::Named(FenixNamedKey::Enter) => {
                let p = self.wasm.prompt.take().unwrap();
                self.code_deliver(&p.ext, Notice::Prompted((p.id, Some(p.input))));
            }
            KeyCode::Named(FenixNamedKey::Backspace) => {
                prompt.input.pop();
            }
            KeyCode::Char(c) if !keypress.mods.ctrl => prompt.input.push(c),
            _ => {}
        }
        self.wake_caret();
        true
    }

    /// Its picker closed: what was picked, or nothing, to its extension.
    pub(crate) fn code_picked(&mut self, ext: &str, id: u64, choice: Option<String>) {
        self.code_deliver(ext, Notice::Picked((id, choice)));
    }

    /// Its picker that marks several closed: what was marked, or nothing.
    pub(crate) fn code_picked_many(&mut self, ext: &str, id: u64, marked: Option<Vec<String>>) {
        self.code_deliver(ext, Notice::PickedMany((id, marked)));
    }

    /// One of its own settings, changed by it: saved as you'd save it.
    fn code_setting_set(&mut self, ext: &str, key: &str, value: Option<String>) {
        let value = match value {
            // A map comes as JSON, as `setting` gives it.
            Some(text) if matches!(fenix_config::schema::setting(key).map(|s| &s.kind), Some(fenix_config::schema::Kind::Map { .. })) => match serde_json::from_str::<Vec<(String, String)>>(&text) {
                Ok(pairs) => (!pairs.is_empty()).then_some(fenix_config::Value::Map(pairs)),
                Err(why) => return self.note_output(ext, format!("  warn  setting-set {key}: a map is JSON, [[key, value], ...] ({why})")),
            },
            Some(text) if matches!(fenix_config::schema::setting(key).map(|s| &s.kind), Some(fenix_config::schema::Kind::Records(_))) => match serde_json::from_str::<Vec<Vec<String>>>(&text) {
                Ok(rows) => (!rows.is_empty()).then_some(fenix_config::Value::Records(rows)),
                Err(why) => return self.note_output(ext, format!("  warn  setting-set {key}: records are JSON, [[field, ...], ...] ({why})")),
            },
            Some(text) => match fenix_config::schema::setting(key).map(|s| s.kind.parse(&text)) {
                Some(Ok(value)) => Some(value),
                Some(Err(why)) => return self.note_output(ext, format!("  warn  setting-set {key}: {why}")),
                None => Some(fenix_config::Value::Text(text)),
            },
            None => None,
        };
        self.config.set_value(key, value);
        if let Err(e) = self.config.save() {
            self.set_error(format!("couldn't save settings.toml: {e}"));
        }
    }

    /// One of its secrets kept or taken away (`secret-set`): in the
    /// keychain, else in `settings.toml` as plain text, which it's told.
    /// Its workers read it from then on.
    fn code_secret_set(&mut self, ext: &str, key: &str, value: Option<String>) {
        match self.wasm.keychain().set(key, value.as_deref()) {
            Ok(()) => {
                // None left behind in the file, from before.
                if self.config.value(key).is_some() {
                    self.config.set_value(key, None);
                    let _ = self.config.save();
                }
            }
            Err(why) => {
                if value.is_some() {
                    self.note_output(ext, format!("  warn  secret-set {key}: kept in settings.toml as plain text -- {why}"));
                }
                self.config.set_value(key, value.clone().map(fenix_config::Value::Text));
                if let Err(e) = self.config.save() {
                    self.set_error(format!("couldn't save settings.toml: {e}"));
                }
            }
        }
        let readers: Vec<String> = self.wasm.installed.iter().filter(|e| fenix_extensions::manifest::secret_allowed(&e.manifest.permissions.secrets, key)).map(|e| e.id.clone()).collect();
        for id in readers {
            if let Some(shared) = self.wasm.secrets.get(&id) {
                let mut shared = shared.write().unwrap();
                match &value {
                    Some(v) => shared.insert(key.to_string(), v.clone()),
                    None => shared.remove(key),
                };
            }
        }
    }

    /// What the workers sent with no event loop to bring it: applied now.
    #[cfg(test)]
    pub(crate) fn pump_wasm(&mut self) {
        let events = std::mem::take(&mut *self.wasm.inbox.lock().unwrap());
        for (ext, event) in events {
            self.apply_wasm_event(ext, event);
        }
    }

    /// Whether extension `ext` is up and taking calls.
    pub(super) fn code_ready(&self, ext: &str) -> bool {
        matches!(self.wasm.status.get(ext), Some(Status::Ready))
    }

    /// A line for extension `ext`'s output page.
    pub(super) fn note_code_output(&mut self, ext: &str, line: String) {
        self.note_output(ext, line);
    }

    pub(super) fn request_window_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    pub(super) fn note_output(&mut self, ext: &str, line: String) {
        let lines = self.wasm.output.entry(ext.to_string()).or_default();
        lines.push(line);
        if lines.len() > KEPT_LINES {
            lines.drain(..lines.len() - KEPT_LINES);
        }
    }

    /// The output page's text: each code extension, where it is, and what
    /// it said.
    pub(crate) fn extension_output_text(&self) -> String {
        if self.wasm.installed.is_empty() {
            return "No extension that runs code is installed and switched on.\n\nThe Extensions page (SPC h x) installs them.\n".into();
        }
        let mut out = String::new();
        for ext in &self.wasm.installed {
            let status = match self.wasm.status.get(&ext.id) {
                None => "not started: it starts the first time one of its commands runs".to_string(),
                Some(Status::Starting) => "starting".into(),
                Some(Status::Ready) => "running".into(),
                Some(Status::Stopped(why)) => format!("stopped -- {why}"),
            };
            out += &format!("{} {} ({}): {status}\n", ext.name, ext.manifest.extension.version, ext.id);
            for line in self.wasm.output.get(&ext.id).into_iter().flatten() {
                out += &format!("  {line}\n");
            }
            out += "\n";
        }
        out
    }

    /// `SPC h l`: what the code extensions said, kept up to date while
    /// it's open.
    pub(crate) fn open_extension_output(&mut self) {
        match self.find_page(|m| matches!(m, PageModel::ExtensionOutput(_))) {
            Some(id) => {
                self.refresh_extension_output();
                self.show_page(id);
            }
            None => {
                let page = crate::extension_output::OutputPage::new(self.extension_output_text());
                self.open_page(PageModel::ExtensionOutput(Box::new(page)));
            }
        }
    }

    fn refresh_extension_output(&mut self) {
        let Some(id) = self.find_page(|m| matches!(m, PageModel::ExtensionOutput(_))) else { return };
        let text = self.extension_output_text();
        if let Some(state) = self.pages.get_mut(&id) {
            if let PageModel::ExtensionOutput(page) = &mut state.model {
                page.focus = page.focus.min(text.lines().count().saturating_sub(1));
                page.text = text;
            }
            state.stale = true;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use fenix_wasm::host::Focus;

    pub(crate) fn hello(manifest_extra: &str) -> CodeExtension {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fenix-wasm/tests/guests");
        let staged = tempfile::tempdir().unwrap().keep();
        std::fs::copy(dir.join("hello.wasm"), staged.join("extension.wasm")).unwrap();
        std::fs::copy(dir.join("hello-sheet.png"), staged.join("hello-sheet.png")).unwrap();
        let manifest = fenix_extensions::Manifest::parse(&format!(
            "[extension]\nid = \"test.hello\"\nname = \"Hello\"\nversion = \"1.0.0\"\nruntime = \"wasm\"\n\n[engine]\napi = \"1.0\"\nfenix = \">=1.0, <2\"\n{manifest_extra}\n[[contributes.commands]]\nid = \"hello.greet\"\ntitle = \"Greet\"\n\n[[contributes.commands]]\nid = \"hello.spin\"\ntitle = \"Spin\"\n"
        ))
        .unwrap();
        CodeExtension { id: "test.hello".into(), name: "Hello".into(), dir: staged, manifest }
    }

    /// A folder it's given is made when it isn't there, and its setting
    /// reads as where it is: set, or not (the notebook names its files to
    /// Fenix and the other extensions by it).
    #[test]
    fn a_folder_is_made_and_its_setting_says_where_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_file(None);
        let ext = hello("[permissions]\nfolders = { notes = \"${setting:hello.folder}\" }\n\n[[contributes.settings]]\nkey = \"hello.folder\"\nkind = \"path\"\ndefault = \"\"\n");
        let want = dir.path().join("notes");
        app.config.set_path_value("hello.folder", Some(&want));
        assert_eq!(app.code_folders(&ext), vec![("notes".to_string(), want.clone())]);
        assert!(want.is_dir(), "made");
        let (settings, _) = app.code_settings_now(&ext);
        assert_eq!(settings.get("hello.folder").map(PathBuf::from), Some(want));
    }

    /// A map of folders (`mib.roots`) is a folder each, at
    /// `/folders/<name>/<its name>`; those not there aren't made, and the
    /// setting reads as the map it is.
    #[test]
    fn a_map_of_folders_is_a_folder_each() {
        let dir = tempfile::tempdir().unwrap();
        let ops = dir.path().join("ops");
        std::fs::create_dir_all(&ops).unwrap();
        let mut app = App::with_file(None);
        let ext = hello("[permissions]\nfolders = { mib = \"${setting:hello.roots}\" }\n\n[[contributes.settings]]\nkey = \"hello.roots\"\nkind = \"map\"\ndefault = \"\"\n");
        let gone = dir.path().join("gone");
        app.config.set_value("hello.roots", Some(fenix_config::Value::Map(vec![("OPS".into(), ops.to_string_lossy().into_owned()), ("OLD".into(), gone.to_string_lossy().into_owned()), ("../x".into(), "y".into())])));
        assert_eq!(app.code_folders(&ext), vec![("mib/OPS".to_string(), ops), ("mib/OLD".to_string(), gone.clone())], "a name that would climb out is left out");
        assert!(!gone.exists(), "not made");
        let (settings, _) = app.code_settings_now(&ext);
        assert!(settings.get("hello.roots").is_some_and(|v| v.starts_with("[[\"OPS\"")), "{settings:?}");
    }

    /// `permissions.settings` lets it read another's settings by the name
    /// before their dot, as it reads its own; others stay out of reach.
    #[test]
    fn another_namespaces_settings_are_read_when_it_asks() {
        static PROBE: [fenix_config::Setting; 1] = [fenix_config::Setting::stored("probe.edition", fenix_config::Category::Editor, "Edition", fenix_config::Kind::Text, "A test's.")];
        fenix_config::schema::register(&PROBE);
        let mut app = App::with_file(None);
        app.config.set_value("probe.edition", Some(fenix_config::Value::Text("a".into())));
        let without = hello("");
        let (settings, _) = app.code_settings_now(&without);
        assert!(!settings.contains_key("probe.edition"));
        let with = hello("[permissions]\nsettings = [\"probe\"]\n");
        let (settings, _) = app.code_settings_now(&with);
        assert_eq!(settings.get("probe.edition").map(String::as_str), Some("a"));
        assert!(!settings.keys().any(|k| k.starts_with("vnc.") || k.starts_with("editor.")), "only what it named");
        assert!(fenix_extensions::Manifest::parse("[extension]\nid = \"x.y\"\nname = \"Y\"\nversion = \"1.0.0\"\nruntime = \"wasm\"\n[engine]\napi = \"1.0\"\nfenix = \">=2.0\"\n[permissions]\nsettings = [\"ccsds.pus\"]\n").is_err(), "a namespace, not a key");
    }

    /// Waits for the worker's events and applies them, until `done`.
    fn pump_until(app: &mut App, done: impl Fn(&App) -> bool) {
        let started = std::time::Instant::now();
        while !done(app) {
            assert!(started.elapsed() < std::time::Duration::from_secs(60), "{}", app.extension_output_text());
            std::thread::sleep(std::time::Duration::from_millis(20));
            app.pump_wasm();
        }
    }

    fn said(app: &App, text: &str) -> bool {
        app.extension_output_text().contains(text)
    }

    /// A command starts its extension, runs once it's ready, and what it
    /// says is on the output page; a trap stops it and later commands
    /// say so.
    #[test]
    fn a_code_extensions_command_starts_it_and_runs() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("")];
        let registry = CommandRegistry::with_builtins().with_code_extensions(&app.wasm.installed);
        let greet = registry.get("hello.greet").expect("its command is registered");
        assert_eq!(greet.source, Some("test.hello"));
        assert!(!greet.external());
        app.run_extension_command("test.hello", "hello.greet");
        pump_until(&mut app, |a| said(a, "hello, Fenix") && a.status_message.as_ref().is_some_and(|m| m.text == "Hello says hello"));
        let text = app.extension_output_text();
        assert!(text.contains("Hello 1.0.0 (test.hello): running") && text.contains("info  hello is up") && text.contains("stdout  and printing to stdout"), "{text}");

        app.open_extension_output();
        let page = app.focused_buffer_id();
        assert_eq!(app.page_title(page), "*extensions' output*");
        app.run_extension_command("test.hello", "hello.spin");
        pump_until(&mut app, |a| matches!(a.wasm.status.get("test.hello"), Some(Status::Stopped(_))));
        let Some(PageModel::ExtensionOutput(shown)) = app.pages.get(&page).map(|s| &s.model) else { panic!("the output page") };
        assert!(shown.text.contains("stopped -- test.hello stopped: it took too long"), "the open page follows: {}", shown.text);
        app.run_extension_command("test.hello", "hello.greet");
        let said = app.status_message.as_ref().filter(|m| m.is_error).map(|m| m.text.clone());
        assert!(said.as_deref().is_some_and(|e| e.contains("test.hello stopped")), "{said:?}");
    }

    /// A service it answers later reaches Fenix's own caller when it
    /// does; Home has its section, and a row picked goes back to it.
    #[test]
    fn a_service_answered_later_and_its_home_section_reach_fenix() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("[contributes]\nservices = [\"hello.later\"]\n")];
        assert_eq!(app.code_server("hello.later", None).as_deref(), Some("test.hello"));
        assert_eq!(app.code_server("hello.later", Some("test.hello")), None, "not its own");
        let got = std::rc::Rc::new(std::cell::RefCell::new(None));
        let kept = got.clone();
        app.code_serve("test.hello", "hello.later", "x".into(), Answer::Fenix(Box::new(move |_, answer| *kept.borrow_mut() = Some(answer))));
        pump_until(&mut app, |_| got.borrow().is_some());
        assert_eq!(got.borrow().clone(), Some(Ok("later: x".to_string())));

        app.wasm.home.insert(
            "test.hello".into(),
            fenix_wasm::host::HomeSection { title: "hello".into(), key: "SPC h h".into(), rows: vec![fenix_wasm::host::HomeRow { name: "a row".into(), detail: String::new(), mark: fenix_wasm::host::HomeMark::Task(true), value: "row-1".into() }], most: 2 },
        );
        let sections = app.code_home_sections();
        assert_eq!((sections[0].title, sections[0].rows[0].mark.clone()), ("hello", crate::dashboard::RowMark::Task(true)));
        assert_eq!(sections[0].rows[0].entry, crate::dashboard::HomeEntry::Code { ext: "test.hello".into(), value: "row-1".into() });
        app.activate_home_entry(sections[0].rows[0].entry.clone());
        pump_until(&mut app, |a| said(a, "home picked row-1"));
    }

    /// A canvas is drawn as it's laid out, and its keys and pastes go
    /// to it.
    #[test]
    fn a_canvas_page_is_drawn_as_it_is_laid_out() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("")];
        app.run_extension_command("test.hello", "hello.canvas");
        pump_until(&mut app, |a| a.find_page(|m| matches!(m, PageModel::Code(_))).is_some());
        let buffer = app.find_page(|m| matches!(m, PageModel::Code(_))).unwrap();
        app.code_page_wants(buffer, 90, 24);
        pump_until(&mut app, |a| matches!(a.pages.get(&buffer).map(|s| &s.model), Some(PageModel::Code(p)) if p.shown.is_some()));
        let Some(PageModel::Code(p)) = app.pages.get(&buffer).map(|s| &s.model) else { unreachable!() };
        let page = crate::code_page::layout(p, 90);
        assert!(page.text.starts_with("  Canvas 90"), "no title of Fenix's: {}", page.text);
        assert_eq!(page.spans[0].role, crate::page::Role::Syntax("keyword"));
        assert_eq!(page.focus, Some((2, 2..20)));
        assert!(p.claims_space() && !p.typing());
        assert_eq!(p.key(crate::page::Key::Space), crate::code_page::Action::Key(fenix_wasm::host::Key::Char(' ')));
        app.code_page_paste(buffer, "a".into());
        let started = std::time::Instant::now();
        loop {
            app.pump_wasm();
            app.code_page_wants(buffer, 90, 24);
            if let Some(PageModel::Code(p)) = app.pages.get(&buffer).map(|s| &s.model) {
                if p.typing() {
                    assert_eq!(p.key(crate::page::Key::Char('q')), crate::code_page::Action::Key(fenix_wasm::host::Key::Char('q')), "q is typed, not closing");
                    break;
                }
            }
            assert!(started.elapsed() < std::time::Duration::from_secs(60));
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Edits land at the places they name in the text as it was, as one
    /// undo step; a request for a buffer's text, or the clipboard's
    /// picture, is answered.
    #[test]
    fn a_buffer_is_edited_and_read_for_an_extension() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.md");
        std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
        let mut app = App::with_file(Some(file.display().to_string()));
        let edit = |start, end, text: &str| fenix_wasm::host::TextEdit { start, end, text: text.into() };
        app.answer_code_request("test.hello", Request::BufferEdit { path: file.clone(), open_only: false, edits: vec![edit((0, 0), (0, 3), "ONE"), edit((2, 5), (2, 5), "!"), edit((1, 0), (1, 0), "- ")], text: None });
        assert_eq!(app.open().buffer.text(), "ONE\n- two\nthree!\n");
        app.open_mut().buffer.undo(&mut Cursor::at_start());
        assert_eq!(app.open().buffer.text(), "one\ntwo\nthree\n", "one undo step");
        let (tx, rx) = std::sync::mpsc::channel();
        let answer = fenix_wasm::host::Answer::new(tx);
        app.answer_code_request("test.hello", Request::BufferText { path: file.clone(), open_only: false, answer });
        assert_eq!(rx.try_recv().unwrap().as_deref(), Some("one\ntwo\nthree\n"));
        let (tx, rx) = std::sync::mpsc::channel();
        app.answer_code_request("test.hello", Request::BufferText { path: dir.path().join("nope.md"), open_only: false, answer: fenix_wasm::host::Answer::new(tx) });
        assert_eq!(rx.try_recv().unwrap(), None);
    }

    /// A file it may not reach, opened at its asking, is opened sealed:
    /// as text, whatever opens it, and its buffer doesn't reach it -- nor
    /// does one not open. One you opened does.
    #[test]
    fn a_file_opened_at_its_asking_is_sealed_from_it() {
        let dir = tempfile::tempdir().unwrap();
        let (secret, yours) = (dir.path().join("id_rsa.bin"), dir.path().join("notes.md"));
        std::fs::write(&secret, "-----BEGIN KEY-----\n").unwrap();
        std::fs::write(&yours, "mine\n").unwrap();
        let mut app = App::with_file(Some(yours.display().to_string()));
        app.wasm.installed = vec![hello("[contributes]\nopens = [\"*.bin\"]\n")];
        let text = |app: &mut App, path: &Path| {
            let (tx, rx) = std::sync::mpsc::channel();
            app.answer_code_request("test.hello", Request::BufferText { path: path.to_path_buf(), open_only: true, answer: fenix_wasm::host::Answer::new(tx) });
            rx.try_recv().unwrap()
        };
        assert_eq!(text(&mut app, &secret), None, "not open");
        app.answer_code_request("test.hello", Request::OpenFile { path: secret.clone(), line: None, sealed: true, text: None });
        assert_eq!(app.open().buffer.path(), Some(secret.as_path()), "opened as text, not given to what opens it");
        assert!(!app.wasm.workers.contains_key("test.hello"));
        assert_eq!(text(&mut app, &secret), None, "sealed");
        app.answer_code_request("test.hello", Request::BufferEdit { path: secret.clone(), open_only: true, edits: vec![fenix_wasm::host::TextEdit { start: (0, 0), end: (0, 0), text: "x".into() }], text: None });
        assert_eq!(app.open().buffer.text(), "-----BEGIN KEY-----\n", "nor edited");
        assert_eq!(text(&mut app, &yours).as_deref(), Some("mine\n"), "yours, open: it reads it");
        let other = dir.path().join("other.md");
        std::fs::write(&other, "x\n").unwrap();
        app.answer_code_request("test.hello", Request::BufferEdit { path: other.clone(), open_only: true, edits: Vec::new(), text: None });
        assert!(app.buffers.id_for_path(&other).is_none(), "not opened for it");
    }

    /// Its language server runs beside the others for its languages'
    /// buffers, in a session of its own per project, and goes when it
    /// stops.
    #[test]
    fn its_language_server_is_a_session_for_its_languages() {
        let dir = tempfile::tempdir().unwrap();
        let note = dir.path().join("note.md");
        std::fs::write(&note, "# x\n").unwrap();
        let mut app = App::with_file(Some(note.display().to_string()));
        app.wasm.installed = vec![hello("[[contributes.language_servers]]\nname = \"hello\"\nlanguages = [\"markdown\"]\n")];
        assert_eq!(app.code_language_servers_for(&note, Some("markdown"), dir.path()), [("test.hello".to_string(), "hello".to_string(), vec!["markdown".to_string()], false)]);
        assert!(app.code_language_servers_for(&dir.path().join("a.rs"), Some("rust"), dir.path()).is_empty());
        app.sync_lsp_for_focused_buffer();
        let ours = |a: &App| a.lsp_sessions.keys().filter(|k| matches!(&k.server, LspServer::Additional(n) if n == "test.hello:hello")).count();
        assert_eq!(ours(&app), 1, "started, and its extension with it");
        assert!(app.wasm.workers.contains_key("test.hello"));
        app.sync_lsp_for_focused_buffer();
        assert_eq!(ours(&app), 1, "once a project");
        app.forget_code_lsp_sessions("test.hello");
        assert_eq!(ours(&app), 0);
    }

    /// Its picker is Fenix's: what's picked goes back to it.
    #[test]
    fn its_picker_is_shown_and_the_choice_goes_back() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("")];
        app.run_extension_command("test.hello", "hello.pick");
        pump_until(&mut app, |a| matches!(a.active_picker, Some(ActivePicker::Code { .. })));
        let Some(ActivePicker::Code { badge, state, .. }) = &app.active_picker else { unreachable!() };
        assert_eq!(*badge, "A COLOUR");
        assert_eq!(state.len(), 2);
        if let Some(ActivePicker::Code { state, .. }) = &mut app.active_picker {
            state.move_selection(1);
        }
        app.picker_confirm();
        assert!(app.active_picker.is_none());
        pump_until(&mut app, |a| said(a, "picked 1: Some(\"green\")"));
    }

    /// Its page is laid out by it, keys go to it, and it's laid out again.
    #[test]
    fn its_page_is_laid_out_by_it_and_keyed() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("")];
        app.run_extension_command("test.hello", "hello.page");
        pump_until(&mut app, |a| a.find_page(|m| matches!(m, PageModel::Code(_))).is_some());
        let buffer = app.find_page(|m| matches!(m, PageModel::Code(_))).unwrap();
        assert_eq!(app.page_title(buffer), "*Hello*");
        app.code_page_wants(buffer, 80, 24);
        pump_until(&mut app, |a| matches!(a.pages.get(&buffer).map(|s| &s.model), Some(PageModel::Code(p)) if p.shown.is_some()));
        let laid = |a: &App| match a.pages.get(&buffer).map(|s| &s.model) {
            Some(PageModel::Code(p)) => crate::code_page::layout(p, 100).all_text(),
            _ => String::new(),
        };
        assert!(laid(&app).contains("counted 0"), "{}", laid(&app));
        app.code_page_key(buffer, fenix_wasm::host::Key::Char('j'));
        let started = std::time::Instant::now();
        while !laid(&app).contains("counted 1") {
            assert!(started.elapsed() < std::time::Duration::from_secs(60), "{}", laid(&app));
            std::thread::sleep(std::time::Duration::from_millis(20));
            app.pump_wasm();
            app.code_page_wants(buffer, 80, 24);
        }
    }

    /// A prompt is Fenix's modeline: what's typed goes back to it.
    #[test]
    fn its_prompt_takes_what_you_type() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("")];
        app.start_code_extension("test.hello");
        pump_until(&mut app, |a| a.wasm.status.get("test.hello") == Some(&Status::Ready));
        app.wasm.prompt = Some(CodePrompt { ext: "test.hello".into(), id: 7, label: "Name".into(), input: "a".into(), masked: false });
        assert_eq!(app.code_prompt_text().as_deref(), Some("Name: a▏"));
        assert!(app.code_prompt_key(KeyPress::char('b')));
        assert!(app.code_prompt_key(KeyPress::named(FenixNamedKey::Enter)));
        assert!(app.wasm.prompt.is_none());
        pump_until(&mut app, |a| said(a, "prompted 7: Some(\"ab\")"));
        assert!(!app.code_prompt_key(KeyPress::char('x')), "none open: not its key");
    }

    /// a masked prompt shows dots for what's typed, and gives it
    /// back as it was typed.
    #[test]
    fn a_masked_prompt_shows_dots() {
        let mut app = App::with_file(None);
        app.answer_code_request("test.hello", Request::Prompt { id: 3, label: "Password".into(), initial: String::new(), masked: true });
        for c in "pw!".chars() {
            app.code_prompt_key(KeyPress::char(c));
        }
        assert_eq!(app.code_prompt_text().as_deref(), Some("Password: •••▏"));
        assert_eq!(app.wasm.prompt.as_ref().map(|p| p.input.as_str()), Some("pw!"));
    }

    /// a secret an extension keeps goes to the keychain, not the
    /// file, and its workers read it; with no keychain it's kept in the
    /// file, and it's told so.
    #[test]
    fn a_secret_it_keeps_goes_to_the_keychain() {
        let mut app = App::with_file(None);
        let keychain = Arc::new(fenix_config::keychain::Keychain::with(fenix_config::keychain::Memory::default()));
        app.wasm.keychain = Some(keychain.clone());
        app.wasm.installed = vec![hello("[permissions]\nsecrets = [\"hello.secret.*\"]\n")];
        app.start_code_extension("test.hello");
        pump_until(&mut app, |a| a.wasm.status.get("test.hello") == Some(&Status::Ready));
        app.answer_code_request("test.hello", Request::SecretSet { key: "hello.secret.pw".into(), value: Some(fenix_wasm::host::Hidden("hunter2".into())) });
        assert_eq!(keychain.get("hello.secret.pw").as_deref(), Some("hunter2"));
        assert!(app.config.value("hello.secret.pw").is_none() && !std::fs::read_to_string(app.config.path()).unwrap_or_default().contains("hunter2"), "not in the file");
        assert_eq!(app.wasm.secrets["test.hello"].read().unwrap().get("hello.secret.pw").map(String::as_str), Some("hunter2"));
        app.answer_code_request("test.hello", Request::SecretSet { key: "hello.secret.pw".into(), value: None });
        assert_eq!(keychain.get("hello.secret.pw"), None);
        assert!(!app.wasm.secrets["test.hello"].read().unwrap().contains_key("hello.secret.pw"));
        // No keychain: the file, as plain text, and said.
        app.wasm.keychain = Some(Arc::new(fenix_config::keychain::Keychain::none()));
        app.answer_code_request("test.hello", Request::SecretSet { key: "hello.secret.pw".into(), value: Some(fenix_wasm::host::Hidden("hunter3".into())) });
        assert_eq!(app.config.value("hello.secret.pw"), Some(&fenix_config::Value::Text("hunter3".into())));
        assert!(app.wasm.output["test.hello"].iter().any(|l| l.contains("kept in settings.toml as plain text")), "{:?}", app.wasm.output["test.hello"]);
    }

    /// a command's deadline is its manifest's `limits.call_ms`, else
    /// five seconds.
    #[test]
    fn a_call_has_the_deadline_its_manifest_gives() {
        assert_eq!(call_ticks(&hello("").manifest), COMMAND_TICKS);
        assert_eq!(call_ticks(&hello("[limits]\ncall_ms = 30000\n").manifest), 3000);
    }

    /// a file it opens whose first bytes aren't what its manifest
    /// says such a file starts with isn't opened to it.
    #[test]
    fn a_file_that_isnt_what_it_opens_opens_as_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("[contributes]\nopens = [\"*.db\"]\n\n[contributes.magic]\n\"*.db\" = \"SQLite format 3\"\n")];
        let (sqlite, other) = (dir.path().join("a.db"), dir.path().join("b.db"));
        std::fs::write(&sqlite, b"SQLite format 3\0rest").unwrap();
        std::fs::write(&other, b"not a database").unwrap();
        assert_eq!(app.code_opener(&sqlite).as_deref(), Some("test.hello"));
        assert_eq!(app.code_opener(&other), None);
    }

    /// A map setting whose values are a "Folder" or "File" is browsed for
    /// them on the settings page (`mib.roots`, `reader.documents`); any
    /// other map isn't.
    #[test]
    fn a_map_of_folders_or_files_is_browsed_for_them() {
        let ext = hello("\n[[contributes.settings]]\nkey = \"hello.roots\"\nkind = \"map\"\ncolumns = [\"Name\", \"Folder\"]\n\n[[contributes.settings]]\nkey = \"hello.names\"\nkind = \"map\"\ncolumns = [\"Key\", \"Name\"]\n");
        let settings = crate::installed::settings_of(&ext);
        assert!(matches!(settings[0].kind, fenix_config::schema::Kind::Map { value: "Folder", paths: true, .. }));
        assert!(matches!(settings[1].kind, fenix_config::schema::Kind::Map { paths: false, .. }));
    }

    /// Its settings are a section of the settings page of their own: the
    /// one each names, else the extension's name.
    #[test]
    fn its_settings_are_in_its_own_section() {
        let ext = hello("\n[[contributes.settings]]\nkey = \"hello.url\"\n\n[[contributes.settings]]\nkey = \"hello.port\"\nsection = \"Hello server\"\n");
        let settings = crate::installed::settings_of(&ext);
        assert_eq!(settings[0].category, fenix_config::Category::Extension("Hello"));
        assert_eq!(settings[1].category, fenix_config::Category::Extension("Hello server"));
    }

    /// Its settings are read from yours; a change reaches it.
    #[test]
    fn its_settings_are_yours_and_changes_reach_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_file(None);
        app.config = fenix_config::Config::load_or_default(dir.path().join("settings.toml"));
        let ext = hello("\n[[contributes.settings]]\nkey = \"hello.url\"\ndefault = \"http://localhost\"\n");
        assert_eq!(crate::installed::settings_of(&ext)[0].key, "hello.url");
        app.wasm.installed = vec![ext.clone()];
        let (settings, _) = app.code_settings_now(&ext);
        assert_eq!(settings.get("hello.url").map(String::as_str), Some("http://localhost"), "its default when unset");
        app.start_code_extension("test.hello");
        pump_until(&mut app, |a| a.wasm.status.get("test.hello") == Some(&Status::Ready));
        app.code_setting_changed("hello.url");
        pump_until(&mut app, |a| said(a, "hello.url changed"));
    }

    /// Its setting edited by hand in `settings.toml` reaches it too, as
    /// one changed on the settings page does.
    #[test]
    fn a_hand_edit_to_its_setting_reaches_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_file(None);
        app.config = fenix_config::Config::load_or_default(dir.path().join("settings.toml"));
        let ext = hello("\n[[contributes.settings]]\nkey = \"hello.edited\"\ndefault = \"one\"\n");
        fenix_config::schema::register(Box::leak(crate::installed::settings_of(&ext).into_boxed_slice()));
        app.wasm.installed = vec![ext];
        app.start_code_extension("test.hello");
        pump_until(&mut app, |a| a.wasm.status.get("test.hello") == Some(&Status::Ready));
        app.config.save().unwrap();
        app.reload_settings_if_changed();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(app.config.path(), "[hello]\nedited = \"two\"\n").unwrap();
        app.reload_settings_if_changed();
        pump_until(&mut app, |a| said(a, "hello.edited changed"));
        assert_eq!(app.wasm.settings["test.hello"].read().unwrap().get("hello.edited").map(String::as_str), Some("two"));
    }

    /// 0.2: a read-only text, a URL, the clipboard and its modeline item.
    #[test]
    fn text_urls_and_its_modeline_item_are_done() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("")];
        app.run_extension_command("test.hello", "hello.extras");
        pump_until(&mut app, |a| !a.wasm.opened.is_empty() && !a.wasm.statuses.is_empty());
        assert_eq!(app.wasm.opened, ["https://example.com/"], "the file: URL wasn't asked for");
        assert_eq!(app.code_statuses(), ["   hello · ok"]);
        let shown = app.focused_buffer_id();
        assert!(app.ref_view_names.values().any(|n| n == "*Hello JSON*"), "{:?}", app.ref_view_names);
        assert_eq!(app.buffers.get(shown).map(|b| b.buffer.text()), Some("{\"hello\": 1}".to_string()));
    }

    /// Its page's popup is drawn, it hears when the page is shown or
    /// hidden, and the page comes back from the session in the state it
    /// was left.
    #[test]
    fn its_page_has_a_popup_hears_shown_and_comes_back_with_the_session() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("")];
        app.run_extension_command("test.hello", "hello.page");
        pump_until(&mut app, |a| a.find_page(|m| matches!(m, PageModel::Code(_))).is_some());
        let buffer = app.find_page(|m| matches!(m, PageModel::Code(_))).unwrap();
        let laid = |a: &App, b: BufferId| match a.pages.get(&b).map(|s| &s.model) {
            Some(PageModel::Code(p)) => crate::code_page::layout(p, 100),
            _ => crate::page::Page::default(),
        };
        for _ in 0..2 {
            app.code_page_key(buffer, fenix_wasm::host::Key::Char('j'));
        }
        let started = std::time::Instant::now();
        while laid(&app, buffer).popup.is_none() {
            assert!(started.elapsed() < std::time::Duration::from_secs(60), "{}", laid(&app, buffer).all_text());
            std::thread::sleep(std::time::Duration::from_millis(20));
            app.pump_wasm();
            app.code_page_wants(buffer, 80, 24);
        }
        assert!(laid(&app, buffer).popup.unwrap().text().contains("reset"));
        // Drawn this frame, then not.
        app.code_page_drawn(buffer, 80, 24);
        app.code_pages_drawn();
        pump_until(&mut app, |a| said(a, "shown: true"));
        app.code_pages_drawn();
        pump_until(&mut app, |a| said(a, "shown: false"));
        // Its state, kept for the session; then put back from it.
        pump_until(&mut app, |a| matches!(a.pages.get(&buffer).map(|s| &s.model), Some(PageModel::Code(p)) if p.state.as_deref() == Some("2")));
        let saved = app.code_pages_to_save();
        assert_eq!(saved.iter().map(|p| (p.ext.as_str(), p.kind.as_str(), p.title.as_str(), p.state.as_str())).collect::<Vec<_>>(), [("test.hello", "counter", "Hello", "2")]);
        let back = app.restore_code_page("test.hello", "counter", "Hello", "7", None, &[]).expect("its extension is on");
        let started = std::time::Instant::now();
        while !laid(&app, back).all_text().contains("counted 7") {
            assert!(started.elapsed() < std::time::Duration::from_secs(60), "{}", laid(&app, back).all_text());
            std::thread::sleep(std::time::Duration::from_millis(20));
            app.pump_wasm();
            app.code_page_wants(back, 80, 24);
        }
        assert!(app.restore_code_page("acme.gone", "x", "Gone", "1", None, &[]).is_none(), "an extension that isn't there");
        let lost = app.restore_code_page("test.hello", "nothing-like-this", "Lost", "1", None, &[]).unwrap();
        pump_until(&mut app, |a| !a.pages.contains_key(&lost));
    }

    /// A terminal it owns opens under the focused pane, is started again
    /// with what it asks, and its closing is told back; a task it runs is
    /// Fenix's own task, and its end is told back too.
    #[test]
    fn its_terminals_and_tasks_are_fenixs_own_and_their_ends_go_back() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("[permissions]\nprocess = [\"cmd\"]\n")];
        let panes = app.windows().windows().len();
        app.run_extension_command("test.hello", "hello.terminal");
        pump_until(&mut app, |a| said(a, "terminal 1") && a.wasm.terminals.values().any(|(_, id)| *id == 1));
        pump_until(&mut app, |a| a.wasm.terminals.keys().any(|b| a.terminal_buffer_programs.get(b).is_some_and(|(_, args)| args.get(2).map(String::as_str) == Some("again"))));
        let buffer = *app.wasm.terminals.keys().next().unwrap();
        assert_eq!(app.windows().windows().len(), panes + 1, "under the focused pane");
        assert_eq!(app.terminal_buffer_labels.get(&buffer).map(String::as_str), Some("*Hello again*"));
        app.tell_extensions(super::Happened::TerminalClosed(buffer));
        pump_until(&mut app, |a| said(a, "terminal 1 closed"));
        assert!(app.wasm.terminals.is_empty());

        app.run_extension_command("test.hello", "hello.task");
        pump_until(&mut app, |a| said(a, "task 2 finished: true"));
        assert!(app.task_session.is_some(), "Fenix's task output pane ran it");
        assert!(app.wasm.task.is_none());
    }

    /// Opening a file in another project starts an extension that reads
    /// the project again there, with its pages kept; a file in the same
    /// project is only told.
    #[test]
    fn a_file_in_another_project_starts_it_again_there_with_its_pages() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("[permissions]\nfilesystem = \"project-read\"\n")];
        app.run_extension_command("test.hello", "hello.page");
        pump_until(&mut app, |a| a.find_page(|m| matches!(m, PageModel::Code(_))).is_some());
        let buffer = app.find_page(|m| matches!(m, PageModel::Code(_))).unwrap();
        app.code_page_key(buffer, fenix_wasm::host::Key::Char('j'));
        pump_until(&mut app, |a| matches!(a.pages.get(&buffer).map(|s| &s.model), Some(PageModel::Code(p)) if p.state.as_deref() == Some("1")));
        let before = app.wasm.projects.get("test.hello").cloned().flatten();

        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir(other.path().join(".git")).unwrap();
        std::fs::write(other.path().join("main.cpp"), "int main() {}\n").unwrap();
        app.open_file_from_picker(&other.path().join("main.cpp"));
        app.tell_extensions(super::Happened::FocusChanged);
        pump_until(&mut app, |a| said(a, "the project changed: starting again there"));
        let now = app.wasm.projects.get("test.hello").cloned().flatten();
        assert_ne!(now, before);
        assert_eq!(now.as_deref().and_then(Path::file_name), other.path().file_name(), "{now:?}");
        let laid = |a: &App| match a.pages.get(&buffer).map(|s| &s.model) {
            Some(PageModel::Code(p)) => (p.page, crate::code_page::layout(p, 100).all_text()),
            _ => (0, String::new()),
        };
        let started = std::time::Instant::now();
        while !laid(&app).1.contains("counted 1") || laid(&app).0 < (1 << 40) {
            assert!(started.elapsed() < std::time::Duration::from_secs(60), "{}", laid(&app).1);
            std::thread::sleep(std::time::Duration::from_millis(20));
            app.pump_wasm();
            app.code_page_wants(buffer, 80, 24);
        }
        // The same project again: told where you are, not started again.
        std::fs::write(other.path().join("b.cpp"), "\n").unwrap();
        app.open_file_from_picker(&other.path().join("b.cpp"));
        app.tell_extensions(super::Happened::FocusChanged);
        pump_until(&mut app, |a| said(a, "in Some(\"/project/b.cpp\")"));
        assert_eq!(app.extension_output_text().matches("starting again there").count(), 1);
    }

    /// What it says about its project -- a language server, doctor checks,
    /// tasks -- is kept for that project and used there; a task it may
    /// not run is refused.
    #[test]
    fn what_it_says_about_its_project_is_used_there() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("[permissions]\nprocess = [\"cmd\"]\nfilesystem = \"project-read\"\n")];
        app.run_extension_command("test.hello", "hello.project");
        pump_until(&mut app, |a| said(a, "powershell isn't a program") && !a.wasm.tasks.is_empty());
        let root = app.wasm.projects.get("test.hello").cloned().flatten().expect("a project: Fenix's own, in tests");
        let server = app.wasm.language_server(&root, fenix_syntax::LanguageId::Cpp).expect("its server for C++");
        assert_eq!((server.executable.as_str(), server.args.last().map(String::as_str)), ("cmd", Some("hello-ls")));
        assert!(app.wasm.language_server(&root, fenix_syntax::LanguageId::Rust).is_none(), "not a language it named");
        assert!(app.wasm.language_server(&std::env::temp_dir(), fenix_syntax::LanguageId::Cpp).is_none(), "nor another project");
        let checks = app.wasm.doctor_checks(&root);
        assert_eq!(checks.len(), 2, "its own, and its server's");
        assert!(checks[0].label == "cmd" && checks[0].detail.starts_with("language server for cpp · test.hello"), "{}", checks[0].detail);
        assert_eq!((checks[1].label.as_str(), checks[1].health), ("board", fenix_project::doctor::Health::Warn));
        assert!(matches!(&checks[1].fix, Some(f) if matches!(&f.action, fenix_project::doctor::FixAction::Editor(fenix_project::doctor::EditorFix::RunCommand(c)) if c == "hello.greet")));
        assert_eq!(app.wasm.tasks(&root).iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["hello: build"]);
    }

    /// A language pack is for a project its contexts' markers or languages
    /// are in; one that names no context is for any. A Go project's doctor
    /// and task list were showing every pack's.
    #[test]
    fn a_pack_is_for_the_projects_its_contexts_name() {
        let mut app = App::with_file(None);
        let mut go = hello("[[contributes.contexts]]\nname = \"go\"\nmarkers = [\"go.mod\"]\n\n[[contributes.contexts]]\nname = \"go-files\"\nlanguages = [\"go\"]\n");
        go.id = "test.go".into();
        let any = hello("");
        app.wasm.installed = vec![go, any];
        let dir = tempfile::tempdir().unwrap();
        let (gomod, loose, other) = (dir.path().join("a"), dir.path().join("b"), dir.path().join("c"));
        for d in [&gomod, &loose.join("src"), &other] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(gomod.join("go.mod"), "module a\n").unwrap();
        std::fs::write(loose.join("src").join("main.go"), "package main\n").unwrap();
        std::fs::write(other.join("Cargo.toml"), "[package]\n").unwrap();
        assert!(app.wasm.is_for("test.go", &gomod), "its marker");
        assert!(app.wasm.is_for("test.go", &loose), "a Go file in src/");
        assert!(!app.wasm.is_for("test.go", &other), "a Rust project");
        assert!(app.wasm.is_for("test.hello", &other), "no context: every project");
        assert!(app.wasm.is_for("not.installed", &other));
    }

    /// A context it declares is a `SPC m` menu wherever a folder holds its
    /// markers; its keymaps' `[[local.<name>]]` entries are the menu.
    #[test]
    fn its_context_is_a_local_menu_where_its_markers_are() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("[[contributes.contexts]]\nname = \"hello-sketch\"\nmarkers = [\"{folder}.hello\"]\n")];
        let dir = tempfile::tempdir().unwrap();
        let sketch = dir.path().join("Blink");
        std::fs::create_dir_all(&sketch).unwrap();
        std::fs::write(sketch.join("main.cpp"), "\n").unwrap();
        app.open_file_from_picker(&sketch.join("main.cpp"));
        assert!(!app.local_contexts().iter().any(|c| c.name() == "hello-sketch"), "no marker yet");
        std::fs::write(sketch.join("Blink.hello"), "").unwrap();
        assert!(app.local_contexts().contains(&crate::keymap::LocalContext::named("hello-sketch")));
        let menu = crate::keymap::extension_local_menu_for_tests("hello-sketch", "[[local.hello-sketch]]\nkeys = \"g\"\nlabel = \"greet\"\ncommand = \"hello.greet\"\n\n[[local.hello-sketch]]\nkeys = \"x\"\ncommand = \"no.such\"\n");
        assert_eq!(menu.1.len(), 1, "the unknown command is said: {:?}", menu.1);
        assert!(menu.1[0].contains("no.such isn't a command"));
        assert!(matches!(menu.0.matcher().feed(KeyPress::char('g')), fenix_keymap::Step::Matched(&"hello.greet")));
    }

    /// A context of its pages' kinds: its menu on its page of that kind,
    /// not on another's.
    #[test]
    fn its_context_is_a_local_menu_on_its_pages_of_a_kind() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("[[contributes.contexts]]\nname = \"hello-screen\"\npages = [\"screen\"]\n")];
        app.run_extension_command("test.hello", "hello.surface");
        pump_until(&mut app, |a| a.find_page(|m| matches!(m, PageModel::Code(p) if p.surface.is_some())).is_some());
        assert!(app.local_contexts().contains(&crate::keymap::LocalContext::named("hello-screen")));
        app.run_extension_command("test.hello", "hello.page");
        pump_until(&mut app, |a| matches!(a.pages.get(&a.focused_buffer_id()).map(|s| &s.model), Some(PageModel::Code(p)) if p.kind == "counter"));
        assert!(!app.local_contexts().contains(&crate::keymap::LocalContext::named("hello-screen")), "a counter page isn't a screen");
    }

    /// Its layer: the sheet decoded, the item kept, its glide's end and
    /// the layout told back, a click on it its own; and the editor events
    /// it asked for.
    #[test]
    fn its_layer_is_kept_animated_and_clicked_and_it_hears_what_you_do() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("[permissions]\ndraw = \"over\"\n\n[contributes]\nevents = [\"saved\", \"typing\"]\n")];
        app.run_extension_command("test.hello", "hello.layer");
        pump_until(&mut app, |a| said(a, "layer ") && a.wasm.layers.layers.values().any(|l| !l.items.is_empty()) && !a.wasm.layers.sheets.is_empty());
        let sheet = app.wasm.layers.sheets.values().next().unwrap();
        assert_eq!((sheet.width, sheet.height, sheet.frame), (4, 2, (2, 2)), "decoded, two frames");
        // A frame's geometry, as `redraw` leaves it.
        app.wasm.layers.geometry = super::super::layers::Geometry { cell: (10.0, 20.0), window: (800.0, 600.0), modeline_top: 580.0, panes: Vec::new(), caret: None };
        std::thread::sleep(std::time::Duration::from_millis(80));
        app.layers_after_frame();
        pump_until(&mut app, |a| said(a, "arrived ") && said(a, "layout 80x30"));
        let (layer, item) = app.wasm.layers.layers.iter().next().map(|((_, l), layer)| (*l, layer.items[0].item.id)).unwrap();
        app.wasm.layers.hits = vec![("test.hello".into(), layer, item, fenix_window::Rect { x: 0.0, y: 560.0, w: 20.0, h: 20.0 })];
        assert!(app.layer_click((5.0, 570.0)), "its item");
        assert!(!app.layer_click((300.0, 300.0)), "elsewhere: Fenix's");
        pump_until(&mut app, |a| said(a, &format!("clicked {layer}/{item}")));
        app.editor_event("saved", Notice::Saved("main.rs".into()));
        app.editor_event("diagnostics", Notice::Diagnostics((1, 0)));
        pump_until(&mut app, |a| said(a, "saved main.rs"));
        assert!(!said(&app, "1 errors"), "not one it asked for");
        // Stopped, its layers go.
        app.apply_wasm_event("test.hello".into(), Event::Stopped("test.hello stopped: gone".into()));
        assert!(app.wasm.layers.layers.is_empty() && app.wasm.layers.sheets.is_empty());
    }

    /// a socket to an echo server a setting of its names: what it
    /// wrote comes back to it.
    #[test]
    fn its_socket_echoes_through_a_local_listener() {
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = server.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = server.accept() {
                use std::io::{Read, Write};
                let mut buf = [0u8; 64];
                if let Ok(n) = stream.read(&mut buf) {
                    let _ = stream.write_all(&buf[..n]);
                }
            }
        });
        let mut app = App::with_file(None);
        app.config = fenix_config::Config::load_or_default(tempfile::tempdir().unwrap().keep().join("settings.toml"));
        app.wasm.installed = vec![hello(&format!("[permissions]\nsockets = [\"${{setting:hello.echo}}\"]\n\n[[contributes.settings]]\nkey = \"hello.echo\"\ndefault = \"127.0.0.1:{port}\"\n"))];
        app.run_extension_command("test.hello", "hello.socket");
        pump_until(&mut app, |a| said(a, "socket 1 said: ping"));
        assert!(said(&app, "socket 1 connected: Ok(())") && said(&app, "isn't somewhere this extension may open a TCP socket to"), "{}", app.extension_output_text());
    }

    /// A surface: opened in the focused pane with the keyboard, told its
    /// pane's size, its pixels shared, its marks placed; keys pressed and
    /// let go go to it until Ctrl+\ gives them back, and then it has a
    /// page's keys.
    #[test]
    fn its_surface_is_fed_sized_and_keyed() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("")];
        app.run_extension_command("test.hello", "hello.surface");
        pump_until(&mut app, |a| said(a, "surface 1") && a.find_page(|m| matches!(m, PageModel::Code(p) if p.surface.is_some())).is_some());
        let buffer = app.find_page(|m| matches!(m, PageModel::Code(p) if p.surface.is_some())).unwrap();
        assert_eq!(app.focused_buffer_id(), buffer, "in the focused pane");
        assert_eq!(app.wasm.captured, Some(buffer), "it captures the keyboard");
        pump_until(&mut app, |a| said(a, "capture true"));
        let surface = app.code_surface(buffer).unwrap();
        assert_eq!(surface.shared.lock().size(), (4, 2));
        assert_eq!(&surface.shared.lock().pixels()[0..4], &[0, 0, 255, 255]);
        // Stretched over a 400 x 200 pane: its 1 x 1 mark is 100 x 100.
        let marks = app.code_surface_marks(buffer, fenix_window::Rect { x: 0.0, y: 0.0, w: 400.0, h: 200.0 });
        assert_eq!(marks.iter().map(|m| m.0).collect::<Vec<_>>(), [(0.0, 0.0, 100.0, 100.0)]);
        // Its pane's size: told at once the first time.
        app.code_surfaces_drawn(vec![(buffer, (800, 600), 1.5)]);
        pump_until(&mut app, |a| said(a, "surface 1 is 800x600 at 1.5"));
        app.code_surfaces_drawn(vec![(buffer, (800, 600), 1.5)]);
        // Keys, pressed and let go, with Ctrl.
        assert!(app.code_surface_captured_key(Some(KeyPress::char('c').with_ctrl()), false, true));
        assert!(app.code_surface_captured_key(Some(KeyPress::char('c')), false, false));
        pump_until(&mut app, |a| said(a, "key SurfaceKey::Char('c') down ctrl") && said(a, "key SurfaceKey::Char('c') up"));
        assert!(app.code_surface_captured_key(Some(KeyPress::char('c')), false, true));
        pump_until(&mut app, |a| said(a, "copied: Ok(())"));
        assert_eq!(&app.code_surface(buffer).unwrap().shared.lock().pixels()[2 * 4..2 * 4 + 4], &[0, 0, 255, 255], "scrolled right");
        assert!(app.code_surface_captured_key(Some(KeyPress::char('\\').with_ctrl()), false, true), "Ctrl+\\ is Fenix's");
        assert_eq!(app.wasm.captured, None);
        pump_until(&mut app, |a| said(a, "capture false"));
        assert_eq!(app.extension_output_text().matches("key SurfaceKey::Char('\\\\')").count(), 0, "not sent on");
        assert!(!app.code_surface_captured_key(Some(KeyPress::char('x')), false, true), "nothing captures now");
        // A page's keys now: Space is the leader, the rest its.
        assert!(!app.code_surface_page_key(buffer, KeyPress::char(' ')));
        assert!(!app.code_surface_page_key(buffer, KeyPress::char('w').with_ctrl()), "Ctrl+W is the windows'");
        assert!(app.code_surface_page_key(buffer, KeyPress::char('x')));
        pump_until(&mut app, |a| said(a, "key SurfaceKey::Char('x') down"));
        assert!(app.code_surface_page_key(buffer, KeyPress::char('q')));
        assert!(!app.pages.contains_key(&buffer), "q closes it");
    }

    /// A language server that serves files by pattern: started for a file
    /// Fenix has no language for, told it's of the server's own kind; not
    /// for one that doesn't match.
    #[test]
    fn its_language_server_is_picked_by_file_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let table = dir.path().join("pcf.dat");
        std::fs::write(&table, "PCF1\tTEMP\n").unwrap();
        let mut app = App::with_file(Some(table.display().to_string()));
        app.wasm.installed = vec![hello("[[contributes.language_servers]]\nname = \"mib\"\nfiles = [\"*.dat\"]\n")];
        assert!(fenix_syntax::detect_language_from_path(&table).is_none(), "no language of Fenix's");
        let root = tool_sessions::lsp_root_for_path(&table);
        assert_eq!(app.code_language_servers_for(&table, None, &root), [("test.hello".to_string(), "mib".to_string(), Vec::new(), true)]);
        let targets = app.lsp_targets(&table);
        assert_eq!(targets.iter().map(|(k, id, by)| (k.server.clone(), id.as_str(), *by)).collect::<Vec<_>>(), [(LspServer::Additional("test.hello:mib".into()), "mib", true)]);
        app.sync_lsp_for_focused_buffer();
        let ours = |a: &App| a.lsp_sessions.keys().filter(|k| matches!(&k.server, LspServer::Additional(n) if n == "test.hello:mib")).count();
        assert_eq!(ours(&app), 1, "started for the table");
        assert!(app.wasm.workers.contains_key("test.hello"));
        let other = dir.path().join("notes.txt");
        assert!(app.code_language_servers_for(&other, None, &root).is_empty());
    }

    /// A file it opens itself is given to it to read; a forge it is answers
    /// Fenix's Git pages, given the repository.
    #[test]
    fn files_it_opens_and_the_forge_it_is_reach_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.bin");
        std::fs::write(&file, [1u8, 2, 3]).unwrap();
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("[contributes]\nopens = [\"*.bin\"]\n\n[[contributes.forges]]\nname = \"Hub\"\nhosts = [\"example.test\"]\n")];
        let open = app.opener_for(&file).expect("it opens .bin files");
        open(&mut app, &file, false);
        pump_until(&mut app, |a| said(a, "opened x.bin: Ok(3)"));
        assert!(app.opener_for(&dir.path().join("x.txt")).is_none());
        // A repository whose origin is on its host.
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| assert!(std::process::Command::new("git").args(args).current_dir(&repo).output().unwrap().status.success(), "git {args:?}");
        git(&["init", "-q"]);
        git(&["remote", "add", "origin", "git@example.test:o/r.git"]);
        let got = std::rc::Rc::new(std::cell::RefCell::new(None));
        let kept = got.clone();
        assert!(app.code_forge_call(&repo, "forge.branch", None, Box::new(move |_, answer| *kept.borrow_mut() = Some(answer))));
        pump_until(&mut app, |_| got.borrow().is_some());
        let line = serde_json::from_str::<Option<ForgeRequest>>(&got.borrow().clone().unwrap().unwrap()).unwrap().unwrap().line();
        assert_eq!((line.reference.as_str(), line.checks, line.unresolved), ("#7", Some("passed".to_string()), 1));
        assert!(app.code_forge_open(&repo, "forge.open", Some(7)));
        pump_until(&mut app, |a| said(a, "forge.open, its HEAD read: Some(true)"));
        git(&["remote", "set-url", "origin", "https://github.com/o/r.git"]);
        assert!(!app.code_forge_open(&repo, "forge.open", None), "not a host it serves");
    }

    /// `page-focus` brings its surface to the pane you're in, out of the
    /// one it was in; closing it is told to it (`page-closed`), its own
    /// `page-close` isn't.
    #[test]
    fn its_surface_is_brought_here_and_its_closing_told() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("")];
        app.run_extension_command("test.hello", "hello.surface");
        pump_until(&mut app, |a| a.find_page(|m| matches!(m, PageModel::Code(p) if p.surface.is_some())).is_some());
        let buffer = app.find_page(|m| matches!(m, PageModel::Code(p) if p.surface.is_some())).unwrap();
        let first = app.focused_pane_id();
        let scratch = app.buffers.open_scratch();
        let other = app.windows_mut().split(SplitKind::Vertical, scratch);
        app.windows_mut().focus(other);
        assert_ne!(app.focused_buffer_id(), buffer);
        app.run_extension_command("test.hello", "hello.focus");
        pump_until(&mut app, |a| a.focused_buffer_id() == buffer);
        assert_ne!(app.windows().content(first), Some(&buffer), "in one pane at a time");
        let page = match app.pages.get(&buffer).map(|s| &s.model) {
            Some(PageModel::Code(p)) => p.page,
            _ => unreachable!(),
        };
        app.close_page(buffer);
        pump_until(&mut app, |a| said(a, &format!("you closed {page}")));
        // One it closes itself isn't told back.
        app.run_extension_command("test.hello", "hello.page");
        pump_until(&mut app, |a| a.find_page(|m| matches!(m, PageModel::Code(p) if p.surface.is_none())).is_some());
        let page_buffer = app.find_page(|m| matches!(m, PageModel::Code(_))).unwrap();
        app.close_page_quietly(page_buffer);
        app.run_extension_command("test.hello", "hello.greet");
        pump_until(&mut app, |a| a.wasm.calls.is_empty());
        assert_eq!(app.extension_output_text().matches("you closed").count(), 1, "{}", app.extension_output_text());
    }

    /// A surface from the last session opened again in its page's place:
    /// its tab and pane, not a new one.
    #[test]
    fn a_surface_from_the_session_comes_back_in_its_place() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("")];
        let back = app.restore_code_page("test.hello", "screen", "Screen", "x", None, &[]).expect("its extension is on");
        app.show_page(back);
        pump_until(&mut app, |a| a.code_surface(back).is_some());
        assert_eq!(app.focused_buffer_id(), back, "where the page was");
        assert_eq!(app.find_page(|m| matches!(m, PageModel::Code(_))), Some(back), "no other page opened");
        assert!(said(&app, "restored"));
    }

    /// The PDF reader's service, as Fenix asks it: a file opened at a
    /// page, given to it to read.
    #[test]
    fn a_pdf_opens_at_a_page_through_its_extension() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("spec.pdf");
        std::fs::write(&file, b"%PDF-1.4").unwrap();
        let mut app = App::with_file(None);
        assert!(!app.open_with_extension_at(&file, Some(38), None), "nothing opens PDFs");
        app.wasm.installed = vec![hello("[contributes]\nopens = [\"*.pdf\"]\nnamespaces = [\"pdf\"]\nservices = [\"pdf.open\", \"pdf.search\"]\n")];
        assert!(app.open_with_extension_at(&file, Some(38), None));
        pump_until(&mut app, |a| said(a, "pdf.open at page 38, read Ok(8)"));
        // Without a page, it's opened as any file it opens.
        assert!(app.open_with_extension_at(&file, None, None));
        pump_until(&mut app, |a| said(a, "opened spec.pdf: Ok(8)"));
    }

    /// where the cursor is, with `here`; whether `here`'s text is a
    /// selection.
    #[test]
    fn the_cursor_is_told_with_here() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn a() {}\nlet x = 1;\n").unwrap();
        let mut app = App::with_file(Some(file.display().to_string()));
        let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
        cursor.char_idx = buffer.line_start_char(1) + 4;
        let (here, at) = app.editor_where();
        assert_eq!((here.line, here.text.as_str()), (2, "let x = 1;"));
        assert_eq!(at, Some(fenix_wasm::host::CursorInfo { at: (1, 4), selection: None, selected: false }));
        app.open_extension_output();
        assert_eq!(app.editor_where().1, None, "a page has no cursor of text");
    }

    /// a scratch buffer of its own: opened in the focused pane, its
    /// text set (not told back) and read; your edits told once they're
    /// still, and its closing.
    #[test]
    fn its_scratch_buffer_is_edited_and_its_changes_told() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("")];
        app.run_extension_command("test.hello", "hello.scratch");
        pump_until(&mut app, |a| said(a, "scratch text: Some(\"two\\n\")"));
        let buffer = app.focused_buffer_id();
        assert_eq!(app.buffer_display_name(buffer), "*Notes*");
        assert_eq!(app.open().buffer.text(), "two\n");
        let now = std::time::Instant::now();
        assert_eq!(app.code_buffers_changed(now), None, "its own change isn't told back");
        let (text, cursor) = app.focused_buffer_and_cursor_mut();
        text.insert_str(cursor, "and ");
        let wake = app.code_buffers_changed(now).expect("a change, waiting to be still");
        assert_eq!(wake, now + CHANGED_AFTER);
        assert_eq!(app.code_buffers_changed(now + CHANGED_AFTER), None);
        pump_until(&mut app, |a| said(a, "changed Changed::Scratch(1)"));
        assert!(!app.open().buffer.is_dirty(), "nothing to save");
        app.kill_buffer();
        app.code_buffers_changed(now + CHANGED_AFTER * 2);
        pump_until(&mut app, |a| said(a, "scratch 1 closed"));
        assert!(app.wasm.scratches.is_empty());
    }

    /// a file's buffer changed, told to an extension that asks
    /// (`changed`), once it's still; not to one that doesn't.
    #[test]
    fn a_files_change_is_told_to_one_that_asks() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.md");
        std::fs::write(&file, "one\n").unwrap();
        let mut app = App::with_file(Some(file.display().to_string()));
        app.wasm.installed = vec![hello("[permissions]\neditor = \"read\"\n\n[contributes]\nevents = [\"changed\"]\n")];
        app.start_code_extension("test.hello");
        pump_until(&mut app, |a| a.code_ready("test.hello"));
        let now = std::time::Instant::now();
        assert_eq!(app.code_buffers_changed(now), None);
        let (text, cursor) = app.focused_buffer_and_cursor_mut();
        text.insert_str(cursor, "x");
        assert!(app.code_buffers_changed(now + CHANGED_AFTER / 2).is_some());
        let (text, cursor) = app.focused_buffer_and_cursor_mut();
        text.insert_str(cursor, "y");
        assert!(app.code_buffers_changed(now + CHANGED_AFTER).is_some(), "typing again waits again");
        app.code_buffers_changed(now + CHANGED_AFTER * 3);
        let want = fenix_lsp::normalize(file.clone()).to_string_lossy().replace('\\', "\\\\");
        pump_until(&mut app, |a| said(a, &format!("changed Changed::File(\"{want}\")")));
        assert_eq!(app.extension_output_text().matches("changed Changed::File").count(), 1, "once");
    }

    /// what has the focus, told to one that asks (`focus`) whatever
    /// its permissions: a file (its path with `editor = "read"`), its own
    /// page, by kind and id.
    #[test]
    fn what_has_the_focus_is_told_to_one_that_asks() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.md");
        std::fs::write(&file, "one\n").unwrap();
        let mut app = App::with_file(Some(file.display().to_string()));
        app.wasm.installed = vec![hello("[contributes]\nevents = [\"focus\"]\n")];
        app.start_code_extension("test.hello");
        pump_until(&mut app, |a| said(a, "focus Focus::File(None)"));
        app.run_extension_command("test.hello", "hello.page");
        pump_until(&mut app, |a| said(a, "focus Focus::Page(FocusedPage { kind: \"counter\", mine: Some(1) })"));
        app.open_extension_output();
        pump_until(&mut app, |a| said(a, "focus Focus::Page(FocusedPage { kind: \"extensions-output\", mine: None })"));
        let with_path = hello("[permissions]\neditor = \"read\"\n");
        app.open_file_from_picker(&file);
        let Focus::File(Some(path)) = app.focus_for(&with_path) else { panic!("{:?}", app.focus_for(&with_path)) };
        assert!(path.ends_with("a.md"));
    }

    /// a file it opens, shown on its page: `SPC f o` goes to
    /// its text and back; `open-file` on such a file of its own opens its
    /// text, not back to it.
    #[test]
    fn a_file_it_opens_goes_between_its_page_and_its_text() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.view");
        std::fs::write(&file, "a view\n").unwrap();
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("[contributes]\nopens = [\"*.view\"]\n")];
        app.open_file_from_picker(&file);
        pump_until(&mut app, |a| a.find_page(|m| matches!(m, PageModel::Code(p) if p.kind == "viewer")).is_some());
        let page = app.find_page(|m| matches!(m, PageModel::Code(p) if p.kind == "viewer")).unwrap();
        assert_eq!(app.focused_buffer_id(), page);
        app.toggle_file_text();
        assert_eq!(app.open().buffer.text(), "a view\n", "its text");
        app.toggle_file_text();
        assert_eq!(app.focused_buffer_id(), page, "and back to the page");
        app.answer_code_request("test.hello", Request::OpenFile { path: file.clone(), line: None, sealed: false, text: None });
        assert_eq!(app.open().buffer.text(), "a view\n", "its own file: as text");
        assert_eq!(app.extension_output_text().matches("opened x.view").count(), 1, "not sent back to it");
    }

    /// what it was given is kept with its pages for the session,
    /// and given again when they come back -- a file outside the project.
    #[test]
    fn what_it_was_given_comes_back_with_its_pages() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.bin");
        std::fs::write(&file, [1u8, 2, 3, 4]).unwrap();
        let mut app = App::with_file(None);
        app.config = fenix_config::Config::load_or_default(dir.path().join("settings.toml"));
        let full = fenix_lsp::normalize(std::fs::canonicalize(&file).unwrap());
        app.wasm.installed = vec![hello(&format!("[contributes]\nopens = [\"*.bin\"]\n\n[[contributes.settings]]\nkey = \"hello.file\"\ndefault = {:?}\n", full.display().to_string()))];
        app.open_file_from_picker(&file);
        pump_until(&mut app, |a| said(a, "opened x.bin: Ok(4)"));
        app.run_extension_command("test.hello", "hello.page");
        pump_until(&mut app, |a| a.find_page(|m| matches!(m, PageModel::Code(_))).is_some());
        let buffer = app.find_page(|m| matches!(m, PageModel::Code(_))).unwrap();
        app.code_page_wants(buffer, 80, 24);
        pump_until(&mut app, |a| a.code_pages_to_save().len() == 1);
        let saved = app.code_pages_to_save().remove(0);
        assert_eq!(saved.given, std::slice::from_ref(&full));
        // A new Fenix: the page back, and the file given again with it.
        let mut app = App::with_file(None);
        app.config = fenix_config::Config::load_or_default(dir.path().join("settings.toml"));
        app.wasm.installed = vec![hello(&format!("[[contributes.settings]]\nkey = \"hello.file\"\ndefault = {:?}\n", full.display().to_string()))];
        app.restore_code_page(&saved.ext, &saved.kind, &saved.title, &saved.state, saved.file.clone(), &saved.given).unwrap();
        app.run_extension_command("test.hello", "hello.read");
        pump_until(&mut app, |a| said(a, "read 4 bytes"));
    }

    /// a sound of its own, played with `sound = true`.
    #[test]
    fn its_sound_is_played() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("[permissions]\nsound = true\n")];
        app.run_extension_command("test.hello", "hello.sound");
        pump_until(&mut app, |a| !a.wasm.played.is_empty());
        assert!(app.wasm.played[0].starts_with(b"RIFF"), "the WAV, read as it was checked");
    }

    /// a page laid out for its pane's height too, with what its
    /// canvas has besides -- backgrounds, underlines, a vertical rule,
    /// another focused row -- in cells, a wide character two; more keys
    /// and the mouse go to it.
    #[test]
    fn a_styled_page_is_drawn_in_cells_and_takes_more_keys_and_the_mouse() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("")];
        app.run_extension_command("test.hello", "hello.styled");
        pump_until(&mut app, |a| a.find_page(|m| matches!(m, PageModel::Code(p) if p.kind == "styled")).is_some());
        let buffer = app.find_page(|m| matches!(m, PageModel::Code(p) if p.kind == "styled")).unwrap();
        app.code_page_wants(buffer, 80, 33);
        pump_until(&mut app, |a| matches!(a.pages.get(&buffer).map(|s| &s.model), Some(PageModel::Code(p)) if p.shown.as_ref().is_some_and(|s| s.more_keys || s.mouse || !s.styles.is_empty())));
        let pane = app.focused_pane_id();
        app.ensure_page_layout(buffer, pane, 80);
        assert!(app.pages[&buffer].page.text.starts_with("日本 ok 80x33"), "laid out for its height: {}", app.pages[&buffer].page.text);
        let (segments, overlay) = app.page_backgrounds(buffer, 0, 10);
        // "日本 ok": characters 3..5, "ok", are cells 5..7.
        assert!(segments.contains(&(0, 5, 7, app.theme.selection)), "{segments:?}");
        assert!(segments.contains(&(1, 0, 3, [10.0 / 255.0, 20.0 / 255.0, 30.0 / 255.0, 1.0])), "{segments:?}");
        assert!(segments.contains(&(0, 0, 4, app.theme.hl_line)), "the other focused row, 日本: {segments:?}");
        assert_eq!(overlay.underlines.iter().map(|u| (u.0, u.1, u.2)).collect::<Vec<_>>(), [(0, 5, 7)]);
        assert_eq!(overlay.vertical_rules, [(9, 0, 2)]);
        assert_eq!(overlay.rail, Some((1, 0, 1)));
        // Ctrl+Z, which Fenix would take, is the page's.
        assert!(app.page_key(KeyPress::char('z').with_ctrl()));
        pump_until(&mut app, |a| said(a, "key press on") && said(a, "Char('z')"));
        // A cell's place under the pointer: the second half of 本 is it.
        let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 800.0, h: 600.0 };
        let cw = text::LINE_HEIGHT * 0.6;
        let at = |cell: f32, line: f32| (text::PAD_LEFT + (cell + 0.5) * cw, text::PAD_TOP + (line + 0.5) * text::LINE_HEIGHT);
        assert_eq!(app.code_page_cell(pane, rect, at(3.0, 0.0)), Some((buffer, 0, 1, 3)));
        assert_eq!(app.code_page_cell(pane, rect, at(5.0, 0.0)), Some((buffer, 0, 3, 5)));
        let mods = fenix_wasm::host::KeyMods { shift: false, ctrl: false, alt: false, meta: false };
        app.code_page_mouse(buffer, fenix_wasm::host::MouseEvent { line: 0, column: 3, cell: 5, action: fenix_wasm::host::MouseAction::Wheel(-1), mods });
        pump_until(&mut app, |a| said(a, "mouse on") && said(a, "Wheel(-1)"));
        // A page that doesn't ask has no cells under the pointer.
        app.run_extension_command("test.hello", "hello.page");
        pump_until(&mut app, |a| a.find_page(|m| matches!(m, PageModel::Code(p) if p.kind == "counter")).is_some());
        assert_eq!(app.code_page_cell(app.focused_pane_id(), rect, at(1.0, 0.0)), None);
    }

    /// its decorations of a file's buffer, and of its scratch
    /// buffer, kept by buffer and moved with your edits; set again, they
    /// take the place of the last; its own restart takes them away.
    #[test]
    fn its_decorations_are_kept_and_moved_with_the_text() {
        use fenix_wasm::host::{Decoration, DecorationLook, Role, VirtualText};
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "let x = 1;\n").unwrap();
        let mut app = App::with_file(Some(file.display().to_string()));
        let buffer = app.focused_buffer_id();
        let hint = |at: (u32, u32)| Decoration { start: at, end: at, look: DecorationLook::Text(VirtualText { text: ": i32".into(), role: Role::Muted, after: true }), id: None };
        app.answer_code_request("test.hello", Request::FileDecorations { path: file.clone(), open_only: false, decorations: vec![hint((0, 4))] });
        let places = |app: &App| app.code_decorations(buffer).iter().map(|d| (d.start, d.end)).collect::<Vec<_>>();
        assert_eq!(places(&app), [(5, 5)], "after the x");
        let (text, cursor) = app.focused_buffer_and_cursor_mut();
        text.insert_str(cursor, "  ");
        app.shift_code_decorations();
        assert_eq!(places(&app), [(7, 7)], "moved with the text");
        let ob = app.buffers.get(buffer).unwrap();
        let layout = crate::decorations::layout_line(&ob.buffer.text_range(0, 12), 0, &app.code_decorations(buffer), &TabStops::Fixed(4), false);
        assert_eq!(layout.display, "  let x: i32 = 1;");
        // Set again: in place of the last; empty, none.
        app.answer_code_request("test.hello", Request::FileDecorations { path: file.clone(), open_only: false, decorations: vec![hint((0, 0)), hint((0, 2))] });
        assert_eq!(places(&app).len(), 2);
        app.answer_code_request("other.ext", Request::FileDecorations { path: file.clone(), open_only: false, decorations: vec![hint((0, 6))] });
        assert_eq!(places(&app).len(), 3, "another's beside");
        app.answer_code_request("test.hello", Request::FileDecorations { path: file.clone(), open_only: false, decorations: Vec::new() });
        assert_eq!(places(&app), [(7, 7)]);
        app.answer_code_request("other.ext", Request::FileDecorations { path: dir.path().join("nope.rs"), open_only: false, decorations: vec![hint((0, 0))] });
        assert_eq!(places(&app).len(), 1, "a file not open: nothing");
        app.restart_code_extension("other.ext");
        assert!(app.code_decorations(buffer).is_empty(), "its restart takes them away");
    }

    /// an extension's page for a file and the file's text are
    /// one: its edits (`buffer-edit`) and yours go into one history, `SPC
    /// f o` shows the buffer it edited, the page hears the file change
    /// without asking, and its undo undoes yours and its own.
    #[test]
    fn a_files_page_and_its_text_are_one() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.view");
        std::fs::write(&file, "a view\n").unwrap();
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("[permissions]\neditor = \"write\"\n\n[contributes]\nopens = [\"*.view\"]\n")];
        app.open_file_from_picker(&file);
        pump_until(&mut app, |a| a.find_page(|m| matches!(m, PageModel::Code(p) if p.kind == "viewer")).is_some());
        let full = fenix_lsp::normalize(std::fs::canonicalize(&file).unwrap());
        app.answer_code_request("test.hello", Request::BufferEdit { path: full.clone(), open_only: false, edits: vec![fenix_wasm::host::TextEdit { start: (0, 0), end: (0, 0), text: "X".into() }], text: Some("a view\n".into()) });
        app.toggle_file_text();
        assert_eq!(app.open().buffer.text(), "Xa view\n", "the buffer it edited");
        let now = std::time::Instant::now();
        app.code_buffers_changed(now);
        let (text, cursor) = app.focused_buffer_and_cursor_mut();
        text.insert_str(cursor, "y");
        app.code_buffers_changed(now);
        app.code_buffers_changed(now + CHANGED_AFTER * 2);
        pump_until(&mut app, |a| said(a, "changed Changed::File(") && said(a, "x.view"));
        app.answer_code_request("test.hello", Request::BufferUndo { path: full.clone(), open_only: false, redo: false });
        assert_eq!(app.open().buffer.text(), "Xa view\n", "yours undone");
        app.answer_code_request("test.hello", Request::BufferUndo { path: full.clone(), open_only: false, redo: false });
        assert_eq!(app.open().buffer.text(), "a view\n", "and its own: one history");
        app.answer_code_request("test.hello", Request::BufferUndo { path: full, open_only: false, redo: true });
        assert_eq!(app.open().buffer.text(), "Xa view\n");
    }

    /// what has the focus says the explorer and the terminal panel
    /// too; and where you are is read in `focus-moved`, not only in a
    /// command.
    #[test]
    fn the_focus_and_where_you_are_are_told_as_they_move() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.md");
        std::fs::write(&file, "one\n").unwrap();
        let mut app = App::with_file(Some(file.display().to_string()));
        app.wasm.installed = vec![hello("[permissions]\neditor = \"read\"\n\n[contributes]\nevents = [\"focus\"]\n")];
        app.start_code_extension("test.hello");
        pump_until(&mut app, |a| said(a, "here at focus: Some(\"a.md\") Some((0, 0))"));
        let ext = app.wasm.installed[0].clone();
        app.sidebar_focused = true;
        assert_eq!(app.focus_for(&ext), Focus::None);
        app.code_focus_moved();
        pump_until(&mut app, |a| said(a, "focus Focus::None"));
        app.sidebar_focused = false;
        app.terminal_focused = true;
        assert_eq!(app.focus_for(&ext), Focus::Text(None));
        app.terminal_focused = false;
        assert!(matches!(app.focus_for(&ext), Focus::File(Some(_))));
    }

    /// the theme's palette, as the extensions read it.
    #[test]
    fn the_palette_is_read() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("")];
        app.run_extension_command("test.hello", "hello.palette");
        pump_until(&mut app, |a| said(a, "palette: 8"));
    }

    #[test]
    fn with_none_installed_the_output_page_says_how() {
        let app = App::with_file(None);
        assert!(app.extension_output_text().starts_with("No extension that runs code"));
    }

    /// The root a project's tools are looked up by for `file`, as the
    /// extension's project.
    fn pack_project(app: &mut App, file: &Path) -> PathBuf {
        let root = super::super::tool_sessions::root_for_path(file);
        app.wasm.projects.insert("test.hello".into(), Some(root.clone()));
        root
    }

    /// a language pack's debug adapter and formatter are its
    /// project's, the project's own `tools.json` still first; each is
    /// checked on the doctor's page, found or not, as its server is.
    #[test]
    fn a_packs_adapter_and_formatter_are_its_projects_and_the_doctor_checks_them() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.py");
        std::fs::write(&file, "x\n").unwrap();
        let mut app = App::with_file(None);
        let root = pack_project(&mut app, &file);
        let python = fenix_syntax::LanguageId::Python;
        app.answer_code_request("test.hello", Request::DebugAdapter { languages: vec!["python".into()], program: Some((("fenix-no-such-adapter".into(), vec!["--stdio".into()], None), vec![("ADAPTER_ENV".into(), "1".into())])) });
        app.answer_code_request("test.hello", Request::Formatter { languages: vec!["python".into()], program: Some(("sort".into(), Vec::new(), None)), keep_output: false });
        assert_eq!(app.wasm.debug_adapter(&root, python).and_then(|a| a.command).map(|s| (s.executable, s.args)), Some(("fenix-no-such-adapter".to_string(), vec!["--stdio".to_string()])));
        assert!(app.wasm.debug_adapter(&root, fenix_syntax::LanguageId::Rust).is_none() && app.wasm.formatter(&std::env::temp_dir().join("elsewhere"), python).is_none());
        // with the environment it gave.
        assert_eq!(app.wasm.debug_adapter(&root, python).and_then(|a| a.command).map(|s| s.env.get("ADAPTER_ENV").cloned()), Some(Some("1".to_string())));
        let tools = fenix_project::tools::ProjectTools::default();
        assert_eq!(app.debug_adapter_command(&root, python, &tools).and_then(|a| a.command).map(|s| s.executable), Some("fenix-no-such-adapter".to_string()), "before Fenix's own");
        let mut own = fenix_project::tools::ProjectTools::default();
        own.dap.insert("python".into(), fenix_project::tools::CommandSpec::new("mine".into(), Vec::new()));
        assert_eq!(app.debug_adapter_command(&root, python, &own).and_then(|a| a.command).map(|s| s.executable), Some("mine".to_string()), "the project's own first");
        let checks = app.wasm.doctor_checks(&root);
        let adapter = checks.iter().find(|c| c.label == "fenix-no-such-adapter").expect("its adapter checked");
        assert_eq!(adapter.health, fenix_project::doctor::Health::Warn);
        assert!(adapter.detail.starts_with("debug adapter for python · test.hello") && adapter.detail.ends_with("not found"), "{}", adapter.detail);
        let formatter = checks.iter().find(|c| c.label == "sort").expect("its formatter checked");
        assert_eq!(formatter.health, fenix_project::doctor::Health::Ok, "{}", formatter.detail);
        app.answer_code_request("test.hello", Request::DebugAdapter { languages: vec!["python".into()], program: None });
        assert!(app.wasm.debug_adapter(&root, python).is_none());
        assert_eq!(app.debug_adapter_command(&root, python, &tools).and_then(|a| a.command).map(|s| s.executable), Some("python".to_string()), "Fenix's own again");
    }

    /// an adapter over a socket is its project's as one over stdio
    /// is, each in place of the other; `${port}` is the port it's given.
    #[test]
    fn a_packs_adapter_over_a_socket_is_its_projects() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.py");
        std::fs::write(&file, "x\n").unwrap();
        let mut app = App::with_file(None);
        let root = pack_project(&mut app, &file);
        let python = fenix_syntax::LanguageId::Python;
        app.answer_code_request("test.hello", Request::DebugAdapterSocket { languages: vec!["python".into()], program: Some((("js-debug".into(), vec!["--port".into(), "${port}".into()], None), Vec::new())), port: 0 });
        let adapter = app.debug_adapter_command(&root, python, &fenix_project::tools::ProjectTools::default()).unwrap();
        assert_eq!(adapter.port, Some(0));
        assert_eq!(crate::dap::with_port(adapter.command.as_ref().unwrap(), 4711).args, ["--port", "4711"]);
        assert_eq!(adapter.describe(Some(4711)), "js-debug --port ${port} (127.0.0.1:4711)");
        app.answer_code_request("test.hello", Request::DebugAdapterSocket { languages: vec!["python".into()], program: None, port: 5678 });
        assert_eq!(app.wasm.debug_adapter(&root, python), Some(crate::dap::Adapter { command: None, port: Some(5678) }), "one already running");
        app.answer_code_request("test.hello", Request::DebugAdapter { languages: vec!["python".into()], program: None });
        assert!(app.wasm.debug_adapter(&root, python).is_none(), "debug-adapter-set takes it away");
    }

    /// its rules decorate a file of theirs before it's first drawn,
    /// again as it's edited, and not other files; taken away, or it
    /// restarting, they're gone.
    #[test]
    fn its_rules_decorate_a_file_from_the_start() {
        use fenix_wasm::host::{DecorationLook, DecorationRule};
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(".env.local");
        std::fs::write(&file, "KEY=s3cret\n# KEY=x\n").unwrap();
        let mut app = App::with_file(Some(file.display().to_string()));
        let buffer = app.focused_buffer_id();
        let rule = DecorationRule { files: vec![".env".into(), ".env.*".into()], pattern: "^[A-Z_]+=(.*)$".into(), group: 1, look: DecorationLook::Hide("••••".into()) };
        app.answer_code_request("test.hello", Request::DecorationRules(vec![rule]));
        app.apply_decoration_rules();
        let places = |app: &App| app.code_decorations(buffer).iter().map(|d| (d.start, d.end)).collect::<Vec<_>>();
        assert_eq!(places(&app), [(4, 10)], "the value, not the comment");
        let (text, cursor) = app.focused_buffer_and_cursor_mut();
        text.insert_str(cursor, "A=1\n");
        app.shift_code_decorations();
        app.apply_decoration_rules();
        assert_eq!(places(&app), [(2, 3), (8, 14)], "applied again");
        let other = dir.path().join("a.rs");
        std::fs::write(&other, "KEY=x\n").unwrap();
        app.open_file_as_text(&other);
        app.apply_decoration_rules();
        assert!(app.code_decorations(app.focused_buffer_id()).is_empty(), "not its files");
        app.answer_code_request("test.hello", Request::DecorationRules(Vec::new()));
        app.apply_decoration_rules();
        assert!(app.code_decorations(buffer).is_empty(), "taken away");
    }

    /// the file its page shows now, as it says.
    #[test]
    fn a_page_says_which_file_it_shows() {
        let dir = tempfile::tempdir().unwrap();
        let next = dir.path().join("b.png");
        std::fs::write(&next, "x").unwrap();
        let mut app = App::with_file(None);
        app.answer_code_request("test.hello", Request::PageOpen { page: 7, kind: "viewer".into(), title: "a.png".into() });
        app.answer_code_request("test.hello", Request::PageFile { page: 7, path: Some(next.clone()) });
        let full = fenix_lsp::normalize(std::fs::canonicalize(&next).unwrap());
        assert!(app.find_page(|m| matches!(m, PageModel::Code(p) if p.page == 7 && p.file.as_ref() == Some(&full))).is_some());
        app.answer_code_request("test.hello", Request::PageFile { page: 7, path: None });
        assert!(app.find_page(|m| matches!(m, PageModel::Code(p) if p.page == 7 && p.file.is_none())).is_some());
    }

    /// a task it offers keeps its environment and where it runs,
    /// as it gave them; the project's own of that name still wins.
    #[test]
    fn its_tasks_keep_their_environment() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_file(None);
        let root = pack_project(&mut app, &dir.path().join("a.py"));
        let task = ("py: test".to_string(), ("uv".to_string(), vec!["run".to_string(), "pytest".to_string()], Some(root.clone())), vec![("PYTHONPATH".to_string(), "src".to_string())]);
        app.answer_code_request("test.hello", Request::Tasks(vec![task]));
        assert_eq!(app.wasm.tasks(&root).iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["py: test"]);
        let spec = app.wasm.task_spec(&root, "py: test").expect("as it gave it");
        assert_eq!((spec.executable.as_str(), spec.env.get("PYTHONPATH").map(String::as_str), spec.cwd), ("uv", Some("src"), Some(root.clone())));
        assert!(app.wasm.task_spec(&root, "other").is_none());
    }

    /// its formatter formats the buffer (`SPC c F`) as one change,
    /// and, with `editor.format_on_save`, what's saved.
    #[test]
    fn its_formatter_formats_the_buffer_and_what_is_saved() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.py");
        std::fs::write(&file, "pear\napple\n").unwrap();
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        app.config = fenix_config::Config::load_or_default(dir.path().join("settings.toml"));
        assert!(!app.format_with_extension(), "none yet");
        pack_project(&mut app, &file);
        app.answer_code_request("test.hello", Request::Formatter { languages: vec!["python".into()], program: Some(("sort".into(), Vec::new(), None)), keep_output: false });
        assert!(app.format_with_extension());
        assert_eq!(app.open().buffer.text().replace('\r', ""), "apple\npear\n");
        let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
        buffer.undo(cursor);
        assert_eq!(app.open().buffer.text(), "pear\napple\n", "one change, undone at once");
        app.save();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "pear\napple\n", "not on save, unless asked");
        app.config.format_on_save = Some(true);
        let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
        buffer.replace_range(cursor, 0, 0, "fig\n");
        app.save();
        assert_eq!(std::fs::read_to_string(&file).unwrap().replace('\r', ""), "apple\nfig\npear\n");
        // One that fails says so, and the text is kept and saved.
        app.answer_code_request("test.hello", Request::Formatter { languages: vec!["python".into()], program: Some(("fenix-no-such-formatter".into(), Vec::new(), None)), keep_output: false });
        let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
        buffer.replace_range(cursor, 0, 0, "kiwi\n");
        app.save();
        assert!(std::fs::read_to_string(&file).unwrap().starts_with("kiwi\n"));
        assert!(app.status_message.as_ref().is_some_and(|m| m.is_error && m.text.contains("unformatted -- fenix-no-such-formatter")), "{:?}", app.status_message.as_ref().map(|m| &m.text));
    }

    /// its problems in a file are shown with the language servers',
    /// columns counted as they count them; they go when it stops.
    #[test]
    fn its_problems_are_shown_with_the_servers_and_go_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.py");
        std::fs::write(&file, "\u{1F600} = x\n").unwrap();
        let mut app = App::with_file(None);
        pack_project(&mut app, &file);
        let problem = fenix_wasm::host::Problem { start: (0, 4), end: (0, 5), severity: fenix_wasm::host::Severity::Warning, message: "x is unknown".into(), source: None, code: Some("E1".into()), fixes: Vec::new() };
        app.answer_code_request("test.hello", Request::Diagnostics { path: file.clone(), open_only: false, problems: vec![problem] });
        let canonical = fenix_lsp::normalize(std::fs::canonicalize(&file).unwrap());
        let shown = app.all_diagnostics(&canonical);
        assert_eq!(shown.len(), 1);
        assert_eq!((shown[0].range.start.character, shown[0].severity), (5, Some(lsp_types::DiagnosticSeverity::WARNING)), "the emoji is two UTF-16 units");
        assert_eq!(shown[0].source.as_deref(), Some("test.hello"));
        // Open only: a file you have open, not another.
        app.answer_code_request("test.hello", Request::Diagnostics { path: dir.path().join("b.py"), open_only: true, problems: Vec::new() });
        assert!(app.wasm.output.get("test.hello").is_some_and(|o| o.iter().any(|l| l.contains("diagnostics-set") && l.contains("isn't open"))));
        app.forget_code_lsp_sessions("test.hello");
        assert!(app.all_diagnostics(&canonical).is_empty());
    }

    /// a scratch buffer and text opened beside share one pane, the
    /// focus staying; once that pane is closed, a new one is split off.
    #[test]
    fn scratch_buffers_and_text_beside_share_one_pane() {
        let mut app = App::with_file(None);
        let (panes, focused) = (app.windows().windows().len(), app.focused_pane_id());
        app.answer_code_request("test.hello", Request::ScratchOpen { id: 1, title: "Result".into(), text: "{}".into(), language: Some("json".into()), beside: Some(0.4) });
        assert_eq!((app.windows().windows().len(), app.focused_pane_id()), (panes + 1, focused));
        let beside = app.wasm.beside["test.hello"];
        app.answer_code_request("test.hello", Request::OpenText { title: "Raw".into(), text: "raw".into(), language: None, beside: Some(0.4) });
        assert_eq!((app.windows().windows().len(), app.focused_pane_id()), (panes + 1, focused), "the same pane");
        let shown = app.windows().content(beside).copied().unwrap();
        assert_eq!(app.buffers.get(shown).map(|ob| ob.buffer.text()), Some("raw".to_string()));
    }


    /// a buffer highlighted as the language it's given, then as its
    /// name says again; a name there's no grammar for is said.
    #[test]
    fn a_buffer_is_given_a_language() {
        let mut app = App::with_file(None);
        app.answer_code_request("test.hello", Request::ScratchOpen { id: 1, title: "Data".into(), text: "{}".into(), language: None, beside: None });
        let buffer = app.focused_buffer_id();
        assert!(app.buffers.get(buffer).unwrap().syntax.is_none());
        app.answer_code_request("test.hello", Request::ScratchLanguage { id: 1, language: Some("json".into()) });
        assert_eq!(app.buffers.get(buffer).unwrap().syntax.as_ref().map(|s| s.language()), Some(fenix_syntax::LanguageId::Json));
        app.answer_code_request("test.hello", Request::ScratchLanguage { id: 1, language: Some("cobol".into()) });
        assert!(app.wasm.output["test.hello"].iter().any(|l| l.contains("no language \"cobol\" here")), "{:?}", app.wasm.output);
        assert_eq!(app.buffers.get(buffer).unwrap().syntax.as_ref().map(|s| s.language()), Some(fenix_syntax::LanguageId::Json), "kept");
        app.answer_code_request("test.hello", Request::ScratchLanguage { id: 1, language: None });
        assert!(app.buffers.get(buffer).unwrap().syntax.is_none(), "as its name says: none");
        // A file's, by its path.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, "x = 1\n").unwrap();
        let mut app = App::with_file(Some(file.display().to_string()));
        app.answer_code_request("test.hello", Request::FileLanguage { path: file.clone(), open_only: false, language: Some("toml".into()) });
        assert_eq!(app.open().syntax.as_ref().map(|s| s.language()), Some(fenix_syntax::LanguageId::Toml));
        assert!(app.editor_status_details().contains("Toml"), "{}", app.editor_status_details());
    }

    /// a click on a tagged decoration -- a swatch -- is its
    /// extension's, by id; a click beside it isn't.
    #[test]
    fn a_click_on_a_tagged_decoration_is_told_to_its_extension() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("")];
        app.run_extension_command("test.hello", "hello.greet");
        pump_until(&mut app, |a| said(a, "hello, Fenix"));
        app.answer_code_request("test.hello", Request::ScratchOpen { id: 1, title: "Swatches".into(), text: "#ff0000\n".into(), language: None, beside: None });
        let swatch = fenix_wasm::host::Decoration { start: (0, 0), end: (0, 0), look: fenix_wasm::host::DecorationLook::Swatch((255, 0, 0)), id: Some(5) };
        app.answer_code_request("test.hello", Request::ScratchDecorations { id: 1, decorations: vec![swatch] });
        let window = app.focused_pane_id();
        let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 400.0, h: 400.0 };
        app.wasm.layers.geometry = super::super::layers::Geometry {
            cell: (10.0, 20.0),
            window: (400.0, 400.0),
            panes: vec![super::super::layers::GeoPane { id: window.number() as u64, rect, text_x: 0.0, text_y: 0.0, base_line: 0, scroll_col: 0, rows: 20, focused: true }],
            ..Default::default()
        };
        assert!(!app.decoration_click((45.0, 5.0)), "on the text beside it");
        assert!(!app.decoration_click((5.0, 25.0)), "on the next line");
        assert!(app.decoration_click((15.0, 5.0)), "on the swatch");
        pump_until(&mut app, |a| said(a, "clicked 5 in TextBuffer::Scratch(1)"));
    }

    /// the lines a pane shows, once they've been still a moment,
    /// told to an extension that asks (`view`), of its scratch buffer.
    #[test]
    fn the_lines_a_pane_shows_are_told_once_still() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("[contributes]\nevents = [\"view\"]\n")];
        app.run_extension_command("test.hello", "hello.greet");
        pump_until(&mut app, |a| said(a, "hello, Fenix"));
        app.answer_code_request("test.hello", Request::ScratchOpen { id: 1, title: "Long".into(), text: "a\nb\nc\nd\n".into(), language: None, beside: None });
        let buffer = app.focused_buffer_id();
        app.wasm.views_drawn = vec![(buffer, 7, 0, 3)];
        let now = std::time::Instant::now();
        assert_eq!(app.code_views_changed(now), Some(now + VIEW_AFTER), "waiting to be still");
        assert_eq!(app.code_views_changed(now + VIEW_AFTER), None);
        pump_until(&mut app, |a| said(a, "view TextBuffer::Scratch(1) 0..=3"));
    }

    /// tasks beside the task pane: each one's output in a buffer of
    /// its own, its end told; brought up by id.
    #[cfg(unix)]
    #[test]
    fn a_background_task_runs_into_a_buffer_of_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_file(None);
        app.answer_code_request("test.hello", Request::BackgroundTask { id: 1, name: "lint".into(), program: "sh".into(), args: vec!["-c".into(), "echo found it".into()], cwd: Some(dir.path().to_path_buf()), env: Vec::new() });
        let (&buffer, task) = app.wasm.background.iter().next().expect("kept");
        assert_eq!((task.ext.as_str(), task.id), ("test.hello", 1));
        let text = app.buffers.get(buffer).unwrap().buffer.text();
        assert!(text.contains("found it") && text.contains("-- done --"), "{text}");
        assert!(app.task_session.is_none(), "not the task pane");
        app.answer_code_request("test.hello", Request::TaskShow(1));
        assert_eq!(app.focused_buffer_id(), buffer);
        assert_eq!(app.buffer_display_name(buffer), "*lint*");
    }

    /// a language server added beside the others, by name, kept for
    /// its project; taken away again.
    #[test]
    fn a_language_server_is_added_beside_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let mut app = App::with_file(None);
        app.wasm.projects.insert("test.hello".into(), Some(root.clone()));
        let program = Some((("ruff".to_string(), vec!["server".to_string()], None), vec![("RUFF".to_string(), "1".to_string())]));
        app.answer_code_request("test.hello", Request::LanguageServerAdd { name: "ruff".into(), languages: vec!["python".into()], program });
        let added = app.wasm.added_servers(&root);
        assert_eq!(added.len(), 1);
        assert_eq!((added[0].name.as_str(), added[0].executable.as_str(), added[0].env.get("RUFF").map(String::as_str)), ("test.hello:ruff", "ruff", Some("1")));
        let servers = app.additional_lsp_servers(&root, fenix_syntax::LanguageId::Python);
        assert!(servers.iter().any(|s| s.name == "test.hello:ruff"), "{servers:?}");
        assert!(app.additional_lsp_servers(&root, fenix_syntax::LanguageId::Rust).is_empty());
        app.answer_code_request("test.hello", Request::LanguageServerAdd { name: "ruff".into(), languages: Vec::new(), program: None });
        assert!(app.wasm.added_servers(&root).is_empty());
        assert!(app.additional_lsp_servers(&root, fenix_syntax::LanguageId::Python).is_empty(), "read again without it");
    }

    /// a file an extension opens is opened from what was read as it
    /// was checked, not read again.
    #[test]
    fn a_file_it_opens_is_opened_as_it_was_read() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "on disk now\n").unwrap();
        let mut app = App::with_file(None);
        app.answer_code_request("test.hello", Request::OpenFile { path: file.clone(), line: None, sealed: false, text: Some("as checked\n".into()) });
        assert_eq!(app.open().buffer.text(), "as checked\n");
        assert_eq!(app.open().buffer.path(), Some(file.as_path()));
        // And one opened for an edit.
        let other = dir.path().join("b.txt");
        std::fs::write(&other, "on disk\n").unwrap();
        app.answer_code_request("test.hello", Request::BufferEdit { path: other.clone(), open_only: false, edits: Vec::new(), text: Some("checked\n".into()) });
        let id = app.buffers.id_for_path(&other).expect("opened");
        assert_eq!(app.buffers.get(id).unwrap().buffer.text(), "checked\n");
    }

    /// A language server session for tests whose messages to the server
    /// can be read back: what it was sent, a message at a time.
    fn reading_server(app: &mut App, key: super::super::LspKey, caps: serde_json::Value) -> std::io::BufReader<std::io::PipeReader> {
        let (from_fenix, to_server) = std::io::pipe().unwrap();
        let (from_server, _keep) = std::io::pipe().unwrap();
        std::mem::forget(_keep);
        let (client, _) = fenix_lsp::LspClient::connect(from_server, to_server);
        app.lsp_sessions.insert(key.clone(), super::super::LspSession {
            generation: 1, client, name: "server".into(), languages: vec!["javascript".into()], root: key.root.clone(), reader: None,
            capabilities: Some(serde_json::from_value(caps).unwrap()), open_documents: HashMap::new(), synced_text: HashMap::new(), pending: HashMap::new(), python_env: None, pulls: HashMap::new(),
        });
        std::io::BufReader::new(from_fenix)
    }

    fn sent(reading: &mut std::io::BufReader<std::io::PipeReader>) -> serde_json::Value {
        serde_json::from_str(&super::super::read_lsp_frame(reading).expect("a message")).unwrap()
    }

    /// a server's settings, asked for (`workspace/configuration`):
    /// each section from the project's `.fenix/lsp/<name>.json` with what
    /// an extension gave over it, null where there's none; and told again
    /// when they change.
    #[test]
    fn a_servers_settings_come_from_the_project_and_its_extensions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".fenix/lsp")).unwrap();
        std::fs::write(dir.path().join(".fenix/project.ini"), "").unwrap();
        std::fs::write(dir.path().join(".fenix/lsp/eslint.json"), r#"{"validate": "on", "run": "onSave"}"#).unwrap();
        let file = dir.path().join("a.js");
        std::fs::write(&file, "var x = 1;\n").unwrap();
        let mut app = App::with_file(None);
        let root = pack_project(&mut app, &file);
        app.wasm.added_servers.insert(("test.hello".into(), root.clone()), vec![fenix_project::tools::AdditionalServer { name: super::super::code_server_name("test.hello", "eslint"), languages: vec!["javascript".into()], executable: "x".into(), ..Default::default() }]);
        let key = super::super::LspKey { server: super::super::LspServer::Additional(super::super::code_server_name("test.hello", "eslint")), root: root.clone() };
        let mut reading = reading_server(&mut app, key.clone(), serde_json::json!({}));
        let ask = |app: &mut App, params: serde_json::Value| {
            app.apply_lsp_event(key.clone(), 1, fenix_lsp::LspEvent::ServerRequest { id: serde_json::json!(5), method: "workspace/configuration".into(), params });
        };
        ask(&mut app, serde_json::json!({"items": [{"section": ""}, {"section": "editor"}]}));
        assert_eq!(sent(&mut reading)["result"], serde_json::json!([{"validate": "on", "run": "onSave"}, null]), "the project's own");
        app.answer_code_request("test.hello", Request::LanguageServerSettings { name: "eslint".into(), settings: Some(serde_json::json!({"run": "onType", "nodePath": null})) });
        let told = sent(&mut reading);
        assert_eq!(told["method"], "workspace/didChangeConfiguration", "a running server is told");
        assert_eq!(told["params"]["settings"]["run"], "onType");
        ask(&mut app, serde_json::json!({"items": [{"section": "run"}, {"section": "validate"}]}));
        assert_eq!(sent(&mut reading)["result"], serde_json::json!(["onType", "on"]), "the extension's over the project's");
        // Not another extension's server, nor no language at all.
        app.answer_code_request("test.hello", Request::LanguageServerSettings { name: "nobodys".into(), settings: Some(serde_json::json!({})) });
        assert!(app.wasm.output["test.hello"].iter().any(|l| l.contains("no server it added, nor a language")));
        app.answer_code_request("test.hello", Request::LanguageServerSettings { name: "eslint".into(), settings: None });
        assert_eq!(sent(&mut reading)["params"]["settings"]["run"], "onSave", "taken away: the project's again");
    }

    /// a server that answers when asked for its diagnostics
    /// (`diagnosticProvider`, ESLint's way) is asked as a document opens
    /// and changes, one ask out at a time; what it says sits beside what it
    /// pushed, and a refresh asks again.
    #[test]
    fn diagnostics_are_asked_of_a_server_that_answers_that_way() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        let file = dir.path().join("a.js");
        std::fs::write(&file, "var x = 1;\n").unwrap();
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        let canonical = fenix_lsp::normalize(std::fs::canonicalize(&file).unwrap());
        let key = super::super::LspKey { server: super::super::LspServer::Additional("eslint".into()), root: super::super::tool_sessions::lsp_root_for_path(&file) };
        app.lsp_additional.entry(key.root.clone()).or_default().push(fenix_project::tools::AdditionalServer { name: "eslint".into(), languages: vec!["javascript".into()], executable: "x".into(), ..Default::default() });
        let mut reading = reading_server(&mut app, key.clone(), serde_json::json!({"textDocumentSync": 2, "diagnosticProvider": {"interFileDependencies": false, "workspaceDiagnostics": false}}));
        app.sync_lsp_for_focused_buffer();
        assert_eq!(sent(&mut reading)["method"], "textDocument/didOpen");
        let asked = sent(&mut reading);
        assert_eq!(asked["method"], "textDocument/diagnostic");
        let id = asked["id"].as_i64().unwrap();
        // Edited while it's out: asked again once it's answered.
        app.test_insert_str("let y;\n");
        app.sync_lsp_for_focused_buffer();
        assert_eq!(sent(&mut reading)["method"], "textDocument/didChange");
        let push = lsp_types::Diagnostic { message: "pushed".into(), ..Default::default() };
        app.apply_lsp_event(key.clone(), 1, fenix_lsp::LspEvent::Notification { method: "textDocument/publishDiagnostics".into(), params: serde_json::json!({"uri": fenix_lsp::path_to_uri(&canonical).unwrap(), "diagnostics": [push]}) });
        let report = serde_json::json!({"kind": "full", "resultId": "1", "items": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 3}}, "message": "Unexpected var", "source": "eslint"}]});
        app.apply_lsp_event(key.clone(), 1, fenix_lsp::LspEvent::Response { id, result: Ok(report) });
        let shown: Vec<String> = app.all_diagnostics(&canonical).into_iter().map(|d| d.message).collect();
        assert_eq!(shown, ["pushed", "Unexpected var"]);
        let again = sent(&mut reading);
        assert_eq!((again["method"].as_str(), again["params"]["previousResultId"].as_str()), (Some("textDocument/diagnostic"), Some("1")), "asked again, with its last result");
        let id = again["id"].as_i64().unwrap();
        app.apply_lsp_event(key.clone(), 1, fenix_lsp::LspEvent::Response { id, result: Ok(serde_json::json!({"kind": "unchanged", "resultId": "1"})) });
        assert_eq!(app.all_diagnostics(&canonical).len(), 2, "unchanged keeps what it said");
        app.apply_lsp_event(key.clone(), 1, fenix_lsp::LspEvent::ServerRequest { id: serde_json::json!(9), method: "workspace/diagnostic/refresh".into(), params: serde_json::Value::Null });
        assert_eq!(sent(&mut reading)["id"], 9, "the refresh answered");
        assert_eq!(sent(&mut reading)["method"], "textDocument/diagnostic", "and asked again");
    }

    /// the fixes an extension's problems come with are code actions
    /// on their lines, while the file is as it was; `language-server-set`
    /// keeps the server's environment and where it runs.
    #[test]
    fn an_extensions_fixes_are_code_actions_while_the_file_is_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.py");
        std::fs::write(&file, "import os\nx = 1\n").unwrap();
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        let root = pack_project(&mut app, &file);
        let canonical = fenix_lsp::normalize(std::fs::canonicalize(&file).unwrap());
        let fix = fenix_wasm::host::QuickFix { title: "Remove the import".into(), edits: vec![fenix_wasm::host::TextEdit { start: (0, 0), end: (1, 0), text: String::new() }] };
        let problem = fenix_wasm::host::Problem { start: (0, 0), end: (0, 9), severity: fenix_wasm::host::Severity::Warning, message: "unused import".into(), source: Some("ruff".into()), code: Some("F401".into()), fixes: vec![fix] };
        app.answer_code_request("test.hello", Request::Diagnostics { path: file.clone(), open_only: false, problems: vec![problem] });
        let actions = app.code_fixes_on(&canonical, 0);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].1.title, "Remove the import");
        assert!(app.code_fixes_on(&canonical, 1).is_empty(), "only on its lines");
        // `SPC c a`, with no server here: the one fix, previewed.
        app.request_code_action();
        assert!(app.refactor_preview.is_some(), "{:?}", app.status_message.as_ref().map(|m| m.text.clone()));
        app.commit_refactor();
        assert_eq!(app.buffers.get(app.buffers.id_for_path(&file).unwrap()).unwrap().buffer.text(), "x = 1\n");
        assert!(app.code_fixes_on(&canonical, 0).is_empty(), "the file changed: it no longer fits");
        // language-server-set: where it said, with its environment.
        app.answer_code_request("test.hello", Request::LanguageServer { languages: vec!["python".into()], program: Some((("srv".into(), vec!["--stdio".into()], Some(root.join("tools"))), vec![("SRV".into(), "1".into())])) });
        let spec = app.wasm.language_server(&root, fenix_syntax::LanguageId::Python).unwrap();
        assert_eq!((spec.cwd, spec.env.get("SRV").map(String::as_str)), (Some(root.join("tools")), Some("1")));
    }

    /// launches an extension gives as it runs are offered by `SPC u
    /// u`; a variable it fills in with one choice is filled, with several
    /// picked; `${pickProcess}` is this machine's processes, its id sent as
    /// a number.
    #[test]
    fn launches_and_their_variables_from_an_extension_are_filled_in() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("CMakeLists.txt"), "").unwrap();
        let file = dir.path().join("main.cpp");
        std::fs::write(&file, "int main() {}\n").unwrap();
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        let root = pack_project(&mut app, &file);
        let me = std::env::current_exe().unwrap();
        let adapter = fenix_project::tools::CommandSpec { env: [("FENIX_LSP_SINK".to_string(), "1".to_string())].into(), ..fenix_project::tools::CommandSpec::new(me.to_string_lossy().into_owned(), vec!["--ignored".into(), "--exact".into(), "app::tests::lsp_sink_fixture".into()]) };
        app.wasm.adapters.insert(("test.hello".into(), root.clone()), (vec!["cpp".into()], adapter));
        let launch = |name: &str, request, program: Option<&str>, attributes: &str| fenix_wasm::host::LaunchConfig {
            name: name.into(), languages: vec!["cpp".into()], request, program: program.map(str::to_string), args: vec!["--target=${target}".into()], cwd: None, env: Vec::new(), attributes: attributes.into(), pre_launch_task: None,
        };
        app.answer_code_request("test.hello", Request::Launches(vec![
            launch("cmake: app", fenix_wasm::host::LaunchRequest::Launch, Some("${project}/build/${target}"), r#"{"type": "lldb", "stopOnEntry": false}"#),
            launch("attach", fenix_wasm::host::LaunchRequest::Attach, None, r#"{"type": "lldb", "pid": "${pickProcess}"}"#),
        ]));
        let item = |v: &str| fenix_wasm::host::PickerItem { label: v.into(), detail: String::new(), value: v.into() };
        app.answer_code_request("test.hello", Request::LaunchVariable { name: "target".into(), choices: vec![item("app")] });
        app.debug_start_or_continue();
        let Some(ActivePicker::Launch(picker)) = &app.active_picker else { panic!("its launches offered") };
        assert_eq!(picker.len(), 2);
        app.active_picker = None;
        let templates = app.wasm.launches(&root);
        app.debug_start_from(templates.iter().find(|t| t.name == "cmake: app").unwrap().clone());
        let session = app.debug_session.as_ref().expect("started");
        assert_eq!(session.launch_attributes["program"], serde_json::json!(format!("{}/build/app", root.display())), "{:?}", session.launch_attributes);
        assert_eq!(session.launch_attributes["args"], serde_json::json!(["--target=app"]));
        assert_eq!(session.launch_attributes["stopOnEntry"], false);
        app.debug_stop();
        // Several choices: picked, each candidate the launch filled in.
        app.answer_code_request("test.hello", Request::LaunchVariable { name: "target".into(), choices: vec![item("app"), item("tests")] });
        app.open_buffer_in_focused_pane(app.buffers.id_for_path(&file).unwrap());
        app.debug_start_from(templates.iter().find(|t| t.name == "cmake: app").unwrap().clone());
        let Some(ActivePicker::Launch(picker)) = &app.active_picker else { panic!("a picker of its choices") };
        assert_eq!(picker.len(), 2);
        let tests = picker.visible_rows(0, 2).map(|(_, c)| c.payload.clone()).find(|t| t.filled.get("target").is_some_and(|v| v == "tests")).unwrap();
        app.active_picker = None;
        app.debug_start_from(tests);
        assert_eq!(app.debug_session.as_ref().unwrap().launch_attributes["program"], serde_json::json!(format!("{}/build/tests", root.display())));
        app.debug_stop();
        // ${pickProcess}: this machine's processes.
        app.open_buffer_in_focused_pane(app.buffers.id_for_path(&file).unwrap());
        let attach = templates.iter().find(|t| t.name == "attach").unwrap().clone();
        app.debug_start_from(attach.clone());
        let Some(ActivePicker::Launch(picker)) = &app.active_picker else { panic!("a picker of processes") };
        assert!(picker.len() > 1);
        app.active_picker = None;
        let mut picked = attach;
        picked.filled.insert("pickProcess".into(), "4242".into());
        picked.filled.insert("target".into(), "app".into());
        app.debug_start_from(picked);
        let session = app.debug_session.as_ref().unwrap();
        assert_eq!(session.launch_attributes["pid"], 4242, "a number");
        assert!(session.attach);
        // The project's last launch is started again, no picker; SPC u U
        // offers them all.
        app.debug_stop();
        app.answer_code_request("test.hello", Request::LaunchVariable { name: "target".into(), choices: vec![item("app")] });
        app.open_buffer_in_focused_pane(app.buffers.id_for_path(&file).unwrap());
        app.debug_start_from(templates.iter().find(|t| t.name == "cmake: app").unwrap().clone());
        app.debug_stop();
        app.open_buffer_in_focused_pane(app.buffers.id_for_path(&file).unwrap());
        app.debug_start_or_continue();
        assert!(app.active_picker.is_none(), "the last launch, again");
        assert_eq!(app.debug_session.as_ref().unwrap().launch_attributes["program"], serde_json::json!(format!("{}/build/app", root.display())));
        app.debug_stop();
        app.open_buffer_in_focused_pane(app.buffers.id_for_path(&file).unwrap());
        app.debug_pick_launch();
        assert!(matches!(&app.active_picker, Some(ActivePicker::Launch(p)) if p.len() == 2), "SPC u U asks");
    }

    /// a task started in the task pane, and what became of a debug
    /// session, told to an extension that asks (`tasks`, `debug`).
    #[test]
    fn tasks_started_and_debug_sessions_are_told() {
        let mut app = App::with_file(None);
        app.wasm.installed = vec![hello("[contributes]\nevents = [\"tasks\", \"debug\"]\n")];
        app.run_extension_command("test.hello", "hello.greet");
        pump_until(&mut app, |a| said(a, "hello, Fenix"));
        let dir = tempfile::tempdir().unwrap();
        let (command, args) = if cfg!(windows) { ("cmd".to_string(), vec!["/c".to_string(), "echo".to_string(), "x".to_string()]) } else { ("echo".to_string(), vec!["x".to_string()]) };
        app.run_task(fenix_tasks::TaskDef { name: "build".into(), command, args }, dir.path().to_path_buf());
        pump_until(&mut app, |a| said(a, "task started: build"));
        app.tell_debug(fenix_wasm::host::DebugState::Started(fenix_wasm::host::DebugSession { id: 5, name: "app.js".into(), language: "javascript".into(), parent: None }));
        app.tell_debug(fenix_wasm::host::DebugState::Ended(5));
        pump_until(&mut app, |a| said(a, "debug started: 5 app.js javascript None") && said(a, "debug ended: 5"));
    }
}
