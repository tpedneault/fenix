//! The MIB, as a `Feature`: which MIBs the focused project uses, reading
//! them off the UI thread and again when their files change, the
//! `SPC k` commands, what the MIB pages' keys ask for (`mib_page`,
//! `mib_def`, `mib_form`), and the MIB in the editor -- `K`, `gd`,
//! completion and script checks on names from it.
//!
//! What it keeps between calls lives on `MibHost`. The lookups that only
//! read settings are free functions over `&App`. The MIB pages are still
//! core `PageModel` variants, and the CCSDS and stream pages, notebook
//! links and the disk poll reach `MibHost` by type: those wait for a page
//! contribution point and services.

use std::any::Any;
use std::sync::Arc;

use fenix_mib::{DefRef, Kind, MibRoot, MibSet};

use super::feature::{page_as, Feature, FeatureCommand, FeaturePage, FeaturePick, FeatureSpec, LeaderEntry, Notice};
use super::pages::{PageEvent, PageModel};
use super::*;
use crate::mib_def::{self, DefPage};
use crate::mib_form::{self, InsertForm};
use crate::mib_page::{self, MibKey, MibPage};

fn on(app: &mut App, f: fn(&mut MibHost, &mut App)) {
    app.with_feature::<MibHost, _>(f);
}

/// The MIB's commands and its entries in the Tcl `SPC m` menu. Its
/// `SPC k` keys stay in the keymap for now: that group is the mission
/// pack's, shared with CCSDS.
/// The ICD issues `mib.icd` offers.
const MIB_ICDS: &[&str] = &["7.x", "6.9", "6.x", "5.x", "4.x", "auto"];
/// The packet identification fields `mib.xtce_fields` names.
const XTCE_FIELDS: &[&str] = &["apid", "service", "subtype", "pi1", "pi2"];

/// An issue as `mib.icd_per_mib` takes one: auto, or 4 to 7 with any
/// minor part (`7.2`, `6.x`).
fn icd_issue(text: &str) -> bool {
    let t = text.trim();
    let (major, minor) = t.split_once('.').unwrap_or((t, "0"));
    t == "auto" || (matches!(major, "4" | "5" | "6" | "7") && (minor == "x" || minor.parse::<u32>().is_ok()))
}

/// A setting's entries, for its check.
fn entries(v: &fenix_config::Value) -> &[(String, String)] {
    match v {
        fenix_config::Value::Map(m) => m,
        _ => &[],
    }
}

/// MIB's settings: `[mib]` in `settings.toml`, most of them per project
/// too (`.fenix/settings.toml`).
static SETTINGS: [fenix_config::Setting; 15] = {
    use fenix_config::{Category::Mib, Kind, Setting};
    [
        Setting::stored("mib.roots", Mib, "MIBs", Kind::Map { key: "Name", value: "Folder", paths: true }, "The MIB folders: their .dat tables. A project's own list takes the place of yours in that project; a folder in a project can be relative to it.").project(),
        Setting::stored("mib.default", Mib, "Default MIB", Kind::Text, "The MIB whose definition is used when several define the same name.").with_default("the first").project(),
        Setting::stored("mib.include_yours", Mib, "Add your MIBs", Kind::Bool, "In a project with MIBs of its own, use yours too.").with_default("off").project(),
        Setting::stored("mib.telecommand_template", Mib, "Telecommand template", Kind::Text, "How an inserted telecommand is written: {mnemo}, {type}, {stype}, {apid}, {description}, {mib} and {arguments} are filled in.").with_default("telecommand_send PUS_T={type} PUS_ST={stype} APID={apid} MNEMO={mnemo} ARGUMENTS=[{arguments}]").project(),
        Setting::stored("mib.telecommand_argument_template", Mib, "Argument template", Kind::Text, "How each argument is written: {name} and {value} are filled in.").with_default("{name}={value}").project(),
        Setting::stored("mib.telecommand_argument_separator", Mib, "Argument separator", Kind::Text, "What goes between arguments.").with_default("\", \"").project(),
        Setting::stored("mib.editor_files", Mib, "Files that know the MIB", Kind::List, "Extensions of the files where K, gd and completion know MIB names (tcl, py). Empty: every file.").with_default("every file").project(),
        Setting::stored("mib.icd", Mib, "ICD issue", Kind::Choice(MIB_ICDS), "The SCOS-2000 Database Import ICD issue the MIBs' .dat tables follow: which tables there are and their columns. Auto works it out from each MIB's files.").with_default("7.x").project(),
        Setting::stored("mib.icd_per_mib", Mib, "ICD issue per MIB", Kind::Map { key: "MIB", value: "Issue", paths: false }, "A MIB that follows another issue than the rest: its name, and 7.x, 6.9, 6.x, 5.x, 4.x or auto.")
            .checked(|v| match entries(v).iter().find(|(_, i)| !icd_issue(i)) {
                Some((name, issue)) => Err(format!("{name}: \"{issue}\" isn't an issue -- one of {} (7.2 counts as 7.x)", MIB_ICDS.join(", "))),
                None => Ok(()),
            })
            .project(),
        Setting::stored("mib.columns", Mib, "Table columns", Kind::Map { key: "Table", value: "Columns", paths: false }, "A table's columns in file order, separated by commas, in place of the ICD's: for a mission whose tables differ, or a table of its own (xyz reads xyz.dat).")
            .checked(|v| match entries(v).iter().find(|(t, cols)| t.trim().is_empty() || t.contains(['/', '\\', '.']) || cols.trim().is_empty()) {
                Some((table, _)) => Err(format!("{table}: a table is a .dat file's name with its columns, like ccf = CCF_CNAME, CCF_DESCR")),
                None => Ok(()),
            })
            .project(),
        Setting::stored("mib.xtce_fields", Mib, "XTCE identification", Kind::Map { key: "Field", value: "Parameters", paths: false }, "In an XTCE MIB, the parameters whose restriction values identify a packet, and the arguments that say a telecommand's: apid, service, subtype, pi1 or pi2, each with its names separated by commas. A field not given goes by the names: APID, SERVICE, SUBTYPE, SID.")
            .checked(|v| match entries(v).iter().find(|(f, _)| !XTCE_FIELDS.contains(&f.trim().to_ascii_lowercase().as_str())) {
                Some((field, _)) => Err(format!("\"{field}\": one of {}", XTCE_FIELDS.join(", "))),
                None => Ok(()),
            })
            .project(),
        Setting::stored("mib.xtce_version", Mib, "XTCE export", Kind::Choice(&["1.2", "1.1"]), "The XTCE version a MIB is exported in (SPC k g): 1.2, or 1.1 for ground systems that read only that.").with_default("1.2").project(),
        Setting::stored("mib.apid_format", Mib, "APIDs in", Kind::Choice(&["hex", "decimal"]), "How APIDs are shown.").with_default("hex"),
        Setting::stored("mib.watch", Mib, "Reload on change", Kind::Bool, "Reload a MIB when one of its .dat files changes on disk.").with_default("on"),
        Setting::stored("mib.check_scripts", Mib, "Check scripts", Kind::Bool, "Underline telecommand calls in scripts that the MIB disagrees with: unknown mnemonics, missing or out-of-range arguments.").with_default("on").project(),
    ]
};

/// Your MIB folders (`mib.roots`), by name.
fn your_roots(app: &App) -> Vec<(String, PathBuf)> {
    app.config.map_value("mib.roots").iter().map(|(name, path)| (name.clone(), PathBuf::from(path))).collect()
}

pub(crate) static SPEC: FeatureSpec = FeatureSpec {
    id: "fenix.mib",
    commands: &[
        FeatureCommand { id: "mib.page", description: "The MIB page: telecommands, TC and TM parameters, TM packets, calibrations", external: true, run: |app| on(app, MibHost::cmd_page) },
        FeatureCommand { id: "mib.search", description: "Search every kind of MIB definition", external: true, run: |app| on(app, MibHost::cmd_search) },
        FeatureCommand { id: "mib.telecommand", description: "Open a MIB telecommand by name", external: false, run: |app| on(app, |m, a| m.cmd_pick(a, Kind::Telecommand)) },
        FeatureCommand { id: "mib.tc_parameter", description: "Open a MIB TC parameter by name", external: false, run: |app| on(app, |m, a| m.cmd_pick(a, Kind::TcParam)) },
        FeatureCommand { id: "mib.tm_packet", description: "Open a MIB TM packet by SPID or name", external: false, run: |app| on(app, |m, a| m.cmd_pick(a, Kind::TmPacket)) },
        FeatureCommand { id: "mib.tm_parameter", description: "Open a MIB TM parameter by name", external: false, run: |app| on(app, |m, a| m.cmd_pick(a, Kind::TmParam)) },
        FeatureCommand { id: "mib.calibration", description: "Open a MIB calibration by id", external: false, run: |app| on(app, |m, a| m.cmd_pick(a, Kind::Calibration)) },
        FeatureCommand { id: "mib.insert", description: "Insert a telecommand from the MIB, with a form for its arguments", external: false, run: |app| on(app, MibHost::cmd_insert) },
        FeatureCommand { id: "mib.edit_call", description: "Edit the telecommand call on this line in the insert form", external: false, run: |app| on(app, MibHost::cmd_edit_call) },
        FeatureCommand { id: "mib.reload", description: "Read the project's MIBs again", external: false, run: |app| on(app, MibHost::cmd_reload) },
        FeatureCommand { id: "mib.settings", description: "This project's MIB settings", external: false, run: |app| open_settings(app, true) },
        FeatureCommand { id: "mib.copy_bytes", description: "Copy the telecommand call on this line as its packet's bytes", external: false, run: |app| on(app, MibHost::cmd_copy_bytes) },
        FeatureCommand { id: "mib.generate", description: "Generate XTCE, a Wireshark dissector, C, Python, an ICD or TC test vectors from the MIB", external: true, run: |app| on(app, MibHost::cmd_generate) },
    ],
    leader: &[],
    local_menu: &[
        LeaderEntry::Bind(&['i'], "insert a telecommand", "mib.insert"),
        LeaderEntry::Bind(&['e'], "edit the telecommand call", "mib.edit_call"),
        LeaderEntry::Bind(&['t'], "telecommand", "mib.telecommand"),
        LeaderEntry::Bind(&['k'], "TM packet", "mib.tm_packet"),
        LeaderEntry::Bind(&['p'], "TM parameter", "mib.tm_parameter"),
        LeaderEntry::Bind(&['c'], "calibration", "mib.calibration"),
        LeaderEntry::Bind(&['m'], "MIB page", "mib.page"),
        LeaderEntry::Bind(&['r'], "read the MIBs again", "mib.reload"),
    ],
    // Its menu entries go in the Tcl context, which Tcl claims.
    local_context: None,
    language_server: None,
    tasks: None,
    hover_fallback: None,
    completion_at_cursor: None,
    completion_pool_at: None,    hover_first: None,
    picker_confirm: None,
    picker_cancel: None,
    modeline_clock: None,
    page_action: Some(page_action),
    page_context: Some(page_context),
    page_title: Some(|app, id| {
        let p = app.feature_page::<DefPage>(id)?;
        Some(mib_def::title(app.mib().and_then(|m| m.set_ready(&p.key)).as_deref(), p))
    }),
    page_after_layout: None,
    on_event: None,
    settings: &SETTINGS,
    services: &[],
    contributes: &[],
    session_documents: None,
    session_reopen: None,
};

/// A set of MIBs, read or being read.
pub(crate) struct MibSlot {
    pub(super) set: Option<Arc<MibSet>>,
    /// A read is under way: the first, or a reload after a change.
    pub(super) loading: bool,
}

/// What asked for a set before it was ready.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MibPending {
    Search,
    Pick(Kind),
    Insert,
    EditCall,
}

/// Where an insert form's command goes: a buffer, and the characters it
/// takes the place of (none, for an insert at the cursor).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MibOrigin {
    pub(super) buffer: BufferId,
    pub(super) range: std::ops::Range<usize>,
}

/// A row of one of the MIB's pickers.
#[derive(Debug, Clone, Copy)]
enum MibPick {
    /// A definition, to open (or to follow on a stream).
    Def(DefRef),
    /// A telecommand, for its insert form.
    Insert(DefRef),
    /// One of `fenix_mib::generate::KINDS`.
    Generate(usize),
}

/// Everything the MIB keeps between calls.
#[derive(Default)]
pub(crate) struct MibHost {
    /// Every set of MIBs read or being read, by what it was read from.
    pub(super) sets: HashMap<MibKey, MibSlot>,
    /// A command waiting for its set to be read.
    pub(super) pending: Option<(MibKey, MibPending)>,
    /// The set a definition picker is choosing from.
    pub(super) picker_key: MibKey,
    /// Each open insert form's target.
    pub(super) origins: HashMap<BufferId, MibOrigin>,
    /// The last values used for each telecommand, by project.
    pub(super) last: HashMap<(Option<PathBuf>, String), HashMap<String, String>>,
    /// The stream page a definition picker is choosing a parameter for.
    pub(super) pick_for_stream: Option<BufferId>,
    /// The script checks' problems, by file.
    pub(super) diagnostics: HashMap<PathBuf, Vec<lsp_types::Diagnostic>>,
    /// Each buffer's edit count when it was last checked.
    pub(super) checked: HashMap<BufferId, u64>,
}

// -- Settings: what only reads `App` -----------------------------------------

/// The MIBs the focused project uses: its own `mib.roots` (paths
/// relative to it), else the project itself when it's a MIB, else yours
/// -- with yours added to the project's when it says so. And where they
/// come from, for the page to say.
pub(super) fn key_here(app: &App) -> (MibKey, String) {
    let project = app.project_root.clone();
    let name = project.as_ref().and_then(|r| r.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut roots: Vec<MibRoot> = Vec::new();
    if let (Some(root), Some(fenix_config::Value::Map(entries))) = (&project, project_value(app, "mib.roots")) {
        for (label, path) in entries {
            let path = PathBuf::from(path);
            let path = if path.is_absolute() { path } else { root.join(path) };
            roots.push(MibRoot { label: label.clone(), path });
        }
    }
    if roots.is_empty() {
        if let Some(root) = &project {
            if app.project_kind_of(root) == fenix_project::ProjectKind::Mib {
                let dir = if root.join("mib").is_dir() { root.join("mib") } else { root.clone() };
                roots.push(MibRoot { label: name.clone(), path: dir });
            }
        }
    }
    let own = !roots.is_empty();
    let include = match project_value(app, "mib.include_yours") {
        Some(fenix_config::Value::Bool(b)) => *b,
        _ => app.config.bool_value("mib.include_yours").unwrap_or(false),
    };
    if !own || include {
        for (label, path) in your_roots(app) {
            if !roots.iter().any(|r| r.path == path) {
                roots.push(MibRoot { label, path });
            }
        }
    }
    let default = match project_value(app, "mib.default") {
        Some(fenix_config::Value::Text(t)) => Some(t.clone()),
        _ => app.config.text_value("mib.default").map(str::to_string),
    };
    let source = match (own, include && !app.config.map_value("mib.roots").is_empty()) {
        (true, false) => format!("from {name}'s settings"),
        (true, true) => format!("from {name}'s settings and yours"),
        (false, _) if roots.is_empty() => String::new(),
        (false, _) => "from your settings".to_string(),
    };
    (MibKey { roots, default, project, options: read_options(app) }, source)
}

fn project_value<'a>(app: &'a App, key: &str) -> Option<&'a fenix_config::Value> {
    app.project_settings.as_ref().and_then(|(_, p)| p.get(key))
}

/// A MIB setting's text: the project's, else yours.
fn text(app: &App, key: &str) -> Option<String> {
    match project_value(app, key) {
        Some(fenix_config::Value::Text(t)) => Some(t.clone()),
        _ => app.config.text_value(key).map(str::to_string),
    }
}

/// A MIB setting's entries: the project's, else yours.
fn map(app: &App, key: &str) -> Vec<(String, String)> {
    match project_value(app, key) {
        Some(fenix_config::Value::Map(m)) => m.clone(),
        _ => app.config.map_value(key).to_vec(),
    }
}

/// How the focused project's MIBs are read: `mib.icd`,
/// `mib.icd_per_mib`, `mib.columns` and `mib.xtce_fields`.
pub(super) fn read_options(app: &App) -> fenix_mib::ReadOptions {
    use fenix_mib::{schema, Icd, Schema, XtceOptions};
    let issue = |t: &str| if t.trim() == "auto" { None } else { Some(Icd::parse(t).unwrap_or(Icd::LATEST)) };
    let schema = Schema {
        icd: text(app, "mib.icd").map_or(Some(Icd::LATEST), |t| issue(&t)),
        per_root: map(app, "mib.icd_per_mib").into_iter().map(|(label, t)| (label, issue(&t))).collect(),
        columns: map(app, "mib.columns").into_iter().map(|(t, c)| (t.trim().to_ascii_lowercase(), schema::parse_columns(&c))).collect(),
    };
    let fields = map(app, "mib.xtce_fields");
    let names = |field: &str| -> Vec<String> {
        fields.iter().filter(|(f, _)| f.trim().eq_ignore_ascii_case(field)).flat_map(|(_, v)| v.split([',', ' ']).map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).collect::<Vec<_>>()).collect()
    };
    let xtce = XtceOptions { apid: names("apid"), service: names("service"), subtype: names("subtype"), pi1: names("pi1"), pi2: names("pi2") };
    fenix_mib::ReadOptions { schema, xtce }
}

pub(super) fn apid_hex(app: &App) -> bool {
    app.config.text_value("mib.apid_format") != Some("decimal")
}

/// How `key`'s project writes a command: its templates, else yours.
fn templates(app: &App, key: &MibKey) -> mib_form::Templates {
    let project = key.project.as_deref().map(fenix_config::ProjectSettings::load);
    let text = |name: &str, default: &str| match project.as_ref().and_then(|p| p.get(name)) {
        Some(fenix_config::Value::Text(t)) => t.clone(),
        _ => app.config.text_value(name).unwrap_or(default).to_string(),
    };
    mib_form::Templates {
        command: text("mib.telecommand_template", fenix_mib::telecommand::DEFAULT_TEMPLATE),
        argument: text("mib.telecommand_argument_template", fenix_mib::telecommand::DEFAULT_ARGUMENT_TEMPLATE),
        separator: text("mib.telecommand_argument_separator", fenix_mib::telecommand::DEFAULT_ARGUMENT_SEPARATOR),
    }
}

/// `SPC k ,`: the MIB settings -- the project's, or yours outside one.
fn open_settings(app: &mut App, project: bool) {
    let scope = match app.settings_project() {
        Some((root, name)) if project => crate::settings_page::Scope::Project { root, name },
        _ => crate::settings_page::Scope::You,
    };
    app.open_settings_page(scope, Some("mib.roots"));
}

/// The text buffer a form's command goes into: the focused one, else
/// the one used most recently.
fn insert_target(app: &App) -> Option<BufferId> {
    let editable = |id: BufferId| !app.is_page_buffer(id) && app.buffers.get(id).is_some_and(|ob| ob.kind.tracks_unsaved_changes());
    let focused = app.focused_buffer_id();
    if editable(focused) {
        return Some(focused);
    }
    app.buffers.mru().iter().copied().find(|&id| editable(id))
}

/// Where `buffer`'s cursor is, in a pane showing it, else where it was
/// left when its tab was last in front.
fn buffer_cursor(app: &App, buffer: BufferId) -> usize {
    let ws = app.workspaces.active_workspace();
    ws.windows
        .windows()
        .into_iter()
        .find(|pane| ws.windows.content(*pane) == Some(&buffer))
        .and_then(|pane| app.workspaces.active_pane_states().get(&pane).map(|s| s.cursor.char_idx))
        .or_else(|| app.buffers.get(buffer).map(|ob| ob.cursor.char_idx))
        .unwrap_or(0)
}

fn copy(app: &mut App, text: String) {
    if let Some(clipboard) = &mut app.clipboard {
        let _ = clipboard.set_text(text.clone());
    }
    app.set_message(format!("Copied {text}"));
}

fn open_file(app: &mut App, path: PathBuf, line: usize) {
    app.jump_to_grep_match(&fenix_project::GrepMatch { path, line, col: 1, text: String::new() });
}

/// `bytes` on the clipboard, written the way `buffer`'s language writes
/// bytes.
fn copy_literal(app: &mut App, bytes: &[u8], buffer: Option<BufferId>, what: &str) {
    let ext = buffer.and_then(|b| app.buffers.get(b)).and_then(|ob| ob.buffer.path()).and_then(|p| p.extension()).map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let text = fenix_ccsds::hex::literal(bytes, &ext);
    if let Some(clipboard) = &mut app.clipboard {
        let _ = clipboard.set_text(text);
    }
    app.set_message(format!("copied {what}'s packet, {} bytes", bytes.len()));
}

/// The MIBs page `id` shows, if it's one of the MIB's pages.
pub(super) fn page_key(app: &App, id: BufferId) -> Option<MibKey> {
    app.feature_page::<MibPage>(id)
        .map(|p| p.key.clone())
        .or_else(|| app.feature_page::<DefPage>(id).map(|p| p.key.clone()))
        .or_else(|| app.feature_page::<InsertForm>(id).map(|p| p.key.clone()))
}

/// What a MIB page reads besides itself: the set and whether it's still
/// being read, where it comes from, how APIDs show, and the form's packet
/// when it shows its bytes.
#[derive(Default)]
pub(super) struct PageCtx {
    set: Option<Arc<MibSet>>,
    loading: bool,
    /// Where the MIBs come from -- only when they're the focused
    /// project's, for the page's heading.
    source_here: String,
    /// The same, always (what a key reads).
    source: String,
    apid_hex: bool,
    form_bytes: Option<Result<Vec<u8>, String>>,
}

impl PageCtx {
    fn mib(&self) -> mib_page::Ctx<'_> {
        mib_page::Ctx { set: self.set.as_deref(), loading: self.loading, apid_hex: self.apid_hex, source: self.source_here.clone() }
    }
}

fn page_context(app: &mut App, id: BufferId, need: super::feature::PageNeed) -> Box<dyn Any> {
    if matches!(need, super::feature::PageNeed::Layout { due: false, .. }) {
        return Box::new(());
    }
    let mut ctx = PageCtx { apid_hex: apid_hex(app), ..PageCtx::default() };
    let key = app.feature_page::<MibPage>(id).map(|p| p.key.clone()).or_else(|| app.feature_page::<DefPage>(id).map(|p| p.key.clone()));
    if let Some(key) = key {
        (ctx.set, ctx.loading) = app.with_mib(|m, a| m.page_ctx_set(a, &key)).unwrap_or((None, false));
        let (here, source) = key_here(app);
        ctx.source_here = if here == key { source.clone() } else { String::new() };
        ctx.source = source;
    }
    if let Some(f) = app.feature_page::<InsertForm>(id).filter(|f| f.show_bytes) {
        ctx.form_bytes = Some(app.mib().map_or_else(|| Err("the MIB isn't read yet".to_string()), |mib| mib.form_bytes(app, f)));
    }
    Box::new(ctx)
}

/// What a key on one of the MIB's pages asked for, by the page's kind.
fn page_action(app: &mut App, _id: BufferId, action: Box<dyn Any>) {
    let action = match action.downcast::<mib_page::Action>() {
        Ok(action) => return app.with_mib(|m, a| m.page_action(a, *action)).unwrap_or_default(),
        Err(action) => action,
    };
    let action = match action.downcast::<mib_def::Action>() {
        Ok(action) => return app.with_mib(|m, a| m.def_action(a, *action)).unwrap_or_default(),
        Err(action) => action,
    };
    if let Ok(action) = action.downcast::<mib_form::Action>() {
        app.with_mib(|m, a| m.form_action(a, *action));
    }
}

fn ctx(ctx: &dyn Any) -> &PageCtx {
    static NONE: std::sync::LazyLock<PageCtx> = std::sync::LazyLock::new(PageCtx::default);
    ctx.downcast_ref().unwrap_or(&NONE)
}

/// The MIB page (`SPC k k`).
impl FeaturePage for MibPage {
    fn feature(&self) -> &'static str {
        SPEC.id
    }

    fn title(&self) -> String {
        "*mib*".to_string()
    }

    fn typing(&self) -> bool {
        MibPage::typing(self)
    }

    fn claims_space(&self) -> bool {
        MibPage::claims_space(self)
    }

    fn paste(&mut self, text: &str) {
        MibPage::paste(self, text);
    }

    fn layout(&mut self, c: Box<dyn Any>, cols: usize) -> crate::page::Page {
        mib_page::layout(self, &ctx(&*c).mib(), cols)
    }

    fn key(&mut self, key: crate::page::Key, c: Box<dyn Any>) -> Box<dyn Any> {
        let c = ctx(&*c);
        let mut m = c.mib();
        m.source = c.source.clone();
        Box::new(MibPage::key(self, key, &m))
    }
}

/// A definition (`Enter` on a MIB row, `gd` on a name).
impl FeaturePage for DefPage {
    fn feature(&self) -> &'static str {
        SPEC.id
    }

    /// `*mib*` until the set's read; `SPEC.page_title` names the
    /// definition once it is.
    fn title(&self) -> String {
        "*mib*".to_string()
    }

    fn typing(&self) -> bool {
        DefPage::typing(self)
    }

    fn paste(&mut self, text: &str) {
        DefPage::paste(self, text);
    }

    /// `Ctrl-o`/`Ctrl-i`: back and forward through the definitions
    /// followed.
    fn claims_ctrl(&self, key: crate::page::Key) -> bool {
        matches!(key, crate::page::Key::CtrlO | crate::page::Key::CtrlI)
    }

    fn layout(&mut self, c: Box<dyn Any>, cols: usize) -> crate::page::Page {
        let c = ctx(&*c);
        mib_def::layout(self, c.set.as_deref(), c.apid_hex, cols)
    }

    fn key(&mut self, key: crate::page::Key, c: Box<dyn Any>) -> Box<dyn Any> {
        Box::new(match &ctx(&*c).set {
            Some(set) => DefPage::key(self, key, set),
            None if matches!(key, crate::page::Key::Char('q') | crate::page::Key::Escape) => mib_def::Action::Close,
            None => mib_def::Action::None,
        })
    }
}

/// The insert form (`SPC k i`, `SPC k e`).
impl FeaturePage for InsertForm {
    fn feature(&self) -> &'static str {
        SPEC.id
    }

    fn title(&self) -> String {
        mib_form::title(self)
    }

    fn typing(&self) -> bool {
        InsertForm::typing(self)
    }

    fn claims_space(&self) -> bool {
        InsertForm::claims_space(self)
    }

    fn paste(&mut self, text: &str) {
        InsertForm::paste(self, text);
    }

    /// `Ctrl-Enter`: insert the command.
    fn claims_ctrl(&self, key: crate::page::Key) -> bool {
        key == crate::page::Key::CtrlEnter
    }

    fn layout(&mut self, c: Box<dyn Any>, cols: usize) -> crate::page::Page {
        mib_form::layout(self, ctx(&*c).form_bytes.as_ref(), cols)
    }

    fn key(&mut self, key: crate::page::Key, _ctx: Box<dyn Any>) -> Box<dyn Any> {
        Box::new(InsertForm::key(self, key))
    }
}

/// The MIBs of the focused MIB page.
fn focused_key(app: &App) -> MibKey {
    page_key(app, app.focused_buffer_id()).unwrap_or_else(|| key_here(app).0)
}

/// Whether the focused file is one where MIB names mean something.
fn in_this_file(app: &App) -> bool {
    if app.is_page_buffer(app.focused_buffer_id()) {
        return false;
    }
    let exts: Vec<String> = match project_value(app, "mib.editor_files") {
        Some(fenix_config::Value::List(l)) => l.clone(),
        _ => app.config.list_value("mib.editor_files").to_vec(),
    };
    if exts.is_empty() {
        return true;
    }
    let ext = app.open().buffer.path().and_then(|p| p.extension()).map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    exts.iter().any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(&ext))
}

/// Every MIB page is laid out again.
fn mark_pages(app: &mut App) {
    for state in app.pages.values_mut() {
        if matches!(&state.model, PageModel::Feature(p) if p.feature() == SPEC.id) {
            state.stale = true;
        }
    }
    if let Some(window) = &app.window {
        window.request_redraw();
    }
}

/// `def`'s page: the open definition page goes to it (so `Ctrl-o` comes
/// back), else a new one opens.
pub(super) fn open_def(app: &mut App, key: MibKey, def: DefRef) {
    let existing = app.find_page(|m| page_as::<DefPage>(m).is_some_and(|p| p.key == key));
    match existing {
        Some(id) => {
            if let Some(page) = app.feature_page_mut::<DefPage>(id) {
                page.go(def);
            }
            app.show_page(id);
        }
        None => {
            app.open_page(PageModel::Feature(Box::new(DefPage::new(key, def))));
        }
    }
}

fn picker_rows(app: &App, set: &MibSet, kinds: &[Kind], pick: fn(DefRef) -> MibPick) -> Vec<fenix_picker::Candidate<FeaturePick>> {
    let hex = apid_hex(app);
    let many = set.roots().len() > 1;
    let mut out = Vec::new();
    for &kind in kinds {
        for (index, e) in set.entries(kind).iter().enumerate() {
            let def = DefRef { kind, index };
            let cols: Vec<String> = (1..kind.headers().len()).map(|c| mib_page::cell(set, def, c, hex)).filter(|c| !c.is_empty()).collect();
            let mib = if many { format!("[{}]  ", set.root_label(e.root)) } else { String::new() };
            let tag = if kinds.len() > 1 { format!("{:<4}", kind.tag()) } else { String::new() };
            let label = format!("{tag}{:<10}  {:<24}  {mib}{}", e.name, cols.join("  "), e.description);
            out.push(FeaturePick::candidate(label, pick(def)));
        }
    }
    out
}

/// A parameter picker for a stream page: the chosen parameter is
/// followed on it. `rows` are the labels and definitions to offer.
pub(super) fn enter_stream_picker(app: &mut App, stream: BufferId, key: MibKey, rows: Vec<(String, DefRef)>) {
    let candidates = rows.into_iter().map(|(label, def)| FeaturePick::candidate(label, MibPick::Def(def))).collect();
    app.with_feature::<MibHost, _>(|mib, _| {
        mib.picker_key = key;
        mib.pick_for_stream = Some(stream);
    });
    app.enter_picker(ActivePicker::Feature { feature: SPEC.id, badge: "MIB", state: fenix_picker::PickerState::new(candidates) });
}

impl MibHost {
    /// The set for `key` when it's been read; starts reading it when it
    /// hasn't.
    pub(super) fn set(&mut self, app: &mut App, key: &MibKey) -> Option<Arc<MibSet>> {
        if key.roots.is_empty() {
            return None;
        }
        match self.sets.get(key) {
            Some(slot) => slot.set.clone(),
            None => {
                self.load(app, key.clone());
                self.sets.get(key).and_then(|s| s.set.clone())
            }
        }
    }

    /// The set for `key`, only if it's already read -- for the editor,
    /// which never waits.
    pub(super) fn set_ready(&self, key: &MibKey) -> Option<Arc<MibSet>> {
        self.sets.get(key).and_then(|s| s.set.clone())
    }

    fn loading(&self, key: &MibKey) -> bool {
        self.sets.get(key).is_some_and(|s| s.loading)
    }

    /// Reads `key`'s MIBs off the UI thread; what was read before stays
    /// in use until the new set arrives.
    pub(super) fn load(&mut self, app: &mut App, key: MibKey) {
        let slot = self.sets.entry(key.clone()).or_insert(MibSlot { set: None, loading: false });
        if slot.loading {
            return;
        }
        slot.loading = true;
        let mission = app.mission(key.project.as_deref());
        app.page_spawn(move |send| {
            let mut set = MibSet::load_with(key.roots.clone(), key.default.as_deref(), key.options.clone());
            let options = fenix_mib::checks::Options { disabled: &mission.checks_off, base: mission.base, profile: &mission.profile };
            let found = fenix_mib::checks::run(&set, &options);
            set.add_problems(found);
            send(PageEvent::feature(SPEC.id, "MibLoaded", (key, Arc::new(set))));
        });
    }

    pub(super) fn apply_loaded(&mut self, app: &mut App, key: MibKey, set: Arc<MibSet>) {
        let problems = set.problems().len();
        let fresh = self.sets.get(&key).is_none_or(|s| s.set.is_none());
        self.sets.insert(key.clone(), MibSlot { set: Some(set), loading: false });
        mark_pages(app);
        if !fresh && problems > 0 {
            app.set_message(format!("MIB read again -- {problems} problem{} (! on the MIB page)", if problems == 1 { "" } else { "s" }));
        } else if !fresh {
            app.set_message("MIB read again");
        }
        if let Some((pending_key, what)) = self.pending.take() {
            if pending_key == key {
                match what {
                    MibPending::Search => self.cmd_search(app),
                    MibPending::Pick(kind) => self.cmd_pick(app, kind),
                    MibPending::Insert => self.cmd_insert(app),
                    MibPending::EditCall => self.cmd_edit_call(app),
                }
            } else {
                self.pending = Some((pending_key, what));
            }
        }
    }

    /// The disk poll's tick: a set whose files changed is read again.
    /// Tests only: the app polls through the disk probe instead.
    #[cfg(test)]
    pub(super) fn poll(&mut self, app: &mut App) {
        if !app.config.bool_value("mib.watch").unwrap_or(true) {
            return;
        }
        let stale: Vec<MibKey> = self
            .sets
            .iter()
            .filter(|(_, slot)| !slot.loading)
            .filter_map(|(key, slot)| slot.set.as_ref().filter(|set| fenix_mib::Stamp::of(&key.roots) != *set.stamp()).map(|_| key.clone()))
            .collect();
        for key in stale {
            self.load(app, key);
        }
    }

    /// A project was focused: its MIBs start being read, so `K`, `gd` and
    /// completion have them by the time they're asked.
    fn preload(&mut self, app: &mut App) {
        let (key, _) = key_here(app);
        if !key.roots.is_empty() && !self.sets.contains_key(&key) {
            self.load(app, key);
        }
    }

    /// The MIB settings changed: the MIB page follows the project's MIBs.
    pub(super) fn settings_changed(&mut self, app: &mut App) {
        let (key, _) = key_here(app);
        if let Some(id) = app.find_page(|m| page_as::<MibPage>(m).is_some()) {
            if let Some(page) = app.feature_page_mut::<MibPage>(id) {
                page.key = key;
            }
        }
        mark_pages(app);
    }

    /// The set for the focused project, or a word about why there's none
    /// yet -- `pending` runs once it's read.
    fn set_here(&mut self, app: &mut App, pending: MibPending) -> Option<(MibKey, Arc<MibSet>)> {
        let (key, _) = key_here(app);
        if key.roots.is_empty() {
            app.set_error("no MIB for this project -- SPC k , lists one in its settings");
            return None;
        }
        match self.set(app, &key) {
            Some(set) => Some((key, set)),
            None => {
                app.set_message("reading the MIB…");
                self.pending = Some((key, pending));
                None
            }
        }
    }

    // -- Commands ---------------------------------------------------------

    /// `SPC k k`: the MIB page, on the focused project's MIBs.
    pub(super) fn cmd_page(&mut self, app: &mut App) {
        let (key, _) = key_here(app);
        let _ = self.set(app, &key);
        match app.find_page(|m| page_as::<MibPage>(m).is_some()) {
            Some(id) => {
                if let Some(page) = app.feature_page_mut::<MibPage>(id) {
                    page.key = key;
                }
                app.show_page(id);
            }
            None => {
                app.open_page(PageModel::Feature(Box::new(MibPage::new(key))));
            }
        }
    }

    fn enter_picker(&mut self, app: &mut App, key: MibKey, badge: &'static str, candidates: Vec<fenix_picker::Candidate<FeaturePick>>) {
        self.picker_key = key;
        app.enter_picker(ActivePicker::Feature { feature: SPEC.id, badge, state: fenix_picker::PickerState::new(candidates) });
    }

    /// `SPC k /`: every kind of definition, by name or description.
    fn cmd_search(&mut self, app: &mut App) {
        let Some((key, set)) = self.set_here(app, MibPending::Search) else { return };
        let candidates = picker_rows(app, &set, &Kind::ALL, MibPick::Def);
        self.pick_for_stream = None;
        self.enter_picker(app, key, "MIB", candidates);
    }

    /// `SPC k t`/`p`/`m`/`n`/`c`: one kind of definition, by name.
    fn cmd_pick(&mut self, app: &mut App, kind: Kind) {
        let Some((key, set)) = self.set_here(app, MibPending::Pick(kind)) else { return };
        let candidates = picker_rows(app, &set, &[kind], MibPick::Def);
        if candidates.is_empty() {
            app.set_message(format!("no {} in this project's MIBs", kind.plural().to_lowercase()));
            return;
        }
        self.pick_for_stream = None;
        self.enter_picker(app, key, "MIB", candidates);
    }

    /// `SPC k i`: a telecommand, then its insert form.
    fn cmd_insert(&mut self, app: &mut App) {
        let Some((key, set)) = self.set_here(app, MibPending::Insert) else { return };
        let candidates = picker_rows(app, &set, &[Kind::Telecommand], MibPick::Insert);
        self.enter_picker(app, key, "INSERT TC", candidates);
    }

    /// `SPC k r`: the MIBs read again now.
    pub(super) fn cmd_reload(&mut self, app: &mut App) {
        let (key, _) = key_here(app);
        if key.roots.is_empty() {
            app.set_error("no MIB for this project -- SPC k , lists one in its settings");
            return;
        }
        app.set_message("reading the MIB again…");
        self.load(app, key);
    }

    /// `SPC k e`: the telecommand call on the cursor's line, back in the
    /// insert form; inserting puts it back in place of the line's call.
    fn cmd_edit_call(&mut self, app: &mut App) {
        let Some((key, set)) = self.set_here(app, MibPending::EditCall) else { return };
        let buffer = app.focused_buffer_id();
        let (line_text, line_start) = {
            let ob = app.open();
            let (line, _) = ob.buffer.line_col(&app.cursor());
            (ob.buffer.line(line).to_string(), ob.buffer.line_start_char(line))
        };
        let line_text = line_text.trim_end_matches(['\n', '\r']).to_string();
        let tc = words(&line_text).into_iter().find_map(|(_, w)| set.resolve(w).filter(|d| d.kind == Kind::Telecommand));
        let Some(tc) = tc else {
            app.set_error("no telecommand from this project's MIBs on this line");
            return;
        };
        let templates = templates(app, &key);
        let names: Vec<String> = fenix_mib::telecommand::tc_parameters(set.index(), &set.get(tc).row).into_iter().filter(|p| !p.fixed).map(|p| p.name).collect();
        let args = mib_form::read_arguments(&line_text, &set.get(tc).name, &names, &templates);
        let indent = line_text.chars().take_while(|c| c.is_whitespace()).count();
        let range = line_start + indent..line_start + line_text.chars().count();
        let mut form = InsertForm::new(&set, key.clone(), tc, templates, None, apid_hex(app));
        form.fill(&args);
        form.replacing = true;
        let id = app.open_page(PageModel::Feature(Box::new(form)));
        self.origins.insert(id, MibOrigin { buffer, range });
    }

    // -- Pages --------------------------------------------------------------

    /// The insert form for `tc`, inserting where the cursor was in the
    /// last file you were in.
    pub(super) fn open_form(&mut self, app: &mut App, key: MibKey, tc: DefRef) {
        let Some(set) = self.set_ready(&key) else { return };
        let Some(buffer) = insert_target(app) else {
            app.set_error("open the file the telecommand goes in first -- the form inserts where its cursor is");
            return;
        };
        let at = if buffer == app.focused_buffer_id() { app.cursor().char_idx } else { buffer_cursor(app, buffer) };
        let remembered = self.last.get(&(key.project.clone(), set.get(tc).name.clone()));
        let templates = templates(app, &key);
        let form = InsertForm::new(&set, key, tc, templates, remembered, apid_hex(app));
        let id = app.open_page(PageModel::Feature(Box::new(form)));
        self.origins.insert(id, MibOrigin { buffer, range: at..at });
    }

    pub(super) fn page_action(&mut self, app: &mut App, action: mib_page::Action) {
        let key = focused_key(app);
        match action {
            mib_page::Action::None => {}
            mib_page::Action::Close => {
                let id = app.focused_buffer_id();
                app.close_page(id);
            }
            mib_page::Action::Open(def) => open_def(app, key, def),
            mib_page::Action::Insert(def) => self.open_form(app, key, def),
            mib_page::Action::Copy(text) => copy(app, text),
            mib_page::Action::Reload => self.cmd_reload(app),
            mib_page::Action::Settings(project) => open_settings(app, project),
            mib_page::Action::OpenFile(path, line) => open_file(app, path, line),
        }
    }

    pub(super) fn def_action(&mut self, app: &mut App, action: mib_def::Action) {
        let key = focused_key(app);
        match action {
            mib_def::Action::None => {}
            mib_def::Action::Close => {
                let id = app.focused_buffer_id();
                app.close_page(id);
            }
            mib_def::Action::Insert(def) => self.open_form(app, key, def),
            mib_def::Action::Copy(text) => copy(app, text),
            mib_def::Action::OpenFile(path, line) => open_file(app, path, line),
            mib_def::Action::MibPage => self.cmd_page(app),
            mib_def::Action::Follow(def) => app.follow_on_stream(def),
        }
    }

    pub(super) fn form_action(&mut self, app: &mut App, action: mib_form::Action) {
        let id = app.focused_buffer_id();
        let key = focused_key(app);
        match action {
            mib_form::Action::None => {}
            mib_form::Action::Close => {
                self.origins.remove(&id);
                app.close_page(id);
            }
            mib_form::Action::Open(def) => open_def(app, key, def),
            mib_form::Action::Insert(text) => self.form_insert(app, id, text),
            mib_form::Action::CopyBytes | mib_form::Action::DecodeBytes => {
                let Some(f) = app.feature_page::<InsertForm>(id) else { return };
                let name = f.name().to_string();
                let origin = self.origins.get(&id).map(|o| o.buffer);
                match self.form_bytes(app, f) {
                    Ok(bytes) if action == mib_form::Action::DecodeBytes => app.open_packet_page(bytes, format!("{name} from the form")),
                    Ok(bytes) => copy_literal(app, &bytes, origin, &name),
                    Err(e) => app.set_error(format!("can't build {name}'s packet: {e}")),
                }
            }
        }
    }

    /// The form's telecommand as a packet, from its arguments.
    pub(super) fn form_bytes(&self, app: &App, f: &InsertForm) -> Result<Vec<u8>, String> {
        let set = self.set_ready(&f.key).ok_or("the MIB isn't read yet")?;
        let mission = app.mission(f.key.project.as_deref());
        fenix_mib::packets::encode_tc(&set, f.tc, &f.arguments(), &mission.profile, f.seq).map(|p| p.bytes)
    }

    /// `SPC k b`: the telecommand call on this line, as its packet's bytes
    /// on the clipboard.
    fn cmd_copy_bytes(&mut self, app: &mut App) {
        let Some((key, set)) = self.set_here(app, MibPending::EditCall) else { return };
        let buffer = app.focused_buffer_id();
        let (line, _) = app.open().buffer.line_col(&app.cursor());
        let text = app.open().buffer.line(line).to_string();
        let text = text.trim_end_matches(['\n', '\r']);
        let Some(tc) = words(text).into_iter().find_map(|(_, w)| set.resolve(w).filter(|d| d.kind == Kind::Telecommand)) else {
            app.set_error("no telecommand from this project's MIBs on this line");
            return;
        };
        let templates = templates(app, &key);
        let names: Vec<String> = fenix_mib::telecommand::tc_parameters(set.index(), &set.get(tc).row).into_iter().filter(|p| !p.fixed).map(|p| p.name).collect();
        let args = mib_form::read_arguments(text, &set.get(tc).name, &names, &templates);
        let mission = app.mission(key.project.as_deref());
        let name = set.get(tc).name.clone();
        match fenix_mib::packets::encode_tc(&set, tc, &args, &mission.profile, 0) {
            Ok(p) => copy_literal(app, &p.bytes, Some(buffer), &name),
            Err(e) => app.set_error(format!("can't build {name}'s packet: {e}")),
        }
    }

    /// The form's command goes where it was opened, the form closes, and
    /// the file it went into is back in front with the cursor after it.
    fn form_insert(&mut self, app: &mut App, id: BufferId, text: String) {
        let Some(origin) = self.origins.remove(&id) else { return };
        let Some((tc, project, values)) = app.feature_page::<InsertForm>(id).map(|form| (form.name().to_string(), form.key.project.clone(), form.values())) else { return };
        self.last.insert((project, tc.clone()), values);
        let Some(ob) = app.buffers.get_mut(origin.buffer) else {
            app.set_error("the file the form was opened from is closed");
            return;
        };
        let len = ob.buffer.len_chars();
        let start = origin.range.start.min(len);
        let end = origin.range.end.min(len);
        let mut cursor = Cursor { char_idx: start, sticky_col: 0 };
        if end > start {
            ob.buffer.replace_range(&mut cursor, start, end, &text);
        } else {
            ob.buffer.insert_str(&mut cursor, &text);
        }
        app.close_page(id);
        app.open_buffer_in_focused_pane(origin.buffer);
        let pane = app.focused_pane_id();
        if let Some(state) = app.workspaces.active_pane_states_mut().get_mut(&pane) {
            state.cursor = cursor;
        }
        app.refresh_project_root();
        app.set_message(format!("inserted {tc}"));
        app.wake_caret();
    }

    /// The focused page's MIB, for laying it out.
    pub(super) fn page_ctx_set(&mut self, app: &mut App, key: &MibKey) -> (Option<Arc<MibSet>>, bool) {
        let set = self.set(app, key);
        (set, self.loading(key))
    }

    // -- The editor ---------------------------------------------------------

    /// The MIB definition the word under the cursor names.
    fn word_here(&mut self, app: &mut App) -> Option<(MibKey, Arc<MibSet>, DefRef)> {
        if !in_this_file(app) {
            return None;
        }
        let (key, _) = key_here(app);
        if key.roots.is_empty() {
            return None;
        }
        let set = self.set(app, &key)?;
        let (line, col) = app.open().buffer.line_col(&app.cursor());
        let text = app.open().buffer.line(line).to_string();
        let word = words(&text).into_iter().find(|(range, _)| range.contains(&col)).map(|(_, w)| w.to_string())?;
        let def = set.resolve(&word)?;
        Some((key, set, def))
    }

    /// `SPC k g`: something to generate from the project's MIBs.
    fn cmd_generate(&mut self, app: &mut App) {
        let (key, _) = key_here(app);
        if key.roots.is_empty() {
            app.set_error("no MIB for this project -- SPC k , lists one in its settings");
            return;
        }
        let candidates = fenix_mib::generate::KINDS.iter().enumerate().map(|(i, (_, file, what))| FeaturePick::candidate(format!("{file:<18} {what}"), MibPick::Generate(i))).collect();
        self.enter_picker(app, key, "GENERATE", candidates);
    }

    /// Writes generated file `kind` into the project's `generated/` and
    /// opens it.
    pub(super) fn generate(&mut self, app: &mut App, kind: usize) {
        let Some((key, set)) = self.set_here(app, MibPending::Search) else { return };
        let Some((name, file, _)) = fenix_mib::generate::KINDS.get(kind) else { return };
        let mission = app.mission(key.project.as_deref());
        let xtce = text(app, "mib.xtce_version").and_then(|v| fenix_mib::generate::XtceVersion::parse(&v).ok()).unwrap_or_default();
        let Some(generated) = fenix_mib::generate::generate_with(name, &set, &mission.profile, mission.base, xtce) else { return };
        let dir = key.project.clone().unwrap_or_else(|| PathBuf::from(".")).join("generated");
        let path = dir.join(file);
        match std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, generated)) {
            Ok(()) => {
                app.open_file_from_picker(&path);
                app.set_message(format!("wrote {} -- regenerate it with SPC k g after the MIB changes", path.display()));
            }
            Err(e) => app.set_error(format!("couldn't write {}: {e}", path.display())),
        }
    }

    /// Checks the telecommand calls of the focused file against the MIB,
    /// when it changed since the last look.
    pub(super) fn check_scripts(&mut self, app: &mut App) {
        let id = app.focused_buffer_id();
        let Some(path) = app.buffers.get(id).and_then(|ob| ob.buffer.path()).map(|p| fenix_lsp::normalize(std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()))) else { return };
        let on = match project_value(app, "mib.check_scripts") {
            Some(fenix_config::Value::Bool(b)) => *b,
            _ => app.config.bool_value("mib.check_scripts").unwrap_or(true),
        };
        if !on || !in_this_file(app) {
            self.diagnostics.remove(&path);
            return;
        }
        let edits = app.open().buffer.edit_count();
        if self.checked.get(&id) == Some(&edits) {
            return;
        }
        let (key, _) = key_here(app);
        let Some(set) = self.set_ready(&key) else { return };
        self.checked.insert(id, edits);
        let text = app.open().buffer.text();
        let templates = templates(app, &key);
        let found = crate::mib_check::check(&text, &set, &templates);
        let diags: Vec<lsp_types::Diagnostic> = found
            .into_iter()
            .map(|f| lsp_types::Diagnostic {
                range: lsp_types::Range {
                    start: lsp_types::Position { line: f.line as u32, character: f.cols.start as u32 },
                    end: lsp_types::Position { line: f.line as u32, character: f.cols.end as u32 },
                },
                severity: Some(match f.severity {
                    crate::mib_check::Severity::Error => lsp_types::DiagnosticSeverity::ERROR,
                    crate::mib_check::Severity::Warning => lsp_types::DiagnosticSeverity::WARNING,
                    crate::mib_check::Severity::Info => lsp_types::DiagnosticSeverity::INFORMATION,
                }),
                source: Some("MIB".into()),
                message: f.message,
                ..Default::default()
            })
            .collect();
        if diags.is_empty() {
            self.diagnostics.remove(&path);
        } else {
            self.diagnostics.insert(path, diags);
        }
        if let Some(window) = &app.window {
            window.request_redraw();
        }
    }

    /// Completion's MIB names: telecommands and parameters, with what
    /// they are.
    pub(super) fn completion_tail(&mut self, app: &mut App) -> Vec<fenix_picker::Candidate<crate::completion::Item>> {
        if !in_this_file(app) {
            return Vec::new();
        }
        let (key, _) = key_here(app);
        let Some(set) = self.set(app, &key) else { return Vec::new() };
        let mut out = Vec::new();
        for kind in [Kind::Telecommand, Kind::TcParam, Kind::TmParam] {
            for e in set.entries(kind) {
                let mut item = crate::completion::Item::text(e.name.clone(), crate::completion::Source::Mib);
                item.detail = format!("{} · {}", kind.tag(), e.description);
                out.push(fenix_picker::Candidate::new(e.name.clone(), item));
            }
        }
        out
    }
}

impl Feature for MibHost {
    fn spec(&self) -> &'static FeatureSpec {
        &SPEC
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    /// A set finished reading (its page job, `PageEvent::Feature`, passed on by the
    /// pages).
    fn on_event(&mut self, app: &mut App, payload: Box<dyn Any + Send>) {
        let Ok(loaded) = payload.downcast::<(MibKey, Arc<MibSet>)>() else { return };
        let (key, set) = *loaded;
        self.apply_loaded(app, key, set);
    }

    fn notice(&mut self, app: &mut App, notice: &Notice) {
        match notice {
            Notice::FocusChanged => self.preload(app),
            Notice::Tick => self.check_scripts(app),
            Notice::TaskFinished { .. } | Notice::TerminalClosed(_) => {}
        }
    }

    /// `K` on a MIB name: its card -- ahead of any language server.
    fn hover(&mut self, app: &mut App) -> Option<String> {
        let (_, set, def) = self.word_here(app)?;
        Some(hover_text(&set, def, apid_hex(app)))
    }

    /// `gd` on a MIB name: its page. Whether it was one.
    fn goto_definition(&mut self, app: &mut App) -> bool {
        let Some((key, _, def)) = self.word_here(app) else { return false };
        let from = JumpEntry { buffer: app.focused_buffer_id(), char_idx: app.cursor().char_idx };
        app.record_jump(from);
        open_def(app, key, def);
        true
    }

    fn diagnostics(&self, path: &Path) -> Vec<lsp_types::Diagnostic> {
        self.diagnostics.get(path).cloned().unwrap_or_default()
    }

    fn picker_confirm(&mut self, app: &mut App, pick: &dyn Any) {
        let Some(&pick) = pick.downcast_ref::<MibPick>() else { return };
        let key = self.picker_key.clone();
        match pick {
            MibPick::Def(def) => match self.pick_for_stream.take() {
                Some(page) => {
                    app.show_page(page);
                    app.stream_follow(page, def);
                }
                None => open_def(app, key, def),
            },
            MibPick::Insert(def) => self.open_form(app, key, def),
            MibPick::Generate(kind) => self.generate(app, kind),
        }
    }
}

/// Typed access for the core code that still reaches the MIB directly:
/// the MIB pages in `pages.rs`, CCSDS, streams, notebook links, the disk
/// poll and settings. `None` while the MIB is inside one of its own
/// hooks.
impl App {
    pub(super) fn mib(&self) -> Option<&MibHost> {
        self.features.get::<MibHost>()
    }

    pub(super) fn with_mib<R>(&mut self, f: impl FnOnce(&mut MibHost, &mut App) -> R) -> Option<R> {
        self.with_feature::<MibHost, _>(f)
    }
}

/// The words of `line` (letters, digits, `_`), each with the columns it
/// spans.
fn words(line: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    let mut out = Vec::new();
    let mut start = None;
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    for (col, &(byte, c)) in chars.iter().enumerate() {
        let word = c.is_alphanumeric() || c == '_';
        match (word, start) {
            (true, None) => start = Some((col, byte)),
            (false, Some((sc, sb))) => {
                out.push((sc..col, &line[sb..byte]));
                start = None;
            }
            _ => {}
        }
    }
    if let Some((sc, sb)) = start {
        out.push((sc..chars.len(), &line[sb..]));
    }
    out
}

/// What `K` shows on a MIB name.
fn hover_text(set: &MibSet, def: DefRef, apid_hex: bool) -> String {
    let e = set.get(def);
    let mut lines = vec![format!("{} {} · {}", def.kind.tag(), e.name, set.root_label(e.root)), e.description.clone()];
    let r = &e.row;
    match def.kind {
        Kind::Telecommand => {
            lines.push(format!("PUS {},{} · APID {} · {}", r.clean("CCF_TYPE"), r.clean("CCF_STYPE"), fenix_mib::types::apid(r.clean("CCF_APID"), apid_hex), r.clean("CCF_SUBSYS")));
            let els: Vec<_> = fenix_mib::detail::tc_elements(set, def).into_iter().filter(|el| el.fixed.is_none() && !el.name.is_empty()).collect();
            if !els.is_empty() {
                lines.push(String::new());
                lines.push("Arguments".to_string());
                for el in els {
                    let what = [el.description.as_str(), el.allowed.as_str()].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" · ");
                    lines.push(format!("  {:<10} {what}", el.name));
                }
            }
            lines.push(String::new());
            lines.push("gd opens it · SPC k e edits this call".to_string());
        }
        _ => {
            let cols: Vec<String> = (1..def.kind.headers().len())
                .map(|c| (def.kind.headers()[c], mib_page::cell(set, def, c, apid_hex)))
                .filter(|(_, v)| !v.is_empty())
                .map(|(h, v)| format!("{} {v}", h.to_lowercase()))
                .collect();
            if !cols.is_empty() {
                lines.push(cols.join(" · "));
            }
            lines.push(String::new());
            lines.push("gd opens it".to_string());
        }
    }
    lines.retain(|l| !l.trim().is_empty() || l.is_empty());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MIB's settings, registered from its spec: saved and loaded back
    /// by key -- a whitespace-only separator quoted so it survives -- and
    /// what isn't an ICD issue, a table or an XTCE field refused.
    #[test]
    fn mib_settings_save_load_and_check_by_key() {
        super::super::feature::register_settings();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let mut config = fenix_config::Config::load_or_default(path.clone());
        let roots = vec![("MIB-A".to_string(), "C:/data/mib-a".to_string()), ("MIB-B".to_string(), "C:/data/mib-b".to_string())];
        config.set_value("mib.roots", Some(fenix_config::Value::Map(roots.clone())));
        config.set_value("mib.telecommand_template", Some(fenix_config::Value::Text("TC {mnemo}".into())));
        config.set_value("mib.telecommand_argument_separator", Some(fenix_config::Value::Text(" ".into())));
        config.save().unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("telecommand_argument_separator = \" \""));
        let reloaded = fenix_config::Config::load(path).unwrap();
        assert_eq!(reloaded.map_value("mib.roots"), roots.as_slice());
        assert_eq!(reloaded.text_value("mib.telecommand_template"), Some("TC {mnemo}"));
        assert_eq!(reloaded.text_value("mib.telecommand_argument_separator"), Some(" "));

        let mut set = |key: &str, entries: Vec<(&str, &str)>| {
            let value = fenix_config::Value::Map(entries.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect());
            fenix_config::schema::setting(key).unwrap().set(&mut config, Some(value))
        };
        assert!(set("mib.icd_per_mib", vec![("OPS", "6.9"), ("SIM", "auto")]).is_ok());
        assert!(set("mib.icd_per_mib", vec![("OPS", "8.1")]).unwrap_err().contains("isn't an issue"));
        assert!(set("mib.columns", vec![("ccf", "CCF_CNAME, CCF_DESCR")]).is_ok());
        assert!(set("mib.columns", vec![("ccf.dat", "CCF_CNAME")]).unwrap_err().contains("a table is"));
        assert!(set("mib.xtce_fields", vec![("pi1", "SID, STRUCT")]).is_ok());
        assert!(set("mib.xtce_fields", vec![("pi3", "X")]).unwrap_err().contains("one of apid"));
        assert!(fenix_config::schema::setting("mib.roots").unwrap().project, "a project can set its own MIBs");
    }

    /// A project with a MIB of its own at `ops/`, listed in its settings
    /// with its own templates, and a Tcl script holding `script`.
    struct Project(PathBuf);
    impl Drop for Project {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn project(name: &str, script: &str) -> (Project, App) {
        let dir = std::env::temp_dir().join(format!("fenix-mib-host-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("ops")).unwrap();
        std::fs::create_dir_all(dir.join(".fenix")).unwrap();
        let dir = fenix_lsp::normalize(std::fs::canonicalize(&dir).unwrap());
        let w = |t: &str, c: &str| std::fs::write(dir.join("ops").join(format!("{t}.dat")), c).unwrap();
        w("ccf", "ZTC08101\tSet heater control mode\t\t\tN\t\t8\t1\t1010\t2\t\t\t\t\tTCS\nZTC17001\tConnection test\t\t\tN\t\t17\t1\t1008\t0\n");
        w("cdf", "ZTC08101\tE\tLine\t8\t0\t0\tPTH00101\t\t\t\nZTC08101\tE\tMode\t8\t8\t0\tPTH00102\t\t\t\n");
        w("cpc", "PTH00101\tHeater line\t3\t4\t\t\t\t\tPRF00017\nPTH00102\tHeater control mode\t2\t8\t\t\t\t\t\t\tPAF00042\t\tAUTO\n");
        w("paf", "PAF00042\tHeater mode\tU\t3\n");
        w("pas", "PAF00042\tOFF\t0\nPAF00042\tON\t1\nPAF00042\tAUTO\t2\n");
        w("prf", "PRF00017\tHeater lines\t\t\t\t1\t\n");
        w("prv", "PRF00017\t1\t8\n");
        w("pcf", "NTH00123\tHeater 3 temperature\t\tdegC\t3\t12\n");
        std::fs::write(
            dir.join(".fenix").join("settings.toml"),
            "[mib]\ntelecommand_template = \"tc::send {mnemo} {arguments}\"\ntelecommand_argument_template = \"-{name} {value}\"\n\n[mib.roots]\nOPS = \"ops\"\n",
        )
        .unwrap();
        let file = dir.join("checkout.tcl");
        std::fs::write(&file, script).unwrap();
        let mut app = App::with_file(Some(file.display().to_string()));
        app.config.set_value("mib.roots", Some(fenix_config::Value::Map(vec![("MINE".to_string(), dir.join("elsewhere").display().to_string())])));
        for key in ["mib.include_yours", "mib.telecommand_template", "mib.telecommand_argument_template", "mib.telecommand_argument_separator", "mib.editor_files"] {
            app.config.set_value(key, None);
        }
        app.refresh_project_root();
        (Project(dir), app)
    }

    fn set_cursor(app: &mut App, char_idx: usize) {
        let pane = app.focused_pane_id();
        if let Some(state) = app.workspaces.active_pane_states_mut().get_mut(&pane) {
            state.cursor = Cursor { char_idx, sticky_col: 0 };
        }
    }

    fn page_text(app: &mut App) -> String {
        let (id, pane) = (app.focused_buffer_id(), app.focused_pane_id());
        app.ensure_page_layout(id, pane, 160);
        app.open().buffer.text()
    }

    fn press(app: &mut App, keys: &[KeyPress]) {
        for &k in keys {
            assert!(app.page_key(k), "the page claims {k:?}");
        }
    }

    #[test]
    fn a_projects_mibs_take_the_place_of_yours_and_are_relative_to_it() {
        let (p, mut app) = project("replace", "");
        let (key, source) = key_here(&app);
        assert_eq!(key.roots, vec![MibRoot { label: "OPS".into(), path: p.0.join("ops") }]);
        assert!(source.contains("settings"), "{source}");
        app.config.set_value("mib.include_yours", Some(fenix_config::Value::Bool(true)));
        assert_eq!(key_here(&app).0.roots.len(), 2, "yours added when asked");
    }

    #[test]
    fn how_the_mibs_are_read_comes_from_the_projects_settings_else_yours() {
        let (p, mut app) = project("read-options", "");
        let options = key_here(&app).0.options;
        assert_eq!(options.schema.icd, Some(fenix_mib::Icd::V7), "7.x unless said");
        app.config.set_value("mib.icd", Some(fenix_config::Value::Text("6.9".into())));
        app.config.set_value("mib.xtce_fields", Some(fenix_config::Value::Map(vec![("pi1".into(), "SID, STRUCT".into())])));
        let options = key_here(&app).0.options;
        assert_eq!(options.schema.icd, Some(fenix_mib::Icd::V6_9));
        assert_eq!(options.xtce.pi1, vec!["SID".to_string(), "STRUCT".into()]);
        std::fs::write(
            p.0.join(".fenix").join("settings.toml"),
            "[mib]\nicd = \"auto\"\n\n[mib.roots]\nOPS = \"ops\"\n\n[mib.icd_per_mib]\nOPS = \"5.x\"\n\n[mib.columns]\nXYZ = \"xyz_a, xyz_b\"\n",
        )
        .unwrap();
        app.project_settings = Some((p.0.clone(), fenix_config::ProjectSettings::load(&p.0)));
        let key = key_here(&app).0;
        assert_eq!(key.options.schema.icd, None, "the project's auto wins over your 6.9");
        assert_eq!(key.options.schema.icd_for("OPS"), Some(fenix_mib::Icd::V5));
        assert_eq!(key.options.schema.columns, vec![("xyz".to_string(), vec!["XYZ_A".to_string(), "XYZ_B".into()])]);
        let set = MibSet::load_with(key.roots.clone(), None, key.options.clone());
        assert_eq!(set.root_info(0).format.as_deref(), Some("ICD 5.x"));
    }

    #[test]
    fn the_mib_page_lists_the_projects_definitions_and_opens_one() {
        let (_p, mut app) = project("page", "");
        app.with_mib(|m, a| m.cmd_page(a));
        let text = page_text(&mut app);
        assert!(text.contains("ZTC08101") && text.contains("ZTC17001"), "{text}");
        assert_eq!(app.buffer_display_name(app.focused_buffer_id()), "*mib*");
        press(&mut app, &[KeyPress::named(FenixNamedKey::Enter)]);
        assert_eq!(app.buffer_display_name(app.focused_buffer_id()), "*mib: ZTC08101*");
        let text = page_text(&mut app);
        assert!(text.contains("PTH00102") && text.contains("OFF ON AUTO"), "{text}");
    }

    #[test]
    fn the_insert_form_writes_the_call_where_the_cursor_was_and_remembers_it() {
        let (_p, mut app) = project("insert", "proc main {} {\n    \n}\n");
        set_cursor(&mut app, 19);
        let script = app.focused_buffer_id();
        let (key, _) = key_here(&app);
        let set = app.with_mib(|m, a| m.set(a, &key)).flatten().unwrap();
        // From the MIB page, where no file is in front.
        app.with_mib(|m, a| m.cmd_page(a));
        let tc = set.find(Kind::Telecommand, 0, "ZTC08101").unwrap();
        app.with_mib(|m, a| m.open_form(a, key, tc));
        assert!(page_text(&mut app).contains("Insert a telecommand"));
        let enter = KeyPress::named(FenixNamedKey::Enter);
        press(&mut app, &[enter, KeyPress::char('3'), enter, KeyPress::char('j'), KeyPress::char('h'), enter.with_ctrl()]);
        assert_eq!(app.focused_buffer_id(), script);
        assert_eq!(app.open().buffer.text(), "proc main {} {\n    tc::send ZTC08101 -PTH00101 3, -PTH00102 ON\n}\n");
        let remembered = app.mib().unwrap().last.values().next().unwrap();
        assert_eq!(remembered.get("PTH00102@").map(String::as_str), Some("ON"));
    }

    #[test]
    fn edit_call_reads_the_line_back_into_the_form_and_puts_it_back() {
        let (_p, mut app) = project("edit", "proc main {} {\n    tc::send ZTC08101 -PTH00101 3, -PTH00102 ON\n}\n");
        set_cursor(&mut app, 25);
        app.with_mib(|m, a| m.cmd_edit_call(a));
        assert!(page_text(&mut app).contains("Edit a telecommand call"));
        press(&mut app, &[KeyPress::char('j'), KeyPress::char('l'), KeyPress::named(FenixNamedKey::Enter).with_ctrl()]);
        assert_eq!(app.open().buffer.text(), "proc main {} {\n    tc::send ZTC08101 -PTH00101 3, -PTH00102 AUTO\n}\n");
    }

    #[test]
    fn k_gd_and_completion_know_the_projects_names() {
        let (_p, mut app) = project("editor", "tc::send ZTC08101\n");
        set_cursor(&mut app, 12);
        app.request_hover();
        let card = app.lsp_hover.clone().unwrap_or_default();
        assert!(card.contains("Set heater control mode") && card.contains("PTH00102"), "{card}");
        let names: Vec<String> = app.completion_candidates().into_iter().filter(|c| c.payload.source == crate::completion::Source::Mib).map(|c| c.label).collect();
        assert!(names.contains(&"ZTC17001".to_string()) && names.contains(&"NTH00123".to_string()), "{names:?}");
        let script = app.focused_buffer_id();
        app.request_goto_definition();
        assert_eq!(app.buffer_display_name(app.focused_buffer_id()), "*mib: ZTC08101*");

        // Only in the files it's asked for.
        app.config.set_value("mib.editor_files", Some(fenix_config::Value::List(vec!["py".into()])));
        app.open_buffer_in_focused_pane(script);
        set_cursor(&mut app, 12);
        app.lsp_hover = None;
        app.request_hover();
        assert!(app.lsp_hover.as_deref().is_none_or(|h| !h.contains("heater")), "not in a .tcl file now");
    }

    #[test]
    fn a_changed_table_is_read_again_on_the_disk_poll() {
        let (p, mut app) = project("poll", "");
        let (key, _) = key_here(&app);
        assert_eq!(app.with_mib(|m, a| m.set(a, &key)).flatten().unwrap().entries(Kind::Telecommand).len(), 2);
        let ccf = p.0.join("ops").join("ccf.dat");
        let mut text = std::fs::read_to_string(&ccf).unwrap();
        text.push_str("ZTC20003\tSet on-board parameter\t\t\tN\t\t20\t3\t1009\t0\n");
        std::fs::write(&ccf, text).unwrap();
        app.with_mib(|m, a| m.poll(a));
        assert_eq!(app.with_mib(|m, a| m.set(a, &key)).flatten().unwrap().entries(Kind::Telecommand).len(), 3);
    }

    #[test]
    fn words_carry_their_columns() {
        let w = words("  tc::send ZTC08101 -PTH00101 3");
        assert_eq!(w[0], (2..4, "tc"));
        assert_eq!(w[2], (11..19, "ZTC08101"));
        assert_eq!(w[3], (21..29, "PTH00101"));
    }

    // Through the `Feature` hooks, the way `App` reaches the MIB now.

    #[test]
    fn the_mib_contributes_its_commands_and_spc_m_keys_from_its_spec() {
        let registry = CommandRegistry::with_builtins();
        let mib: Vec<(&str, bool)> = registry.iter().filter(|c| c.id.starts_with("mib.")).map(|c| (c.id.as_ref(), c.external())).collect();
        assert_eq!(mib.len(), 13, "{mib:?}");
        for id in ["mib.page", "mib.search", "mib.generate"] {
            assert!(mib.contains(&(id, true)), "{id} stays external");
        }
        let menu = keymap::local_trie(&[crate::keymap::LocalContext::Tcl]);
        assert!(matches!(menu.matcher().feed(KeyPress::char('e')), fenix_keymap::Step::Matched(&"mib.edit_call")));
        assert!(matches!(menu.matcher().feed(KeyPress::char('s')), fenix_keymap::Step::Matched(&"code.symbols")), "Tcl's entries still there");
    }

    #[test]
    fn a_definition_picked_from_its_picker_opens_its_page() {
        let (_p, mut app) = project("picker", "");
        let (key, _) = key_here(&app);
        assert!(app.with_mib(|m, a| m.set(a, &key)).flatten().is_some(), "read inline without an event loop");
        app.with_mib(|m, a| m.cmd_pick(a, Kind::Telecommand));
        assert!(matches!(app.active_picker, Some(ActivePicker::Feature { feature: "fenix.mib", badge: "MIB", .. })));
        let Some(ActivePicker::Feature { state, .. }) = &mut app.active_picker else { unreachable!() };
        for c in "ZTC17001".chars() {
            state.push_char(c);
        }
        app.picker_confirm();
        assert!(app.feature_page::<DefPage>(app.focused_buffer_id()).is_some());
        assert!(page_text(&mut app).contains("ZTC17001"));
    }

    #[test]
    fn a_definition_page_is_named_and_laid_out_from_its_features_context_and_claims_ctrl_o() {
        let (_p, mut app) = project("def_page", "");
        let (key, _) = key_here(&app);
        let set = app.with_mib(|m, a| m.set(a, &key)).flatten().expect("read inline");
        let tc = set.resolve("ZTC17001").expect("in the sample MIB");
        open_def(&mut app, key, tc);
        let id = app.focused_buffer_id();
        assert_eq!(app.page_title(id), "*mib: ZTC17001*", "named through `page_title` once the set is read");
        app.ensure_page_layout(id, app.focused_pane_id(), 120);
        assert!(app.open().buffer.text().contains("ZTC17001"), "laid out with the set from `page_context`");
        let Some(PageModel::Feature(page)) = app.pages.get(&id).map(|s| &s.model) else { panic!("a feature page") };
        assert!(page.claims_ctrl(crate::page::Key::CtrlO) && !page.claims_ctrl(crate::page::Key::CtrlEnter));
        assert!(app.page_key(KeyPress::char('q')));
        assert!(!app.is_page_buffer(id), "q closed it, through `page_action`");
    }

    #[test]
    fn a_set_read_while_the_mib_is_busy_lands_once_it_is_back() {
        let (_p, mut app) = project("missed-event", "");
        // A key nothing has read yet (focusing the project read the usual one).
        let key = MibKey { default: Some("OTHER".to_string()), ..key_here(&app).0 };
        let set = Arc::new(MibSet::load_with(key.roots.clone(), key.default.as_deref(), key.options.clone()));
        app.with_mib(|m, a| {
            let event = super::super::feature::FeatureEvent { feature: SPEC.id, label: "MibLoaded", payload: Box::new((key.clone(), set)) };
            a.deliver_feature_event(event);
            assert!(m.set_ready(&key).is_none(), "not yet: the MIB is out");
        });
        assert!(app.mib().unwrap().set_ready(&key).is_some());
    }

    #[test]
    fn its_script_problems_show_with_the_files_diagnostics() {
        let (p, mut app) = project("diagnostics", "tc::send ZTC08101 -Line 99 -Mode AUTO
");
        let (key, _) = key_here(&app);
        let _ = app.with_mib(|m, a| m.set(a, &key));
        app.notify_features(Notice::Tick);
        let path = fenix_lsp::normalize(std::fs::canonicalize(p.0.join("checkout.tcl")).unwrap());
        let found: Vec<_> = app.features.iter().flat_map(|f| f.diagnostics(&path)).collect();
        assert!(!found.is_empty(), "the out-of-range line was flagged");
        assert!(found.iter().all(|d| d.source.as_deref() == Some("MIB")));
    }
}
