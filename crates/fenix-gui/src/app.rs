mod agents;
mod agent_calls;
mod agent_toolsets;
mod agent_lsp;
mod agent_debug;
mod agent_context;
mod agent_pipelines;
mod notifications;
mod control_agent;
mod refactor;
mod snippets;
mod tool_sessions;
mod session;
mod editor_ui;
mod mouse;
mod todos;
mod home;
mod tabs;
mod local_leader;
mod xml;
mod projects;
mod pages;
use pages::{PageModel, PageState};
mod git_page;
mod git_editor;
mod git_compare_page;
mod side_by_side;
mod rows;
mod row_scroll;
mod semantic;
mod code_lens;
mod server_install;
mod editorconfig_host;
mod inlay_hints;
use server_install::spawn_lsp_server;
mod git_log_page;
mod graph_draw;
mod git_rebase_page;
mod settings_host;
mod snippets_host;
mod tutorial_host;
mod gpu_host;
mod motion_host;
mod debug_host;
mod debug_drawer;
mod debug_trail;
mod debug_breakpoints;
mod panels;
mod palette;
mod terminal_view;
mod terminal_mouse;
mod terminal_keys;
pub(crate) use debug_host::{debug_dock_height, DOCK_TEXT_X, DOCK_TEXT_Y};
pub(crate) use terminal_view::{BOLD as TERM_BOLD, ITALIC as TERM_ITALIC};
pub(crate) use projects::window_title;
pub(crate) use debug_breakpoints::BreakpointPart;
mod polish;
mod which_key;
mod keys_host;
pub(crate) use keys_host::translate_leader;
pub(crate) use a11y_host::{adapter_for as a11y_adapter_for, wanted as a11y_wanted};
mod edit_commands;
mod carets;
mod updates;
mod auto_save;
mod problems;
mod a11y_host;
#[cfg(test)]
mod test_keys;
#[cfg(test)]
mod idle_tests;
#[cfg(test)]
mod standard_tests;
#[cfg(test)]
mod text_input_tests;
#[cfg(test)]
mod bidi_tests;
pub(crate) use edit_commands::SortLines;
mod file_index;
mod explorer_host;
mod explorer_watch;
mod git_views;
mod lsp_client;
mod debugger;
mod terminal_panel;
mod pickers;
mod completion_host;
mod render;
mod input;
mod tasks_runner;
mod page_select;
mod search_host;
pub(crate) use explorer_host::ExplorerTarget;
pub(crate) use search_host::SearchOpen;
pub(crate) use agent_context::PROMPTS as FENIX_PROMPTS;
use explorer_host::{ExplorerPurpose, ExplorerPrompt, SLOW_LISTING_AFTER, ExplorerRequest, ExplorerJob, ExplorerConflict};
use crate::explorer_page;
use crate::page::Key as PageKey;
#[cfg(test)]
use explorer_host::{is_bare_server, is_unc, PromptKind};
mod disk_probe;
mod remote;
mod trust;
mod extensions_host;
mod welcome_host;
mod import_host;
mod wasm_host;
mod tests_host;
pub(crate) use tests_host::TESTS_METHODS;
pub(crate) use wasm_host::image_key as code_image_key;
mod layers;
mod reading_host;
mod screenshot_host;
mod control_host;
mod startup;
use tool_sessions::{LspKey, LspServer};
use wasm_host::Happened;
use rows::FocusedOverlays;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use fenix_buffers::{BufferId, BufferKind, BufferList, OpenBuffer};
use fenix_explorer::{ExplorerAction, ExplorerState};
use fenix_keymap::{KeyCode, KeyPress, Matcher, Mods, NamedKey as FenixNamedKey, Step};
// `ScrollTarget` collides with this file's own hit-test enum of the same
// name (`Sidebar`/`Terminal`/`Pane` -- used by click/wheel routing), so
// the Vim one (`Center`/`Top`/`Bottom`, for `zz`/`zt`/`zb`) is aliased.
use fenix_vim::{Mode, ScrollTarget as VimScrollTarget, VimEvent, VimState, VisualKind};
use fenix_window::{NavDirection, SplitKind, WindowTree};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::monitor::MonitorHandle;
use winit::window::{Icon, Window, WindowId};

use fenix_core::{Buffer, Cursor};

use crate::commands::CommandRegistry;
use crate::completion;
use crate::dashboard;
use crate::diff_view;
use crate::git_panel;
use crate::gpu::{GpuContext, GpuState};
use crate::graph_view;
use crate::icon;
use crate::merge_view;
use crate::keymap;
use crate::markdown;
use crate::popup;
use crate::rect::RectRenderer;
use crate::tabstops::{self, TabStops};
use crate::text::{self, TextPipeline};
use crate::theme::{self, Theme};
use crate::surface::{Blend, SurfacePipeline};

const BLINK_INTERVAL: Duration = Duration::from_millis(500);
/// How long the caret blinks after the last key or click, before it
/// stays lit: as Emacs stops after its tenth blink, so an idle window
/// stops drawing.
const CARET_BLINKS_FOR: Duration = Duration::from_secs(30);
/// The shortest time between two frames drawn only because an
/// extension's layer moves (a pet walking): 30 frames a second, rather
/// than every frame the display shows.
const LAYER_TICK: Duration = Duration::from_millis(33);
/// The longest the loop sleeps with nothing due: other deadlines (the
/// disk check) come sooner anyway.
const IDLE_WAIT: Duration = Duration::from_secs(60);
/// The extensions that start with Fenix start once the window has drawn
/// its first frame (#33), or this long after Fenix started without one: a
/// window stuck on the splash (#65), or one never shown, still has them.
const STARTUP_EXTENSIONS_WITHIN: Duration = Duration::from_secs(3);
/// How long after the splash hands over the editor's first frame may
/// take before the window is taken to be stuck on the splash (#65).
const FIRST_FRAME_WITHIN: Duration = Duration::from_secs(10);
/// How long the caret takes to fade in/out at each blink toggle, instead
/// of flipping instantly.
const BLINK_FADE: Duration = Duration::from_millis(120);
/// Redraw cadence while an animation (blink fade, later scroll/pulse) is
/// actively transitioning. Idle time uses the much longer `WaitUntil`s
/// below instead -- see the plan's "animations are short bursts, not
/// continuous idle work" rationale.
const ANIM_TICK: Duration = Duration::from_millis(16);
/// How long a yank/paste pulse stays visible before fully fading out, by
/// default -- `motion.speed` scales the real one (`Prefs::pulse_d`).
/// Modeled on orbit-emacs's own yank/paste pulse feature.
#[cfg(test)]
const PULSE_DURATION: Duration = Duration::from_millis(300);
/// Alpha the pulse starts at before fading -- brighter than the steady
/// Visual-selection overlay so it reads as a distinct flash, not a
/// held selection.
const PULSE_PEAK_ALPHA: f32 = 0.45;
/// How long the viewport takes to ease to a new scroll position, by
/// default -- `motion.scroll_ms` sets the real one (`Prefs::scroll_d`).
#[cfg(test)]
const SCROLL_DURATION: Duration = Duration::from_millis(150);
/// How long a status/error message (`App::set_message`/`set_error`)
/// stays visible in the modeline before the normal filename/Ln/Col
/// display returns -- long enough to actually read a short sentence,
/// short enough not to hide real state (the cursor position, whether
/// the buffer's dirty) for long. No fade, unlike `Pulse` -- a message is
/// text you read, not a highlight you glance at; an abrupt cutoff reads
/// fine for text the way it wouldn't for a color wash.
const MESSAGE_DURATION: Duration = Duration::from_secs(4);
/// Real Vim's own `:set tabstop` default -- how many visual columns a
/// literal `\t` advances by when `config.tab_width` isn't set.
const DEFAULT_TAB_WIDTH: usize = 8;
/// Blank visual columns between a table view's elastic columns (`SPC f
/// t`), in addition to each column's own widest-value width.
const TABLE_COLUMN_GAP: usize = 2;
/// Total keystrokes a `@{register}` replay (including every nested
/// `@`-call it makes) may play before `App::play_macro` bails out --
/// real Vim itself would just hang forever on a self-referential macro
/// (`qaa@aq`) too; a GUI app shouldn't get to. Comfortably above
/// anything a legitimate macro (even one replaying itself many times
/// over a large file) would need, and fast enough that hitting it is
/// still instant.
const MACRO_REPLAY_BUDGET: u32 = 10_000;
/// Nested `@`-call depth a replay may reach before `App::play_macro`
/// bails out -- a *second*, independent guard from `MACRO_REPLAY_
/// BUDGET`: a self-referential macro (`qaa@aq`) recurses one Rust stack
/// frame deeper per replayed key, so the key-count budget alone would
/// let it recurse thousands of frames deep (each several calls through
/// `route_keypress`/`VimState::handle_key`) before ever tripping --
/// real recursion, real risk of overflowing the actual call stack, not
/// just spending time. Real Vim has the exact same guard for the exact
/// same reason (`E169: command too recursive`, `'maxmapdepth'`).
const MAX_MACRO_REPLAY_DEPTH: u32 = 100;

/// How long `:{range}!command` waits for its command.
const FILTER_TIMEOUT: Duration = Duration::from_secs(10);
/// Jumps larger than this many screens snap instantly instead of
/// animating. Panning smoothly through a huge jump (`G` on a large file)
/// would either blur past unreadably fast or need fetching far more
/// content than the viewport actually needs, fighting the whole point of
/// windowed rendering (`Buffer::visible_text` only ever fetches what's
/// on screen).
const SCROLL_SNAP_SCREENS: usize = 3;

/// Thickness of a themed window border (`Theme::border`), when the
/// active theme draws one. Comfortably inside `text::PAD_TOP` (4px) and
/// `text::PAD_LEFT` (8px) so it can never clip into body text.
const BORDER_WIDTH: f32 = 3.0;

/// How often the Docker and Git panels' background pollers tick --
/// generous enough to feel genuinely "live" without hammering `docker
/// stats`/`git status` (a real subprocess spawn) many times a second
/// for no perceptible benefit.
const STATS_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// How often the open files are checked against what's on disk.
///
/// Two seconds because the thing being caught is usually a command in
/// Fenix's own terminal panel -- `git checkout`, a formatter, a codegen
/// script -- and the gap between running it and looking at the file is
/// about that long. The check is a `stat` per open file, so the interval
/// could be far shorter without costing anything; it isn't, because a
/// buffer that reloads under the cursor the instant an editor elsewhere
/// writes its own temp file is worse than one that takes a moment.
const DISK_CHECK_INTERVAL: Duration = Duration::from_secs(2);

/// What two spellings of one folder or file have in common: the folder
/// it really is (a link followed, `/tmp` and `/private/tmp` one) and, on
/// Windows, in one case. A path that isn't there, is on a share or on
/// another machine is taken as written.
pub(crate) fn place_key(path: &Path) -> PathBuf {
    let local = !fenix_vfs::is_remote(path) && !explorer_host::is_unc(path);
    let real = if local { std::fs::canonicalize(path).ok() } else { None };
    fenix_core::path_key(real.as_deref().unwrap_or(path))
}

/// Whether `a` and `b` are in the same git repository (or, in none,
/// are the same folder).
pub(crate) fn same_repository(a: &Path, b: &Path) -> bool {
    let repository = |p: &Path| fenix_project::vcs::repository_root(p).map(|r| place_key(&r)).unwrap_or_else(|| place_key(p));
    repository(a) == repository(b)
}

/// Whether `path` is `root` or inside it, however each was spelled.
pub(crate) fn path_within(path: &Path, root: &Path) -> bool {
    path.starts_with(root) || place_key(path).starts_with(place_key(root))
}

/// How long a recovery snapshot is kept before being cleaned up.
///
/// Two weeks: long enough to cover a crash before a holiday, short
/// enough that the directory doesn't accumulate work from files that
/// were recovered by hand, renamed, or deleted outright -- none of
/// which Fenix ever hears about.
const RECOVERY_MAX_AGE: Duration = Duration::from_secs(60 * 60 * 24 * 14);
/// The stats-poller thread's stop check granularity -- it sleeps in
/// steps this short (rather than one `STATS_POLL_INTERVAL`-long sleep)
/// so a stop request (session closed, app quitting) lands within this
/// long, not up to a full 2s late.
const STATS_POLL_STOP_CHECK: Duration = Duration::from_millis(100);

/// Vertical padding inside the which-key popup, above its first row and
/// below its last -- factored into both its own height and `popup::
/// max_rows`'s "how many rows actually fit" calculation, so the two stay
/// consistent with each other.
const WHICH_KEY_PADDING: f32 = 8.0;

/// How much `SPC t =`/`Ctrl-=` and `SPC t -`/`Ctrl--` change the font
/// size by per press.
const FONT_SIZE_STEP: f32 = 2.0;

/// Same role as `WHICH_KEY_PADDING`, for the completion popup.
const COMPLETION_PADDING: f32 = 8.0;
/// Clear space kept between the completion popup and the window edges
/// it's clamped against -- same role as `text::WHICH_KEY_MARGIN`, just a
/// smaller value since the popup is anchored to a point inside the
/// content area rather than pinned to a corner.
const COMPLETION_MARGIN: f32 = 4.0;
/// Hard cap on how many candidate rows the popup ever shows at once,
/// regardless of how much vertical room is available -- most editors cap
/// completion popups similarly (a huge list is unwieldy even when there's
/// room to render it).
const COMPLETION_MAX_ROWS: usize = 10;

/// An active yank/paste highlight, fading out over `Prefs::pulse_d`.
struct Pulse {
    range: std::ops::Range<usize>,
    started: Instant,
}

/// A status/error message set via `App::set_message`/`set_error`, shown
/// in the modeline in place of the filename/Ln/Col display for
/// `MESSAGE_DURATION` -- vim's own "a message overwrites the command
/// line" convention. `is_error` picks the color (`modeline_error_color`
/// below); expiry is checked by timestamp on every redraw rather than
/// cleared by a timer, the same posture `Pulse`'s own fade already has.
struct StatusMessage {
    text: String,
    is_error: bool,
    set_at: Instant,
}

/// An in-flight scroll transition: eases `rendered_scroll` from `from`
/// to the (now-current) `scroll_line` target.
struct ScrollAnim {
    from: f32,
    to: usize,
    started: Instant,
}

/// One window pane's own live editing position -- cursor and scroll,
/// independent of whatever other pane(s) might be showing the same
/// buffer (see `fenix_buffers::OpenBuffer`'s own doc comment: `cursor`
/// there is just the buffer's *remembered* position, used only to seed
/// a pane's `PaneState` the first time that buffer is shown in it).
/// Lives on `Workspace`, not `App`, keyed by `fenix_window::WindowId` --
/// `WindowId`s are only unique *within* one `WindowTree`, not globally
/// (each tree's own internal counter starts fresh), so storing this
/// globally on `App` would let panes in different workspaces collide on
/// the same key. Scoping it to the `Workspace` that owns the tree those
/// ids came from rules that out entirely, and as a side benefit means
/// switching workspaces leaves every pane's cursor/scroll exactly where
/// it was.
#[derive(Clone, Copy)]
struct PaneState {
    cursor: Cursor,
    scroll_line: usize,
    rendered_scroll: f32,
    /// Leftmost visual column currently shown -- the horizontal analog
    /// of `scroll_line`, but with no `rendered_scroll`-style smooth-
    /// scroll animation (an instant snap is enough to fix "there's no
    /// horizontal scrolling at all"; a second full easing system for
    /// this axis isn't worth it yet). See `ensure_cursor_visible_
    /// horizontally`.
    scroll_col: usize,
    /// In a pane whose lines wrap: how many rows of the line at
    /// `scroll_line` are above the top.
    scroll_row: usize,
}

impl PaneState {
    fn seeded_at(cursor: Cursor) -> Self {
        Self { cursor, scroll_line: 0, rendered_scroll: 0.0, scroll_col: 0, scroll_row: 0 }
    }
}

/// The build/task runner's single-pane session (`SPC t t`) -- much
/// smaller than `DockerSession`'s six panes (see `BufferKind::
/// TaskOutput`'s own doc comment): one pane, one buffer, one live
/// runner. `root` is remembered so `SPC t r` (rerun) and a future
/// `[cmake] build_dir` (Milestone C's own stated follow-up) don't need
/// to re-derive it from whatever buffer happens to be focused by the
/// time either fires.
struct TaskSession {
    cwd: PathBuf,
    workspace_index: usize,
    pane: fenix_window::WindowId,
    buffer: BufferId,
    root: PathBuf,
    /// The live child, if a task is still running -- `None` once it
    /// finishes (`App::finish_task_run` drops it) or before the very
    /// first task in this session has been run. Held only for its
    /// cancellation-on-drop contract; all process and pipe ownership is in
    /// fenix-tasks, so closing a pane never joins a worker.
    runner: Option<fenix_tasks::TaskRunner>,
    run_id: Option<fenix_tasks::RunId>,
    /// When the run under way started, for its notification.
    started: Option<Instant>,
}

/// The History view's session (`SPC g l`) -- the commit graph, the refs
/// tree, and the selected commit's diff, in their own workspace.
///
/// Its own session rather than more panes on `GitSession`, per the
/// overhaul's "separate purpose-built views" shape: the working tree
/// and the history answer different questions and want different
/// layouts. Both can be open at once, on their own workspaces.
struct HistorySession {
    workspace_index: usize,
    graph_pane: fenix_window::WindowId,
    refs_pane: fenix_window::WindowId,
    detail_pane: fenix_window::WindowId,
    graph_buffer: BufferId,
    refs_buffer: BufferId,
    detail_buffer: BufferId,
    repo_root: PathBuf,
    /// Cached so the cursor's row resolves to a commit without
    /// re-shelling `git log` -- same reasoning as `GitSession::commits`.
    commits: Vec<fenix_git::GraphCommit>,
    /// The commit `history_sync_detail` last fetched a diff for, so
    /// moving the cursor within one commit's rows doesn't re-fetch.
    last_detail_commit: Option<String>,
    /// Staleness guard for that fetch -- mirrors `GitSession::
    /// main_request_id`.
    detail_request_id: u64,
    /// The same, for re-reading the graph and the refs.
    refresh_request_id: u64,
}

/// What the History view shows, read off the UI thread: on a large
/// repository the log and the refs' ahead/behind counts take a while.
#[derive(Debug)]
pub struct HistoryData {
    commits: Vec<fenix_git::GraphCommit>,
    branches: Vec<fenix_git::Branch>,
    remotes: Vec<String>,
    tags: Vec<String>,
    fetch_age: Option<u64>,
}

impl HistoryData {
    fn read(repo_root: &Path, limit: usize) -> Self {
        HistoryData {
            commits: fenix_git::commit_graph(repo_root, limit),
            branches: fenix_git::list_branches(repo_root),
            remotes: fenix_git::list_remote_branches(repo_root),
            tags: fenix_git::list_tags(repo_root),
            fetch_age: fenix_git::seconds_since_fetch(repo_root),
        }
    }
}

/// A cheap "has this file moved under us" check: modification time and
/// length, as of the last time Fenix read or wrote it.
///
/// Only ever used to decide whether the *real* check -- reading the file
/// and comparing it with the buffer -- is worth doing. Neither field is
/// trustworthy on its own: a filesystem with one-second timestamps hides
/// two edits in the same second, and a same-length rewrite is invisible
/// to `len`. Together they catch essentially everything, and whatever
/// they miss costs nothing, because the save guard reads the file
/// outright before writing over it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct DiskFingerprint {
    mtime: Option<std::time::SystemTime>,
    len: u64,
}

impl DiskFingerprint {
    /// `None` when the path can't be stat'd at all -- it was deleted,
    /// replaced by a directory, or was never there.
    fn of(path: &Path) -> Option<Self> {
        Self::look(path).flatten()
    }

    /// `of`, telling a file that isn't there (`Some(None)`) from one
    /// that couldn't be looked at (`None`): a file on a remote machine
    /// that isn't answering is not a file that's gone.
    fn look(path: &Path) -> Option<Option<Self>> {
        if !fenix_vfs::is_remote(path) {
            return Some(Self::from_meta(std::fs::metadata(path).ok()));
        }
        match fenix_vfs::metadata(path) {
            Ok(meta) => Some(meta.is_file().then(|| DiskFingerprint { mtime: Some(meta.modified()), len: meta.len })),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(None),
            Err(_) => None,
        }
    }

    /// `of`, from a stat already made (off the UI thread, by the disk probe).
    fn from_meta(meta: Option<std::fs::Metadata>) -> Option<Self> {
        let meta = meta?;
        if !meta.is_file() {
            return None;
        }
        Some(DiskFingerprint { mtime: meta.modified().ok(), len: meta.len() })
    }
}

/// What one sweep of the open files found.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct DiskSweep {
    /// Clean buffers whose file had changed; re-read in place.
    reloaded: Vec<String>,
    /// Buffers with unsaved edits whose file also changed. Left exactly
    /// as they are -- the edits are the user's -- and flagged so the
    /// next save refuses rather than silently discarding the other
    /// version.
    conflicted: Vec<String>,
    /// Files that stopped existing while open.
    vanished: Vec<String>,
}

impl DiskSweep {
    fn is_empty(&self) -> bool {
        self.reloaded.is_empty() && self.conflicted.is_empty() && self.vanished.is_empty()
    }

    /// One line naming what happened, or `None` when nothing did.
    ///
    /// Names the files rather than counting them: "1 file changed" makes
    /// you go looking, and the whole point is that you weren't expecting
    /// this.
    fn message(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        if !self.reloaded.is_empty() {
            parts.push(format!("reloaded {}", join_names(&self.reloaded)));
        }
        if !self.conflicted.is_empty() {
            parts.push(format!("{} changed on disk and has unsaved edits -- :e! takes theirs, :w! keeps yours", join_names(&self.conflicted)));
        }
        if !self.vanished.is_empty() {
            parts.push(format!("{} no longer exists on disk", join_names(&self.vanished)));
        }
        Some(parts.join("; "))
    }

    /// Whether this is bad news rather than housekeeping -- a reload is
    /// the expected outcome and shouldn't read as an error.
    fn is_bad(&self) -> bool {
        !self.conflicted.is_empty() || !self.vanished.is_empty()
    }
}

/// "4 minutes ago", "2 days ago" -- enough to tell a snapshot from
/// this morning's crash apart from one left behind months ago, without
/// a date-formatting dependency for a single label.
fn describe_age(age: Option<Duration>) -> String {
    let Some(age) = age else { return "at an unknown time".to_string() };
    let seconds = age.as_secs();
    match seconds {
        0..=90 => "just now".to_string(),
        91..=5400 => format!("{} minutes ago", seconds / 60),
        5401..=172_800 => format!("{} hours ago", seconds / 3600),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

/// `a`, `a and b`, `a, b and 2 more` -- a handful of names is more use
/// than a count, and a hundred of them is less.
fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [a, b, rest @ ..] => format!("{a}, {b} and {} more", rest.len()),
    }
}

/// What a compose buffer's text is going to be used for once it's
/// submitted.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ComposePurpose {
    /// A commit message. The one use of this buffer that has nothing to
    /// do with a forge -- and the reason the plan called for a compose
    /// buffer in the first place: a commit message is the most ordinary
    /// multi-line text an editor asks for, and typing one into a
    /// single-line prompt means no body, no bullet list, and no way to
    /// see what you have written.
    CommitMessage { repo_root: PathBuf },
    /// A commit, amend or reword from the Git status page -- the mode
    /// and flags its commit menu chose.
    GitCommit { repo_root: PathBuf, compose: crate::git_status::Compose },
    /// A reword's new message, for the rebase page `page`.
    RebaseMessage { page: BufferId, hash: String },
    /// A code extension's (`compose`): what's written goes back to it as
    /// `composed` for its call `id`.
    /// `ask`, when it has one, is what `A` asks your agent to write here.
    Code { ext: String, id: u64, what: String, verb: &'static str, allow_empty: bool, ask: Option<String> },
    /// What a pipeline's gate passes on, edited before it goes (`e`).
    PipelineGate { run: u64 },
}

/// Where an extension's compose pane `what` is kept unsent.
fn compose_draft_key(ext: &str, what: &str) -> String {
    format!("{ext}\u{1f}{what}")
}

/// The unsent compose text kept in `file`, by `compose_draft_key`.
fn read_compose_drafts(file: &Path) -> std::collections::BTreeMap<String, String> {
    std::fs::read_to_string(file).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn write_compose_drafts(file: &Path, drafts: &std::collections::BTreeMap<String, String>) {
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if drafts.is_empty() {
        let _ = std::fs::remove_file(file);
    } else if let Ok(text) = serde_json::to_string_pretty(drafts) {
        let _ = fenix_storage::write(file, text.as_bytes());
    }
}

/// Opens `url` in the default browser (an extension's `open-url`; a
/// test only records it).
#[cfg(not(test))]
fn open_in_browser(url: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    let mut command = {
        let mut c = std::process::Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    command.spawn().map(|_| ())
}

impl ComposePurpose {
    /// The pane title, which is also where the keys are documented --
    /// a compose buffer is the one surface here you type prose into, so
    /// "how do I send this" has to be visible without pressing anything.
    fn title(&self) -> String {
        let (what, verb) = match self {
            ComposePurpose::CommitMessage { .. } => ("Commit message".to_string(), "commit"),
            ComposePurpose::GitCommit { compose, .. } => match compose {
                crate::git_status::Compose::New(_) => ("Commit message".to_string(), "commit"),
                crate::git_status::Compose::Amend(_) => ("Amend the last commit".to_string(), "amend"),
                crate::git_status::Compose::Reword(_) => ("Reword the last commit".to_string(), "reword"),
            },
            ComposePurpose::RebaseMessage { hash, .. } => (format!("New message for {}", &hash[..7.min(hash.len())]), "keep"),
            ComposePurpose::Code { what, verb, .. } => (what.clone(), *verb),
            ComposePurpose::PipelineGate { .. } => ("What the pipeline passes on".to_string(), "go on (q stops)"),
        };
        let agent = if self.agent_writes() { ", A while it's untouched has the agent write it" } else { "" };
        format!("{what}  --  Esc then Enter to {verb}{agent}, q to cancel")
    }

    /// Whether `A` in the empty pane has the agent write it: a new
    /// commit's message (from what's staged), and what an extension gave
    /// an ask for. Not an amend or a reword: they start from a message,
    /// and what's staged isn't what they're about.
    fn agent_writes(&self) -> bool {
        match self {
            ComposePurpose::CommitMessage { .. } | ComposePurpose::GitCommit { compose: crate::git_status::Compose::New(_), .. } => true,
            ComposePurpose::Code { ask, .. } => ask.is_some(),
            _ => false,
        }
    }
}

/// A scratch buffer for writing something more than one line long.
///
/// Fenix's existing prompts are single-line, which is fine for a branch
/// name and wrong for a review comment. This is a real buffer in a real
/// pane: ordinary Vim editing, ordinary undo, and no modal dialog to
/// fight with -- submitted from Normal mode with `Enter`, abandoned
/// with `q`.
struct ComposeSession {
    pane: fenix_window::WindowId,
    buffer: BufferId,
    purpose: ComposePurpose,
    /// The pane focus returns to once this closes, so sending a reply
    /// puts you back where you were reading.
    returning_to: fenix_window::WindowId,
    /// The text it opened with: what's written past it, unsent, is kept
    /// (`App::keep_compose_draft`).
    seed: String,
    /// `seed` is only a starting point, a template: `A` writes over it
    /// until it's been touched. Never what you wrote and didn't send.
    agent_over_seed: bool,
}

/// The Merge view (`SPC g x`): the conflicted files on the left, the
/// selected one shown as two aligned columns on the right.
///
/// Separate from `GitSession` for the same reason History and Compare
/// are: resolving conflicts is a whole task with its own keys, not a
/// mode of the working-tree view, and it wants the full width for two
/// columns of file text.
struct MergeSession {
    workspace_index: usize,
    files_pane: fenix_window::WindowId,
    merge_pane: fenix_window::WindowId,
    files_buffer: BufferId,
    merge_buffer: BufferId,
    repo_root: PathBuf,
    /// Repo-relative paths git still reports as unmerged.
    files: Vec<String>,
    /// Which of them the right pane is showing.
    selected: Option<String>,
    /// The two sides as branches, resolved once per refresh --
    /// re-resolving per row would shell out to `git name-rev` for every
    /// keypress, and the answer can't change without a refresh anyway.
    labels: merge_view::SideLabels,
}

/// What a `BufferKind::Diff` buffer is currently showing. Cached
/// alongside the rendered text so a hunk action can rebuild that exact
/// hunk's patch (`fenix_diff::hunk_patch`) from the same parse the user
/// is looking at, rather than re-shelling `git diff` and trusting that
/// nothing moved in between -- the staleness that would otherwise let a
/// keypress stage a *different* hunk than the highlighted one.
struct DiffModel {
    repo_root: PathBuf,
    files: Vec<fenix_diff::FileDiff>,
    /// Display paths of files folded down to their header row.
    collapsed: HashSet<String>,
    source: DiffSource,
    /// Rows rendered above the diff (a commit's author, date and
    /// message). Stored rather than passed once, because folding a file
    /// re-renders the whole buffer -- without this, the first `Tab`
    /// silently dropped the header.
    header: Vec<(String, diff_view::DiffStyle)>,
    /// Review threads drawn inline under the lines they hang on. Stored
    /// here for exactly the reason `header` is: `Tab` re-renders the
    /// whole buffer, and threads passed only at build time would vanish
    /// the first time a file was folded.
    threads: Vec<diff_view::ThreadAnnotation>,
}

/// Which diff a `DiffModel` holds, and therefore what a hunk action on
/// it can mean: an unstaged hunk can be staged or discarded, a staged
/// one can only be unstaged, and a commit's or comparison's hunks
/// aren't editable at all (there's no index relationship to change).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffSource {
    /// `git diff` -- working tree vs. index.
    Unstaged,
    /// `git diff --cached` -- index vs. `HEAD`.
    Staged,
    /// A commit, a stash, or a ref-to-ref comparison.
    ReadOnly,
}

/// The tool status panel's single-pane session (`SPC l m`) -- smaller
/// even than `TaskSession`: no live process at all, since a `PATH` scan
/// is synchronous and fast enough to just redo outright on every open/
/// refresh (see `tool_status::is_on_path`'s own doc comment for why it
/// deliberately never spawns anything). Exists purely so a second
/// `SPC l m` reuses the same pane/buffer instead of piling up a new
/// workspace every time, the same "reuse if already open" posture
/// `run_task` already established for `TaskSession`.
struct ToolStatusSession {
    workspace_index: usize,
    pane: fenix_window::WindowId,
    buffer: BufferId,
}

/// The active DAP debug session (`SPC u u`, F5), if any -- shown in the
/// code (`debug_host`: the gutter, the stopped line, values inline), on
/// the modeline, and in the debug dock under the panes (`SPC u d`), which
/// draws from it each frame. No workspace or panes of its own. There's no
/// periodic poller: it changes on a `stopped`/`continued`/`terminated`
/// event or a request's own response landing, which happen exactly when
/// there's something new to show.
struct DebugSession {
    language: fenix_syntax::LanguageId,
    generation: u64,
    launch_attributes: serde_json::Map<String, serde_json::Value>,
    /// Sent as `attach`, not `launch` (a launch file's `request`).
    attach: bool,
    /// What it debugs, for its header and the extensions told of it
    /// (`debug-changed`).
    name: String,
    /// The adapter, and the port it's on when over a socket: where a
    /// child session it asks for (`startDebugging`) connects.
    adapter: crate::dap::Adapter,
    port: Option<u16>,
    /// The child sessions it asked for, in the order they came: js-debug
    /// runs each program in one.
    children: Vec<DebugChild>,
    /// The connection `stopped_thread` is on: this one's `generation`, or
    /// a child's -- where stepping, the stack and watches go.
    stopped_on: u64,
    /// Its own program's end has been told.
    ended: bool,
    client: fenix_dap::DapClient,
    /// `None` when opened without a real `event_proxy` (every test) --
    /// same posture as `LspSession::reader`. Held
    /// only for its `Drop` side effect once set.
    #[allow(dead_code)]
    reader: Option<DapReader>,
    /// What each still-outstanding request this app sent is actually
    /// *for*, keyed by the `seq` `DapClient::request` returned -- same
    /// role as `LspSession::pending`.
    pending: HashMap<i64, PendingDapRequest>,
    /// Project ownership for controls, breakpoints, and source-path resolution.
    root: PathBuf,
    /// Set once the adapter's `initialized` event has arrived -- gates
    /// sending `setBreakpoints`/`configurationDone`, which per spec
    /// can't go out before that (the adapter isn't necessarily ready to
    /// receive them until then, even though the `launch`/`attach`
    /// request that triggers it was already sent).
    initialized: bool,
    /// `Some(thread_id)` while stopped at a breakpoint/step/pause;
    /// `None` while running -- what `SPC u u`'s own dual role (start a
    /// session vs. continue an existing one) and the stepping actions
    /// key off.
    stopped_thread: Option<i32>,
    /// The thread it last told of (`thread` started, or stopped): what F6
    /// pauses.
    last_thread: Option<i32>,
    /// The current top stack frame's id once a `stackTrace` response
    /// has landed while stopped -- what `scopes`/`evaluate` (Watches)
    /// requests are issued against.
    current_frame_id: Option<i32>,
    /// One `(function name, file, 1-indexed line)` per frame in the
    /// last `stackTrace` response, top frame first -- what the Call
    /// Stack pane renders, and what `goto_debug_location` uses to move
    /// the cursor to the top frame's own location.
    call_stack: Vec<(String, PathBuf, usize)>,
    /// Each frame's id, beside `call_stack`: what selecting one in the
    /// drawer asks its scopes of.
    frame_ids: Vec<i32>,
    /// The frame the drawer has selected, 0 the top: whose values are
    /// written in the code and listed in the drawer.
    selected_frame: usize,
    /// The selected frame's scopes, in the order `scopes` gave them, each
    /// with its variables as far as they've been opened.
    variables: Vec<debug_drawer::DebugScope>,
    /// The scopes and variables opened or closed against their default,
    /// by path (`["Locals", "o", "items"]`): kept from stop to stop, so
    /// what you opened stays open.
    toggled: std::collections::HashSet<Vec<String>>,
    /// The drawer's console: the program's output and what the evaluate
    /// line was asked and said, the newest last.
    console: std::collections::VecDeque<String>,
    /// Watched expressions, in the order `SPC u w` added them --
    /// re-evaluated (`evaluate` request) after every stop.
    watches: Vec<String>,
    /// The last evaluated value for each of `watches`, keyed by the
    /// expression text itself -- `None` (rendered as `<not evaluated>`)
    /// until the first `evaluate` response for it lands, e.g. because
    /// the program hasn't stopped yet since it was added.
    watch_values: HashMap<String, String>,
    /// How it ended, once it has: the exit code `exited` gave, or why the
    /// adapter refused or went away -- shown in the Call Stack pane.
    end: Option<DebugEnd>,
    /// The program's last lines of output (`output` events, stdout and
    /// stderr), shown once it's over: what a program that died at once
    /// said about why.
    output_tail: std::collections::VecDeque<String>,
    /// `SPC u q` closed its adapter's connection.
    client_closed: bool,
    /// The exceptions its adapter can stop on (`initialize`'s answer).
    exception_filters: Vec<debug_breakpoints::ExceptionFilter>,
    /// The pins' values, and whether each is this stop's (fresh) or one
    /// before's.
    pin_values: HashMap<debug_breakpoints::Pin, (String, bool)>,
    /// The connection that last told of a thread running: what F6 pauses,
    /// a child's when js-debug runs the program in one.
    running_on: u64,
}

/// How many lines of a debugged program's output the panel keeps.
const DEBUG_OUTPUT_TAIL: usize = 12;

/// Why a debug session is over.
#[derive(Debug, Clone, PartialEq)]
enum DebugEnd {
    /// The program exited (with its code, when `exited` gave one).
    Exited(Option<i64>),
    /// The adapter refused `initialize`, `launch` or `attach`.
    Refused(String),
    /// The adapter's connection went.
    Disconnected(String),
    /// `SPC u q` stopped it.
    Stopped,
}

/// A child session an adapter asked for (`startDebugging`): a connection
/// of its own to the same adapter, launched with the configuration it
/// gave, under the session it came from in the Call Stack pane.
struct DebugChild {
    generation: u64,
    /// The connection that asked for it.
    parent: u64,
    name: String,
    client: fenix_dap::DapClient,
    /// Held only for its `Drop` side effect, as `DebugSession::reader`.
    #[allow(dead_code)]
    reader: Option<DapReader>,
    pending: HashMap<i64, PendingDapRequest>,
    configuration: serde_json::Map<String, serde_json::Value>,
    attach: bool,
    initialized: bool,
}

impl DebugSession {
    /// Connection `on`'s client: this one's, or a child's.
    fn client_of(&self, on: u64) -> Option<&fenix_dap::DapClient> {
        if on == self.generation {
            return Some(&self.client);
        }
        self.children.iter().find(|c| c.generation == on).map(|c| &c.client)
    }

    fn pending_of(&mut self, on: u64) -> Option<&mut HashMap<i64, PendingDapRequest>> {
        if on == self.generation {
            return Some(&mut self.pending);
        }
        self.children.iter_mut().find(|c| c.generation == on).map(|c| &mut c.pending)
    }

    /// `req` sent on connection `on`, and what it's for kept: false when
    /// it couldn't be.
    fn send(&mut self, on: u64, req: fenix_dap::requests::Request, what: PendingDapRequest) -> bool {
        let Some(Ok(seq)) = self.client_of(on).map(|c| c.request(req)) else { return false };
        if let Some(pending) = self.pending_of(on) {
            pending.insert(seq, what);
        }
        true
    }
}

impl Drop for DebugSession {
    /// Its children disconnected before it is: the newest first.
    fn drop(&mut self) {
        while let Some(child) = self.children.pop() {
            drop(child);
        }
    }
}

/// `initialize`'s arguments: what the crate's type holds, and that child
/// sessions are started when asked (`supportsStartDebuggingRequest`, which
/// it has no room for).
fn debug_initialize_arguments() -> serde_json::Value {
    let args = fenix_dap::requests::InitializeRequestArguments::builder().adapter_id("fenix".to_string()).client_id(Some("fenix".to_string())).build();
    let mut value = serde_json::to_value(args).unwrap_or_else(|_| serde_json::json!({ "adapterID": "fenix", "clientID": "fenix" }));
    value["supportsStartDebuggingRequest"] = serde_json::Value::Bool(true);
    value
}

/// A debug session waiting for its `preLaunchTask` to succeed: what it
/// starts with then.
struct PendingLaunch {
    language: fenix_syntax::LanguageId,
    root: PathBuf,
    adapter: crate::dap::Adapter,
    attributes: serde_json::Map<String, serde_json::Value>,
    attach: bool,
    what: String,
}

/// What an in-flight DAP request was for -- see `DebugSession::pending`'s
/// own doc comment. Only requests whose *response* needs its own
/// handling get a variant here -- `continue`/`next`/`stepIn`/`stepOut`/
/// `pause` don't (the state change they cause arrives via a `stopped`/
/// `continued` *event* instead, not their own response, which is just a
/// bare acknowledgement).
#[derive(Debug, Clone)]
enum PendingDapRequest {
    Initialize,
    Launch,
    /// Its failure ends the session: netcoredbg refuses it when the
    /// program it was told to run didn't start.
    ConfigurationDone,
    SetBreakpoints,
    StackTrace,
    /// The scopes of frame `frame`: dropped when another's been selected
    /// since.
    Scopes { frame: i32 },
    /// The variables under `path` (a scope's name, then variables' names)
    /// in frame `frame`.
    Variables { frame: i32, path: Vec<String> },
    /// A watch's value.
    Evaluate { expression: String },
    /// The evaluate line's, for the console.
    Repl { expression: String },
    /// The lens's (`K`), for its card.
    Lens { expression: String },
    /// The children of what the lens shows, under `card` once they come.
    LensChildren { card: String },
    /// `fenix/debug.evaluate`'s, answered to its client.
    Control { reply: std::sync::mpsc::Sender<crate::ipc::Reply> },
    /// A pin's value.
    Pin { pin: debug_breakpoints::Pin },
}

/// The Call Stack pane's content -- one `name  file:line` row per frame
/// (top first), or a placeholder while running/before the first stop.
fn render_call_stack_text(session: &DebugSession) -> String {
    let mut out = render_debug_sessions(session);
    if let Some(end) = &session.end {
        out.push_str(&match end {
            DebugEnd::Exited(Some(code)) => format!("(program ended, exit code {code})\n"),
            DebugEnd::Exited(None) => "(program ended)\n".to_string(),
            DebugEnd::Refused(why) => format!("(the debug adapter refused: {why})\n"),
            DebugEnd::Disconnected(why) => format!("(adapter disconnected: {why})\n"),
            DebugEnd::Stopped => "(stopped)\n".to_string(),
        });
        if !session.output_tail.is_empty() {
            out.push_str("\nits last output:\n");
            for line in &session.output_tail {
                out.push_str(&format!("  {line}\n"));
            }
        }
        out.push_str("\nSPC u u starts another session\n");
        return out;
    }
    if session.stopped_thread.is_none() {
        return out + "(running)\n";
    }
    if session.call_stack.is_empty() {
        return out + "(no stack)\n";
    }
    for (name, path, line) in &session.call_stack {
        if path.as_os_str().is_empty() {
            out.push_str(&format!("{name}\n"));
        } else {
            out.push_str(&format!("{name}  {}:{line}\n", path.display()));
        }
    }
    out
}

/// The sessions, when it has children (js-debug's, a program each): each
/// under the one that asked for it, the one stopped marked. Nothing for a
/// session alone.
fn render_debug_sessions(session: &DebugSession) -> String {
    if session.children.is_empty() {
        return String::new();
    }
    let mark = |on: u64| if session.stopped_thread.is_some() && session.stopped_on == on { "paused" } else { "running" };
    let mut out = format!("{} ({})\n", session.name, mark(session.generation));
    fn under(out: &mut String, session: &DebugSession, parent: u64, depth: usize, mark: &dyn Fn(u64) -> &'static str) {
        for child in session.children.iter().filter(|c| c.parent == parent) {
            out.push_str(&format!("{}{} ({})\n", "  ".repeat(depth), child.name, mark(child.generation)));
            under(out, session, child.generation, depth + 1, mark);
        }
    }
    under(&mut out, session, session.generation, 1, &mark);
    out.push('\n');
    out
}

/// Which of Fenix's shells a chunk of PTY output, or a finished spawn,
/// belongs to.
///
/// There are two kinds, deliberately, because they answer two different
/// questions. The **panel** is the one shell you keep around: a strip
/// that drops down over whatever you are looking at, follows you from
/// workspace to workspace, and keeps running while hidden -- for the
/// `cargo build` you start, glance at, and dismiss. A **buffer**
/// terminal is a shell that belongs *somewhere*: it lives in a pane, in
/// one workspace, beside the code it goes with, and stays there while
/// you switch away and back. Neither substitutes for the other, and a
/// single shared session could not be both places at once.
///
/// Everything downstream of the PTY is common to the two -- the same
/// `fenix_terminal::Terminal`, the same reader thread, the same
/// `vt100` screen rendering -- so the split is carried as this tag on
/// the events rather than as two parallel sets of plumbing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum TerminalTarget {
    /// The popup panel (`SPC o t`) -- one per application, shared by
    /// every workspace and every frame.
    Panel,
    /// One of the panel's shells by its id (`App::terminal_shell`): the
    /// one shown, or another project's, kept running while you're
    /// elsewhere (`App::parked_terminals`).
    PanelShell(u64),
    /// One pane-resident shell (`SPC o T`), identified by its own
    /// buffer -- see `BufferKind::Terminal` for why a buffer, and not a
    /// pane, is the identity.
    Buffer(BufferId),
}

/// Streams raw PTY output bytes for one shell into
/// `FenixUserEvent::TerminalOutput` -- same "background thread, stopped
/// via a kill that unblocks its blocking read" shape as `DockerLog
/// Follower`, just forwarding raw bytes instead of pre-split lines
/// (there's no line discipline to split on here -- an interactive
/// shell's output is arbitrary ANSI, not newline-delimited log lines).
/// Deliberately does *not* own the `fenix_terminal::Terminal` itself
/// (only its reader half) -- the parser stays single-threaded, updated
/// only from the main thread in `App::apply_terminal_output`, so no
/// `Arc<Mutex<_>>` is needed anywhere in this path.
struct TerminalReader {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl TerminalReader {
    fn spawn(
        target: TerminalTarget,
        mut reader: Box<dyn std::io::Read + Send>,
        send: impl Fn(FenixUserEvent) -> bool + Send + 'static,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let handle = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                if thread_stop.load(Ordering::Relaxed) {
                    return;
                }
                match reader.read(&mut buf) {
                    Ok(0) => return,
                    Ok(n) => {
                        if thread_stop.load(Ordering::Relaxed) {
                            return;
                        }
                        if !send(FenixUserEvent::TerminalOutput(target, buf[..n].to_vec())) {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
        });
        Self { stop, handle: Some(handle) }
    }
}

impl Drop for TerminalReader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Note: unlike `DockerLogFollower`, killing the shell itself
        // isn't this type's job -- `TerminalState`'s own teardown kills
        // `session` first (see its own `Drop`), which is what actually
        // unblocks this thread's blocking read via EOF/error.
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// One live shell -- the PTY, its parsed screen, and the reader thread
/// feeding it -- with no opinion about which kind of terminal owns it.
/// The popup panel keeps one of these in `App::terminal`; each pane-
/// resident terminal keeps its own in `App::terminal_buffers`, keyed by
/// its buffer.
///
/// Never dropped just because the thing showing it went away: hiding
/// the panel (`App::terminal_open` is purely a rendering/layout
/// concern) and pointing a pane at another buffer both leave the shell
/// running, which is the entire point -- a build you started keeps
/// building. The panel's session is only replaced when the shell has
/// exited and the panel is opened again; a buffer's is dropped when its
/// buffer is closed, which is the one gesture that says the shell has
/// no owner left.
struct TerminalState {
    /// Taken apart only by `Drop`, which hands it to a thread of its own.
    session: std::mem::ManuallyDrop<fenix_terminal::Terminal>,
    /// `None` only in tests that read the shell's output themselves
    /// (`pump_until`); every spawn through `App` gets one, see
    /// `App::spawn_terminal_reader`. Never read again once set -- held
    /// only for its `Drop` side effect, same idiom as
    /// `stats_poller`/`log_follower`.
    #[allow(dead_code)]
    reader: Option<TerminalReader>,
}

impl TerminalState {
    fn new(session: fenix_terminal::Terminal, reader: Option<TerminalReader>) -> Self {
        Self { session: std::mem::ManuallyDrop::new(session), reader }
    }
}

impl Drop for TerminalState {
    fn drop(&mut self) {
        // Order matters: killing the shell first is what unblocks
        // `reader`'s own blocking read (see `TerminalReader::drop`'s own
        // note) -- dropping fields in the other order would leave
        // `reader`'s `Drop` waiting to join a thread that can never wake
        // up on its own.
        self.session.kill();
        // The rest happens off this thread. Until Windows 11 24H2,
        // closing a pseudoconsole blocks until its console host exits,
        // and with another shell still open that can take arbitrarily
        // long -- a UI thread closing a terminal would freeze with it.
        // SAFETY: `session` is never touched again after this.
        let session = unsafe { std::mem::ManuallyDrop::take(&mut self.session) };
        let reader = self.reader.take();
        // If no thread can be started, `spawn` drops the closure right
        // here, which closes the session on this thread after all.
        let _ = std::thread::Builder::new()
            .name("fenix-terminal-close".into())
            .spawn(move || drop((session, reader)));
    }
}

/// One running language server in one canonical project root: a
/// language's primary server, or an additional one serving several
/// languages (see `LspKey`). Unlike `PdfSession`/etc., this has no pane/buffer/
/// workspace of its own -- a language server has no UI surface by
/// itself; it's attached to whichever ordinary text buffers share its
/// language, feeding diagnostics/completion/hover/etc. into panes that
/// already exist for other reasons.
struct LspSession {
    generation: u64,
    client: fenix_lsp::LspClient,
    /// What it's called in messages and listings: an additional server's
    /// configured name, or the program a primary server runs.
    name: String,
    /// The language names (`lsp::language_config_name`) whose buffers it
    /// is told about.
    languages: Vec<String>,
    /// The project root this session was spawned for (`ensure_lsp_
    /// session`'s own `cwd`) -- kept around so the `initialize`
    /// response handler can run a per-language environment resolver
    /// (`fenix_lsp::per_language`) against the same root without
    /// needing to re-derive it from whatever buffer happens to be
    /// focused by the time that response actually arrives.
    root: PathBuf,
    /// `None` when opened without a real `event_proxy` (every test) --
    /// same posture as `TerminalState::reader`.
    /// Held only for its `Drop` side effect once set.
    #[allow(dead_code)]
    reader: Option<LspReader>,
    /// `None` until the `initialize` response has actually landed --
    /// no request beyond `initialize` itself is sent before this, per
    /// spec (a server hasn't necessarily finished setting up its own
    /// state to answer anything else yet).
    capabilities: Option<lsp_types::ServerCapabilities>,
    /// When it was started: one that hasn't answered `initialize`
    /// `LSP_INITIALIZE_TIMEOUT` later is given up on.
    started: Instant,
    /// Canonicalized paths that have had `textDocument/didOpen` sent to
    /// this server, each mapped to `(last_synced_edit_count, next_
    /// version)` -- `last_synced_edit_count` is `Buffer::edit_count()`'s
    /// value as of the last successful `didOpen`/`didChange` for this
    /// path (compared against the buffer's *current* count every frame
    /// to decide whether another `didChange` is due, see `App::sync_lsp_
    /// for_focused_buffer`); `next_version` is the LSP document version
    /// to send on the next `didChange` (the spec requires a
    /// monotonically increasing version per document). Also doubles as
    /// "does this server already know about this path" -- so a second
    /// buffer for the same path doesn't double-open it, and `didClose`
    /// only fires for a path this server actually knows about.
    open_documents: HashMap<PathBuf, (u64, i32)>,
    /// The text this server was last sent for each open document -- what
    /// an incremental `didChange` is worked out against (see
    /// `fenix_lsp::change_event`). Kept for every server, whichever sync
    /// kind it asked for, so the choice lives in one place.
    synced_text: HashMap<PathBuf, String>,
    /// What each still-outstanding request this app sent is actually
    /// *for*, keyed by the id `LspClient::request` returned -- a
    /// response arrives bearing only that id (per JSON-RPC), so this is
    /// what lets `apply_lsp_event`'s `Response` arm route it to the
    /// right handling instead of every request needing its own bespoke
    /// "remember what I asked" side channel. Removed once answered
    /// (or once the connection that would have answered it is gone).
    pending: HashMap<i64, PendingLspRequest>,
    /// For a Python server: the environment it was last told about and
    /// the stamp it had then -- `refresh_python_environments` tells it
    /// again when the stamp moves (a package added, the venv created).
    python_env: Option<fenix_lsp::per_language::python::EnvironmentStamp>,
    /// For a server that's asked for its diagnostics
    /// (`textDocument/diagnostic`), each document's: its last
    /// result id, and whether an ask is out and another is due after it.
    pulls: HashMap<PathBuf, LspPull>,
    /// Each document's semantic tokens, from the one server of its asked
    /// for them (`App::semantic_server`).
    semantic: HashMap<PathBuf, semantic::SemanticDoc>,
    /// Each document's code lenses from this server.
    lenses: HashMap<PathBuf, code_lens::LensDoc>,
}

/// Why `SPC u u` can't debug a `language` file: no adapter from the
/// project, an extension or Fenix -- and where to look, since the usual
/// reasons are a tool not on `PATH` or something changed since start.
fn no_debug_adapter(language: fenix_syntax::LanguageId) -> String {
    let name = crate::lsp::language_config_name(language);
    format!(
        "no debug adapter for {name}: the project's .fenix/tools.json names none, no extension running gives one, and Fenix has none built in \
         -- an extension's adapter needs its tool on PATH (SPC p h, the project doctor, names what's missing), and SPC h l shows what the extension said as it started"
    )
}

/// How long a language server has to answer `initialize` before it's
/// given up on, said to be failing, and stopped: one that never will
/// (typescript-language-server with no `tsserver.js`) otherwise shows
/// "starting" for good.
const LSP_INITIALIZE_TIMEOUT: Duration = Duration::from_secs(30);

/// `, last said: <its last line>` of what a server wrote to stderr, for
/// a one-line message; empty when it said nothing.
fn last_line_of(said: &str) -> String {
    said.lines().rev().map(str::trim).find(|l| !l.is_empty()).map(|l| format!(" -- it said: {l}")).unwrap_or_default()
}

/// A failed server's row in Tool Status: why, then the last lines it
/// wrote to stderr.
fn failure_text(why: &str, said: &str) -> String {
    let lines: Vec<&str> = said.lines().filter(|l| !l.trim().is_empty()).collect();
    let last = &lines[lines.len().saturating_sub(8)..];
    if last.is_empty() {
        return format!("{why} (it wrote nothing to stderr)");
    }
    format!("{why}\n{}", last.iter().map(|l| format!("  | {l}")).collect::<Vec<_>>().join("\n"))
}

/// One document's diagnostics asked of a server (`LspSession::pulls`).
#[derive(Debug, Default)]
struct LspPull {
    result_id: Option<String>,
    asking: bool,
    again: bool,
}

/// Asks `session` for `canonical`'s diagnostics, if it answers that way:
/// once an ask is out, the next waits for its answer, so typing doesn't
/// pile them up.
fn pull_lsp_diagnostics(session: &mut LspSession, canonical: &Path) {
    if !session.capabilities.as_ref().is_some_and(fenix_lsp::pull::pulls) || !session.open_documents.contains_key(canonical) {
        return;
    }
    let pull = session.pulls.entry(canonical.to_path_buf()).or_default();
    if pull.asking {
        pull.again = true;
        return;
    }
    let Some(uri) = fenix_lsp::path_to_uri(canonical) else { return };
    let params = lsp_types::DocumentDiagnosticParams {
        text_document: lsp_types::TextDocumentIdentifier { uri },
        identifier: None,
        previous_result_id: pull.result_id.clone(),
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };
    if let Ok(id) = session.client.request::<lsp_types::request::DocumentDiagnosticRequest>(params) {
        pull.asking = true;
        pull.again = false;
        session.pending.insert(id, PendingLspRequest::PullDiagnostics { path: canonical.to_path_buf() });
    }
}

/// Tells a Python server which interpreter the project uses (so it
/// looks in that venv's `site-packages`), and remembers the stamp it
/// was told at.
fn push_python_environment(session: &mut LspSession) {
    use fenix_lsp::per_language::python;
    let env = python::resolve(&session.root);
    session.python_env = Some(python::environment_stamp(&session.root, &env));
    let settings = serde_json::json!({ "python": { "pythonPath": env.interpreter.to_string_lossy() } });
    let _ = session.client.notify::<lsp_types::notification::DidChangeConfiguration>(lsp_types::DidChangeConfigurationParams { settings });
}

/// Brings one server's picture of one document up to date: `didOpen` the
/// first time (once the server is initialized, and only if it serves
/// `language`), `didChange` whenever the buffer was edited since.
///
/// `semantic`: what it means for this server's semantic tokens of the
/// document (`semantic::after_sync`).
///
/// The edit count alone doesn't say the server has the text: a buffer
/// opened again starts its count afresh, over a file that may have
/// changed on disk meanwhile. So the text is compared as well.
#[allow(clippy::too_many_arguments)]
fn sync_lsp_document(session: &mut LspSession, canonical: &Path, language_id: &str, by_pattern: bool, text: &str, edit_count: u64, semantic: semantic::SemanticSync, lens: semantic::SemanticSync) {
    if session.capabilities.is_none() {
        return;
    }
    // A server an extension is serves files by pattern too, whatever
    // their language (`contributes.language_servers` `files`).
    if !by_pattern && !session.languages.iter().any(|l| l == language_id) {
        return;
    }
    let language_id = language_id.to_string();
    let Some(uri) = fenix_lsp::path_to_uri(canonical) else { return };
    match session.open_documents.get(canonical).copied() {
        None => {
            let params = lsp_types::DidOpenTextDocumentParams { text_document: lsp_types::TextDocumentItem { uri, language_id, version: 0, text: text.to_string() } };
            if session.client.notify::<lsp_types::notification::DidOpenTextDocument>(params).is_ok() {
                record_lsp_sync(session, canonical, text, edit_count, 1, semantic, lens);
                pull_lsp_diagnostics(session, canonical);
            }
        }
        Some((last_synced_edit_count, next_version)) if last_synced_edit_count != edit_count || session.synced_text.get(canonical).is_none_or(|t| t != text) => {
            let incremental = session.capabilities.as_ref().is_some_and(fenix_lsp::wants_incremental);
            let change = fenix_lsp::change_event(session.synced_text.get(canonical).map(String::as_str), text, incremental);
            let params = lsp_types::DidChangeTextDocumentParams {
                text_document: lsp_types::VersionedTextDocumentIdentifier { uri, version: next_version },
                content_changes: vec![change],
            };
            if session.client.notify::<lsp_types::notification::DidChangeTextDocument>(params).is_ok() {
                record_lsp_sync(session, canonical, text, edit_count, next_version + 1, semantic, lens);
                pull_lsp_diagnostics(session, canonical);
            }
        }
        // Already open and already in sync -- the common case, most
        // frames; its tokens asked for if it's become the server asked
        // for this buffer (one before it stopped, the buffer opened
        // again), let go if it no longer is.
        Some(_) => {
            let held = session.semantic.get(canonical).map(|d| d.buffer);
            let redo = match semantic {
                semantic::SemanticSync::Ask(buffer) => held != Some(buffer),
                semantic::SemanticSync::Drop => held.is_some(),
                semantic::SemanticSync::Follow => false,
            };
            if redo {
                semantic::after_sync(session, canonical, None, text, edit_count, semantic);
            }
            let held = session.lenses.get(canonical).map(|d| d.buffer);
            let redo = match lens {
                semantic::SemanticSync::Ask(buffer) => held != Some(buffer),
                semantic::SemanticSync::Drop => held.is_some(),
                semantic::SemanticSync::Follow => false,
            };
            if redo {
                code_lens::after_sync(session, canonical, None, text, lens);
            }
        }
    }
}

/// What `session` was just sent of `canonical` (`didOpen` or
/// `didChange`): its text at `edit_count`, the next version to send. The
/// one place that's recorded, so what's held by position (semantic
/// tokens) always moves with it -- every writer of `synced_text` and
/// `open_documents` comes through here.
fn record_lsp_sync(session: &mut LspSession, canonical: &Path, text: &str, edit_count: u64, next_version: i32, semantic: semantic::SemanticSync, lens: semantic::SemanticSync) {
    let old_count = session.open_documents.insert(canonical.to_path_buf(), (edit_count, next_version)).map(|(count, _)| count);
    let old = session.synced_text.insert(canonical.to_path_buf(), text.to_string());
    let change = old.as_deref().zip(old_count);
    semantic::after_sync(session, canonical, change, text, edit_count, semantic);
    // Code lenses, which move by line, likewise.
    code_lens::after_sync(session, canonical, old.as_deref(), text, lens);
}

/// What an in-flight LSP request was for -- see `LspSession::pending`'s
/// own doc comment. One variant per request kind this app actually
/// sends; each carries whatever its own response handling needs beyond
/// the response body itself (which buffer to act on, mainly, since a
/// slow response can outlast the focus having moved elsewhere).
///
/// Requests sent to several servers at once carry the id of their
/// `LspFanout`, where the answers are gathered.
#[derive(Debug, Clone)]
enum PendingLspRequest {
    Initialize,
    Hover { fanout: u64 },
    Definition { fanout: u64 },
    References { fanout: u64 },
    Rename { buffer: BufferId, context: refactor::Context },
    CodeAction { fanout: u64 },
    Format { buffer: BufferId, revision: u64 },
    Completion { buffer: BufferId, prefix_start: usize, generation: u64 },
    /// `textDocument/diagnostic` for the document at `path` (canonical).
    PullDiagnostics { path: PathBuf },
    /// `textDocument/semanticTokens/*` for `path` (canonical) as it was
    /// at `edit_count`: the whole of it, a delta or a range (`how`), by
    /// request `request`.
    SemanticTokens { path: PathBuf, edit_count: u64, how: semantic::SemanticAsk, request: i64 },
    /// `textDocument/codeLens` for `path` (canonical) as it was at
    /// `edit_count`, by request `request`.
    CodeLens { path: PathBuf, edit_count: u64, request: i64 },
    /// `textDocument/inlayHint` for buffer `buffer`'s lines in view, by
    /// request `request`.
    InlayHint { buffer: BufferId, request: i64 },
    /// `codeLens/resolve` for lens `index` of `path`'s answer `generation`.
    CodeLensResolve { path: PathBuf, generation: u64, index: usize },
    /// `workspace/executeCommand` for the lens titled `title`.
    LensCommand { title: String },
    /// `workspace/symbol`, for the project's procs a Tcl call is colored
    /// by (`App::known_commands`).
    WorkspaceNames,
    /// `fenix/lsp`'s, for its client (`LspFanout::Control`).
    Control { fanout: u64 },
    /// `workspace/symbol`, for the symbol picker's question `generation`.
    ProjectSymbols { generation: u64 },
    /// An agent's, through Fenix's tools (`agent_lsp`).
    Agent { id: u64 },
}

/// The project whose language server settings `path` is
/// (`.fenix/lsp/<name>.json`), if it's one.
fn lsp_settings_file(path: &Path) -> Option<PathBuf> {
    let lsp = path.parent().filter(|_| path.extension().is_some_and(|x| x == "json"))?;
    let fenix = lsp.parent().filter(|_| lsp.file_name().is_some_and(|n| n == "lsp"))?;
    fenix.file_name().is_some_and(|n| n == ".fenix").then(|| fenix.parent().map(Path::to_path_buf)).flatten()
}

/// An extension's language server, as its session is named: the
/// extension, then the server.
fn code_server_name(ext: &str, server: &str) -> String {
    format!("{ext}:{server}")
}

/// Whether `path` is a Tcl file, as its highlighting is chosen.
fn is_tcl_path(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).and_then(fenix_syntax::detect_language) == Some(fenix_syntax::LanguageId::Tcl)
}

/// `buffer`'s text made `new` by replacing only the lines that differ,
/// the last first, as one undo step -- so what's anchored in the lines
/// that didn't change (extra cursors, marks, an extension's decorations)
/// stays where it was. `cursor` is moved as the text around it moved;
/// in a line that was replaced, it keeps its line and column (or the
/// line's end). The syntax tree hears of it as one edit.
pub(crate) fn replace_text_by_lines(buffer: &mut Buffer, cursor: &mut Cursor, new: &str) {
    let old = buffer.text();
    let replacements = fenix_diff::replacements(&old, new);
    if replacements.is_empty() {
        return;
    }
    let at = cursor_through_replacements(&old, &replacements, cursor.char_idx);
    let mut scratch = *cursor;
    buffer.replace_ranges(&mut scratch, &replacements);
    cursor.char_idx = at.min(buffer.len_chars());
    cursor.sticky_col = buffer.line_col(cursor).1;
}

/// Where `at` in `old` is once `replacements` (in order, char ranges of
/// `old`) are made: moved by those before it; in one, on the same line of
/// what replaced it (or its last) at the same column, or the line's end.
fn cursor_through_replacements(old: &str, replacements: &[(std::ops::Range<usize>, String)], at: usize) -> usize {
    let mut moved: isize = 0;
    for (range, text) in replacements {
        if at <= range.start {
            break;
        }
        let new_len = text.chars().count();
        if at >= range.end {
            moved += new_len as isize - range.len() as isize;
            continue;
        }
        let start = (range.start as isize + moved) as usize;
        let before: String = old.chars().skip(range.start).take(at - range.start).collect();
        let line = before.matches('\n').count();
        let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count());
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        let Some(last) = lines.len().checked_sub(1) else { return start };
        let line = line.min(last);
        let ahead: usize = lines[..line].iter().map(|l| l.chars().count()).sum();
        let width = lines[line].trim_end_matches(['\n', '\r']).chars().count();
        return start + ahead + column.min(width);
    }
    (at as isize + moved).max(0) as usize
}

/// The functions and methods a `workspace/symbol` answer names.
fn functions_named(response: lsp_types::WorkspaceSymbolResponse) -> std::collections::HashSet<String> {
    let function = |kind: lsp_types::SymbolKind| matches!(kind, lsp_types::SymbolKind::FUNCTION | lsp_types::SymbolKind::METHOD);
    match response {
        lsp_types::WorkspaceSymbolResponse::Flat(symbols) => symbols.into_iter().filter(|s| function(s.kind)).map(|s| s.name).collect(),
        lsp_types::WorkspaceSymbolResponse::Nested(symbols) => symbols.into_iter().filter(|s| function(s.kind)).map(|s| s.name).collect(),
    }
}

/// One message as a language server frames it (`Content-Length`, a blank
/// line, the body); `None` once the stream ends or makes no sense.
fn read_lsp_frame(reader: &mut impl std::io::BufRead) -> Option<String> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse::<usize>().ok();
            }
        }
    }
    let mut body = vec![0; length?];
    reader.read_exact(&mut body).ok()?;
    String::from_utf8(body).ok()
}

/// How long a request sent to several servers waits for a slow one
/// before acting on what the others answered.
const LSP_FANOUT_DEADLINE: std::time::Duration = std::time::Duration::from_millis(800);

/// One request sent to several language servers, its answers gathered
/// as they arrive (`fenix_lsp::fanout::Gather`).
enum LspFanout {
    /// `K`: shown as answers arrive, the primary's first.
    Hover { buffer: BufferId, gather: fenix_lsp::fanout::Gather<LspKey, Option<String>> },
    /// `gd`/`gr`: merged and acted on once everyone answered, or once the
    /// deadline passed with something to show.
    Locations { buffer: BufferId, references: bool, gather: fenix_lsp::fanout::Gather<LspKey, Vec<lsp_types::Location>> },
    /// `SPC c a`: every server's actions in one list, each remembering its
    /// server's view of the documents (`refactor::Context`).
    CodeActions {
        buffer: BufferId,
        gather: fenix_lsp::fanout::Gather<LspKey, Vec<lsp_types::CodeActionOrCommand>>,
        contexts: HashMap<LspKey, refactor::Context>,
        errors: Vec<String>,
        /// The fixes extensions gave with their problems on the line
        /// (`diagnostics-set-ex`), shown after the servers' actions.
        fixes: Vec<(lsp_types::CodeActionOrCommand, refactor::Context, Option<String>)>,
    },
    /// `fenix/lsp`: answered to the client that asked. Several can be
    /// under way, unlike the others: an agent's questions don't replace
    /// each other.
    Control { question: control_agent::LspQuestion, gather: fenix_lsp::fanout::Gather<LspKey, serde_json::Value>, reply: std::sync::mpsc::Sender<crate::ipc::Reply> },
}

/// The completion popup's server answers while it stays anchored at one
/// place: complete lists are kept and filtered as you type; only servers
/// that said `isIncomplete` are asked again.
struct LspCompletion {
    buffer: BufferId,
    prefix_start: usize,
    answers: fenix_lsp::fanout::CompletionSet<LspKey>,
}

/// What starting a language server off the UI thread came to: its
/// process and events, or why it couldn't start, with what
/// `App::lsp_started` needs to take it from there.
pub struct LspStart {
    key: LspKey,
    generation: u64,
    name: String,
    languages: Vec<String>,
    executable: String,
    cwd: PathBuf,
    result: std::io::Result<(fenix_lsp::LspClient, std::sync::mpsc::Receiver<fenix_lsp::LspEvent>)>,
}

impl std::fmt::Debug for LspStart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LspStart").field("key", &self.key).field("name", &self.name).field("started", &self.result.is_ok()).finish()
    }
}

/// Background reader for one `LspSession` -- drains `fenix_lsp::
/// LspClient::spawn`'s returned channel for as long as the connection
/// lives, forwarding every decoded event as `FenixUserEvent::Lsp`.
/// LSP messages (diagnostics, completion results, ...) arrive at nowhere
/// near a pixel stream's volume, so there's no need to apply anything directly on this thread to avoid
/// stalling input behind a burst -- everything just gets forwarded.
struct LspReader {
    stop: Arc<AtomicBool>,
}

impl LspReader {
    fn spawn(receiver: std::sync::mpsc::Receiver<fenix_lsp::LspEvent>, key: LspKey, generation: u64, send: impl Fn(FenixUserEvent) -> bool + Send + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        std::thread::spawn(move || loop {
            if thread_stop.load(Ordering::Relaxed) {
                return;
            }
            match receiver.recv() {
                Ok(event) => {
                    if thread_stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let disconnected = matches!(event, fenix_lsp::LspEvent::Disconnected(_));
                    if !send(FenixUserEvent::Lsp { key: key.clone(), generation, event }) || disconnected {
                        return;
                    }
                }
                Err(_) => return, // the LspClient side hung up -- session closed/dropped
            }
        });
        Self { stop }
    }
}

impl Drop for LspReader {
    /// Not joined: the thread ends with the first event after this, or
    /// when the client's read loop does, and that's when every process
    /// holding the server's pipes has gone. The UI thread isn't to wait
    /// for that.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Forwards a `fenix_dap::DapClient`'s event stream into
/// `FenixUserEvent::Dap` -- identical shape and reasoning to
/// `LspReader`, just for a debug session instead of a language server
/// (there's only ever one live debug session at a time, unlike LSP's
/// per-language map, so this carries no extra key the way `LspReader`
/// carries `language`).
struct DapReader {
    stop: Arc<AtomicBool>,
}

impl DapReader {
    fn spawn(receiver: std::sync::mpsc::Receiver<fenix_dap::DapEvent>, generation: u64, send: impl Fn(FenixUserEvent) -> bool + Send + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        std::thread::spawn(move || loop {
            if thread_stop.load(Ordering::Relaxed) {
                return;
            }
            match receiver.recv() {
                Ok(event) => {
                    if thread_stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let disconnected = matches!(event, fenix_dap::DapEvent::Disconnected(_));
                    if !send(FenixUserEvent::Dap { generation, event }) || disconnected {
                        return;
                    }
                }
                Err(_) => return, // the DapClient side hung up -- session closed/dropped
            }
        });
        Self { stop }
    }
}

impl Drop for DapReader {
    /// Not joined, as `LspReader` isn't.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// A Lazygit-style Git management session (`SPC g g`) -- mirrors
/// `DockerSession`'s own shape and reasoning almost exactly (seven panes
/// instead of five, one background poller instead of two, action-key
/// routing keyed off *which pane is focused* rather than `BufferKind`
/// alone). The one real structural difference: every `fenix-git` call
/// targets a *specific repository* (`repo_root`), not a daemon reachable
/// regardless of cwd the way `fenix-docker` is -- see `fenix-git::
/// process`'s own doc comment for why every function there takes an
/// explicit root path.
struct GitSession {
    workspace_index: usize,
    status_pane: fenix_window::WindowId,
    staged_pane: fenix_window::WindowId,
    unstaged_pane: fenix_window::WindowId,
    branches_pane: fenix_window::WindowId,
    commits_pane: fenix_window::WindowId,
    stash_pane: fenix_window::WindowId,
    main_pane: fenix_window::WindowId,
    status_buffer: BufferId,
    staged_buffer: BufferId,
    unstaged_buffer: BufferId,
    branches_buffer: BufferId,
    commits_buffer: BufferId,
    stash_buffer: BufferId,
    main_buffer: BufferId,
    repo_root: PathBuf,
    /// Which left-column section the accordion was last sized for. The
    /// weights are only reapplied when this stops matching the focused
    /// pane, so a deliberate `SPC w` resize survives until you move to
    /// another section, instead of being undone on the very next frame.
    accordion_focus: Option<fenix_window::WindowId>,
    /// Last-listed files/branches/commits/stashes, cached so `git_sync_
    /// main`/action handlers can look up whichever row is under the
    /// cursor without re-shelling `git` on every keystroke -- same
    /// reasoning as `DockerSession::containers`/`images`/`volumes`. One
    /// flat list feeds both the Staged and Unstaged panes (each just
    /// filters it differently at render time, see `git_panel::render_
    /// staged`/`render_unstaged`) -- a `FileEntry` already carries both
    /// halves independently, so there's nothing to split at the data
    /// level.
    files: Vec<fenix_git::FileEntry>,
    branches: Vec<fenix_git::Branch>,
    commits: Vec<fenix_git::Commit>,
    stashes: Vec<fenix_git::Stash>,
    /// The most recent status poll -- `None` before the first tick lands
    /// (or outside a real repo). Feeds the Status pane, which -- unlike
    /// the Docker panel's own Status pane -- always shows the same
    /// repo-overview info regardless of cursor position elsewhere (see
    /// `git_panel::render_status`'s own doc comment for why that's the
    /// real Lazygit-accurate behavior, not a simplification).
    status: Option<fenix_git::RepoStatus>,
    /// A suspended rebase/merge/cherry-pick, if the repo is in one --
    /// what the Status pane's banner reports and what `SPC g x c`/`SPC g x a`
    /// act on.
    in_progress: Option<fenix_git::InProgress>,
    /// Which branch each side of a conflict's markers actually is,
    /// resolved once per refresh -- see `fenix_git::conflict_sides`.
    sides: Option<fenix_git::ConflictSides>,
    /// Same "held only for its `Drop` side effect" RAII shape as
    /// `DockerSession::stats_poller`.
    #[allow(dead_code)]
    status_poller: Option<GitStatusPoller>,
    /// Full repo-relative paths of every directory currently expanded in
    /// the Staged pane's own tree (`Tab` toggles membership, see
    /// `git_toggle_dir_expand`) -- a directory not in here renders
    /// collapsed, showing just its own row. Starts empty (every
    /// directory collapsed) and persists across `git_refresh_session`,
    /// so refreshing the session doesn't fold everything you'd opened
    /// back up. Kept separate from `unstaged_expanded_dirs` -- each pane
    /// manages its own view state, since the same directory being open
    /// in one doesn't imply anything about the other.
    staged_expanded_dirs: HashSet<String>,
    /// Same role as `staged_expanded_dirs`, for the Unstaged pane.
    unstaged_expanded_dirs: HashSet<String>,
    /// The `GitEntry` `git_sync_main` last actually fetched a diff for --
    /// lets it skip re-shelling `git diff`/`show` when the cursor hasn't
    /// moved to a *different* entry (most keystrokes in Staged/Unstaged/
    /// Commits/Stash don't). `None` both "nothing selected yet" and
    /// "force a re-fetch even if the entry looks unchanged" -- `git_
    /// refresh_session` resets this before its own trailing `git_sync_
    /// main` call, since a manual refresh means the underlying content
    /// may have changed even if the selected path/hash/index hasn't.
    last_main_entry: Option<git_panel::GitEntry>,
    /// What `last_main_entry` resolved to for fetching -- kept so a
    /// completed background fetch knows which *kind* of diff it just
    /// produced (`DiffSource::of`), which is what decides whether its
    /// hunks are stageable. `main_request_id` already guarantees only
    /// the newest request's result is ever applied, so reading the
    /// session's current value when it lands is correct by construction.
    last_main_fetch: MainFetch,
    /// Bumped every time `git_sync_main` issues a real async fetch (a
    /// `File`/`Commit`/`Stash` selection, not `None`/an untracked
    /// placeholder, neither of which need a shell-out) -- `apply_git_
    /// main` discards a completed fetch whose id no longer matches this,
    /// the guard against a slow, superseded request overwriting Main
    /// with a stale diff after a faster, later one already landed.
    main_request_id: u64,
    /// Bumped every time `git_refresh_session` issues a real async
    /// refresh -- `apply_git_refresh` discards a completed refresh whose
    /// id no longer matches this, same "slow request superseded by a
    /// faster later one" guard as `main_request_id`, just for the whole-
    /// session refresh instead of a single diff fetch.
    refresh_request_id: u64,
}

/// A background `git status --porcelain=v2 --branch` poller for one open
/// Git session -- ticks every `STATS_POLL_INTERVAL`, mirrors
/// `DockerStatsPoller` exactly (same interval, same stop/join-on-drop
/// teardown discipline) except it needs to know *which* repository to
/// poll, since `fenix-git` has no daemon-wide equivalent to shell
/// against.
struct GitStatusPoller {
    stop: Arc<AtomicBool>,
}

impl GitStatusPoller {
    fn spawn(repo_root: PathBuf, send: impl Fn(FenixUserEvent) -> bool + Send + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        std::thread::spawn(move || loop {
            if thread_stop.load(Ordering::Relaxed) {
                return;
            }
            let status = fenix_git::status(&repo_root);
            if !send(FenixUserEvent::GitStatusReady(status)) {
                return;
            }
            let mut waited = Duration::ZERO;
            while waited < STATS_POLL_INTERVAL {
                if thread_stop.load(Ordering::Relaxed) {
                    return;
                }
                let step = STATS_POLL_STOP_CHECK.min(STATS_POLL_INTERVAL - waited);
                std::thread::sleep(step);
                waited += step;
            }
        });
        Self { stop }
    }
}

impl Drop for GitStatusPoller {
    /// Not joined: a `git status` on a share that has gone away takes as
    /// long as it takes, and the UI thread isn't to wait for it.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Which of a `GitSession`'s seven panes is currently focused, if any --
/// mirrors `DockerPaneRole`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitPaneRole {
    Status,
    Staged,
    Unstaged,
    Branches,
    Commits,
    Stash,
    Main,
}

/// A destructive Git action armed by its own key until the next keypress
/// confirms (`y`) or cancels -- same "wait for exactly one more raw key"
/// shape as `DockerHost::confirm_remove`, generalized to the several different
/// kinds of destructive action the Git panel has (Docker only ever had
/// one: remove).
#[derive(Debug, Clone, PartialEq, Eq)]
enum GitConfirmAction {
    DiscardFile { path: String, untracked: bool },
    /// Discard every changed file under a directory (`fenix_git::
    /// discard_dir`) -- unlike `DiscardFile`, no `untracked` flag: a
    /// directory routinely holds both tracked and untracked changes at
    /// once, so `discard_dir` always handles both.
    DiscardDir { path: String },
    DeleteBranch { name: String },
    DropStash { index: usize },
    /// Rewrite a published branch (`git.force_push`). Always
    /// `--force-with-lease`, but still confirmed: it changes history
    /// someone else may already have.
    ForcePush,
    /// Throw away the hunk under the cursor in a file buffer (`SPC g d`),
    /// by the file line it covers.
    DiscardEditorHunk { buffer: BufferId, line: usize },
    /// Local changes stop a branch switch: stash them, then switch.
    SwitchWithStash { branch: String },
    /// Throw away one hunk of the working tree (`d` in the diff pane).
    /// Carries the buffer and anchor rather than a rebuilt patch so the
    /// patch is regenerated from the model at confirm time -- one less
    /// thing to hold stale across the keypress the confirmation waits
    /// for.
    DiscardHunk { buffer: BufferId, anchor: diff_view::DiffAnchor },
}

/// Which free-text Git prompt (if any) is capturing the next keystrokes --
/// same "next key(s) mean something special" pattern `ExplorerPrompt`
/// already uses for file-manager text input, applied here to a commit
/// message or a new branch's name. Stashing (`s` on Files) needs no
/// prompt -- real Lazygit's own plain `s` just stashes everything with
/// git's own default message; `S` for named/partial stash variants isn't
/// implemented (see the Git panel's own scope notes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitPromptKind {
    NewBranch,
}

struct GitPrompt {
    kind: GitPromptKind,
    input: String,
}

/// Cross-thread wake events -- the only way anything outside the winit
/// event loop's own thread (the Docker panel's stats poller/log
/// follower, Phases 5-6) can get `App` to mutate state and request a
/// redraw (see `ApplicationHandler::user_event`, and `App::event_proxy`'s
/// own doc comment). Each variant names *what changed*, not *what to
/// do* -- `App::user_event` decides the "do."
#[derive(Debug)]
pub enum FenixUserEvent {
    /// The main window's accessibility adapter: the tree asked for, an
    /// action, the assistive technology gone (`a11y_host`).
    Accessibility(accesskit_winit::Event),
    /// The machine was asleep this long (`watchdog`). Winit doesn't say.
    Woke(Duration),
    /// One decoded event from a language server's connection (a
    /// response, notification, server-initiated request, or a
    /// disconnect), from that server's `LspReader`. Carries its `key`
    /// explicitly since more than one server can be live at once -- several
    /// on one buffer -- and events from a slow-to-answer one can arrive
    /// well after focus moved elsewhere.
    Lsp { key: LspKey, generation: u64, event: fenix_lsp::LspEvent },
    /// A language server's process started, or failed to, on its own
    /// thread (`start_lsp_session`): finding the program on `PATH` and
    /// starting it can take seconds, and the window must not wait.
    LspStarted(Box<LspStart>),
    /// A request sent to several language servers reached its deadline:
    /// act on what has arrived if a slow server is still out.
    LspDeadline(u64),
    /// One decoded event from the active debug session's adapter
    /// connection, from `DapReader` -- only one debug session is ever
    /// live at a time. The generation rejects queued events from a retired adapter.
    Dap { generation: u64, event: fenix_dap::DapEvent },
    /// Ordered task output/completion tagged by run identity. The same buffer
    /// can display a newer run before the previous run's events are delivered.
    TaskEvent(BufferId, fenix_tasks::RunId, fenix_tasks::TaskEvent),
    /// A test run's step said something, or ended, by its tag
    /// (`tests_host`).
    TestsStep(u64, fenix_tasks::TaskEvent),
    /// A fresh `git status --porcelain=v2 --branch` snapshot (`None`
    /// outside a repo), from the active Git session's background
    /// poller.
    GitStatusReady(Option<fenix_git::RepoStatus>),
    /// A `git diff`/`show`/`stash show` result for the Git panel's Main
    /// pane, from a one-shot background thread `git_sync_main` spawned.
    /// Carries the target `BufferId` explicitly (same reasoning as
    /// `LogLine`) and the `request_id` the fetch was issued under, so a
    /// slow request superseded by a faster later one can be told apart
    /// and discarded instead of clobbering Main with a stale diff --
    /// see `GitSession::main_request_id`'s own doc comment.
    GitMainReady { request_id: u64, buffer: BufferId, diff: Option<String> },
    /// A History-view commit's metadata and diff finished fetching --
    /// same request_id-guarded shape as `GitMainReady`, for the other
    /// view. `None` when the commit couldn't be read at all.
    GitHistoryDetailReady { request_id: u64, buffer: BufferId, diff: Option<CommitDetail> },
    /// The History view's graph and refs, read again.
    GitHistoryReady { request_id: u64, data: Box<HistoryData> },
    /// A `SPC g f` fetch finished. Carries git's own message so a
    /// failure (no remote, auth, offline) surfaces verbatim rather than
    /// as a generic "fetch failed".
    GitFetched { result: Result<String, String> },
    /// A completed `git_refresh_session` re-list (files/branches/
    /// commits/stashes/status), from a one-shot background thread it
    /// spawned. Carries the `request_id` the refresh was issued under,
    /// same stale-result guard as `GitMainReady` -- see `GitSession::
    /// refresh_request_id`'s own doc comment.
    GitRefreshReady { request_id: u64, data: GitRefreshData },
    /// A chunk of raw output bytes from the terminal panel's shell, from
    /// `TerminalReader`'s background thread -- fed straight into the
    /// `vt100::Parser` living in `App::terminal` on the main thread
    /// (kept single-threaded on purpose, see `TerminalReader`'s own doc
    /// comment).
    TerminalOutput(TerminalTarget, Vec<u8>),
    /// A directory read finished, from the thread `explorer_navigate`
    /// spawned. Carries `request` so a result that has been superseded
    /// (or abandoned) can be dropped rather than yanking the pane to a
    /// directory the user has already navigated away from.
    ///
    /// The error is a `String`, not an `io::Error`: by the time it
    /// reaches here the only thing anyone can do with it is show it.
    ExplorerListed { target: ExplorerTarget, request: u64, result: Result<fenix_explorer::Listing, String> },
    /// What the disk poll's worker saw of the open files (`disk_probe`).
    DiskProbed(Box<disk_probe::ProbeResult>),
    /// Folders an explorer shows whose names changed (`explorer_watch`).
    ExplorerFoldersChanged(Vec<PathBuf>),
    /// `ssh`, connecting to a remote machine, has a question (`remote`).
    RemoteQuestion(remote::RemoteQuestion),
    /// A connection to a remote machine was made, or couldn't be.
    RemoteConnected(Box<remote::RemoteConnected>),
    /// A remote machine's connection changed: lost, being tried again,
    /// back.
    RemoteStatus(String, fenix_ssh::Status),
    /// What Home's recent files looked like, looked at off the UI thread.
    RecentProbed(HashMap<PathBuf, home::RecentSeen>),
    /// The extension registry read, or an install from it done, for the
    /// Extensions page (`extensions_host`).
    Registry(extensions_host::RegistryEvent),
    /// What the update check found (`app/updates.rs`).
    Updates(updates::UpdateAnswer),
    /// What a code extension's worker sent (`wasm_host`).
    Wasm { ext: String, event: fenix_wasm::host::Event },
    /// One message from a coding agent's connection, or its end
    /// (`agents`). The generation drops what a process that has since
    /// been restarted said.
    Acp { conn: fenix_acp::hub::ConnId, generation: u32, event: fenix_acp::client::AcpEvent },
    /// A root's files, listed for the find-file picker (`file_index`).
    FilesListed { root: PathBuf, include_ignored: bool, files: Vec<fenix_picker::Candidate<PathBuf>> },
    /// Home's TODO column for a project root, swept off the UI thread.
    HomeTodos { root: PathBuf, items: Vec<dashboard::TodoItem> },
    /// The `git status` pass for a listing that already arrived -- the
    /// second half of the same background thread. Deliberately separate
    /// so badges never hold up the listing itself, which is the whole
    /// difference between a directory appearing at once and appearing
    /// after a subprocess has walked the repo.
    ExplorerGitStatus { target: ExplorerTarget, request: u64, statuses: HashMap<PathBuf, fenix_explorer::GitStatus> },
    /// What a listing's filter found below its folder, searched off the
    /// UI thread (`explorer_search`); `search` says which search.
    ExplorerFound { target: ExplorerTarget, search: u64, found: Vec<fenix_explorer::Entry> },
    /// What a server answered when asked what it shares -- from the
    /// thread `explorer_browse_shares` spawned, because `net view`
    /// against a host that is not there blocks for tens of seconds.
    ExplorerShares { host: String, result: Result<Vec<String>, String> },
    /// The machine's drives, from the one-off background query
    /// `ensure_volumes` starts -- see there for why it is not done on
    /// demand.
    ExplorerVolumes(Vec<fenix_fs::Volume>),
    /// How far a long file operation has got. Sent per file, which for
    /// a directory of thousands is a lot of events -- cheap ones, and
    /// the alternative is a progress line that only moves per top-level
    /// item and so does not move at all for a single big tree.
    ExplorerJobProgress(fenix_fs::Progress),
    /// A long file operation finished, was cancelled, or failed.
    ExplorerJobDone(Vec<fenix_fs::Outcome>),
    /// A recursive directory measurement finished (`SPC e i` on a
    /// folder), reported separately because it answers a question
    /// rather than changing anything.
    ExplorerMeasured { path: PathBuf, total: fenix_fs::Total },
    /// The result of `fenix_terminal::Terminal::spawn`, from the one-
    /// shot background thread `toggle_terminal` spawns it on -- see
    /// that function's own doc comment for why spawning can't happen
    /// synchronously on the main thread. Wrapped (`TerminalSpawnResult`)
    /// purely so this enum can keep deriving `Debug`: `fenix_terminal::
    /// Terminal` (holding `Box<dyn Child>` et al.) has no `Debug` impl
    /// of its own, and doesn't need one anywhere else.
    TerminalSpawned(TerminalTarget, TerminalSpawnResult),
    /// Every restored buffer's gutter marks, worked out off the UI thread
    /// after a session restore (`App::refresh_all_gutter_hunks`), and when
    /// that started.
    GutterHunks(Instant, Vec<(BufferId, Option<(PathBuf, fenix_diff::FileDiff)>)>),
    /// A control-channel request (`crate::ipc`): a later `fenix`
    /// launch -- double-clicking a file in Explorer while Fenix is
    /// already running, `fenix x.rs:12` in a terminal -- or another
    /// tool asking to open files or a project, or what's in focus.
    /// Answered on `reply`; the connection's thread is waiting on it.
    Control { call: fenix_control::Call, reply: std::sync::mpsc::Sender<crate::ipc::Reply> },
    /// A project page's background job reporting in -- see
    /// `pages::PageEvent`.
    Page(pages::PageEvent),
}

/// The extension `id` names, for saying where a command comes from.
fn extension_name(id: &str) -> String {
    crate::installed::code_extensions().iter().find(|e| e.id == id).map_or_else(|| id.to_string(), |e| e.name.clone())
}

impl App {
    /// Runs command `id` (see `CommandRegistry::run`), with or without an
    /// event loop to hand (a test driving the keys has none; a command
    /// that needs one does nothing then).
    pub(crate) fn run_command_with(&mut self, event_loop: Option<&ActiveEventLoop>, id: &str) -> bool {
        self.note_command_run(id);
        let commands = Rc::clone(&self.commands);
        commands.run(self, event_loop, id)
    }

    /// Remembers `id` as the command run last, for the palette's order.
    fn note_command_run(&mut self, id: &str) {
        if id == "commands.palette" || self.commands.get(id).is_none() {
            return;
        }
        self.recent_commands.retain(|c| c != id);
        self.recent_commands.insert(0, id.to_string());
        self.recent_commands.truncate(RECENT_COMMANDS);
    }

    /// `SPC h c`: the palette, saying what a command is instead of
    /// running it.
    pub(crate) fn describe_command_picker(&mut self) {
        self.open_command_palette();
        self.palette_describes = true;
    }

    /// What command `id` is: what it does, the keys that run it, where
    /// it's from.
    fn describe_command(&self, id: &str) -> String {
        let Some(command) = self.commands().get(id) else { return format!("{id} isn't a command") };
        let mut keys: Vec<String> = keymap::leader_trie().bindings().into_iter().filter(|(_, c)| **c == id).map(|(seq, _)| seq.iter().map(keymap::describe_keypress).collect::<Vec<_>>().join(" ")).collect();
        keys.sort();
        let mut chords: Vec<String> = keymap::chord_map().in_use(self.key_scheme()).into_iter().filter(|c| c.command == Some(id)).map(|c| keymap::describe_keypress(&c.key)).collect();
        chords.dedup();
        keys.splice(0..0, chords);
        let keys = if keys.is_empty() { "no key".to_string() } else { keys.join(", ") };
        let from = command.source.map_or_else(|| "core".to_string(), extension_name);
        format!("{id}: {} -- {keys} -- {from}", command.description)
    }

    /// `SPC h k`: the next keys typed are described, not run.
    pub(crate) fn describe_key_start(&mut self) {
        self.describing_keys = Some(Vec::new());
        self.set_message("Describe key: type it (SPC ...), Esc to stop");
    }

    /// A key while `SPC h k` waits: once the keys run something (or
    /// can't), says what.
    fn describe_key_feed(&mut self, key: KeyPress) {
        let Some(mut keys) = self.describing_keys.take() else { return };
        if key.legacy() == KeyPress::named(FenixNamedKey::Escape) {
            self.set_message("");
            return;
        }
        // Leader keys are read as the leader reads them; a chord as it came.
        keys.push(if keys.is_empty() { key } else { key.legacy() });
        let text = keys.iter().map(keymap::describe_keypress).collect::<Vec<_>>().join(" ");
        if keys[0] != KeyPress::char(' ') {
            match self.chord_command(key) {
                Some(id) => {
                    let what = self.describe_command(id);
                    let yours = keymap::user_keys().iter().any(|u| u.keys == keymap::describe_keypress(&key.chord()));
                    self.set_message(format!("{text} runs {what}{}", if yours { " -- yours" } else { "" }));
                }
                None if key.mods.ctrl || key.mods.alt || key.mods.super_ || key.is_function() => self.set_message(format!("{text} isn't bound here")),
                None => self.set_message(format!("{text} -- SPC h k describes leader keys (SPC ...) and chords (C-s, F12); the palette (SPC :) lists the rest")),
            }
            return;
        }
        let local = keys.get(1) == Some(&KeyPress::char('m'));
        let contexts = if local { self.local_contexts() } else { Vec::new() };
        let trie = if local { keymap::local_trie(&contexts) } else { keymap::leader_trie() };
        let fed: &[KeyPress] = if local { &keys[2..] } else { &keys };
        let mut matcher = trie.matcher();
        let mut step = None;
        for k in fed {
            step = Some(matcher.feed(*k));
        }
        // A key of a feature that's off is as unbound as one that isn't
        // there (`command_hidden`, `leader_hidden`).
        let off = !local && fed.len() > 1 && self.leader_hidden(&fed[..fed.len() - 1], fed[fed.len() - 1]);
        match step {
            Some(fenix_keymap::Step::Matched(_)) | Some(fenix_keymap::Step::Pending(_)) if off => self.set_message(format!("{text} isn't bound")),
            Some(fenix_keymap::Step::Matched(id)) => {
                let what = self.describe_command(id);
                let yours = keymap::user_keys().iter().any(|u| u.keys == text);
                self.set_message(format!("{text} runs {what}{}", if yours { " -- yours" } else { "" }));
            }
            Some(fenix_keymap::Step::NoMatch) => self.set_message(format!("{text} isn't bound")),
            Some(fenix_keymap::Step::Pending(_)) | None => {
                self.set_message(format!("Describe key: {text} ..."));
                self.describing_keys = Some(keys);
            }
        }
    }

    /// `SPC h b`: the keys bound here, the leader's then this buffer's
    /// `SPC m` -- yours marked -- and Enter runs one.
    pub(crate) fn open_bindings_picker(&mut self) {
        let yours: HashMap<String, Option<&'static str>> = keymap::user_keys().into_iter().filter(|u| u.command.is_some()).map(|u| (u.keys, u.replaced)).collect();
        let describe = |id: &str| self.commands().get(id).map(|c| c.description.to_string()).unwrap_or_default();
        let text = |seq: &[KeyPress]| seq.iter().map(keymap::describe_keypress).collect::<Vec<_>>().join(" ");
        let mut rows: Vec<(String, String)> = keymap::leader_trie().bindings().into_iter().filter(|(_, id)| !self.command_hidden(id)).map(|(seq, id)| (text(&seq), id.to_string())).collect();
        let contexts = self.local_contexts();
        if !contexts.is_empty() {
            rows.extend(keymap::local_trie(&contexts).bindings().into_iter().map(|(seq, id)| (format!("SPC m {}", text(&seq)), id.to_string())));
        }
        rows.extend(keymap::chord_map().in_use(self.key_scheme()).into_iter().filter_map(|c| Some((keymap::describe_keypress(&c.key), c.command?.to_string()))));
        rows.sort();
        rows.dedup();
        let candidates = rows
            .into_iter()
            .map(|(keys, id)| {
                let mark = match yours.get(&keys) {
                    Some(Some(was)) => format!("  (yours, was {was})"),
                    Some(None) => "  (yours)".to_string(),
                    None => String::new(),
                };
                fenix_picker::Candidate::new(format!("{keys:<14} {}  {id}{mark}", describe(&id)), id)
            })
            .collect();
        self.completion = None;
        self.enter_picker(ActivePicker::Bindings(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC h e`: your keybindings file, started with an example when
    /// there isn't one; saving it puts it in use.
    pub(crate) fn edit_keybindings(&mut self) {
        let Some(path) = keymap::user_keymap_path() else {
            self.set_error("no settings folder to keep keybindings.toml in");
            return;
        };
        if !path.exists() {
            let start = "# Your keys, over Fenix's. Save this file and they're in use.\n# SPC h b lists what's bound, SPC h c every command.\n#\n# [[leader]]\n# keys = \"SPC o s\"\n# command = \"file.save\"\n# label = \"save\"\n#\n# [[leader]]\n# keys = \"SPC a i\"\n# unbind = true\n";
            if let Err(e) = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, start)) {
                self.set_error(format!("couldn't start {}: {e}", path.display()));
                return;
            }
        }
        self.open_file_from_picker(&path);
    }

    /// After a save: your keybindings file, saved, is put in use.
    fn reload_keybindings_if_saved(&mut self) {
        let Some(saved) = self.open().buffer.path().map(Path::to_path_buf) else { return };
        self.reload_keybindings_if(&saved);
    }

    /// The keys read again when `saved` is your keybindings.toml.
    pub(super) fn reload_keybindings_if(&mut self, saved: &Path) {
        let Some(path) = keymap::user_keymap_path() else { return };
        if saved != path.as_path() {
            return;
        }
        let problems = keymap::reload_leader();
        self.leader_matcher = keymap::leader_trie().matcher();
        *self.leader_places.borrow_mut() = None;
        if problems.is_empty() {
            self.set_message(format!("Your keybindings are in use: {}", keymap::user_keys().len()));
        } else {
            self.set_error(format!("keybindings.toml: {}", problems.join("; ")));
        }
    }

    /// `SPC :` / Alt-x: every command -- what it does, its id, its key,
    /// and the extension it's from -- the ones run lately first.
    pub(crate) fn open_command_palette(&mut self) {
        self.palette_describes = false;
        let candidates = self.command_candidates();
        self.completion = None;
        self.enter_picker(ActivePicker::Commands(fenix_picker::PickerState::with_mode(candidates, fenix_picker::MatchMode::Words)));
    }

    /// Tab in the palette: the next keys typed are `command`'s.
    fn bind_keys_start(&mut self, command: String) {
        self.set_message(format!("Keys for {command}: type them (SPC ..., or a chord: C-s, F12), Enter binds, Esc stops"));
        self.binding_keys = Some((command, Vec::new()));
    }

    /// A key while the palette waits for keys to bind.
    fn bind_keys_feed(&mut self, key: KeyPress) {
        let Some((command, mut keys)) = self.binding_keys.take() else { return };
        if key.legacy() == KeyPress::named(FenixNamedKey::Escape) {
            self.set_message("");
            return;
        }
        if key.legacy() == KeyPress::named(FenixNamedKey::Enter) {
            return self.bind_keys_finish(&command, &keys);
        }
        let key = if keys.is_empty() { key } else { key.legacy() };
        if key == KeyPress::named(FenixNamedKey::Backspace) {
            keys.pop();
        } else {
            keys.push(key);
        }
        let text = keys.iter().map(keymap::describe_keypress).collect::<Vec<_>>().join(" ");
        self.set_message(format!("Keys for {command}: {text} -- Enter binds, Esc stops"));
        self.binding_keys = Some((command, keys));
    }

    /// Binds `keys` to `command` in your keybindings file and puts it in
    /// use -- unless the keys would hide something: a group (`SPC f`),
    /// or keys under another binding, which it would always run first.
    fn bind_keys_finish(&mut self, command: &str, keys: &[KeyPress]) {
        let text = keys.iter().map(keymap::describe_keypress).collect::<Vec<_>>().join(" ");
        if let [key] = keys {
            let key = key.chord();
            if key.mods.ctrl || key.mods.alt || key.mods.super_ || key.is_function() {
                return self.bind_chord_finish(command, key);
            }
        }
        if keys.first() != Some(&KeyPress::char(' ')) || keys.len() < 2 {
            self.set_error(format!("{text}: your keys start with SPC, then at least one more -- or are one chord (C-s, F12)"));
            return;
        }
        let trie = keymap::leader_trie();
        if let Some((n, under)) = (1..keys.len()).find_map(|n| trie.get(&keys[..n]).map(|c| (n, *c))) {
            let prefix = keys[..n].iter().map(keymap::describe_keypress).collect::<Vec<_>>().join(" ");
            self.set_error(format!("{prefix} runs {under}, so {text} could never be reached -- pick other keys"));
            return;
        }
        if trie.bindings().iter().any(|(seq, _)| seq.len() > keys.len() && seq.starts_with(keys)) {
            self.set_error(format!("{text} is a group -- binding it would hide what's under it; add a key"));
            return;
        }
        let Some(path) = keymap::user_keymap_path() else {
            self.set_error("no settings folder to keep keybindings.toml in");
            return;
        };
        let was = trie.get(keys).copied().filter(|old| *old != command);
        if let Err(e) = fenix_config::keybindings::write_binding(&path, &text, command) {
            self.set_error(format!("couldn't write {}: {e}", path.display()));
            return;
        }
        let problems = keymap::reload_leader();
        self.leader_matcher = keymap::leader_trie().matcher();
        *self.leader_places.borrow_mut() = None;
        if !problems.is_empty() {
            self.set_error(format!("keybindings.toml: {}", problems.join("; ")));
            return;
        }
        self.set_message(match was {
            Some(old) => format!("{text} runs {command} now (it ran {old}) -- in keybindings.toml"),
            None => format!("{text} runs {command} now -- in keybindings.toml"),
        });
    }

    /// A chord given to `command` from the palette: a `[[keys]]` entry in
    /// your keybindings, for the key scheme in use.
    fn bind_chord_finish(&mut self, command: &str, key: KeyPress) {
        let text = keymap::describe_keypress(&key);
        let Some(path) = keymap::user_keymap_path() else {
            self.set_error("no settings folder to keep keybindings.toml in");
            return;
        };
        let was = self.chord_command(key).filter(|old| *old != command);
        if let Err(e) = fenix_config::keybindings::write_chord(&path, &text, command, self.key_scheme().name()) {
            self.set_error(format!("couldn't write {}: {e}", path.display()));
            return;
        }
        let problems = keymap::reload_leader();
        self.leader_matcher = keymap::leader_trie().matcher();
        *self.leader_places.borrow_mut() = None;
        if !problems.is_empty() {
            self.set_error(format!("keybindings.toml: {}", problems.join("; ")));
            return;
        }
        self.set_message(match was {
            Some(old) => format!("{text} runs {command} now (it ran {old}) -- in keybindings.toml"),
            None => format!("{text} runs {command} now -- in keybindings.toml"),
        });
    }

    /// `SPC h K`: where keys disagree -- Fenix's own clashes, where yours
    /// replace or take away Fenix's, and what's wrong with your file.
    /// Enter opens your file.
    pub(crate) fn open_key_conflicts(&mut self) {
        let mut rows: Vec<String> = keymap::keymap_conflicts().into_iter().map(|c| format!("Fenix: {c}")).collect();
        for key in keymap::user_keys() {
            match (&key.command, key.replaced) {
                (Some(command), Some(old)) => rows.push(format!("{}: yours runs {command}, over Fenix's {old}", key.keys)),
                (None, Some(old)) => rows.push(format!("{}: you took away Fenix's {old}", key.keys)),
                _ => {}
            }
        }
        rows.extend(keymap::user_key_problems().into_iter().map(|p| format!("keybindings.toml: {p}")));
        if rows.is_empty() {
            self.set_message("No conflicts: your keys and Fenix's agree");
            return;
        }
        let candidates = rows.into_iter().map(|row| fenix_picker::Candidate::new(row, "keys.edit".to_string())).collect();
        self.completion = None;
        self.enter_picker(ActivePicker::Bindings(fenix_picker::PickerState::new(candidates)));
    }

    /// Runs command `id` for a caller outside Fenix (see
    /// `CommandRegistry::run_external`).
    pub(crate) fn run_external_command(&mut self, id: &str) -> Result<(), crate::commands::ExternalError> {
        self.note_command_run(id);
        let commands = Rc::clone(&self.commands);
        commands.run_external(self, id)
    }

    /// Every command (`Command`), in its order.
    pub(crate) fn commands(&self) -> &CommandRegistry {
        &self.commands
    }
}

impl FenixUserEvent {
    /// Which event this is, for the hang log (`watchdog`).
    fn label(&self) -> &'static str {
        match self {
            Self::Accessibility(_) => "Accessibility",
            Self::Woke(_) => "Woke",
            Self::Lsp { .. } => "Lsp",
            Self::LspStarted(_) => "LspStarted",
            Self::LspDeadline(_) => "LspDeadline",
            Self::Dap { .. } => "Dap",
            Self::TaskEvent { .. } => "TaskEvent",
            Self::TestsStep { .. } => "TestsStep",
            Self::GitStatusReady { .. } => "GitStatusReady",
            Self::GitMainReady { .. } => "GitMainReady",
            Self::GitHistoryDetailReady { .. } => "GitHistoryDetailReady",
            Self::GitHistoryReady { .. } => "GitHistoryReady",
            Self::GitFetched { .. } => "GitFetched",
            Self::GitRefreshReady { .. } => "GitRefreshReady",
            Self::TerminalOutput { .. } => "TerminalOutput",
            Self::ExplorerListed { .. } => "ExplorerListed",
            Self::DiskProbed { .. } => "DiskProbed",
            Self::ExplorerFoldersChanged(_) => "ExplorerFoldersChanged",
            Self::RemoteQuestion { .. } => "RemoteQuestion",
            Self::RemoteConnected { .. } => "RemoteConnected",
            Self::RemoteStatus { .. } => "RemoteStatus",
            Self::RecentProbed { .. } => "RecentProbed",
            Self::Registry { .. } => "Registry",
            Self::Updates(_) => "Updates",
            Self::Wasm { .. } => "Wasm",
            Self::Acp { .. } => "Acp",
            Self::FilesListed { .. } => "FilesListed",
            Self::HomeTodos { .. } => "HomeTodos",
            Self::ExplorerGitStatus { .. } => "ExplorerGitStatus",
            Self::ExplorerFound { .. } => "ExplorerFound",
            Self::ExplorerShares { .. } => "ExplorerShares",
            Self::ExplorerVolumes { .. } => "ExplorerVolumes",
            Self::ExplorerJobProgress { .. } => "ExplorerJobProgress",
            Self::ExplorerJobDone { .. } => "ExplorerJobDone",
            Self::ExplorerMeasured { .. } => "ExplorerMeasured",
            Self::TerminalSpawned { .. } => "TerminalSpawned",
            Self::GutterHunks { .. } => "GutterHunks",
            Self::Control { .. } => "Control",
            Self::Page { .. } => "Page",
        }
    }
}

/// Starts a terminal's program in `cwd`. In a folder on a remote
/// project's machine, the terminal is there, and its shell the user's own
/// there (`terminal.shell` names one on this machine). `one_tool`: the
/// program is a tool of this machine's, which runs here as before.
fn spawn_shell(rows: u16, cols: u16, cwd: Option<&Path>, program: Option<&(String, Vec<String>)>, one_tool: bool) -> std::io::Result<(fenix_terminal::Terminal, Box<dyn std::io::Read + Send>)> {
    if let Some(cwd) = cwd.filter(|cwd| !one_tool && fenix_vfs::is_remote(cwd)) {
        if let Some(remote) = fenix_terminal::Terminal::spawn_remote(rows, cols, cwd, None) {
            return remote;
        }
    }
    match program {
        Some((program, args)) => fenix_terminal::Terminal::spawn_program(rows, cols, cwd, program, args),
        None => fenix_terminal::Terminal::spawn_in(rows, cols, cwd),
    }
}

/// See `FenixUserEvent::TerminalSpawned`'s own doc comment for why this
/// wrapper exists.
pub struct TerminalSpawnResult(std::io::Result<(fenix_terminal::Terminal, Box<dyn std::io::Read + Send>)>);

impl std::fmt::Debug for TerminalSpawnResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Ok(_) => write!(f, "TerminalSpawnResult(Ok(..))"),
            Err(err) => write!(f, "TerminalSpawnResult(Err({err:?}))"),
        }
    }
}

/// The autocompletion popup's live state -- see `App::completion`'s own
/// doc comment for the "recomputed fresh every keystroke" lifecycle.
struct CompletionState {
    /// Char offset where the identifier prefix being completed starts --
    /// `accept_completion` replaces `[prefix_start, cursor.char_idx)`
    /// with the chosen candidate's full label.
    prefix_start: usize,
    picker: fenix_picker::PickerState<completion::Item>,
}

/// Per-visible-line highlight segments: (view_row, col_start, col_end).
type Segments = Vec<(usize, usize, usize)>;

/// A line as drawn (`App::drawn_line`): its display text, the colours of
/// its bytes, and what to add to a byte of `text` to find it in them.
pub(super) struct DrawnLine<'a> {
    pub text: String,
    pub ranges: std::borrow::Cow<'a, [(std::ops::Range<usize>, glyphon::Color)]>,
    pub base: usize,
}

/// Highlight segments in the text: (buffer line, start char column, end
/// char column), put on rows by the pane's `rows::RowMap`.
type LineSegments = Vec<(usize, usize, usize)>;

/// A content or sidebar row's rich-text spans: (text, color, use_icon_font).
type RowSpans = Vec<(String, glyphon::Color, bool)>;

/// Line-number gutter display: `editor.line_numbers` when Fenix starts
/// (and when the setting changes), cycled by `SPC t n` until then.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineNumberMode {
    Off,
    Absolute,
    Relative,
}

impl LineNumberMode {
    /// `editor.line_numbers`' value; absolute when it isn't set.
    fn from_setting(config: &fenix_config::Config) -> LineNumberMode {
        match config.text_value("editor.line_numbers") {
            Some("relative") => LineNumberMode::Relative,
            Some("off") => LineNumberMode::Off,
            _ => LineNumberMode::Absolute,
        }
    }
}

/// What occupies the main content area: the editor buffer, a full-buffer
/// directory listing, or a fuzzy-filtered picker (find-file/grep/switch-
/// project) -- all three "visit something else instead of the buffer for
/// a moment," the same reasoning that lets `Picker` reuse `Explorer`'s
/// stash/restore machinery unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainView {
    Editor,
    Picker,
}

/// One position in the `Ctrl-O`/`Ctrl-I` jumplist -- a buffer plus a
/// char index within it, not a `Cursor` (no `sticky_col` to carry
/// around; `goto_jump_entry` derives a fresh one from the buffer at
/// landing time, same as every other programmatic cursor placement in
/// this file). `buffer` is only ever looked up, never assumed to still
/// exist -- `BufferId`s are never reused within a session (see
/// `BufferList::insert`), so a closed buffer's old entries just fail
/// their lookup and get silently skipped rather than resurrecting an
/// unrelated buffer that happens to reuse the id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct JumpEntry {
    buffer: BufferId,
    char_idx: usize,
}

/// One entry in the generalized "list of places, step next/prev"
/// `App::quickfix` holds -- kept as separate variants (one per real
/// source) rather than converted to one shared position representation
/// at construction time, since LSP's `Position` (0-indexed, UTF-16
/// columns) and `rg --vimgrep`'s own convention (1-indexed, char
/// columns, via `fenix_project::GrepMatch`) need the target file's
/// actual content to convert between correctly, which isn't
/// necessarily available (or worth reading off disk) until the entry
/// is actually jumped to -- see `App::quickfix_step`'s own dispatch.
/// `Task` reuses `GrepMatch`'s exact shape (path/line/col/text) rather
/// than inventing its own -- a task's recovered `file:line:col:
/// message` location is already in that same 1-indexed convention (see
/// `fenix_tasks::TaskLocation`), and `jump_to_grep_match` needs nothing
/// source-specific to act on it.
#[derive(Debug, Clone, PartialEq)]
enum QuickfixEntry {
    Grep(fenix_project::GrepMatch),
    Lsp { path: PathBuf, position: lsp_types::Position },
    Task(fenix_project::GrepMatch),
}

/// A picker's payload type varies by what it's picking between, but
/// `App` only ever has one active at a time -- an enum, not three
/// separate `Option` fields, the same reasoning `ExplorerPrompt` already
/// uses for "one thing, a few kinds."
/// How many commands the palette remembers as run lately.
const RECENT_COMMANDS: usize = 30;

enum ActivePicker {
    FindFile(fenix_picker::PickerState<PathBuf>),
    /// `SPC u u` in a project with no launch configuration: the ones the
    /// extensions offer for its language (`launch/*.toml`).
    Launch(fenix_picker::PickerState<crate::dap::Template>),
    /// `SPC :` / Alt-x, the command palette: every command, payload its
    /// id -- confirming runs it (`queued_command`), or, from `SPC h c`
    /// (`palette_describes`), says what it is.
    Commands(fenix_picker::PickerState<String>),
    /// `SPC h b`: the keys bound here -- the leader's and this buffer's
    /// `SPC m` -- payload the command, which confirming runs.
    Bindings(fenix_picker::PickerState<String>),
    /// `SPC s t`: the TODO-style comments in the focused buffer, payload
    /// the keyword's char offset -- confirming jumps there, the same as
    /// `BufferSearch`.
    BufferTodos(fenix_picker::PickerState<usize>),
    /// `SPC s T`: the TODO-style comments across the project, as grep
    /// matches -- confirming behaves exactly like `Grep`'s, and the same
    /// list becomes the quickfix list for `SPC p n`/`SPC p N`.
    ProjectTodos(fenix_picker::PickerState<fenix_project::GrepMatch>),
    /// `code.problems` (Ctrl+Shift+M): every problem reported, in every
    /// file, as places to go -- confirming goes there, as `ProjectTodos`.
    Problems(fenix_picker::PickerState<fenix_project::GrepMatch>),
    /// `code.lens` (`SPC c l`): the cursor line's code lenses -- confirming
    /// runs the one chosen.
    Lenses(fenix_picker::PickerState<code_lens::LensPick>),
    /// `code.project_symbols` (Ctrl+T): what the language servers find
    /// for what's typed (`workspace/symbol`), asked again as it changes.
    ProjectSymbols(fenix_picker::PickerState<fenix_project::GrepMatch>),
    SwitchProject(fenix_picker::PickerState<PathBuf>),
    SwitchBuffer(fenix_picker::PickerState<BufferId>),
    /// `SPC f v`: unsaved work a previous session left behind, one row
    /// per file. Confirming puts the recovered text back into the
    /// buffer as an unsaved edit -- so `:w` accepts it and `:e!`
    /// throws it away, which is the vocabulary the file-watching work
    /// already established for "these two versions disagree".
    Recovery(fenix_picker::PickerState<fenix_recovery::Snapshot>),
    /// `SPC e b`/`SPC e R`, and the share list a server answers with:
    /// somewhere to go. One variant for all three because the payload
    /// and the confirm action are identical -- a path, opened -- and
    /// three variants that did the same thing would be three places for
    /// them to drift apart.
    Places(fenix_picker::PickerState<PathBuf>),
    /// `SPC a`'s pickers: an agent, a thread, a mode, an option, a login
    /// (`agents::Pick`).
    Agents(fenix_picker::PickerState<agents::Pick>),
    /// `SPC p d`: same candidate list as `SwitchProject`, but confirming
    /// removes the selected root from `known_projects` instead of
    /// switching to it.
    DeleteProject(fenix_picker::PickerState<PathBuf>),
    /// `SPC p R`: a machine to open a project on -- the ones connected,
    /// the hosts of `~/.ssh/config`, or whatever is typed.
    RemoteHost(fenix_picker::PickerState<String>),
    /// `SPC t p`: jump straight to a specific theme by name, fuzzy-
    /// filtered over `theme::ALL` -- confirming applies it via
    /// `apply_theme`.
    Theme(fenix_picker::PickerState<&'static Theme>),
    /// `SPC f E` and `file.save_with_encoding`: which encoding, and whether
    /// to read the file with it (or else save it in it).
    Encoding(fenix_picker::PickerState<(fenix_core::TextEncoding, bool)>),
    Snippet(fenix_picker::PickerState<snippets::SnippetChoice>),
    /// `SPC c s`: fuzzy-find any known Tcl definition by its fully-
    /// qualified name (same `ctags`-sourced list `tcl_candidates`'s own
    /// `Tag`-kind completion entries come from, see `tcl_tags`) --
    /// confirming jumps straight to where it's defined, the payload
    /// carrying the file/line `tcl_candidates`'s own `CompletionItem`
    /// conversion discards.
    Symbol(fenix_picker::PickerState<fenix_completion::ctags::TagEntry>),
    /// `SPC p t`: fuzzy-find a discovered task (`fenix_tasks::
    /// discover_tasks` -- built-in per-project-marker defaults plus any
    /// `.fenix/project.ini` `[tasks]` overrides) -- confirming runs it
    /// (`App::run_task`) in the Task Output panel.
    /// Each task with the project it runs in.
    Task(fenix_picker::PickerState<(fenix_tasks::TaskDef, PathBuf)>),
    /// `SPC g c`: the ref to compare HEAD with (the Compare page).
    CompareTarget(fenix_picker::PickerState<String>),
    /// `R` on the Compare page: your own side, other than HEAD. Carries
    /// the ref it's compared with, since confirming needs both.
    CompareOwn { target: String, picker: fenix_picker::PickerState<String> },
    /// `SPC g v`: the ref to set the file at `path` beside.
    Versus { path: PathBuf, picker: fenix_picker::PickerState<String> },
    /// Enter on a settings-page row with more choices than `h`/`l` should
    /// step through (the installed fonts): one of them, by name. What
    /// it's for is `key`; confirming hands the choice back to the page.
    SettingChoice { key: &'static str, picker: fenix_picker::PickerState<String> },

    /// `SPC g r`: pick the ref to replay the current branch onto.
    RebaseOnto(fenix_picker::PickerState<String>),
    /// `SPC g m`: pick the ref to merge into the current branch.
    MergeFrom(fenix_picker::PickerState<String>),
    /// The Git status page's `b b`: a branch to switch to -- local ones
    /// first, then remote branches with no local counterpart.
    SwitchBranch(fenix_picker::PickerState<String>),

    /// `c` on a `BufferKind::Table` buffer: fuzzy-find a column by its
    /// synthesized name, confirming jumps the cursor to that column's
    /// start on the current row (`table_column_start`). The payload is
    /// the column's 0-indexed tab-occurrence position, exactly what
    /// `table_column_start` takes.
    TableColumn(fenix_picker::PickerState<usize>),
    /// `SPC s l`: fuzzy-find any line in the focused buffer, live-
    /// filtered by `PickerState`'s own already-built fuzzy matching --
    /// candidates are one per buffer line (label `"{line:>4}  {trimmed
    /// text}"`, payload that line's start char offset), confirming
    /// jumps the cursor there.
    BufferSearch(fenix_picker::PickerState<usize>),
    /// `SPC TAB TAB`: a fuzzy picker over every currently open
    /// workspace, by name -- picking one directly rather than relative
    /// `]`/`[` cycling. Payload is the workspace's index, exactly what
    /// `WorkspaceList::switch_to_index` takes; the active one's label
    /// is marked so re-opening the picker shows at a glance which
    /// workspace you're already on.
    SwitchWorkspace(fenix_picker::PickerState<usize>),
    /// `SPC TAB f`: `config.workspaces`' configured launcher list, one
    /// candidate per `[workspaces]` entry, a hand-configured shelf of
    /// one candidate list. Payload is the entry's own name; confirming
    /// switches to an already-open workspace of that name or else runs
    /// the configured action to create one (`open_configured_
    /// workspace`).
    WorkspaceLauncher(fenix_picker::PickerState<String>),
    /// `SPC c o`: a fuzzy picker over every ATX heading in the focused
    /// Markdown buffer -- same "one candidate per matching line, payload
    /// its start char offset" shape `BufferSearch` already established,
    /// just filtered to headings and labeled by nesting depth
    /// (`markdown::heading_label`) instead of a line number.
    Outline(fenix_picker::PickerState<usize>),
    /// A code extension's picker (`wasm_host`): what's picked goes back to it.
    /// An extension's; `many` when Tab marks rows (`show-picker-many`).
    Code { ext: String, id: u64, badge: &'static str, many: bool, state: fenix_picker::PickerState<String> },
}

// The three `ActivePicker` variants wrap `PickerState<T>` for different
// `T`, so there's no single method call that works across all of them --
// these free functions do the one-line match each caller would otherwise
// repeat.
fn picker_push_char(picker: &mut ActivePicker, c: char) {
    match picker {
        ActivePicker::FindFile(s) => s.push_char(c),
        ActivePicker::SwitchProject(s) => s.push_char(c),
        ActivePicker::SwitchBuffer(s) => s.push_char(c),
        ActivePicker::Recovery(s) => s.push_char(c),
        ActivePicker::Places(s) => s.push_char(c),
        ActivePicker::Agents(s) => s.push_char(c),
        ActivePicker::DeleteProject(s) => s.push_char(c),
        ActivePicker::RemoteHost(s) => s.push_char(c),
        ActivePicker::Encoding(s) => s.push_char(c),
        ActivePicker::Theme(s) => s.push_char(c),
        ActivePicker::Commands(s) => s.push_char(c),
        ActivePicker::Bindings(s) => s.push_char(c),
        ActivePicker::Snippet(s) => s.push_char(c),
        ActivePicker::Symbol(s) => s.push_char(c),
        ActivePicker::TableColumn(s) => s.push_char(c),
        ActivePicker::BufferSearch(s) => s.push_char(c),
        ActivePicker::BufferTodos(s) => s.push_char(c),
        ActivePicker::ProjectTodos(s) => s.push_char(c),
        ActivePicker::Problems(s) => s.push_char(c),
        ActivePicker::Lenses(s) => s.push_char(c),
        ActivePicker::ProjectSymbols(s) => s.push_char(c),
        ActivePicker::SwitchWorkspace(s) => s.push_char(c),
        ActivePicker::WorkspaceLauncher(s) => s.push_char(c),
        ActivePicker::Outline(s) => s.push_char(c),
        ActivePicker::Task(s) => s.push_char(c),
        ActivePicker::Launch(s) => s.push_char(c),
        ActivePicker::CompareTarget(s) => s.push_char(c),
        ActivePicker::RebaseOnto(s) => s.push_char(c),
        ActivePicker::MergeFrom(s) => s.push_char(c),
        ActivePicker::SwitchBranch(s) => s.push_char(c),
        ActivePicker::Code { state: s, .. } => s.push_char(c),
        ActivePicker::CompareOwn { picker, .. } => picker.push_char(c),
        ActivePicker::Versus { picker, .. } => picker.push_char(c),
        ActivePicker::SettingChoice { picker, .. } => picker.push_char(c),
    }
}

fn picker_backspace(picker: &mut ActivePicker) {
    match picker {
        ActivePicker::FindFile(s) => s.backspace(),
        ActivePicker::SwitchProject(s) => s.backspace(),
        ActivePicker::SwitchBuffer(s) => s.backspace(),
        ActivePicker::Recovery(s) => s.backspace(),
        ActivePicker::Places(s) => s.backspace(),
        ActivePicker::Agents(s) => s.backspace(),
        ActivePicker::DeleteProject(s) => s.backspace(),
        ActivePicker::RemoteHost(s) => s.backspace(),
        ActivePicker::Encoding(s) => s.backspace(),
        ActivePicker::Theme(s) => s.backspace(),
        ActivePicker::Commands(s) => s.backspace(),
        ActivePicker::Bindings(s) => s.backspace(),
        ActivePicker::Snippet(s) => s.backspace(),
        ActivePicker::Symbol(s) => s.backspace(),
        ActivePicker::TableColumn(s) => s.backspace(),
        ActivePicker::BufferSearch(s) => s.backspace(),
        ActivePicker::BufferTodos(s) => s.backspace(),
        ActivePicker::ProjectTodos(s) => s.backspace(),
        ActivePicker::Problems(s) => s.backspace(),
        ActivePicker::Lenses(s) => s.backspace(),
        ActivePicker::ProjectSymbols(s) => s.backspace(),
        ActivePicker::SwitchWorkspace(s) => s.backspace(),
        ActivePicker::WorkspaceLauncher(s) => s.backspace(),
        ActivePicker::Outline(s) => s.backspace(),
        ActivePicker::Task(s) => s.backspace(),
        ActivePicker::Launch(s) => s.backspace(),
        ActivePicker::CompareTarget(s) => s.backspace(),
        ActivePicker::RebaseOnto(s) => s.backspace(),
        ActivePicker::MergeFrom(s) => s.backspace(),
        ActivePicker::SwitchBranch(s) => s.backspace(),
        ActivePicker::Code { state: s, .. } => s.backspace(),
        ActivePicker::CompareOwn { picker, .. } => picker.backspace(),
        ActivePicker::Versus { picker, .. } => picker.backspace(),
        ActivePicker::SettingChoice { picker, .. } => picker.backspace(),
    }
}

fn picker_move_selection(picker: &mut ActivePicker, delta: isize) {
    match picker {
        ActivePicker::FindFile(s) => s.move_selection(delta),
        ActivePicker::SwitchProject(s) => s.move_selection(delta),
        ActivePicker::SwitchBuffer(s) => s.move_selection(delta),
        ActivePicker::Recovery(s) => s.move_selection(delta),
        ActivePicker::Places(s) => s.move_selection(delta),
        ActivePicker::Agents(s) => s.move_selection(delta),
        ActivePicker::DeleteProject(s) => s.move_selection(delta),
        ActivePicker::RemoteHost(s) => s.move_selection(delta),
        ActivePicker::Encoding(s) => s.move_selection(delta),
        ActivePicker::Theme(s) => s.move_selection(delta),
        ActivePicker::Commands(s) => s.move_selection(delta),
        ActivePicker::Bindings(s) => s.move_selection(delta),
        ActivePicker::Snippet(s) => s.move_selection(delta),
        ActivePicker::Symbol(s) => s.move_selection(delta),
        ActivePicker::TableColumn(s) => s.move_selection(delta),
        ActivePicker::BufferSearch(s) => s.move_selection(delta),
        ActivePicker::BufferTodos(s) => s.move_selection(delta),
        ActivePicker::ProjectTodos(s) => s.move_selection(delta),
        ActivePicker::Problems(s) => s.move_selection(delta),
        ActivePicker::Lenses(s) => s.move_selection(delta),
        ActivePicker::ProjectSymbols(s) => s.move_selection(delta),
        ActivePicker::SwitchWorkspace(s) => s.move_selection(delta),
        ActivePicker::WorkspaceLauncher(s) => s.move_selection(delta),
        ActivePicker::Outline(s) => s.move_selection(delta),
        ActivePicker::Task(s) => s.move_selection(delta),
        ActivePicker::Launch(s) => s.move_selection(delta),
        ActivePicker::CompareTarget(s) => s.move_selection(delta),
        ActivePicker::RebaseOnto(s) => s.move_selection(delta),
        ActivePicker::MergeFrom(s) => s.move_selection(delta),
        ActivePicker::SwitchBranch(s) => s.move_selection(delta),
        ActivePicker::Code { state: s, .. } => s.move_selection(delta),
        ActivePicker::CompareOwn { picker, .. } => picker.move_selection(delta),
        ActivePicker::Versus { picker, .. } => picker.move_selection(delta),
        ActivePicker::SettingChoice { picker, .. } => picker.move_selection(delta),
    }
}

/// `Tab` on any picker -- toggles the mark on the currently selected row.
/// Harmless no-op for every single-select variant (nothing ever reads
/// their marks); real effect only for `JiraStatusFilter` today.
fn picker_toggle_mark(picker: &mut ActivePicker) {
    match picker {
        ActivePicker::FindFile(s) => s.toggle_mark(),
        ActivePicker::SwitchProject(s) => s.toggle_mark(),
        ActivePicker::SwitchBuffer(s) => s.toggle_mark(),
        ActivePicker::Recovery(s) => s.toggle_mark(),
        ActivePicker::Places(s) => s.toggle_mark(),
        ActivePicker::Agents(s) => s.toggle_mark(),
        ActivePicker::DeleteProject(s) => s.toggle_mark(),
        ActivePicker::RemoteHost(s) => s.toggle_mark(),
        ActivePicker::Encoding(s) => s.toggle_mark(),
        ActivePicker::Theme(s) => s.toggle_mark(),
        ActivePicker::Commands(s) => s.toggle_mark(),
        ActivePicker::Bindings(s) => s.toggle_mark(),
        ActivePicker::Snippet(s) => s.toggle_mark(),
        ActivePicker::Symbol(s) => s.toggle_mark(),
        ActivePicker::TableColumn(s) => s.toggle_mark(),
        ActivePicker::BufferSearch(s) => s.toggle_mark(),
        ActivePicker::BufferTodos(s) => s.toggle_mark(),
        ActivePicker::ProjectTodos(s) => s.toggle_mark(),
        ActivePicker::Problems(s) => s.toggle_mark(),
        ActivePicker::Lenses(s) => s.toggle_mark(),
        ActivePicker::ProjectSymbols(s) => s.toggle_mark(),
        ActivePicker::SwitchWorkspace(s) => s.toggle_mark(),
        ActivePicker::WorkspaceLauncher(s) => s.toggle_mark(),
        ActivePicker::Outline(s) => s.toggle_mark(),
        ActivePicker::Task(s) => s.toggle_mark(),
        ActivePicker::Launch(s) => s.toggle_mark(),
        ActivePicker::CompareTarget(s) => s.toggle_mark(),
        ActivePicker::RebaseOnto(s) => s.toggle_mark(),
        ActivePicker::MergeFrom(s) => s.toggle_mark(),
        ActivePicker::SwitchBranch(s) => s.toggle_mark(),
        ActivePicker::Code { state: s, .. } => s.toggle_mark(),
        ActivePicker::CompareOwn { picker, .. } => picker.toggle_mark(),
        ActivePicker::Versus { picker, .. } => picker.toggle_mark(),
        ActivePicker::SettingChoice { picker, .. } => picker.toggle_mark(),
    }
}

fn picker_query(picker: &ActivePicker) -> &str {
    match picker {
        ActivePicker::FindFile(s) => s.query(),
        ActivePicker::SwitchProject(s) => s.query(),
        ActivePicker::SwitchBuffer(s) => s.query(),
        ActivePicker::Recovery(s) => s.query(),
        ActivePicker::Places(s) => s.query(),
        ActivePicker::Agents(s) => s.query(),
        ActivePicker::DeleteProject(s) => s.query(),
        ActivePicker::RemoteHost(s) => s.query(),
        ActivePicker::Encoding(s) => s.query(),
        ActivePicker::Theme(s) => s.query(),
        ActivePicker::Commands(s) => s.query(),
        ActivePicker::Bindings(s) => s.query(),
        ActivePicker::Snippet(s) => s.query(),
        ActivePicker::Symbol(s) => s.query(),
        ActivePicker::TableColumn(s) => s.query(),
        ActivePicker::BufferSearch(s) => s.query(),
        ActivePicker::BufferTodos(s) => s.query(),
        ActivePicker::ProjectTodos(s) => s.query(),
        ActivePicker::Problems(s) => s.query(),
        ActivePicker::Lenses(s) => s.query(),
        ActivePicker::ProjectSymbols(s) => s.query(),
        ActivePicker::SwitchWorkspace(s) => s.query(),
        ActivePicker::WorkspaceLauncher(s) => s.query(),
        ActivePicker::Outline(s) => s.query(),
        ActivePicker::Task(s) => s.query(),
        ActivePicker::Launch(s) => s.query(),
        ActivePicker::CompareTarget(s) => s.query(),
        ActivePicker::RebaseOnto(s) => s.query(),
        ActivePicker::MergeFrom(s) => s.query(),
        ActivePicker::SwitchBranch(s) => s.query(),
        ActivePicker::Code { state: s, .. } => s.query(),
        ActivePicker::CompareOwn { picker, .. } => picker.query(),
        ActivePicker::Versus { picker, .. } => picker.query(),
        ActivePicker::SettingChoice { picker, .. } => picker.query(),
    }
}

fn picker_len(picker: &ActivePicker) -> usize {
    match picker {
        ActivePicker::FindFile(s) => s.len(),
        ActivePicker::SwitchProject(s) => s.len(),
        ActivePicker::SwitchBuffer(s) => s.len(),
        ActivePicker::Recovery(s) => s.len(),
        ActivePicker::Places(s) => s.len(),
        ActivePicker::Agents(s) => s.len(),
        ActivePicker::DeleteProject(s) => s.len(),
        ActivePicker::RemoteHost(s) => s.len(),
        ActivePicker::Encoding(s) => s.len(),
        ActivePicker::Theme(s) => s.len(),
        ActivePicker::Commands(s) => s.len(),
        ActivePicker::Bindings(s) => s.len(),
        ActivePicker::Snippet(s) => s.len(),
        ActivePicker::Symbol(s) => s.len(),
        ActivePicker::TableColumn(s) => s.len(),
        ActivePicker::BufferSearch(s) => s.len(),
        ActivePicker::BufferTodos(s) => s.len(),
        ActivePicker::ProjectTodos(s) => s.len(),
        ActivePicker::Problems(s) => s.len(),
        ActivePicker::Lenses(s) => s.len(),
        ActivePicker::ProjectSymbols(s) => s.len(),
        ActivePicker::SwitchWorkspace(s) => s.len(),
        ActivePicker::WorkspaceLauncher(s) => s.len(),
        ActivePicker::Outline(s) => s.len(),
        ActivePicker::Task(s) => s.len(),
        ActivePicker::Launch(s) => s.len(),
        ActivePicker::CompareTarget(s) => s.len(),
        ActivePicker::RebaseOnto(s) => s.len(),
        ActivePicker::MergeFrom(s) => s.len(),
        ActivePicker::SwitchBranch(s) => s.len(),
        ActivePicker::Code { state: s, .. } => s.len(),
        ActivePicker::CompareOwn { picker, .. } => picker.len(),
        ActivePicker::Versus { picker, .. } => picker.len(),
        ActivePicker::SettingChoice { picker, .. } => picker.len(),
    }
}

fn picker_selected_row(picker: &ActivePicker) -> usize {
    match picker {
        ActivePicker::FindFile(s) => s.selected_row(),
        ActivePicker::SwitchProject(s) => s.selected_row(),
        ActivePicker::SwitchBuffer(s) => s.selected_row(),
        ActivePicker::Recovery(s) => s.selected_row(),
        ActivePicker::Places(s) => s.selected_row(),
        ActivePicker::Agents(s) => s.selected_row(),
        ActivePicker::DeleteProject(s) => s.selected_row(),
        ActivePicker::RemoteHost(s) => s.selected_row(),
        ActivePicker::Encoding(s) => s.selected_row(),
        ActivePicker::Theme(s) => s.selected_row(),
        ActivePicker::Commands(s) => s.selected_row(),
        ActivePicker::Bindings(s) => s.selected_row(),
        ActivePicker::Snippet(s) => s.selected_row(),
        ActivePicker::Symbol(s) => s.selected_row(),
        ActivePicker::TableColumn(s) => s.selected_row(),
        ActivePicker::BufferSearch(s) => s.selected_row(),
        ActivePicker::BufferTodos(s) => s.selected_row(),
        ActivePicker::ProjectTodos(s) => s.selected_row(),
        ActivePicker::Problems(s) => s.selected_row(),
        ActivePicker::Lenses(s) => s.selected_row(),
        ActivePicker::ProjectSymbols(s) => s.selected_row(),
        ActivePicker::SwitchWorkspace(s) => s.selected_row(),
        ActivePicker::WorkspaceLauncher(s) => s.selected_row(),
        ActivePicker::Outline(s) => s.selected_row(),
        ActivePicker::Task(s) => s.selected_row(),
        ActivePicker::Launch(s) => s.selected_row(),
        ActivePicker::CompareTarget(s) => s.selected_row(),
        ActivePicker::RebaseOnto(s) => s.selected_row(),
        ActivePicker::MergeFrom(s) => s.selected_row(),
        ActivePicker::SwitchBranch(s) => s.selected_row(),
        ActivePicker::Code { state: s, .. } => s.selected_row(),
        ActivePicker::CompareOwn { picker, .. } => picker.selected_row(),
        ActivePicker::Versus { picker, .. } => picker.selected_row(),
        ActivePicker::SettingChoice { picker, .. } => picker.selected_row(),
    }
}

/// Labels only (rendering doesn't need the payload) for the windowed
/// slice `[offset, offset + count)`, paired with whether each is the
/// current selection -- same windowing shape `ExplorerState`'s rendering
/// already uses for a directory listing.
#[cfg(test)]
fn picker_visible_labels(picker: &ActivePicker, offset: usize, count: usize) -> Vec<(bool, String)> {
    match picker {
        ActivePicker::FindFile(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::SwitchProject(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::SwitchBuffer(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::Recovery(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::Places(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::Agents(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::DeleteProject(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::RemoteHost(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::Encoding(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::Theme(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::Commands(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::Bindings(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::Snippet(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::Symbol(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::TableColumn(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::BufferSearch(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::BufferTodos(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::ProjectTodos(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::Problems(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::Lenses(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::ProjectSymbols(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::SwitchWorkspace(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::WorkspaceLauncher(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::Outline(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::Task(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::Launch(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::CompareTarget(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::RebaseOnto(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::MergeFrom(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::SwitchBranch(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::Code { state: s, .. } => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::CompareOwn { picker, .. } => picker.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::Versus { picker, .. } => picker.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::SettingChoice { picker, .. } => picker.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
    }
}

/// A task picker row: the task's name, its command line, and where it
/// comes from, so two tasks of one name can be told apart.
fn task_picker_label(task: &fenix_tasks::TaskDef, source: &str) -> String {
    let command = std::iter::once(task.command.as_str()).chain(task.args.iter().map(String::as_str)).collect::<Vec<_>>().join(" ");
    // A name that is already its command line (`cargo build`) says it.
    if command == task.name || command.starts_with(&format!("{} ", task.name)) {
        return format!("{}   [{source}]", task.name);
    }
    format!("{}   {command}   [{source}]", task.name)
}

/// Where the explorer's own "places I have been" list lives.
///
/// Somewhere throwaway under test. The tests here open real directories
/// through the real code path, which records every one of them -- and
/// without this they land in the user's actual list, which is how the
/// crash-recovery work found the same mistake in itself. A test suite
/// must not leave its temp directories in the user's history.
/// Under test, a file of `name` no other `App` shares: tests run in
/// parallel, and one that saves a MIB root or a project must not leak it
/// into another -- or into the Fenix you actually use.
fn isolated_test_path(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("fenix-test-state-{}", std::process::id())).join(format!("{n}-{name}"))
}

fn default_project_meta_path() -> PathBuf {
    if cfg!(test) {
        isolated_test_path("project_meta.json")
    } else {
        fenix_project::meta::ProjectMeta::default_path().unwrap_or_else(|| PathBuf::from("project_meta.json"))
    }
}

fn default_recent_dirs_path() -> PathBuf {
    if cfg!(test) {
        std::env::temp_dir().join(format!("fenix-test-recent-dirs-{}.txt", std::process::id()))
    } else {
        fenix_project::RecentFiles::default_dirs_path().unwrap_or_else(|| PathBuf::from("recent_dirs.txt"))
    }
}

/// A path as a person would read it, without Windows' extended-length
/// `\\?\` prefix.
///
/// Canonicalising a path on Windows adds that prefix, so anything that
/// has been through `canonicalize` -- every registered project root --
/// shows up wearing it. It means nothing to a reader and makes an
/// otherwise familiar path unrecognisable at a glance.
fn readable_path(path: &Path) -> String {
    // `normalize` unwraps the UNC form, `\\?\UNC\server\share`, back to
    // `\\server\share`, and on Windows makes the separators one kind.
    fenix_lsp::normalize(path.to_path_buf()).display().to_string()
}

/// A path's own name, for a message that has already said where.
fn file_label(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| readable_path(path))
}

/// Smallest adjustment to `scroll_line` that brings `cursor_line` into the
/// `[scroll_line, scroll_line + visible_lines)` window.
fn scroll_to_include(scroll_line: usize, cursor_line: usize, visible_lines: usize) -> usize {
    if cursor_line < scroll_line {
        cursor_line
    } else if cursor_line >= scroll_line + visible_lines {
        cursor_line + 1 - visible_lines
    } else {
        scroll_line
    }
}

/// Where a pane's view goes for its cursor on `cursor_line`, `total`
/// display lines long: as Vim does it, a cursor more than half a window
/// past the view's edge -- a jump to a symbol, a search result, a
/// definition, another file -- is put in the middle of the window, as
/// `zz` would; one that only stepped past the edge (`j` on the last
/// line) scrolls just far enough to show it. A jump near the end of the
/// file keeps the last line at the bottom rather than scrolling past it.
/// `jumped`: the cursor got there by a jump (a picker, a search result,
/// a definition, the jumplist), which is centred whenever it's out of
/// view at all -- a function found just past the bottom edge is shown
/// with its body, not on the last row.
fn scroll_for_cursor(scroll_line: usize, cursor_line: usize, visible_lines: usize, total: usize, jumped: bool) -> usize {
    let visible = visible_lines.max(1);
    let past = if cursor_line < scroll_line {
        scroll_line - cursor_line
    } else if cursor_line >= scroll_line + visible {
        cursor_line + 1 - (scroll_line + visible)
    } else {
        0
    };
    if past > visible / 2 || (jumped && past > 0) {
        // Never past the last line's own top -- but always showing the cursor.
        let last_top = total.saturating_sub(visible).max(cursor_line.saturating_sub(visible - 1));
        cursor_line.saturating_sub(visible / 2).min(last_top)
    } else {
        scroll_to_include(scroll_line, cursor_line, visible)
    }
}

/// Ease-out cubic: fast start, gentle settle. Used for every short
/// transition animation (caret fade now; scroll/pulse reuse it too).
fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// A pane-relative `(row, col)` caret position, in window pixel
/// coordinates -- shared by the real caret rect and the completion
/// popup's `BelowPoint` anchor, which both need to land on exactly the
/// same spot.
/// Where the focused page is drawn, for placing its popup.
#[derive(Debug, Clone, Copy)]
struct PagePopupAt {
    buffer: BufferId,
    pane: fenix_window::Rect,
    /// The first page line in view, and how far into it the pane has
    /// scrolled.
    first_line: usize,
    frac: f32,
    gutter_px: f32,
}

/// A `width` x `height` popup beside page line `line`: under it with its
/// text starting at column `col`, or over it when there's no room below
/// `bottom` -- kept inside the pane and the window. `None` while the line
/// is scrolled out of view.
#[allow(clippy::too_many_arguments)]
fn popup_beside_row(
    at: PagePopupAt,
    line: usize,
    col: usize,
    width: f32,
    height: f32,
    bottom: f32,
    window_width: f32,
    char_width: f32,
    line_height: f32,
) -> Option<fenix_window::Rect> {
    let row = line.checked_sub(at.first_line)?;
    let (x, top) = caret_pixel_pos(at.pane, row, col, at.gutter_px, at.frac, char_width, line_height);
    if top >= bottom {
        return None;
    }
    const GAP: f32 = 2.0;
    let below = top + line_height + GAP;
    let y = if below + height <= bottom {
        below
    } else if top - GAP - height >= at.pane.y {
        top - GAP - height
    } else {
        // Room on neither side: as low as it fits.
        (bottom - height).max(at.pane.y)
    };
    let right = (at.pane.x + at.pane.w).min(window_width);
    let x = (x - text::PAD_LEFT).min(right - width).max(at.pane.x.max(0.0));
    Some(fenix_window::Rect { x, y, w: width, h: height })
}

/// A key in a prompt whose prefilled `input` is selected (`selected`):
/// the selection ends with any key; a character or a paste replaces the
/// text, Backspace or Delete clears it (and is spent). Returns whether
/// the key was spent here.
fn take_selected_prefill(input: &mut String, selected: &mut bool, key: KeyPress) -> bool {
    if !std::mem::take(selected) {
        return false;
    }
    match key.code {
        KeyCode::Char(_) if !key.mods.alt && !key.mods.super_ && (!key.mods.ctrl || key == KeyPress::char('v').with_ctrl()) => {
            input.clear();
            false
        }
        KeyCode::Named(FenixNamedKey::Backspace | FenixNamedKey::Delete) => {
            input.clear();
            true
        }
        _ => false,
    }
}

/// Ctrl+Backspace in a one-line prompt: the word before the end goes,
/// with any spaces or punctuation after it -- `count_words` all at once,
/// `src/lib` back to `src/`.
fn delete_word_back(input: &mut String) {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    while input.ends_with(|c: char| !is_word(c)) {
        input.pop();
    }
    while input.ends_with(is_word) {
        input.pop();
    }
}

fn caret_pixel_pos(rect: fenix_window::Rect, row: usize, col: usize, gutter_px: f32, content_frac: f32, char_width: f32, line_height: f32) -> (f32, f32) {
    let content_x = rect.x + text::PAD_LEFT + gutter_px;
    let x = content_x + col as f32 * char_width;
    let y = rect.y + text::PAD_TOP + row as f32 * line_height - content_frac * line_height;
    (x, y)
}

/// A themed glyph-text color (`glyphon::Color`, 0-255 `u8` channels) in
/// `bg_rect`'s own rect-fill representation (`[f32; 4]`, 0.0-1.0 channels)
/// -- the inline git gutter marks (Part 5) are the one place a `Theme`'s
/// text-shaped git colors (`git_staged`/`git_modified`/`git_conflicted`)
/// need to paint a flat rect rather than color glyphs, so this bridges
/// the same two representations every `rgba`/`text_color` pair in
/// `theme.rs` already keeps separate for every other field.
fn glyphon_to_rgba(color: glyphon::Color) -> [f32; 4] {
    [color.r() as f32 / 255.0, color.g() as f32 / 255.0, color.b() as f32 / 255.0, 1.0]
}

/// The inverse of `glyphon_to_rgba` -- the modeline's mode label now
/// renders *in* its mode's accent color (Signal Rail's rail-plus-label
/// treatment) rather than as contrasting text over a filled badge box,
/// so the same `[f32; 4]` `mode_colors()` already hands the rail needs
/// to also become a `glyphon::Color` for the label's own glyphs.
fn rgba_to_glyphon(color: [f32; 4]) -> glyphon::Color {
    glyphon::Color::rgb((color[0] * 255.0).round() as u8, (color[1] * 255.0).round() as u8, (color[2] * 255.0).round() as u8)
}

/// Skips the first `n` characters of `s` for horizontal scroll --
/// returns the remaining substring plus how many *bytes* that was, so
/// a caller matching byte-offset syntax-highlight ranges against the
/// substring can shift its own coordinate space by exactly that much.
/// `n` at or past `s`'s own character count returns an empty
/// remainder (nothing left to show once scrolled past a short line)
/// and `s.len()` skipped bytes.
fn skip_chars(s: &str, n: usize) -> (&str, usize) {
    match s.char_indices().nth(n) {
        Some((byte_idx, _)) => (&s[byte_idx..], byte_idx),
        None => ("", s.len()),
    }
}

/// The most of one line a row hands to text shaping, from where it's
/// scrolled to: more than any pane shows. Shaping is the slow part of a
/// frame, and a line of minified JSON can be megabytes long.
const MAX_ROW_CHARS: usize = 4096;

/// Past this many characters on screen (so some line is very long), the
/// grammar is asked about what `MAX_ROW_CHARS` leaves of each row, not the
/// whole of the lines.
const MAX_HIGHLIGHT_CHARS: usize = 1 << 16;

/// `s` up to its `n`th character.
fn keep_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((byte_idx, _)) => &s[..byte_idx],
        None => s,
    }
}

/// Splits `line_text` into colored sub-spans according to `highlights`
/// (document-wide byte ranges, already resolved to colors, sorted and
/// non-overlapping -- the contract `fenix_syntax::SyntaxState::
/// highlights_in_range` guarantees) that overlap `[line_byte_start,
/// line_byte_end)`. Gaps between them fall back to `default_color`. Pure
/// and independent of `Buffer`/`App` so it's testable with hand-built
/// strings and ranges, not a real parsed buffer.
fn split_line_by_highlights(
    line_text: &str,
    line_byte_start: usize,
    line_byte_end: usize,
    highlights: &[(std::ops::Range<usize>, glyphon::Color)],
    default_color: glyphon::Color,
) -> Vec<(String, glyphon::Color)> {
    let mut spans = Vec::new();
    let mut cursor = 0usize; // byte offset within line_text
    for (range, color) in highlights {
        if range.end <= line_byte_start || range.start >= line_byte_end {
            continue;
        }
        let local_start = range.start.max(line_byte_start) - line_byte_start;
        let local_end = range.end.min(line_byte_end) - line_byte_start;
        if local_start > cursor {
            spans.push((line_text[cursor..local_start].to_string(), default_color));
        }
        if local_end > local_start {
            spans.push((line_text[local_start..local_end].to_string(), *color));
        }
        cursor = cursor.max(local_end);
    }
    if cursor < line_text.len() {
        spans.push((line_text[cursor..].to_string(), default_color));
    }
    if spans.is_empty() {
        spans.push((line_text.to_string(), default_color));
    }
    spans
}

/// The byte ranges of `buffer` highlighting is asked about for `rows`
/// rows from display line `base`: all of them at once, or, past
/// `MAX_HIGHLIGHT_CHARS`, each line's columns shown (`columns`, by line,
/// for a wrapped pane; from `scroll_col` otherwise). `None` past the end.
fn highlight_windows(buffer: &Buffer, base: usize, rows: usize, scroll_col: usize, columns: Option<&HashMap<usize, (usize, usize)>>) -> Option<Vec<std::ops::Range<usize>>> {
    let visual_lines = buffer.visual_line_count();
    if base >= visual_lines {
        return None;
    }
    let last_line = (base + rows).min(visual_lines) - 1;
    let start_char = buffer.line_start_char(base);
    let end_char = buffer.line_start_char(last_line) + buffer.line_len(last_line);
    if end_char - start_char <= MAX_HIGHLIGHT_CHARS {
        let everything = buffer.char_to_byte(start_char)..buffer.char_to_byte(end_char);
        return Some(vec![everything]);
    }
    Some(
        (base..=last_line)
            .map(|line| {
                let first = buffer.line_start_char(line);
                let len = buffer.line_len(line);
                let (from, to) = columns.and_then(|c| c.get(&line)).copied().unwrap_or((scroll_col, scroll_col + MAX_ROW_CHARS));
                let (from, to) = (first + from.min(len), first + to.min(len));
                buffer.char_to_byte(from)..buffer.char_to_byte(to)
            })
            .filter(|range| !range.is_empty())
            .collect(),
    )
}

/// Remaps `highlights` (document-wide byte ranges against the buffer's
/// *real* text) onto `display`'s own local byte offsets, for a line that
/// contains a `\t` and was tab-expanded into `display` by `tabstops::
/// expand_line` -- so a tab-indented line in a real language (C, Bash,
/// Python, ... all have tree-sitter grammars registered in this project,
/// and commonly use tab indentation) still gets correct syntax
/// highlighting instead of falling back to plain text.
///
/// Leans on the same "1 char = 1 visual column" invariant the rest of
/// this renderer already assumes: every char `expand_line` writes into
/// `display` (an original character, or one space per padded column)
/// advances the visual column by exactly one, so a visual column *is*
/// a char index into `display` -- `col_map` (original char column ->
/// visual column) is directly usable as "char index into `display`"
/// once a highlight's buffer-wide byte range is converted to a line-
/// relative char range first.
fn remap_highlights_for_display(
    buffer: &Buffer,
    line_start_char: usize,
    col_map: &[usize],
    display: &str,
    line_byte_start: usize,
    line_byte_end: usize,
    highlights: &[(std::ops::Range<usize>, glyphon::Color)],
) -> Vec<(std::ops::Range<usize>, glyphon::Color)> {
    let line_char_len = col_map.len().saturating_sub(1);
    highlights
        .iter()
        .filter(|(range, _)| range.end > line_byte_start && range.start < line_byte_end)
        .filter_map(|(range, color)| {
            let start_byte = range.start.max(line_byte_start);
            let end_byte = range.end.min(line_byte_end);
            let start_char = buffer.byte_to_char(start_byte).saturating_sub(line_start_char).min(line_char_len);
            let end_char = buffer.byte_to_char(end_byte).saturating_sub(line_start_char).min(line_char_len);
            let visual_start = tabstops::visual_col(col_map, start_char);
            let visual_end = tabstops::visual_col(col_map, end_char);
            if visual_start >= visual_end {
                return None;
            }
            let byte_start = display.char_indices().nth(visual_start).map(|(b, _)| b).unwrap_or(display.len());
            let byte_end = display.char_indices().nth(visual_end).map(|(b, _)| b).unwrap_or(display.len());
            Some((byte_start..byte_end, *color))
        })
        .collect()
}

/// A decorated line's colours: the syntax's (`remapped`, by byte range of
/// the text drawn) where the decorations' own text (`runs`) isn't, theirs
/// where it is -- sorted, none overlapping, as `split_line_by_highlights`
/// takes them.
fn decorated_ranges(
    remapped: Vec<(std::ops::Range<usize>, glyphon::Color)>,
    runs: &[(std::ops::Range<usize>, crate::page::Role)],
    color: impl Fn(crate::page::Role) -> glyphon::Color,
) -> Vec<(std::ops::Range<usize>, glyphon::Color)> {
    let mut out: Vec<(std::ops::Range<usize>, glyphon::Color)> = Vec::new();
    for (range, c) in remapped {
        let mut pieces = vec![range];
        for (run, _) in runs {
            pieces = pieces
                .into_iter()
                .flat_map(|p| [p.start..p.end.min(run.start), p.start.max(run.end)..p.end])
                .filter(|p| p.start < p.end)
                .collect();
        }
        out.extend(pieces.into_iter().map(|p| (p, c)));
    }
    out.extend(runs.iter().map(|(r, role)| (r.clone(), color(*role))));
    out.sort_by_key(|(r, _)| r.start);
    out
}

/// Whether `style` belongs to the banner block (the logo + tagline) as
/// opposed to the content block (section headers, project/recent-file
/// entries, the footer hint) -- `dashboard_center_offset` centers these
/// two blocks *independently*, each against its own widest line, rather
/// than the whole buffer sharing one width. The banner (a fixed ~30
/// chars) is narrower than the content block (whose widest line is
/// usually the footer hint or a long file path) -- centering everything
/// against one shared width left the banner sitting off-center, visibly
/// left of the pane's true middle, which is what this fixes.
/// Turns the shares a right-nested stack of panes should end up with
/// into the per-split ratios that produce them.
///
/// Panes stacked by repeatedly splitting the last one form a chain where
/// each split hands the entire remainder of the stack to its own second
/// subtree. So a split's ratio isn't its pane's share of the whole
/// column, it's that pane's share of whatever is left at that depth --
/// which is why an evenly-weighted stack needs visibly uneven ratios
/// (1/2, 1/3, 1/4 ...), and why setting them all to one value produces
/// the halving cascade this replaced.
///
/// Returns `shares.len() - 1` ratios, one per split, since an n-pane
/// chain has n-1 of them -- the last two panes share a split, and it's
/// the second-to-last pane's ratio that sets it. Deliberately not one
/// ratio per pane: zipping panes against ratios is the obvious way to
/// apply these, and a trailing ratio would land on that shared split a
/// second time and overwrite the value that had just sized it.
fn stack_split_ratios(shares: &[f32]) -> Vec<f32> {
    let mut remaining: f32 = shares.iter().sum();
    shares
        .iter()
        .take(shares.len().saturating_sub(1))
        .map(|share| {
            let ratio = if remaining > 0.0 { share / remaining } else { 0.5 };
            remaining -= share;
            ratio
        })
        .collect()
}

/// Extra vertical padding baked into the tab strip's own background bar
/// on top of one line of text -- the design's `.tab-b` padding (8px) a
/// single shared `line_height` slot couldn't express: the tab row is
/// supposed to read as the taller, more prominent of the two chrome
/// rows, not just another `line_height`-tall strip.
const TAB_STRIP_PAD: f32 = 8.0;
/// Same idea for the breadcrumb bar directly below it, but far smaller --
/// the design's `.crumbs-b` is deliberately the quieter, more compact
/// second row (a few px of padding vs. the tab row's).
const BREADCRUMB_STRIP_PAD: f32 = 2.0;

/// The tab strip's own on-screen height -- one line of (smaller-scale)
/// text plus `TAB_STRIP_PAD`, never just `line_height` alone.
fn tab_strip_height(line_height: f32) -> f32 {
    line_height + TAB_STRIP_PAD
}

/// The breadcrumb bar's own on-screen height, shorter than the tab strip
/// above it -- see `tab_strip_height`.
fn breadcrumb_strip_height(line_height: f32) -> f32 {
    line_height + BREADCRUMB_STRIP_PAD
}

/// Total chrome height a titled pane reserves above its content: the tab
/// strip plus the breadcrumb bar, each its own height rather than two
/// equal `line_height` slots.
fn title_reserved_height(line_height: f32) -> f32 {
    tab_strip_height(line_height) + breadcrumb_strip_height(line_height)
}

/// A pane's actual *content* rect, given its full layout rect and
/// whether it carries a breadcrumb bar under its title row
/// (`App::pane_has_breadcrumb`).
///
/// Every pane reserves a title row. Only a pane that actually renders
/// breadcrumbs reserves the shorter bar beneath it as well -- an
/// informational panel (the Git dashboard's sections, Docker's, a
/// terminal) has no cursor position to describe, never draws anything
/// there, and used to get a blank strip of chrome under its title for
/// its trouble. This parameter used to be `has_title`, which every live
/// caller passed `true`: the distinction that actually varies between
/// panes is the breadcrumb, not the title.
///
/// Extracted as a pure function (rather than left inline in `redraw()`'s
/// per-pane loop) specifically so the "how much does a pane's chrome
/// take off the top" invariant is directly unit-testable without a GPU.
fn pane_content_rect(rect: fenix_window::Rect, line_height: f32, has_breadcrumb: bool) -> fenix_window::Rect {
    let h = pane_chrome_height(line_height, has_breadcrumb);
    fenix_window::Rect { y: rect.y + h, h: (rect.h - h).max(0.0), ..rect }
}

/// How much vertical space a pane's chrome takes off the top of it --
/// the title/tab row always, plus the breadcrumb bar only for the panes
/// that render one. The single source of truth for that height: the
/// content rect, the background fill, the title text's own position and
/// mouse hit-testing all derive from this, so a pane can't end up with
/// its content starting somewhere its chrome doesn't stop.
fn pane_chrome_height(line_height: f32, has_breadcrumb: bool) -> f32 {
    if has_breadcrumb { title_reserved_height(line_height) } else { tab_strip_height(line_height) }
}

/// Insets a tab strip's own rect by `PAD_LEFT` on the left -- the same
/// left margin the strip's text (icon/filename/close glyphs) actually
/// renders at (`text.rs`'s title `TextArea` uses `rect.x + PAD_LEFT`).
/// Both call sites that lay out tabs from this rect (`redraw`, for
/// drawing, and `frame_geometry`, for hit-testing) go through this one
/// function so they can never drift apart -- before this existed, both
/// independently used the strip's raw, un-inset rect, so every tab's
/// background fill, divider, and click target sat a constant `PAD_LEFT`
/// to the left of the glyphs actually drawn over it: dividers cut through
/// the tail of the previous tab's own text instead of the gap between
/// tabs, and a click square on the last tab's visible `×` landed `PAD_
/// LEFT` short of it.
fn tab_strip_rect(rect: fenix_window::Rect) -> fenix_window::Rect {
    fenix_window::Rect { x: rect.x + text::PAD_LEFT, w: (rect.w - text::PAD_LEFT).max(0.0), ..rect }
}

/// The modeline's bottom-right clock text -- real local wall-clock time,
/// re-formatted fresh every `redraw()` (no caching/dirty-tracking) so it
/// naturally freeloads on the existing ~500ms caret-blink redraw cadence
/// instead of needing its own timer plumbing.
fn modeline_clock_text() -> String {
    format_clock(chrono::Local::now())
}

/// Turn the modeline's deliberately plain, testable text into a calmer
/// visual hierarchy: the active document/status text stays at full
/// contrast, while later metadata fields and the dots between them recede
/// to `gutter_fg`. The three-character separators become compact centered
/// dots without changing the line's width.
fn modeline_suffix_spans(suffix: &str, theme: &Theme, foreground: glyphon::Color) -> Vec<(String, glyphon::Color)> {
    if foreground != theme.fg_modeline {
        return vec![(suffix.to_string(), foreground)];
    }

    let mut spans = Vec::new();
    // An empty field -- a page's details, say -- gets no dot of its own,
    // so the line never ends on one.
    let fields = suffix.split("   ").enumerate().filter(|(index, field)| *index == 0 || !field.trim().is_empty());
    for (index, field) in fields {
        if index != 0 {
            spans.push((" · ".to_string(), theme.gutter_fg));
        }
        let color = if index == 0 { foreground } else { theme.gutter_fg };
        spans.push((field.to_string(), color));
    }
    spans
}

/// Pure formatting split out from `modeline_clock_text` so it's
/// unit-testable against a fixed instant instead of the real clock.
fn format_clock(now: chrono::DateTime<chrono::Local>) -> String {
    now.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Whether the clock has room to show at all without visually overlapping
/// `existing_chars` already rendered on the modeline's left side (the
/// badge+suffix, or the raw `:`/`/` command-line text) -- a coarse,
/// char-count-based estimate (same approximation `which_key_popup`'s own
/// width sizing already uses elsewhere) that only ever decides *whether*
/// to show the clock, never *where* to draw it: real positioning is
/// `TextPipeline::set_clock_rich`'s job now, via cosmic-text's own
/// `Align::Right` layout against the glyphs' actual shaped widths, not
/// this crate approximating a pixel width from a char count. Two earlier
/// approaches both tried to compute the clock's exact on-screen position
/// from `char_count * char_width` (first embedded in a shared string via
/// space-padding, then as this function's own return value) and both
/// were visibly wrong in practice -- real glyph shaping doesn't sum
/// perfectly uniformly even in a "monospace" font, and any per-glyph
/// slop is directly how much of the clock ends up clipped by the
/// window's edge. A coarse fit *estimate* has no such consequence if
/// it's slightly off: worst case the clock shows with a little visual
/// overlap, or hides one column earlier than it strictly needed to.
/// The mode segment's leading glyph -- the design's own `▲` ahead of the
/// mode name, drawn in the mode's accent color along with the label, so
/// the rail, the glyph, and the word all read as one signal. A plain
/// Geometric-Shapes character, not a Nerd Font icon: the modeline is a
/// single body-font text run (`set_modeline_text` takes no icon-font
/// flag), and this glyph is common enough in ordinary monospace faces
/// not to need one.
const MODE_GLYPH: char = '\u{25B2}';

/// The modeline's own internal hairlines -- the rule closing the mode
/// segment and the one opening the clock's. Deliberately *not*
/// `theme.divider`: that one is calibrated to separate panes against
/// `theme.bg`, and on shipped themes (Visual Studio Dark, TempleOS) it
/// is literally the same color as `bg_modeline`, so a rule drawn in it
/// is invisible on this bar specifically. The design specifies a
/// translucent overlay for exactly that reason -- an overlay reads
/// against whatever the bar underneath happens to be, rather than
/// having to be re-tuned per theme. This is that overlay in the
/// modeline's own foreground rather than a hardcoded white, so it still
/// holds if a theme ever ships a light modeline.
fn modeline_rule_color(theme: &Theme) -> [f32; 4] {
    let [r, g, b, _] = glyphon_to_rgba(theme.fg_modeline);
    // The design's own figure is 0.06, but that's a CSS border on a bar
    // rendered much larger than one of these 1px rules ends up on a real
    // display -- at 0.06 the rule is there in the framebuffer and simply
    // can't be seen. This is the same intent (present, quiet, structural)
    // at an alpha that actually reads.
    [r, g, b, 0.28]
}
/// The mode segment written exactly as it renders: the leading `MODE_
/// GLYPH`, the label centered in its fixed `MODE_BADGE_CHARS` field
/// (which is what gives the suffix after it a stable starting column
/// regardless of which mode is active), then two trailing spaces. Those
/// two are a *guaranteed* gap -- the centered field's own right padding
/// disappears for a full-width label like `FINDFROM`, so the divider
/// `redraw` draws at this segment's end would otherwise sit flush
/// against the suffix for some modes and not others. Shared by `redraw`
/// and the test-only `modeline_text` so assertions describe what's
/// actually on screen.
fn modeline_mode_segment(mode_label: &str) -> String {
    format!(" {MODE_GLYPH} {mode_label:^width$}  ", width = text::MODE_BADGE_CHARS)
}

fn modeline_clock_fits(existing_chars: usize, window_width: f32, char_width: f32, clock_chars: usize) -> bool {
    let existing_px = existing_chars as f32 * char_width;
    let clock_px = clock_chars as f32 * char_width;
    let available = window_width - 2.0 * text::PAD_LEFT;
    // At least one char_width of breathing room between the two sides,
    // so the clock never reads as glued directly onto existing text.
    available - clock_px - char_width >= existing_px
}

/// (staged, unstaged, untracked) counts for `render_status`'s summary
/// line -- a file counts as staged if its index status is a real change
/// (not `.`/unset), unstaged likewise for the worktree status, and
/// untracked when both sides read `?`. A file can be both staged and
/// unstaged at once (partially staged), matching real `git status`.
fn file_counts(files: &[fenix_git::FileEntry]) -> (usize, usize, usize) {
    let mut staged = 0;
    let mut unstaged = 0;
    let mut untracked = 0;
    for f in files {
        if f.index_status == '?' && f.worktree_status == '?' {
            untracked += 1;
            continue;
        }
        if f.index_status != '.' {
            staged += 1;
        }
        if f.worktree_status != '.' {
            unstaged += 1;
        }
    }
    (staged, unstaged, untracked)
}

/// How many files are in an unmerged (conflicted) state -- git's own
/// `U` porcelain status on either side, which `fenix_git::files` already
/// parses. Counted rather than listed here: the Status pane reports the
/// number, and the files themselves are already in the Unstaged list
/// with their `UU` badge.
fn conflict_count(files: &[fenix_git::FileEntry]) -> usize {
    files.iter().filter(|f| f.index_status == 'U' || f.worktree_status == 'U').count()
}

/// What `git_sync_main` needs to fetch for Main, resolved from a
/// `GitEntry` (plus, for a `File`, a lookup into the session's own
/// cached `files`) *before* any thread is spawned -- every variant here
/// is owned/`Send`, so a background closure never needs to touch
/// `self`/`GitSession` at all. `Skip` covers both "nothing selected" and
/// entries with no single diff of their own (`Branch`, `Dir`) -- Main is
/// left untouched, matching `git_sync_main`'s pre-existing behavior for
/// those. `Untracked` needs no shell-out (there's nothing for `git diff`
/// to show, see `fenix_git::file_diff`'s own doc comment), so it's
/// handled inline in `git_sync_main` rather than through `fetch_main_
/// diff`/a spawned thread.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MainFetch {
    Skip,
    Untracked,
    File { path: String, staged: bool },
    Commit(String),
    Stash(usize),
}

impl DiffSource {
    /// What kind of diff a resolved `MainFetch` produces -- which is
    /// what decides whether a hunk in it can be staged, unstaged, or
    /// neither. A file's diff is against the index when it's the
    /// unstaged side and against `HEAD` when it's the staged one; a
    /// commit's or a stash's is history, with no index relationship to
    /// change.
    fn of(fetch: &MainFetch) -> Self {
        match fetch {
            MainFetch::File { staged: false, .. } => DiffSource::Unstaged,
            MainFetch::File { staged: true, .. } => DiffSource::Staged,
            MainFetch::Commit(_) | MainFetch::Stash(_) | MainFetch::Skip | MainFetch::Untracked => DiffSource::ReadOnly,
        }
    }
}

/// One commit's metadata and its diff, fetched together so the History
/// view's detail pane can show who wrote it and what it says above what
/// it changed.
#[derive(Debug, Clone)]
pub(crate) struct CommitDetail {
    meta: fenix_git::CommitMeta,
    diff: String,
}

/// Both halves of a commit, off the input thread. Two `git` calls
/// rather than one parsed apart, because `git show`'s header is dropped
/// by `fenix_diff::parse` on principle -- see `fenix_git::commit_meta`.
fn commit_detail(repo_root: &Path, hash: &str) -> Option<CommitDetail> {
    let meta = fenix_git::commit_meta(repo_root, hash)?;
    let diff = fenix_git::commit_diff(repo_root, hash).unwrap_or_default();
    Some(CommitDetail { meta, diff })
}

/// A commit's metadata as diff-view header rows: an identity line, the
/// subject, then any message body, then a blank spacer before the diff.
fn commit_header_rows(meta: &fenix_git::CommitMeta) -> Vec<(String, diff_view::DiffStyle)> {
    let mut rows = vec![
        (format!("commit {}", meta.short_hash), diff_view::DiffStyle::Title),
        (format!("  {} <{}>  {}", meta.author, meta.email, meta.date), diff_view::DiffStyle::Meta),
        (String::new(), diff_view::DiffStyle::Meta),
        (format!("  {}", meta.subject), diff_view::DiffStyle::HunkHeader),
    ];
    if !meta.body.is_empty() {
        // Wrapped the same way the Jira panel wraps a description, so a
        // long body reads as prose instead of running off the pane.
        for line in meta.body.lines() {
            for wrapped in crate::wrap::wrap_text(line, crate::wrap::DEFAULT_WRAP_WIDTH.saturating_sub(2).max(20)) {
                rows.push((format!("  {wrapped}"), diff_view::DiffStyle::Context));
            }
        }
    }
    rows.push((String::new(), diff_view::DiffStyle::Meta));
    rows
}

/// The actual `git diff`/`show`/`stash show` shell-out for a resolved
/// `MainFetch` -- called either inline (test mode, no `event_proxy`) or
/// from inside `git_sync_main`'s spawned background thread. Never called
/// with `MainFetch::Skip`/`Untracked`, both handled by the caller before
/// this is reached.
fn fetch_main_diff(repo_root: &Path, fetch: &MainFetch) -> Option<String> {
    match fetch {
        MainFetch::File { path, staged } => fenix_git::file_diff(repo_root, path, *staged).ok(),
        MainFetch::Commit(hash) => fenix_git::commit_diff(repo_root, hash).ok(),
        MainFetch::Stash(index) => fenix_git::stash_diff(repo_root, *index).ok(),
        MainFetch::Skip | MainFetch::Untracked => None,
    }
}

/// What kind of inline change a gutter mark (`App::gutter_hunks`) shows
/// beside a line -- the same three-way distinction most editors' own git-
/// gutter indicator makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GutterMarkKind {
    Added,
    Modified,
    /// Anchored at the *following* line (or end of file) -- there is no
    /// line of its own left in the new file to sit beside.
    Deleted,
}

/// One inline git gutter mark -- `line` is 1-based, in the *new* (current)
/// file's own line numbering, matching `fenix_diff::DiffLine::new_line`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GutterMark {
    line: usize,
    kind: GutterMarkKind,
}

/// Walks a parsed diff's hunks into gutter marks -- pure and GPU/git-free,
/// so it's directly unit-testable against small synthetic diff text. Git's
/// own diff output groups a change as a contiguous run of removed lines
/// immediately followed by a contiguous run of added lines (never
/// interleaved), so this walks each hunk in maximal non-context runs: a
/// run with any added lines marks every one of them `Modified` if the run
/// also removed something, else `Added`; a run of pure removals (nothing
/// added at all) becomes one `Deleted` mark at the run's own end -- the
/// next line's new-file position, or the hunk's own end if the removal
/// runs off it.
fn gutter_marks_from_hunks(hunks: &[fenix_diff::Hunk]) -> Vec<GutterMark> {
    let mut marks = Vec::new();
    for hunk in hunks {
        let mut i = 0;
        while i < hunk.lines.len() {
            let is_change = |k: fenix_diff::LineKind| matches!(k, fenix_diff::LineKind::Removed | fenix_diff::LineKind::Added);
            if !is_change(hunk.lines[i].kind) {
                i += 1;
                continue;
            }
            let start = i;
            while i < hunk.lines.len() && is_change(hunk.lines[i].kind) {
                i += 1;
            }
            let run = &hunk.lines[start..i];
            let has_removed = run.iter().any(|l| l.kind == fenix_diff::LineKind::Removed);
            let has_added = run.iter().any(|l| l.kind == fenix_diff::LineKind::Added);
            if has_added {
                let kind = if has_removed { GutterMarkKind::Modified } else { GutterMarkKind::Added };
                for line in run {
                    if line.kind == fenix_diff::LineKind::Added {
                        if let Some(new_line) = line.new_line {
                            marks.push(GutterMark { line: new_line, kind });
                        }
                    }
                }
            } else if has_removed {
                let anchor = hunk.lines[i..].iter().find_map(|l| l.new_line).unwrap_or(hunk.new_start + hunk.new_len);
                marks.push(GutterMark { line: anchor, kind: GutterMarkKind::Deleted });
            }
        }
    }
    marks
}

/// One line's own indentation guide columns -- a thin vertical line per
/// *complete* indent level already present in `leading_columns` (the
/// line's own leading-whitespace width, already tab-expanded to display
/// columns by the caller), positioned at that level's own start column.
/// 8 leading columns at a 4-column indent width gives `[0, 4]`: one guide
/// marking "inside the first block," one marking "inside the second" --
/// not `[4, 8]`, which would mark each level's *end* instead of its
/// start, and not include a guide for the outermost level at all.
///
/// Pure and buffer-free (the caller does the tab-expansion/line-reading),
/// so this is directly unit-testable. A disclosed v1 simplification:
/// guides are computed independently per line, not bridged across a
/// blank line to the next non-blank line's own indent the way some
/// editors do -- a blank line inside an indented block simply shows no
/// guides of its own.
/// How far a file indents each level, read from its own lines: the step
/// most often taken from one non-blank line to a deeper next one. `None`
/// when it doesn't say (no indented lines, or tabs, which the tab width
/// handles) -- the indent width setting is used then. A step of one isn't
/// a level (a block comment's ` * ` under its `/*`); steps over 8 are an
/// alignment, not a level. A tie goes to the smaller step.
fn file_indent_step<'a>(lines: impl Iterator<Item = &'a str>) -> Option<usize> {
    let mut steps = [0usize; 9];
    let mut previous = 0;
    for line in lines.take(2000) {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with('\t') {
            return None;
        }
        let depth = line.chars().take_while(|&c| c == ' ').count();
        if depth > previous && depth - previous >= 2 && depth - previous <= 8 {
            steps[depth - previous] += 1;
        }
        previous = depth;
    }
    let (step, &count) = steps.iter().enumerate().rev().max_by_key(|(_, &n)| n)?;
    (count > 0).then_some(step)
}

fn indent_guide_columns(leading_columns: usize, indent_width: usize) -> impl Iterator<Item = usize> {
    // Returning an iterator keeps the visible-row redraw path allocation
    // free. It used to build and discard one Vec per visible line.
    let levels = leading_columns.checked_div(indent_width).unwrap_or(0);
    (0..levels).map(move |level| level * indent_width)
}

/// The final document row represented by a pane's display-row slice.
/// A pane is often taller than its buffer; in that case there is no row at
/// `visible_rows`, but syntax highlighting must still extend through the
/// buffer's final row rather than silently stopping at the first one.
fn visible_document_end_line(display_lines: &[usize], visible_rows: usize) -> usize {
    display_lines
        .get(visible_rows)
        .or_else(|| display_lines.last())
        .copied()
        .unwrap_or(0)
}

/// Coalesces same-column guides that occupy consecutive visible rows.
///
/// Rendering every guide independently turns a moderately indented viewport
/// into hundreds of rectangles (six vertices and a CPU coordinate transform
/// each) every frame. A guide that continues through neighbouring rows is
/// visually one uninterrupted line, so preserve the pixels while emitting a
/// single `(first_row, last_row, column)` rectangle run instead.
fn indent_guide_runs(guides: &[(usize, usize)]) -> Vec<(usize, usize, usize)> {
    // The number of simultaneously-active columns is bounded by the nesting
    // depth, which is normally tiny; a short Vec avoids hashing every guide
    // in this per-frame hot path.
    let mut active: Vec<(usize, usize, usize)> = Vec::new(); // (first, last, col)
    let mut completed = Vec::new();
    for &(row, col) in guides {
        match active.iter_mut().find(|run| run.2 == col) {
            Some(run) if run.1 + 1 == row => run.1 = row,
            Some(run) => {
                completed.push(*run);
                *run = (row, row, col);
            }
            None => active.push((row, row, col)),
        }
    }
    completed.extend(active);
    completed
}

/// Everything `git_refresh_session` re-lists in one shell-out round --
/// owned/`Send`, so `git_refresh_session`'s spawned background thread
/// never needs to touch `self`/`GitSession` at all (same reasoning as
/// `MainFetch`). Carried whole through `FenixUserEvent::GitRefreshReady`
/// to `apply_git_refresh`, which is the only place any of it lands in
/// the session.
#[derive(Debug)]
pub(crate) struct GitRefreshData {
    status: Option<fenix_git::RepoStatus>,
    files: Vec<fenix_git::FileEntry>,
    branches: Vec<fenix_git::Branch>,
    commits: Vec<fenix_git::Commit>,
    stashes: Vec<fenix_git::Stash>,
    /// A rebase/merge/cherry-pick left suspended, if any -- read in the
    /// same pass as everything else so the Status banner can never
    /// disagree with the file list beside it.
    in_progress: Option<fenix_git::InProgress>,
    sides: Option<fenix_git::ConflictSides>,
}

/// The actual four `git` shell-outs `git_refresh_session` needs -- called
/// either inline (test mode, no `event_proxy`) or from inside its
/// spawned background thread.
fn fetch_git_refresh_data(repo_root: &Path) -> GitRefreshData {
    let (status, files) = fenix_git::status_and_files(repo_root);
    let branches = fenix_git::list_branches(repo_root);
    let commits = fenix_git::list_commits(repo_root, 50);
    let stashes = fenix_git::list_stashes(repo_root);
    let in_progress = fenix_git::in_progress(repo_root);
    let sides = fenix_git::conflict_sides(repo_root);
    GitRefreshData { status, files, branches, commits, stashes, in_progress, sides }
}

/// The standard xterm 256-color palette -- 0-15 the fixed default
/// 16-color set (verified against xterm's own documented defaults, not
/// guessed), 16-231 a closed-form 6x6x6 RGB cube (component levels
/// 0/95/135/175/215/255), 232-255 a closed-form 24-step grayscale ramp
/// (8..238, step 10). Standard, unambiguous, no crate needed for it.
fn xterm_256_rgb(idx: u8) -> (u8, u8, u8) {
    const BASE_16: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    match idx {
        0..=15 => BASE_16[idx as usize],
        16..=231 => {
            let i = idx - 16;
            let level = |n: u8| if n == 0 { 0 } else { 55 + n * 40 };
            (level(i / 36), level((i % 36) / 6), level(i % 6))
        }
        232..=255 => {
            let level = 8 + (idx - 232) * 10;
            (level, level, level)
        }
    }
}

/// The byte ranges `fenix_format::reindent`/`reindent_lines` should
/// exclude from their bracket scan -- every string/comment span in
/// `source`, from a fresh, one-shot syntax parse rather than the
/// buffer's own `OpenBuffer::syntax` (which is only resynced lazily,
/// from `redraw`'s own rendering pass -- not guaranteed current the
/// instant a format command actually fires). A one-shot, user-triggered
/// action easily affords a full reparse; there's no per-frame cost
/// concern here the way there is for live syntax highlighting.
///
/// `None` (no detected language) yields no exclusions at all --
/// `reindent` still works without them, just without knowing which
/// brace-shaped characters are actually inside a string or comment
/// rather than real nesting; a buffer with no recognized extension
/// stays formattable, just slightly less accurately.
///
/// A free function, not an `App` method, for the same reason `format_
/// with_external_tool` used to be: `format_selection` needs to call
/// this in between two separate mutable borrows of `self.buffers`/
/// `self.workspaces`.
fn reindent_skip_ranges(language: Option<fenix_syntax::LanguageId>, source: &str) -> Vec<std::ops::Range<usize>> {
    let Some(language) = language else { return Vec::new() };
    let syntax = fenix_syntax::SyntaxState::new(language, source);
    syntax
        .highlights_in_range(source, 0..source.len())
        .into_iter()
        .filter(|(_, name)| *name == "string" || name.starts_with("string.") || *name == "comment" || name.starts_with("comment."))
        .map(|(range, _)| range)
        .collect()
}

/// `toggle_comment_lines`'s own language -> line-comment-token table.
/// `None` for a language with no clean single-token *line* comment --
/// JSON (no comments at all in the spec), Markdown (block `<!-- -->`
/// only), Batch (`REM`/`::` are both real but neither is unambiguous
/// enough to toggle automatically) -- same "no-op, not a wrong guess"
/// posture the rest of this file takes for a language with no support
/// for a given feature wired up.
fn line_comment_token(language: fenix_syntax::LanguageId) -> Option<&'static str> {
    use fenix_syntax::LanguageId;
    // A language file's `comment`, over Fenix's own.
    if let Some(comment) = crate::languages::comment(language) {
        return Some(comment);
    }
    match language {
        LanguageId::Rust
        | LanguageId::C
        | LanguageId::Cpp
        | LanguageId::JavaScript
        | LanguageId::TypeScript
        | LanguageId::Tsx
        | LanguageId::Go
        | LanguageId::Java
        | LanguageId::CSharp => Some("//"),
        LanguageId::Toml | LanguageId::Yaml | LanguageId::Python | LanguageId::Bash | LanguageId::Tcl | LanguageId::Dockerfile => Some("#"),
        LanguageId::Sql | LanguageId::Lua => Some("--"),
        LanguageId::Mermaid => Some("%%"),
        // CSS has only `/* */`, HTML only `<!-- -->` (see
        // `block_comment_tokens`).
        LanguageId::Json | LanguageId::Markdown | LanguageId::Batch | LanguageId::Xml | LanguageId::Dtd | LanguageId::Html | LanguageId::Css => None,
        // An extension's grammar whose language file doesn't say.
        LanguageId::Grammar(_) => None,
    }
}

/// The `(open, close)` pair `gcc` wraps each line in, for a language
/// whose only comment syntax is a block comment -- XML, a DTD, and
/// Markdown's HTML comments. Consulted only when `line_comment_token`
/// has nothing.
fn block_comment_tokens(language: fenix_syntax::LanguageId) -> Option<(&'static str, &'static str)> {
    use fenix_syntax::LanguageId;
    match language {
        LanguageId::Xml | LanguageId::Dtd | LanguageId::Markdown | LanguageId::Html => Some(("<!--", "-->")),
        LanguageId::Css => Some(("/*", "*/")),
        _ => None,
    }
}

/// The char index of the first non-blank (non-space, non-tab) character
/// on `line`, or the line's own start if it's blank/empty -- `'{mark}`'s
/// landing spot (`goto_jump_entry`), real Vim's own `'` (vs. `` ` ``,
/// which wants the exact position) convention.
fn line_first_non_blank_char(buffer: &Buffer, line: usize) -> usize {
    let start = buffer.line_start_char(line);
    let len = buffer.line_len(line);
    for i in 0..len {
        if buffer.char_at(start + i).is_some_and(|c| c != ' ' && c != '\t') {
            return start + i;
        }
    }
    start
}

/// `GitBadgeColor` -> a real theme color -- mirrors the Docker panel's
/// `badge_color`.
fn git_badge_color(color: git_panel::GitBadgeColor, theme: &Theme) -> glyphon::Color {
    match color {
        git_panel::GitBadgeColor::Good => theme.git_staged,
        git_panel::GitBadgeColor::Warn => theme.git_modified,
        git_panel::GitBadgeColor::Bad => theme.git_conflicted,
        git_panel::GitBadgeColor::Neutral => theme.gutter_fg,
    }
}

/// Resolves a real `BufferKind::Git` buffer's per-line syntax-highlight
/// ranges from its cached `GitLine` metadata -- mirrors the Docker
/// panel's `highlights_for_visible_range` exactly. Diff coloring lives in
/// `diff_highlights_for_visible_range` instead, against its own richer
/// per-row metadata.
fn git_highlights_for_visible_range(
    ob: &OpenBuffer,
    lines: Option<&[Option<git_panel::GitLine>]>,
    render_base_line: usize,
    rows: usize,
    theme: &Theme,
) -> Vec<(std::ops::Range<usize>, glyphon::Color)> {
    let Some(lines) = lines else { return Vec::new() };
    let visual_lines = ob.buffer.visual_line_count();
    let mut ranges = Vec::new();
    for line in render_base_line..(render_base_line + rows).min(visual_lines) {
        let Some(Some(meta)) = lines.get(line) else { continue };
        let start = ob.buffer.line_start_char(line);
        let len = ob.buffer.line_len(line);
        let line_start_byte = ob.buffer.char_to_byte(start);
        let line_end_byte = ob.buffer.char_to_byte(start + len);
        match meta.style {
            git_panel::GitLineStyle::Empty => {
                ranges.push((line_start_byte..line_end_byte, theme.gutter_fg));
            }
            git_panel::GitLineStyle::Header => ranges.push((line_start_byte..line_end_byte, theme.syntax_keyword)),
            // `File` shares `Detail`'s dim-from-here handling -- its
            // trailing readable status word (`describe_status`) is the
            // same kind of secondary info a `Detail` row's own dim
            // suffix already marks, just on a row that also carries a
            // leading badge.
            git_panel::GitLineStyle::Detail | git_panel::GitLineStyle::File => {
                if let Some(dim_from) = meta.dim_from {
                    let dim_start_byte = ob.buffer.char_to_byte(start + dim_from);
                    ranges.push((dim_start_byte..line_end_byte, theme.gutter_fg));
                }
            }
            // Dimmed like `Empty`/`Detail`'s own dim portion -- a
            // directory row has no per-row status badge (it aggregates
            // an unknown mix underneath), so the whole row reads as
            // secondary/structural rather than a real change.
            git_panel::GitLineStyle::Dir => ranges.push((line_start_byte..line_end_byte, theme.gutter_fg)),
            git_panel::GitLineStyle::Branch | git_panel::GitLineStyle::Commit | git_panel::GitLineStyle::Stash => {}
        }
        if let Some((badge_len, color)) = meta.badge {
            let badge_end_byte = ob.buffer.char_to_byte(start + badge_len);
            ranges.push((line_start_byte..badge_end_byte, git_badge_color(color, theme)));
        }
    }
    ranges
}

/// Resolves a real `BufferKind::Diff` buffer's per-line highlight
/// ranges from its cached `DiffViewLine` metadata -- mirrors
/// `git_highlights_for_visible_range`, with one addition it doesn't
/// need: a diff row is *two* colored spans, a dim line-number gutter
/// and the patch line itself, so the gutter never competes with the
/// change for attention.
///
/// The two ranges are pushed in ascending, non-overlapping order
/// because `split_line_by_highlights` walks its input with a forward-
/// only cursor -- ranges out of order would silently drop spans rather
/// than fail.
fn diff_highlights_for_visible_range(
    ob: &OpenBuffer,
    lines: Option<&[Option<diff_view::DiffViewLine>]>,
    render_base_line: usize,
    rows: usize,
    theme: &Theme,
) -> Vec<(std::ops::Range<usize>, glyphon::Color)> {
    let Some(lines) = lines else { return Vec::new() };
    let visual_lines = ob.buffer.visual_line_count();
    let mut ranges = Vec::new();
    for line in render_base_line..(render_base_line + rows).min(visual_lines) {
        let Some(Some(meta)) = lines.get(line) else { continue };
        let start = ob.buffer.line_start_char(line);
        let len = ob.buffer.line_len(line);
        let line_start_byte = ob.buffer.char_to_byte(start);
        let line_end_byte = ob.buffer.char_to_byte(start + len);
        // `min(len)` guards a metadata/text mismatch (a row shorter than
        // its own recorded gutter) from producing an inverted range.
        let content_start_byte = ob.buffer.char_to_byte(start + meta.content_from.min(len));
        if meta.content_from > 0 {
            ranges.push((line_start_byte..content_start_byte, theme.gutter_fg));
        }
        let color = match meta.style {
            // Same roles the Jira panel already established for a page
            // title vs. a section header.
            diff_view::DiffStyle::Title | diff_view::DiffStyle::FileHeader => theme.syntax_function,
            diff_view::DiffStyle::HunkHeader => theme.syntax_keyword,
            diff_view::DiffStyle::Added => theme.git_staged,
            diff_view::DiffStyle::Removed => theme.git_conflicted,
            diff_view::DiffStyle::Meta => theme.gutter_fg,
            // A review comment reads as prose beside code: its author
            // row takes the same accent a file header does, its body
            // the string color, so a thread is scannable among the
            // diff lines it's wedged between without competing with
            // the additions and removals for the eye.
            diff_view::DiffStyle::NoteHeader => theme.syntax_function,
            diff_view::DiffStyle::NoteBody => theme.syntax_string,
            // Unchanged context reads as ordinary text -- no range at
            // all, so it falls through to the pane's default color.
            diff_view::DiffStyle::Context => continue,
        };
        ranges.push((content_start_byte..line_end_byte, color));
    }
    ranges
}

/// Highlight ranges for git's conflict markers in an ordinary text
/// buffer: the marker lines themselves, and each side's content colored
/// the way the merge view colors it, so the same two colors mean the
/// same two things whichever surface you're reading the conflict on.
///
/// Empty for a file with no markers at all, which is how the caller
/// decides whether to use this instead of syntax highlighting -- so
/// this walks the *whole* buffer for an opening marker rather than only
/// the visible rows: scrolling into a conflict from a clean part of the
/// file has to light it up too.
fn conflict_marker_highlights(
    ob: &OpenBuffer,
    render_base_line: usize,
    rows: usize,
    theme: &Theme,
) -> Vec<(std::ops::Range<usize>, glyphon::Color)> {
    if ob.kind != BufferKind::Text {
        return Vec::new();
    }
    let text = ob.buffer.text();
    if !text.contains("<<<<<<<") {
        return Vec::new();
    }
    let conflicts = fenix_git::find_conflicts(&text);
    if conflicts.is_empty() {
        return Vec::new();
    }
    let visual_lines = ob.buffer.visual_line_count();
    let mut ranges = Vec::new();
    for line in render_base_line..(render_base_line + rows).min(visual_lines) {
        let Some(conflict) = conflicts.iter().find(|c| c.contains(line)) else { continue };
        let color = if line == conflict.start || line == conflict.end || conflict.ours.end == line || conflict.base.as_ref().is_some_and(|b| b.start == line + 1)
        {
            // The `<<<<<<<`, `=======`, `|||||||` and `>>>>>>>` lines:
            // structure, not content.
            theme.syntax_keyword
        } else if conflict.ours.contains(&line) {
            theme.git_conflicted
        } else if conflict.base.as_ref().is_some_and(|b| b.contains(&line)) {
            theme.syntax_comment
        } else {
            theme.git_staged
        };
        let start = ob.buffer.line_start_char(line);
        let len = ob.buffer.line_len(line);
        ranges.push((ob.buffer.char_to_byte(start)..ob.buffer.char_to_byte(start + len), color));
    }
    ranges
}

/// Resolves a real `BufferKind::Merge` buffer's highlight ranges from
/// its cached `MergeViewLine` metadata.
///
/// A conflict row is genuinely two-colored: the left column is one
/// branch's version and the right column the other's, and telling them
/// apart at a glance is the entire point of the view. So a row with a
/// `gutter` gets three spans -- ours, the dim divider, theirs -- rather
/// than the single span every other panel here uses.
fn merge_highlights_for_visible_range(
    ob: &OpenBuffer,
    lines: Option<&[Option<merge_view::MergeViewLine>]>,
    render_base_line: usize,
    rows: usize,
    theme: &Theme,
) -> Vec<(std::ops::Range<usize>, glyphon::Color)> {
    let Some(lines) = lines else { return Vec::new() };
    let visual_lines = ob.buffer.visual_line_count();
    let mut ranges = Vec::new();
    for line in render_base_line..(render_base_line + rows).min(visual_lines) {
        let Some(Some(meta)) = lines.get(line) else { continue };
        let start = ob.buffer.line_start_char(line);
        let len = ob.buffer.line_len(line);
        let byte = |col: usize| ob.buffer.char_to_byte(start + col.min(len));
        let (line_start, line_end) = (byte(0), byte(len));

        // Ours and theirs read as removed/added would in a diff: the
        // same two colors, meaning the same two things (what's there
        // now vs. what's coming in), so there's one palette to learn
        // rather than two.
        let (ours, theirs) = (theme.git_conflicted, theme.git_staged);
        match (meta.style, meta.gutter) {
            (merge_view::MergeStyle::Header, Some(gutter)) => {
                ranges.push((line_start..byte(gutter), ours));
                ranges.push((byte(gutter)..byte(gutter + 1), theme.gutter_fg));
                ranges.push((byte(gutter + 1)..line_end, theirs));
            }
            (merge_view::MergeStyle::Header, None) => ranges.push((line_start..line_end, theme.syntax_function)),
            (merge_view::MergeStyle::Separator, _) => ranges.push((line_start..line_end, theme.syntax_keyword)),
            (merge_view::MergeStyle::Meta, Some(gutter)) => {
                // The role line under the heading: dim, but still split,
                // so each explanation sits under the branch it explains.
                ranges.push((line_start..byte(gutter), theme.gutter_fg));
                ranges.push((byte(gutter)..line_end, theme.gutter_fg));
            }
            (merge_view::MergeStyle::Meta, None) => ranges.push((line_start..line_end, theme.gutter_fg)),
            (merge_view::MergeStyle::Base, _) => ranges.push((line_start..line_end, theme.syntax_comment)),
            (merge_view::MergeStyle::Context, Some(gutter)) => {
                ranges.push((line_start..byte(gutter), ours));
                ranges.push((byte(gutter)..byte(gutter + 1), theme.gutter_fg));
                ranges.push((byte(gutter + 1)..line_end, theirs));
            }
            // Shared text outside any conflict: ordinary file content,
            // so no range at all and the pane's own color applies.
            (merge_view::MergeStyle::Context, None) => continue,
        }
    }
    ranges
}

/// Resolves a real `BufferKind::Graph` buffer's highlight ranges from
/// its cached `GraphLine` metadata. Unlike every other panel here, a
/// graph row is several differently-colored spans rather than one, so
/// each row walks its own span list -- each span running from its start
/// column to the next span's start, the last to end of line.
fn graph_highlights_for_visible_range(
    ob: &OpenBuffer,
    lines: Option<&[Option<graph_view::GraphLine>]>,
    render_base_line: usize,
    rows: usize,
    theme: &Theme,
) -> Vec<(std::ops::Range<usize>, glyphon::Color)> {
    let Some(lines) = lines else { return Vec::new() };
    let visual_lines = ob.buffer.visual_line_count();
    let mut ranges = Vec::new();
    for line in render_base_line..(render_base_line + rows).min(visual_lines) {
        let Some(Some(meta)) = lines.get(line) else { continue };
        let start = ob.buffer.line_start_char(line);
        let len = ob.buffer.line_len(line);
        for (i, (col, span)) in meta.spans.iter().enumerate() {
            let next = meta.spans.get(i + 1).map(|(c, _)| *c).unwrap_or(len);
            let (from, to) = ((*col).min(len), next.min(len));
            if from >= to {
                continue;
            }
            let color = match span {
                // Rails are structure, not content -- dim, so the
                // subjects beside them stay the thing you read.
                graph_view::GraphSpan::Rails => theme.gutter_fg,
                // The same accent the modeline's own mode rail and the
                // active tab's top-edge bar use -- Signal Rail's one
                // accent vocabulary, reused here instead of a fifth
                // one-off color. `mode_normal` is `bg_rect`'s `[f32; 4]`
                // shape (it colors a filled rect everywhere else it's
                // used), so it needs the same conversion `rgba_to_glyphon`
                // gives the modeline's own rail-colored label text.
                graph_view::GraphSpan::Node => rgba_to_glyphon(theme.mode_normal),
                graph_view::GraphSpan::Hash => theme.syntax_number,
                // Where each branch actually points: the one thing this
                // view exists to answer at a glance.
                graph_view::GraphSpan::Refs => theme.syntax_function,
                graph_view::GraphSpan::Meta => theme.gutter_fg,
                // Ordinary text, left at the pane's default color.
                graph_view::GraphSpan::Subject => continue,
            };
            ranges.push((ob.buffer.char_to_byte(start + from)..ob.buffer.char_to_byte(start + to), color));
        }
    }
    ranges
}

/// Human-readable file size (`"1.2K"`, `"340B"`) -- no crate for this,
/// just repeated division, so no new dependency for one small column.
fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes}B") } else { format!("{size:.1}{}", UNITS[unit]) }
}

/// Coarse relative age (`"3m"`, `"5h"`, `"2d"`) since `modified` --
/// avoids pulling in a date/time crate just to show a calendar date;
/// elapsed-duration bucketing needs no calendar math at all.
fn format_age(modified: Option<std::time::SystemTime>) -> String {
    // Blank, not a fabricated date: an entry whose metadata could not be
    // read has no timestamp, and inventing one would be a lie the user
    // cannot tell from a real answer.
    let Some(modified) = modified else { return String::new() };
    let Ok(elapsed) = std::time::SystemTime::now().duration_since(modified) else { return "now".to_string() };
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

/// One named task layout -- a Doom-Emacs-style workspace. Each remembers
/// its own split layout; every workspace shares the one global
/// `BufferList` (`App::buffers`), so a buffer open in two workspaces at
/// once stays in sync for free (same `BufferId`, same underlying
/// `OpenBuffer`) -- only the window *layout* is per-workspace, per
/// design, not a full persp-mode-style buffer-list isolation.
struct Workspace {
    name: String,
    windows: WindowTree<BufferId>,
    /// Keyed by `fenix_window::WindowId` -- see `PaneState`'s own doc
    /// comment for why this lives here, not on `App` directly.
    pane_states: HashMap<fenix_window::WindowId, PaneState>,
    scroll_anims: HashMap<fenix_window::WindowId, ScrollAnim>,
    /// Every buffer a pane has ever shown, in first-opened order -- the
    /// open-tabs strip a `show_tabs` theme renders (`App::pane_tab_layout`).
    /// Keyed by `WindowId` for the same reason `pane_states` is: `WindowId`s
    /// are only unique within one `WindowTree`. Maintained for every
    /// theme, not just tabbed ones (see `App::set_pane_content`), so
    /// switching *to* a tabbed theme mid-session already has real tab
    /// history instead of a blank strip.
    pane_tabs: HashMap<fenix_window::WindowId, Vec<BufferId>>,
    /// The project this workspace belongs to, once one's been opened
    /// into it from the hub -- where opening that project again returns.
    project: Option<PathBuf>,
    /// This workspace's own Home buffer: the tab pinned first in every
    /// pane's strip, never closed, scoped to `project` when there is one.
    /// Filled lazily (`App::ensure_workspace_home`), so every path that
    /// makes a workspace needn't know about it. Never listed in
    /// `pane_tabs` -- the strip puts it in front itself.
    home: Option<BufferId>,
    /// The buffer each pane showed before its current one -- `g<Tab>`.
    last_tab: HashMap<fenix_window::WindowId, BufferId>,
    /// Each pane's preview tab, if it has one: the tab a jump (`gd`, a
    /// search result) reuses until it's edited or kept.
    preview: HashMap<fenix_window::WindowId, BufferId>,
    /// Each pane's closed tabs, newest last, with the place each held --
    /// what `SPC b u` reopens.
    closed_tabs: HashMap<fenix_window::WindowId, Vec<(BufferId, usize)>>,
    /// The agent thread the dock showed here last: shown again when you
    /// come back, while the others go on in the background.
    agent_thread: Option<fenix_acp::hub::ThreadId>,
}

impl Workspace {
    fn new(name: String, windows: WindowTree<BufferId>, initial_cursor: Cursor) -> Self {
        let mut pane_states = HashMap::new();
        pane_states.insert(windows.focused_id(), PaneState::seeded_at(initial_cursor));
        let mut pane_tabs = HashMap::new();
        pane_tabs.insert(windows.focused_id(), vec![*windows.content(windows.focused_id()).expect("freshly created window has content")]);
        Self { name, windows, pane_states, scroll_anims: HashMap::new(), pane_tabs, project: None, home: None, last_tab: HashMap::new(), preview: HashMap::new(), closed_tabs: HashMap::new(), agent_thread: None }
    }
}

/// A non-empty, ordered list of workspaces with one active at a time.
/// Switching workspaces is just moving `active` -- the `WindowTree`s
/// (and each one's own `pane_states`/`scroll_anims`) themselves are
/// untouched, so a workspace's layout *and* every pane's cursor/scroll
/// position within it are exactly as they were left.
struct WorkspaceList {
    workspaces: Vec<Workspace>,
    active: usize,
}

impl WorkspaceList {
    fn new(initial_windows: WindowTree<BufferId>, initial_cursor: Cursor) -> Self {
        Self { workspaces: vec![Workspace::new("workspace-1".to_string(), initial_windows, initial_cursor)], active: 0 }
    }

    fn active(&self) -> &WindowTree<BufferId> {
        &self.workspaces[self.active].windows
    }

    fn active_mut(&mut self) -> &mut WindowTree<BufferId> {
        &mut self.workspaces[self.active].windows
    }

    fn active_pane_states(&self) -> &HashMap<fenix_window::WindowId, PaneState> {
        &self.workspaces[self.active].pane_states
    }

    fn active_pane_states_mut(&mut self) -> &mut HashMap<fenix_window::WindowId, PaneState> {
        &mut self.workspaces[self.active].pane_states
    }

    fn active_scroll_anims(&self) -> &HashMap<fenix_window::WindowId, ScrollAnim> {
        &self.workspaces[self.active].scroll_anims
    }

    fn active_scroll_anims_mut(&mut self) -> &mut HashMap<fenix_window::WindowId, ScrollAnim> {
        &mut self.workspaces[self.active].scroll_anims
    }

    fn active_workspace(&self) -> &Workspace {
        &self.workspaces[self.active]
    }

    fn active_workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspaces[self.active]
    }

    fn active_pane_tabs_mut(&mut self) -> &mut HashMap<fenix_window::WindowId, Vec<BufferId>> {
        &mut self.workspaces[self.active].pane_tabs
    }

    fn active_name(&self) -> &str {
        &self.workspaces[self.active].name
    }

    /// Every workspace's index and name, in order -- what `SPC TAB TAB`
    /// lists. The index is what `switch_to_index` takes, so a picker
    /// built from this needs no separate name-to-index lookup on
    /// confirm.
    fn summaries(&self) -> Vec<(usize, &str)> {
        self.workspaces.iter().enumerate().map(|(i, w)| (i, w.name.as_str())).collect()
    }

    /// The index of the workspace named exactly `name`, if any --
    /// `open_configured_workspace`'s way of recognizing "this launcher
    /// entry already has a workspace open" rather than creating a
    /// duplicate every time the same entry is picked again.
    fn index_of_name(&self, name: &str) -> Option<usize> {
        self.workspaces.iter().position(|w| w.name == name)
    }

    /// `SPC TAB r`: renames the active workspace. The default
    /// `workspace-N` name and a `[workspaces]` launcher entry's own
    /// name are otherwise indistinguishable in `SPC TAB TAB`'s list and
    /// the modeline's workspace indicator.
    fn rename_active(&mut self, name: String) {
        self.workspaces[self.active].name = name;
    }
    /// Every pane in *every* workspace here, not just the active one --
    /// what tearing down a whole frame needs (`App::close_sessions_in_
    /// active_frame`). A panel session opens in a workspace of its
    /// own, so "which sessions live in this frame" can't be answered
    /// from the active workspace alone.
    fn all_panes(&self) -> Vec<fenix_window::WindowId> {
        self.workspaces.iter().flat_map(|workspace| workspace.windows.windows()).collect()
    }

    /// A workspace other than the active one with a pane showing `buffer`,
    /// and that pane: the nearest after the active one first.
    fn showing_elsewhere(&self, buffer: BufferId) -> Option<(usize, fenix_window::WindowId)> {
        let n = self.workspaces.len();
        (1..n).map(|step| (self.active + step) % n).find_map(|i| {
            let windows = &self.workspaces[i].windows;
            windows.windows().into_iter().find(|&pane| windows.content(pane) == Some(&buffer)).map(|pane| (i, pane))
        })
    }

    fn len(&self) -> usize {
        self.workspaces.len()
    }

    fn active_index(&self) -> usize {
        self.active
    }

    /// `SPC TAB n`: a new, auto-named workspace (`workspace-2`, etc. --
    /// no name-entry prompt in v1) seeded with a single pane showing
    /// `content`, so it starts on the buffer you were already looking
    /// at rather than nothing (`cursor` seeds that pane's own live
    /// position, from the buffer's remembered one). Becomes active.
    fn new_workspace(&mut self, content: BufferId, cursor: Cursor) {
        // The lowest number free: after a removal, `len + 1` can be one
        // that's still open (two "workspace-5"s).
        let name = (1..).map(|n| format!("workspace-{n}")).find(|name| self.index_of_name(name).is_none()).expect("some number is free");
        self.workspaces.push(Workspace::new(name, WindowTree::new(content), cursor));
        self.active = self.workspaces.len() - 1;
    }

    /// Jumps directly to workspace `idx` (clamped to a valid index) --
    /// unlike `next`/`prev` (relative cycling), used when the caller
    /// already knows exactly which workspace it wants regardless of
    /// whatever's currently active (`DockerHost::open` refocusing an
    /// already-open session, `DockerHost::close` making sure it
    /// removes the *session's* workspace and not whichever one happens
    /// to be active when it's called).
    fn switch_to_index(&mut self, idx: usize) {
        self.active = idx.min(self.workspaces.len() - 1);
    }

    /// `SPC TAB ]`/`SPC TAB [`: cycles the active workspace, wrapping.
    fn next(&mut self) {
        self.active = (self.active + 1) % self.workspaces.len();
    }

    fn prev(&mut self) {
        self.active = (self.active + self.workspaces.len() - 1) % self.workspaces.len();
    }

    /// `SPC TAB d`: removes the active workspace. Refuses (returns
    /// `false`, no-op) if it's the last one -- same safety posture as
    /// `WindowTree::close_focused` refusing to close the last window.
    fn remove_active(&mut self) -> bool {
        if self.workspaces.len() <= 1 {
            return false;
        }
        self.workspaces.remove(self.active);
        if self.active >= self.workspaces.len() {
            self.active = self.workspaces.len() - 1;
        }
        true
    }
}

/// What a `[workspaces]` launcher entry's action string means, once
/// parsed -- see `parse_workspace_action`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum WorkspaceAction {
    Git,
    Jira,
    Containers,
    /// The host name to connect to, one of `vnc.hosts`: the VNC
    /// extension's, through its `vnc.connect` service.
    Vnc(String),
    /// A project root to switch to -- `project:PATH`. Not required to be
    /// in `known_projects` already; opening it via this action adds it,
    /// same as picking it through `SPC p p` would.
    Project(PathBuf),
    /// No live session behind it -- what "editor" would be.
    Plain,
}

/// Parses one `[workspaces]` entry's action field. `git` opens the Git
/// page; `jira` and `containers` the Jira and Containers extensions'
/// pages, when they're installed; `vnc:HOST`
/// connects to that `vnc.hosts` entry by name, through the VNC
/// extension when it's installed; `project:PATH` switches to
/// (and, if it isn't tracked yet, registers) that project root -- `PATH`
/// is taken verbatim as everything after the prefix, not split further,
/// so a Windows path's own drive-letter colon (`project:C:\src\fenix`)
/// isn't mistaken for another separator; anything else -- including an
/// empty string, and any typo -- is a plain workspace with no live
/// session, rather than an error: a launcher entry with a bad action is
/// still a usable named workspace, just not a self-populating one.
fn parse_workspace_action(action: &str) -> WorkspaceAction {
    match action.trim() {
        "git" => WorkspaceAction::Git,
        "jira" => WorkspaceAction::Jira,
        "containers" => WorkspaceAction::Containers,
        other => match other.strip_prefix("vnc:") {
            Some(host) if !host.trim().is_empty() => WorkspaceAction::Vnc(host.trim().to_string()),
            _ => match other.strip_prefix("project:") {
                Some(path) if !path.trim().is_empty() => WorkspaceAction::Project(PathBuf::from(path.trim())),
                _ => WorkspaceAction::Plain,
            },
        },
    }
}

/// One `BufferKind::Table` buffer's current elastic-column layout --
/// synthesized column names plus `fenix_table::tab_stops`'s cumulative
/// stop positions, fed to the renderer as `TabStops::Custom(stops)`
/// (`App::tab_stops_for`). Recomputed from the buffer's *current* text
/// whenever its `edit_count()` has moved since `last_edit_count` (see
/// `App::sync_table_view`), so editing a cell and having every row
/// re-align happens "for free" -- no buffer mutation, just fresh
/// arithmetic over the current rope content.
struct TableView {
    columns: Vec<String>,
    stops: Vec<usize>,
    last_edit_count: u64,
}

/// Whether `kind` may never be edited directly -- Docker/Git/Jira panel
/// buffers are all generated, wholesale-regenerated on every refresh, so
/// letting Vim edits slip through and then reverting them (see `route_
/// keypress`'s own enforcement, right where it already tracks `edit_
/// count_before`/`after` for the `` `. `` mark) costs nothing real: a
/// locally-corrupted undo stack on a buffer that gets fully rewritten on
/// the next refresh anyway has no observable consequence. Navigation
/// (`hjkl`, `gg`/`G`, `/`, `n`/`N`, ...) is unaffected either way -- only
/// dispatch that actually mutates the buffer text is ever reverted.
fn is_readonly_buffer_kind(kind: BufferKind) -> bool {
    matches!(
        kind,
        BufferKind::WorkspaceEdit
            | BufferKind::Git
            | BufferKind::TaskOutput
            | BufferKind::Debug
            | BufferKind::ToolStatus
            | BufferKind::Diff
            | BufferKind::Graph
            | BufferKind::Merge
            // Not because a shell is read-only -- because its buffer is
            // not where its text lives. Vim edits here would change a
            // rope nobody renders while leaving the shell untouched.
            | BufferKind::Terminal
            // A listing's text stands for rows; editing it would desync
            // the two. The exception is rename mode, where the text *is*
            // the edit -- which is why callers go through `App::buffer_
            // edits_are_reverted` rather than asking this directly.
            | BufferKind::Explorer
            // Laid out from the wizard's state on every change.
            | BufferKind::Page
    )
}

/// Somewhere `SPC w h/j/k/l` can move focus to, anywhere across every
/// open frame -- see `App::nav_targets`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NavTarget {
    Pane { frame: usize, pane: fenix_window::WindowId },
    /// A frame's sidebar. Not a leaf in any `WindowTree` -- it's a
    /// panel drawn beside one -- but it is somewhere focus can land, so
    /// it joins the candidate list as an ordinary rectangle. That
    /// replaces the hand-written "Left from the leftmost pane focuses
    /// the sidebar" rule with plain geometry, which also gets the
    /// harder cases right for free: leftwards out of a sidebar
    /// continues to the window on the monitor to its left, and a
    /// sidebar is preferred over another monitor's pane only because
    /// it is nearer.
    Sidebar { frame: usize },
}

/// The client-area size assumed for a frame that hasn't got a real
/// window yet (only a headless test). Navigation compares candidates
/// against each other, so only *relative* geometry matters -- the same
/// reason `fenix_window`'s own `navigate` has always laid its tree out
/// against an arbitrary reference size.
const NAV_REFERENCE_SIZE: (f32, f32) = (1600.0, 900.0);

/// Where `new_frame` should put the window it's about to create --
/// see `App::next_frame_placement`. Each part is optional because a
/// platform that won't report monitors or window positions leaves the
/// decision to the window manager, which is a better answer than
/// guessing coordinates.
struct FramePlacement {
    position: Option<PhysicalPosition<i32>>,
    size: Option<PhysicalSize<u32>>,
    /// Whether to maximize onto the monitor `position` lands on, once
    /// the window exists.
    maximized: bool,
}

/// Whether a freshly measured window layout is worth writing to disk.
///
/// An empty measurement means there was nothing to measure -- a
/// headless run, or an exit before the window ever came up -- and
/// recording it would wipe a real arrangement off disk.
///
/// The unchanged check matters more than it looks. `Config::save`
/// regenerates the whole file from struct state, so it drops any
/// comments and hand-formatting in `config.ini`. That was already true
/// of every other `save` call, but those only fire when you
/// deliberately change a setting; this one would otherwise fire on
/// every single exit.
fn layout_worth_saving(measured: &[fenix_config::WindowLayout], saved: &[fenix_config::WindowLayout]) -> bool {
    !measured.is_empty() && measured != saved
}

impl From<fenix_config::WindowLayout> for FramePlacement {
    fn from(layout: fenix_config::WindowLayout) -> Self {
        Self {
            position: Some(PhysicalPosition::new(layout.x, layout.y)),
            size: Some(PhysicalSize::new(layout.width, layout.height)),
            maximized: layout.maximized,
        }
    }
}

/// Whether a desktop-coordinate point falls on `monitor`. A thin
/// `MonitorHandle`-reading wrapper around `desktop_rect_contains`,
/// which is where the actual rule lives -- a `MonitorHandle` can only
/// come from a real event loop, so the geometry has to be reachable
/// without one to be testable at all.
fn monitor_contains(monitor: &MonitorHandle, point: (i32, i32)) -> bool {
    desktop_rect_contains(monitor.position(), monitor.size(), point)
}

/// Point-in-rectangle over desktop coordinates. Left/top inclusive,
/// right/bottom exclusive, so two monitors sharing an edge never both
/// claim the same pixel -- the same convention
/// `fenix_window::Rect::contains_point` already uses for panes.
fn desktop_rect_contains(at: PhysicalPosition<i32>, size: PhysicalSize<u32>, point: (i32, i32)) -> bool {
    point.0 >= at.x && point.0 < at.x + size.width as i32 && point.1 >= at.y && point.1 < at.y + size.height as i32
}

/// Everything that belongs to one OS window rather than to the editor
/// as a whole: its winit window and swapchain, its own text and rect
/// renderers, its own split layout and workspaces, and its own last-
/// known cursor position.
///
/// "Frame" is Emacs's word for an OS window, borrowed here because this
/// codebase already calls a *split* a window (`fenix_window::WindowId`,
/// `SPC w`, `WindowTree`) and so needs a different word for the thing
/// the window manager draws.
///
/// This type only ever holds a frame that is *parked* -- see
/// `App::frames` for the whole arrangement.
struct FrameState {
    window: Option<Arc<Window>>,
    gpu: Option<GpuState>,
    text: Option<TextPipeline>,
    bg_rect: Option<RectRenderer>,
    popup_rect: Option<RectRenderer>,
    caret_rect: Option<RectRenderer>,
    motion_gpu: Option<motion_host::MotionGpu>,
    workspaces: WorkspaceList,
    cursor_pos: Option<(f32, f32)>,
    /// This frame's top-left corner in desktop coordinates -- see
    /// `App::frame_origin`.
    origin: (i32, i32),
    /// The sidebar is per-frame: a project tree opened on one monitor
    /// stays on that monitor instead of appearing in every window at
    /// once. Its listing state travels with it, so switching frames
    /// doesn't reset where you'd browsed to.
    sidebar: Option<ExplorerState>,
    sidebar_open: bool,
    sidebar_focused: bool,
    /// Whether *this* frame shows the terminal panel. The shell itself
    /// (`App::terminal`) stays global -- one session, one process. Two
    /// frames can therefore both have the panel open and both show the
    /// same live shell, which is odd but harmless; nothing breaks, and
    /// giving each frame its own PTY is a bigger question than window
    /// layout.
    terminal_open: bool,
    terminal_focused: bool,
}

pub struct App {
    /// What a screen reader is told (`a11y_host`).
    a11y: a11y_host::A11yHost,
    /// Every open frame, in a stable order -- `frames[0]` is the one
    /// the app started with. Each entry holds that frame's state *while
    /// it is parked*, and is `None` for exactly one index --
    /// `active_frame` -- whose state is instead the live set of fields
    /// on `App` itself: `window`, `gpu`, `text`, the three
    /// `RectRenderer`s, `workspaces` and `cursor_pos`.
    ///
    /// That split is deliberate. Keeping the active frame's state
    /// directly on `App`, rather than reaching through
    /// `frames[active_frame]` at every use, is what lets the whole
    /// editor keep being written against one implicit "the window" the
    /// way it always has been -- every `self.windows()`,
    /// `self.pane_state(..)`, `self.gpu`, `self.text` reads the frame
    /// that currently has focus, with no call site changed and no
    /// second way to spell it.
    ///
    /// Code that needs to act on a *different* frame calls
    /// `activate_frame` first, which swaps that frame's parked state
    /// into the live fields and parks the outgoing one. `window_event`
    /// is the main caller: it activates whichever frame an event
    /// arrived from before dispatching it, which is also what makes a
    /// background frame's `RedrawRequested` paint its own layout
    /// through its own swapchain.
    frames: Vec<Option<FrameState>>,
    /// Index into `frames` of the frame whose state is live on `App`.
    active_frame: usize,
    /// Index into `frames` of the frame that actually has editor focus.
    /// Equal to `active_frame` except while something has *temporarily*
    /// activated another frame -- which is what handling an event from,
    /// or repainting, a background frame does.
    ///
    /// The difference matters for anything that should exist in exactly
    /// one place at a time no matter how many windows are open: the
    /// which-key popup, an open picker, the file explorer, a prompt.
    /// Those all live on `App` rather than per frame (only one window
    /// can be typed into, so only one can have raised them), and
    /// `redraw` draws them only when the frame it is painting is this
    /// one. Without that check, opening a picker would show it in every
    /// window at once.
    focused_frame: usize,
    /// The active frame's top-left corner in desktop coordinates,
    /// refreshed whenever it moves or resizes. Cached rather than read
    /// from the window on demand so that directional navigation can
    /// place *every* frame -- including parked ones, whose position is
    /// otherwise only reachable through their own window handle -- and
    /// so headless tests can set one.
    frame_origin: (i32, i32),
    /// Frames asked for by a `fenix --new-window` hand-off (a
    /// `fenix/open` with `newWindow`) that haven't been opened yet, each
    /// with the files to open in it (possibly none) and whether to bring
    /// it forward. Drained by `about_to_wait`, which -- unlike
    /// `handle_user_event`, where the request arrives -- has the
    /// `&ActiveEventLoop` that creating a window needs.
    pending_frames: Vec<(Vec<fenix_tasks::PathPosition>, bool)>,
    window: Option<Arc<Window>>,
    /// The wgpu instance/adapter/device/queue every frame shares -- see
    /// `GpuContext`'s own doc comment for why there's one of these
    /// rather than one per window. `None` until the first `resumed`,
    /// same posture as `gpu`/`text`/`bg_rect`.
    gpu_context: Option<GpuContext>,
    /// The font database, glyph caches and GPU atlas every frame's
    /// `TextPipeline` shares -- see `text::FontContext`. Built once
    /// alongside `gpu_context`, because building it scans the system
    /// font database and is the single most expensive thing opening a
    /// window would otherwise repeat.
    fonts: Option<Rc<RefCell<text::FontContext>>>,
    gpu: Option<GpuState>,
    text: Option<TextPipeline>,
    /// Opaque panel backgrounds (modeline bar, which-key popup) and the
    /// selection highlight -- drawn *before* text, so they sit behind it
    /// instead of covering it. A single alpha-blended draw call can't do
    /// both "opaque background behind this text" and "opaque background
    /// in front of that text" at once, so the caret gets its own renderer
    /// drawn after text instead of sharing this one.
    bg_rect: Option<RectRenderer>,
    /// A second, independent `RectRenderer` for the popup overlay pass,
    /// for exactly the reason `text::TextPipeline::popup_renderer` is a
    /// second `TextRenderer`: `RectRenderer::flush` rewrites its *one*
    /// vertex buffer via `queue.write_buffer`, and queued writes land
    /// before any command buffer submitted after them. Reusing `bg_rect`
    /// for the overlay pass therefore overwrote the start of the very
    /// buffer the base pass's already-recorded draw call reads from, in
    /// the same not-yet-submitted encoder -- so the base layer's *first*
    /// rect of the frame silently became the popup's background rect
    /// every time any popup was open. With a surface pane focused (no
    /// current-line or selection highlights to be pushed ahead of it)
    /// that first rect is the modeline's own background bar, which is
    /// why opening the which-key menu made the modeline appear to vanish
    /// while the mode badge -- pushed second -- stayed put.
    popup_rect: Option<RectRenderer>,
    caret_rect: Option<RectRenderer>,
    /// What the animations and polish draw with -- see `MotionGpu`.
    motion_gpu: Option<motion_host::MotionGpu>,
    /// Draws every surface: extensions' surfaces and their cursors,
    /// pictures on pages, Home's logo -- each owned (as a
    /// `Surface`) by whatever owns its pixels. `None` until a real GPU
    /// exists, same posture as `text`/`bg_rect`/`caret_rect`.
    surfaces: Option<SurfacePipeline>,

    /// Every open buffer, keyed by `BufferId` -- syntax state lives on
    /// each buffer's own `OpenBuffer`. Live cursor/scroll position is
    /// *per-window*, not per-buffer (see `PaneState`'s own doc comment)
    /// -- owned by whichever `Workspace` the pane belongs to, reached via
    /// `pane_state`/`pane_state_mut`/`focused_pane_id`, not through a
    /// buffer at all. `App` never touches a buffer directly, only through
    /// this and `windows` -- use the `open`/`open_mut`/`focused_buffer_id`
    /// helpers.
    buffers: BufferList,
    /// Every workspace's own split layout (and, per workspace, every
    /// pane's own live cursor/scroll and in-flight scroll animation --
    /// see `Workspace`'s own fields) -- `windows()`/`windows_mut()`
    /// (right after the constructor) are the accessors to use everywhere
    /// else; they read/write whichever workspace is currently active, the
    /// same "always go through the helper, not the field" discipline
    /// `open`/`open_mut` already establish for buffers.
    workspaces: WorkspaceList,

    /// See `LineNumberMode` doc comment -- hardcoded for now.
    line_number_mode: LineNumberMode,

    main_view: MainView,
    /// The directory picker's listing (`SPC p a`, `SPC f e`; its page is
    /// `ExplorerTarget::Overlay`'s). A separate field from `sidebar`, so
    /// picking a folder never disturbs the sidebar.
    explorer: Option<ExplorerState>,
    /// The sidebar's own listing, independent of `explorer`. `Some` for
    /// as long as the sidebar is open, regardless of whether it's
    /// currently focused.
    sidebar: Option<ExplorerState>,
    /// How the sidebar's page is drawn: its header, `?`, its scroll.
    sidebar_view: explorer_page::View,
    /// The sidebar's width in pixels; its edge can be dragged
    /// (`explorer.width`).
    sidebar_width: f32,
    /// Each project's explorer as it was left -- the sidebar open or
    /// not, its width, its open folders, its filter -- by project root.
    explorer_sessions: HashMap<PathBuf, explorer_host::ExplorerSession>,
    /// Whether `explorer_sessions` is on the disk as it is.
    explorer_sessions_saved: bool,
    /// The file the sidebar last followed to, so it follows each change
    /// of file once rather than every frame.
    sidebar_followed: Option<PathBuf>,
    /// A path to show in a listing once it's been read again: the
    /// sidebar's file in its new root, a name just made.
    explorer_reveal_pending: HashMap<ExplorerTarget, PathBuf>,
    /// Being dragged by its edge.
    sidebar_drag: bool,
    /// The mouse in the text, on the dividers and tabs, its menu and
    /// its hover -- see `app/mouse.rs`.
    mouse: mouse::MouseState,
    /// Bumped for every filter search started, so what a superseded one
    /// finds is dropped.
    explorer_search: u64,
    /// The explorer's preview (`p`): the listing it follows, and its
    /// page.
    explorer_preview: Option<(ExplorerTarget, BufferId)>,
    /// The last click on a listing's row -- when, which listing, which
    /// row -- to tell a double-click.
    explorer_last_click: Option<(Instant, ExplorerTarget, usize)>,
    sidebar_open: bool,
    /// Whether keys currently route to the sidebar's trie instead of Vim
    /// -- only meaningful while `sidebar_open`. The sidebar stays visible
    /// but unfocused while editing, like a persistent project tree.
    sidebar_focused: bool,
    /// The terminal panel's live shell session (PTY + parser + reader
    /// thread) -- created once, on the first `SPC o t`, and deliberately
    /// *not* dropped just by toggling visibility off (see `TerminalState`'s
    /// own doc comment: the underlying process must keep running while
    /// hidden). `None` before the panel's ever been opened, or right
    /// after a respawn is triggered because the previous shell exited.
    terminal: Option<TerminalState>,
    /// The panel shell's id and the folder it was started in: its
    /// project's.
    terminal_shell: u64,
    terminal_root: Option<PathBuf>,
    /// Other projects' panel shells, running on while you're in another:
    /// each comes back as you open the panel in its project again.
    parked_terminals: Vec<(u64, PathBuf, TerminalState)>,
    /// Whether the terminal panel is currently shown -- a pure layout/
    /// rendering concern, independent of whether the session underneath
    /// it is alive (compare `sidebar_open`, which the sidebar's `Option`
    /// itself already tracked -- the terminal can't reuse that trick
    /// since its `Option` needs to survive being "closed").
    terminal_open: bool,
    /// Whether keys currently route to the terminal (as raw input bytes)
    /// instead of Vim -- only meaningful while `terminal_open`. Unlike
    /// `sidebar_focused`'s small action-key trie, a focused terminal
    /// swallows *every* key (including Space) and forwards it to the
    /// shell; `Ctrl-\` is the one chord reserved to unfocus without
    /// closing (mirrors Neovim's own `:terminal` convention), letting
    /// the panel stay visible-but-unfocused while editing elsewhere.
    terminal_focused: bool,
    /// `true` from the moment `toggle_terminal` kicks off a background
    /// shell spawn until `apply_terminal_spawned` hears back -- lets
    /// `redraw`'s own `terminal_render` show a "starting shell..."
    /// placeholder instead of an unexplained blank panel for however
    /// long that takes (see `toggle_terminal`'s own doc comment for why
    /// it can take a while on Windows specifically).
    terminal_spawning: bool,
    /// Every pane-resident shell (`SPC o T`), keyed by its own buffer --
    /// the workspace-bound counterpart to `terminal`'s one shared popup
    /// (see `TerminalTarget` for why both exist).
    ///
    /// Keyed by buffer, not by pane, for the reason 1.x's VNC sessions
    /// settled on the hard way: a buffer can be shown by any number of
    /// panes at once (that is what a split *is*), so a session pinned to
    /// one pane silently stops existing for every other pane showing the
    /// same thing.
    terminal_buffers: HashMap<BufferId, TerminalState>,
    /// Which pane-resident terminal is currently swallowing keystrokes,
    /// if any -- the buffer counterpart to `terminal_focused`, and
    /// released by the same reserved `Ctrl-\` chord.
    ///
    /// Deliberately *not* derived from which pane happens to be focused.
    /// A terminal that grabs the keyboard back every time focus lands on
    /// it would make `Ctrl-\` useless -- the release would last exactly
    /// until the next pane switch and back. Capture is engaged by asking
    /// for it (`SPC o T`, or a click into the pane), the same bargain a
    /// surface that captures the keyboard strikes.
    terminal_buffer_focused: Option<BufferId>,
    /// Buffers whose shell is still being spawned -- same "show an
    /// honest placeholder rather than an unexplained blank" job as
    /// `terminal_spawning`, per terminal rather than for the one panel.
    terminal_buffers_spawning: HashSet<BufferId>,
    /// Display names for pane-resident terminals (`*terminal 2*`), so
    /// several open at once are tellable apart in the pane title bar and
    /// in `SPC b b`. Numbered on creation and never reused, because a
    /// number that got recycled would point at a different shell than
    /// the one you remembered.
    terminal_buffer_labels: HashMap<BufferId, String>,
    /// Where each pane-resident shell was started, for the ones opened
    /// somewhere in particular (`SPC e T`).
    terminal_buffer_cwds: HashMap<BufferId, PathBuf>,
    /// What's selected in a terminal with the mouse (`terminal_mouse`).
    term_select: Option<terminal_mouse::TermSelection>,
    /// The link `Ctrl` is held over in a terminal: which, its row and
    /// columns (`terminal_mouse::terminal_hover`).
    term_hover: Option<(terminal_mouse::TermTarget, u16, usize, usize)>,
    /// A run of clicks in a terminal: a second selects a word, a third
    /// the line.
    term_clicks: crate::mouse::ClickCounter,
    /// The pane a jump just moved the cursor in (`mark_jump`): its next
    /// scroll centres the cursor when it's out of view.
    jump_view: Option<fenix_window::WindowId>,
    /// Panes without the focus whose cursor was moved for you (following
    /// an agent): scrolled to it, centred, on the next frame -- only the
    /// focused pane follows its cursor by itself.
    reveal_panes: HashSet<fenix_window::WindowId>,
    /// A terminal buffer that runs one program instead of a shell (a
    /// serial monitor, a debugger console) -- what `spawn_terminal_for`
    /// starts, and restarts, in it.
    terminal_buffer_programs: HashMap<BufferId, (String, Vec<String>)>,
    /// How many pane-resident terminals have ever been opened -- the
    /// source of the numbers in `terminal_buffer_labels`.
    terminal_buffers_opened: usize,
    /// Latest known cursor position, in physical window pixels -- `None`
    /// until the first `CursorMoved`. `MouseWheel`/`MouseInput` carry no
    /// position of their own in `winit`, so this is what click-to-focus
    /// and wheel-scroll hit-test against.
    cursor_pos: Option<(f32, f32)>,
    /// The OS cursor icon last actually set (`App::update_hover_cursor`),
    /// so `window.set_cursor` is only called on a real change rather than
    /// on every one of potentially hundreds of `CursorMoved` events per
    /// second. Deliberately not preserved across `FrameState` park/
    /// activate the way `cursor_pos` is -- it's cheap, self-correcting
    /// UI state that recomputes correctly the moment the mouse next moves
    /// in a reactivated frame, unlike `cursor_pos` itself.
    current_cursor_icon: Option<winit::window::CursorIcon>,
    /// Every visible tab's on-screen rect, as measured from the glyphs
    /// the last `redraw` actually shaped and drawn (see the `title_span_
    /// bounds` call there). Mouse hit-testing reads this rather than
    /// re-deriving tab positions from character counts, which is what
    /// used to let a click and the pixel it landed on disagree -- and,
    /// more visibly, let a tab's own highlight sit right of the tab.
    /// Empty until the first frame, and on a theme with `show_tabs`
    /// off; `frame_geometry` falls back to no tabs in both cases, which
    /// is correct for each.
    tab_geometry: Vec<(fenix_window::WindowId, TabRect)>,
    /// What `explorer` (the directory picker's listing) is currently for
    /// -- meaningless while the picker isn't open. See `ExplorerPurpose`'s
    /// own doc comment.
    explorer_purpose: ExplorerPurpose,
    explorer_prompt: Option<ExplorerPrompt>,
    /// A copy/move waiting on an answer about what to overwrite -- see
    /// `ExplorerConflict`. Capturing, like `explorer_prompt`.
    explorer_conflict: Option<ExplorerConflict>,
    /// A bulk rename worked out and waiting to be confirmed -- nothing
    /// has been renamed while this is `Some`.
    rename_confirm: Option<Vec<fenix_explorer::Rename>>,
    /// The long file operation currently running, if any -- see
    /// `ExplorerJob`. At most one: two copies competing for the same
    /// disk finish no sooner than one after the other, and a single
    /// progress line is something a person can actually read.
    explorer_job: Option<ExplorerJob>,
    /// Completions offered for what is currently typed in the path bar,
    /// and how far through them Tab has cycled. Recomputed on every
    /// keystroke rather than tracked incrementally -- listing one
    /// directory is cheap, and a cache would be one more thing that can
    /// disagree with the text.
    explorer_completions: Vec<String>,
    /// The text buffers of names being edited (`SPC e w`): the listing
    /// page each came from, and the entries their lines stood for when
    /// editing began.
    ///
    /// The snapshot is the whole mechanism: line N means entry N of
    /// *this list*, not of whatever the directory happens to contain
    /// when the edit is applied. Without it a listing that refreshed
    /// mid-edit would silently re-point every line.
    rename_mode: HashMap<BufferId, (BufferId, Vec<fenix_explorer::Entry>)>,
    /// The machine's drives, read once on the first Places listing.
    ///
    /// Cached because the query costs a subprocess, and re-run only when
    /// asked (`SPC e b` on an empty cache) rather than on a timer:
    /// drives do not come and go often, and paying half a second every
    /// time the picker opens would be worse than occasionally missing a
    /// USB stick that was plugged in mid-session.
    volumes: Vec<fenix_fs::Volume>,
    /// Whether the drive query is already in flight, so asking twice
    /// before it lands does not run two subprocesses.
    volumes_loading: bool,
    /// Directories the explorer has actually been, most recent first --
    /// the same machinery as `recent_files`, kept in its own file
    /// because "reopen a file" and "go back to a folder" are different
    /// questions (see `RecentFiles::default_dirs_path`).
    recent_dirs: fenix_project::RecentFiles,
    /// Whether the delete being confirmed right now is the permanent
    /// one (`D!`) rather than the Recycle Bin one (`D`). Read when the
    /// confirmation is answered, so the prompt and the action can never
    /// disagree about which was asked for.
    explorer_delete_permanently: bool,
    /// Topmost visible row of the picker's candidate list. Reset to 0
    /// whenever a new picker is entered.
    picker_scroll: usize,

    /// The current buffer's project root (re-derived whenever a file is
    /// opened -- same "always fresh, never stale" posture as everything
    /// else computed from buffer state). `None` outside any recognized
    /// project.
    project_root: Option<PathBuf>,
    /// Each project root's kind (`fenix_project::detect_kind`), looked up
    /// once -- the modeline and the window title ask every frame. Behind
    /// a `RefCell` so `&self` readers like `modeline_pieces` can fill it.
    /// Cleared when a project is created, the one time a kind can change
    /// under Fenix's own hands.
    project_kinds: RefCell<HashMap<PathBuf, fenix_project::ProjectKind>>,
    /// How many times the extensions' project kinds changed this run: a
    /// hub fetch says which it ran under, and what one from before the
    /// last change sends back is dropped (`project_kinds_changed`).
    kinds_generation: u64,
    /// The project pages open right now (wizard, hub, doctor, settings),
    /// by their buffer -- see `pages`.
    pages: HashMap<BufferId, pages::PageState>,
    /// Pins and groups -- yours, not the projects', so kept beside
    /// `projects.txt`.
    project_meta: fenix_project::meta::ProjectMeta,
    /// The doctor's last word on each project (worst health, problems),
    /// for the hub's health dot.
    project_health: HashMap<PathBuf, (fenix_project::doctor::Health, usize)>,
    active_picker: Option<ActivePicker>,
    /// The file the palette's preview last read, and its first lines.
    palette_preview: RefCell<Option<(PathBuf, Vec<String>)>>,
    /// Where each command sits under the leader, worked out once per
    /// leader keymap (`palette::leader_places`).
    leader_places: RefCell<Option<std::rc::Rc<palette::Places>>>,
    /// `Ctrl-O`/`Ctrl-I` -- a global, not per-pane/per-workspace,
    /// back/forward pair of stacks (like a browser's history, not real
    /// Vim's single circular list with a pointer -- a deliberate
    /// simplification: `jump_back`/`jump_forward` pushes the *other*
    /// stack's entry directly rather than reconciling against wherever
    /// a pointer currently sits). Populated by `record_jump`, called
    /// from `VimEvent::JumpRecorded` (same-buffer `gg`/`G`/`%`/search)
    /// and from `picker_confirm`'s Grep/Symbol arms (cross-file jumps,
    /// the reason this exists at all -- `fenix-vim` only ever knows
    /// about one buffer at a time, so a jumplist that can point at a
    /// *different* file can't live there).
    jump_back_stack: Vec<JumpEntry>,
    jump_forward_stack: Vec<JumpEntry>,
    /// `m{name}`/`` `{name} ``/`'{name}` -- a named position bookmark,
    /// keyed by the mark char. `JumpEntry` reused verbatim: a mark is
    /// exactly "a saved (buffer, char_idx) pair," the same shape a
    /// jumplist entry already is, including the same closed-buffer-goes-
    /// stale simplification (see `JumpEntry`'s own doc comment) -- marks
    /// aren't persisted across a buffer close/reopen the way real Vim's
    /// path-keyed marks are. Doesn't distinguish lowercase (buffer-
    /// local) from uppercase (global) the way real Vim does either --
    /// every mark here can jump across files, one flat namespace, the
    /// same simplification `jump_back_stack`/`jump_forward_stack`
    /// already made for the same reason (simpler, and arguably more
    /// useful, than replicating Vim's split exactly).
    marks: HashMap<char, JumpEntry>,
    /// The full match/location list from whichever of `SPC p s`
    /// (`run_grep`) or an LSP `gr`/multi-result `gd` most recently ran --
    /// kept around after the picker (grep) or the jump itself (LSP)
    /// closes/lands so `SPC p n`/`SPC p N` (`quickfix_next`/`quickfix_
    /// prev`) can keep sweeping through it without reopening a picker or
    /// re-running the search/request. Real Vim's quickfix list,
    /// generalized just far enough to cover the two sources this app
    /// actually has (no `:make`/build-error integration yet -- that's
    /// the build/task-runner milestone).
    quickfix: Vec<QuickfixEntry>,
    /// The Search page's running search, history and last replace.
    search: search_host::SearchHost,
    /// `[3/12]`'s match positions: (buffer, edits, pattern) -> starts.
    search_count_cache: std::cell::RefCell<Option<((BufferId, u64, String), Vec<usize>)>>,
    /// Index into `quickfix` of wherever `quickfix_next`/`quickfix_prev`/
    /// confirming a `Grep` picker entry last landed -- `None` before
    /// any match in the current list has been visited.
    quickfix_index: Option<usize>,
    /// The TODO-style comment keywords `syntax_highlights_for_visible_
    /// range` found in each buffer's visible window on its last run, for
    /// the renderer to tint behind -- recorded there because that's the
    /// one place the capture names are still known, before they're
    /// turned into colors.
    todo_spans: HashMap<BufferId, Vec<(std::ops::Range<usize>, fenix_syntax::TodoKind)>>,
    /// `SPC f f`'s typed-path prompt, when in progress -- a plain
    /// capturing prompt, same shape as `pending_grep_query`.
    find_file_prompt: Option<String>,
    /// `SPC f R`'s new-name prompt, when in progress -- pre-seeded with
    /// the current filename (see `start_rename_file_prompt`).
    rename_file_prompt: Option<String>,
    /// `SPC c r`'s new-name prompt (LSP rename), when in progress --
    /// pre-seeded with the identifier under the cursor, same capturing-
    /// prompt shape as `rename_file_prompt` (Enter sends the actual
    /// `textDocument/rename` request rather than applying anything
    /// synchronously -- the edit only lands once the server answers,
    /// see `PendingLspRequest::Rename`).
    lsp_rename_prompt: Option<String>,
    /// With standard keys, a rename prompt's prefilled name is selected
    /// until a key: typing replaces it, Backspace clears it, an arrow
    /// keeps it to change.
    prompt_prefill_selected: bool,
    refactor_preview: Option<refactor::Preview>,
    /// The coding agents: their connections, the threads, the panel and
    /// its prompt (`agents`).
    agents: agents::AgentHost,
    /// What MCP bridges asked (`fenix/agent.*`): questions, toolsets, news.
    agent_calls: agent_calls::AgentCalls,
    /// Agents' requests to the language servers (`lsp`, `refactor`).
    agent_lsp: agent_lsp::AgentLsp,
    /// Resources MCP clients subscribed to (`agent_context`).
    agent_subscriptions: agent_context::Subscriptions,
    /// Pipelines under way (`SPC a P`).
    pipelines: agent_pipelines::Pipelines,
    refactor_undo: Option<refactor::Undo>,
    /// `SPC TAB r`'s new-name prompt, when in progress -- pre-seeded
    /// with the active workspace's current name, same capturing-prompt
    /// shape as `rename_file_prompt`. Lives on `App` rather than per
    /// frame like `workspaces` itself: only one frame can be actively
    /// typing at a time, and it always acts on `self.workspaces`, which
    /// is always whichever frame is currently active.
    workspace_rename_prompt: Option<String>,
    /// Standard keys: Escape was pressed once in a compose pane with text
    /// in it; again throws the text away.
    compose_discard_armed: bool,
    /// `nav.goto_line`'s prompt: the number typed so far.
    goto_line_prompt: Option<String>,
    /// How often the leader's chord opened it this run: the modeline says
    /// which chord does until it's been used a few times.
    leader_uses: u32,
    /// When the last key came: standard keys' undo pause.
    last_key_at: Option<Instant>,
    /// Each pane's extra carets (standard keys' more cursors).
    carets: carets::PaneCarets,
    /// Edits made at several carets, by the tag on their undo step: the
    /// carets before and after, for undo and redo to put back.
    caret_steps: carets::CaretSteps,
    /// The switcher `buffer.switch_recent` (Ctrl+Tab) opened is up: letting
    /// go of Ctrl picks its row.
    switching_recent: bool,
    /// `SPC f D`'s y/n gate, armed until the next keypress confirms
    /// (`y`) or cancels (anything else) -- same shape as `docker_
    /// confirm_remove`.
    delete_file_confirm: bool,
    /// Loaded once at startup; saved back to disk whenever `SPC p a`/
    /// `SPC p d` change it, or the switch-project picker re-selects an
    /// already-known root (an MRU bump, not a new registration -- see
    /// `refresh_project_root`'s doc comment for why opening a file no
    /// longer registers its project automatically). Empty (silently) on
    /// a platform with no config-directory concept, or if the file can't
    /// be read for some other reason -- a picker just starting with no
    /// history isn't worth failing over.
    known_projects: fenix_project::KnownProjects,
    /// Same persisted-list shape as `known_projects`, but automatic --
    /// every real file opened (startup CLI arg, the picker, the
    /// explorer) gets recorded via `record_recent_file`, no explicit
    /// `SPC p a`-style curation step. Read by the dashboard.
    recent_files: fenix_project::RecentFiles,
    /// Per-line metadata for whichever dashboard buffer(s) are
    /// currently open, keyed by `BufferId` (not one flat field) so an
    /// older still-open dashboard buffer elsewhere in the window tree
    /// never gets misattributed metadata from the newest one -- same
    /// per-buffer-keyed-state precedent as `scroll_anims`. Consulted by
    /// `dashboard_activate_selected` and the dashboard's own syntax-
    /// highlight coloring.
    /// Every open Home buffer's current layout (see `app/home.rs`),
    /// rebuilt whenever its pane's size in cells changes.
    home_views: HashMap<BufferId, dashboard::HomeView>,
    /// What Home shows, shared by every Home buffer.
    home_data: dashboard::HomeData,
    /// A page saw `g` and is waiting to learn whether it's a tab key
    /// (`gt`, `gT`, `gh`, `g<Tab>`) -- see `App::page_key`.
    page_g_pending: bool,
    /// The last tab clicked, and when -- a second click on it soon after
    /// is a double-click, which keeps a preview.
    last_tab_click: Option<(Instant, fenix_window::WindowId, BufferId)>,
    /// A tab the left button went down on, carried to wherever it's
    /// released to move it there.
    tab_drag: Option<(fenix_window::WindowId, BufferId)>,
    /// A project workspace's Home data, by project root -- see
    /// `App::home_data_for`.
    home_project_data: HashMap<PathBuf, dashboard::HomeData>,
    /// The find-file picker's listings, per root (`file_index`).
    file_index: file_index::FileIndex,
    /// When the disk probe now out was sent, and what each buffer's file
    /// looked like then (`disk_probe`).
    disk_probe_out: Option<Instant>,
    /// The remote machines Fenix is connected to (`remote`).
    remote: remote::Remotes,
    /// Which projects may have the programs they name run (`trust`).
    trust: trust::ProjectTrust,
    disk_probe_basis: HashMap<BufferId, DiskFingerprint>,
    /// Home's recent files as last looked at, when, and when the look now
    /// out was sent (`home::request_recent_probe`).
    recent_seen: HashMap<PathBuf, home::RecentSeen>,
    recent_probed: Option<Instant>,
    recent_probe_out: Option<Instant>,
    /// The modeline's `git status` now out: when it was sent, and whether
    /// it has finished (`refresh_chrome_git`).
    chrome_git_out: Option<(Instant, Arc<AtomicBool>)>,
    /// Home's TODO column, per project root, as the last sweep found it,
    /// and the roots being swept now (see `refresh_home_data`).
    home_todos: HashMap<PathBuf, Vec<dashboard::TodoItem>>,
    home_todos_pending: HashSet<PathBuf>,
    /// The logo lockup image and its GPU copy.
    home_logo: Option<home::HomeLogo>,

    /// The listing of every explorer page open in a pane (`SPC f j`),
    /// by its page's buffer. Distinct from `explorer`/`sidebar`, the
    /// directory picker's and the sidebar's.
    dired_states: HashMap<BufferId, ExplorerState>,
    /// The newest directory read for each explorer buffer -- see
    /// `ExplorerRequest`. At most one per buffer: asking for a different
    /// directory supersedes whatever was being read, rather than
    /// queueing behind it.
    explorer_requests: HashMap<ExplorerTarget, ExplorerRequest>,
    /// What each listing's folders looked like on disk (`ExplorerStamps`),
    /// for the disk poll to tell when something else changed them.
    explorer_stamps: HashMap<ExplorerTarget, explorer_host::ExplorerStamps>,
    /// The explorers' folders watched (`explorer_watch`), and the listings
    /// a change touched, to read again once it's settled (`..._due`).
    explorer_watch: Option<explorer_watch::ExplorerWatch>,
    explorer_dirty: HashSet<ExplorerTarget>,
    explorer_dirty_due: Option<Instant>,
    /// A drag over a page's text under way: its pane, the page, and where
    /// the page's text starts on screen (`page_select`).
    page_dragging: Option<(fenix_window::WindowId, BufferId, (f32, f32))>,
    /// Source of `ExplorerRequest::id`. Global rather than per-buffer so
    /// an id is never reused across buffers, which keeps a late result
    /// from a closed buffer from ever matching a live request.
    next_explorer_request: u64,

    /// A short title string for whichever panes currently have one --
    /// a general primitive (see `text.rs`'s matching `titles` field),
    /// populated only by the Docker panel's fixed multi-pane layout
    /// today; an ordinary `SPC w v`/`SPC w s` split never gets an entry
    /// here, so its rendering is completely unaffected.
    pane_titles: HashMap<fenix_window::WindowId, String>,
    /// The inline git gutter marks (`App::refresh_gutter_hunks`) for every
    /// buffer that currently has any -- a buffer with no backing path, or
    /// outside a git repo, or with no changes against `HEAD`, simply has
    /// no entry (not an empty `Vec`, so "never computed" and "computed,
    /// nothing changed" both collapse to the same absent-key case).
    /// Keyed by `BufferId`, not pane -- the marks describe the *file*,
    /// not any particular view of it, same reasoning `BufferList`'s own
    /// dirty-tracking is buffer-scoped rather than per-pane.
    gutter_hunks: HashMap<BufferId, Vec<GutterMark>>,
    /// When each buffer's gutter marks were last worked out, so marks from
    /// `refresh_all_gutter_hunks`' thread don't replace newer ones.
    gutter_refreshed: HashMap<BufferId, Instant>,
    /// The diff each buffer's gutter marks came from, with its
    /// repository's root -- what `]h`, `SPC g a`/`d`/`i` act on.
    gutter_diffs: HashMap<BufferId, (PathBuf, fenix_diff::FileDiff)>,
    /// Blame shown beside a file (`SPC g B`), by path.
    blames: HashMap<PathBuf, git_editor::Blame>,
    /// Where the focused file's repository stands, for the modeline.
    chrome_git: Option<git_editor::ChromeGit>,
    /// When `[git] auto_fetch` last tried each repository.
    fetch_attempts: HashMap<PathBuf, Instant>,
    /// Collapsed structural scope headers, keyed by buffer. The rendered
    /// row list is derived from the current syntax tree every frame, so an
    /// edit can never leave stale byte or character offsets behind.
    code_folds: HashMap<BufferId, HashSet<usize>>,
    /// Buffers whose wrap was turned on or off (`view.toggle_wrap`),
    /// whatever the default for them is.
    wrap_overrides: HashMap<BufferId, bool>,
    /// Wrapped lines' rows, found once for the text as it is.
    break_cache: std::cell::RefCell<row_scroll::BreakCache>,
    /// Rows above lines that aren't text, by buffer and line: what a code
    /// lens will put there. A wrapped pane lays them out.
    virtual_rows: HashMap<BufferId, std::collections::BTreeMap<usize, Vec<String>>>,
    /// Where each buffer's semantic tokens come from (its server and its
    /// path as servers know it), worked out when it's synced, so drawing
    /// it looks at nothing on disk.
    semantic_of: HashMap<BufferId, semantic::SemanticTarget>,
    /// Each buffer's lens servers and its path as they know it, worked
    /// out when it's synced.
    lens_of: code_lens::LensOf,
    /// Each buffer's rows of lenses, by line: what a click on one runs.
    lens_rows: HashMap<BufferId, std::collections::BTreeMap<usize, fenix_lsp::code_lens::LensRow<code_lens::LensTag>>>,
    /// Fenix's own "Run test" lenses, by buffer, as of an edit count.
    own_lenses: HashMap<BufferId, (u64, Option<u64>, Vec<lsp_types::CodeLens>)>,
    /// The lens under the pointer: its buffer, its row's line, its columns.
    lens_hover: Option<(fenix_window::WindowId, usize, std::ops::Range<usize>)>,
    /// A test's seam: every pane that could wrap goes by rows (at
    /// `rows::UNBOUNDED` when it doesn't wrap), with or without rows
    /// above its lines, so the row path can be checked against the
    /// line path it stands in for.
    force_row_path: bool,
    /// The grammar's comments and strings (bytes) in what was last drawn
    /// of each buffer with semantic tokens: where none is laid.
    semantic_protected: HashMap<BufferId, Vec<std::ops::Range<usize>>>,
    /// The focused buffer's decorations as moves by row see them, and the
    /// `decorations_rev` they were copied at.
    screen_decorations: Option<(BufferId, u64, std::sync::Arc<Vec<crate::decorations::Decoration>>)>,
    /// The columns of text each pane had room for at the last frame:
    /// what its lines wrap at.
    pane_text_cols: HashMap<fenix_window::WindowId, usize>,
    /// Each buffer's fold level (`zm`, `zr`): folds this deep or more are
    /// closed. None: every one open.
    fold_levels: HashMap<BufferId, usize>,
    /// Notifications: the toasts showing and the history (`notify`).
    notifications: crate::notify::Center,
    /// Where each toast was drawn last frame, for clicks.
    toast_rects: Vec<(u64, fenix_window::Rect)>,
    /// When each extension's last notifications came, for their rate.
    notification_rates: HashMap<String, Vec<Instant>>,
    /// The optional parts of Fenix that live behind `feature::Feature`
    /// (Docker so far), each owning its own state.
    /// Every command, built once at startup -- shared (`Rc`) so a command
    /// can run with the app borrowed mutably (`App::run_command`).
    commands: Rc<CommandRegistry>,
    /// The extensions that run code, and their workers (`wasm_host`).
    wasm: wasm_host::WasmHost,
    /// The test explorer's host: providers run, tests found and run
    /// (`tests_host`), behind `tests.enabled`.
    tests: tests_host::TestsHost,
    /// The commands run lately, newest first (the palette's order).
    recent_commands: Vec<String>,
    /// A command the palette chose, run by `handle_key` (which has the
    /// event loop) once the picker has let go.
    queued_command: Option<String>,
    /// The palette is describing commands (`SPC h c`), not running them.
    palette_describes: bool,
    /// `SPC h k`: the keys typed so far, to say what they run.
    describing_keys: Option<Vec<KeyPress>>,
    /// Tab in the palette: the command being given keys, and the keys
    /// typed so far.
    binding_keys: Option<(String, Vec<KeyPress>)>,
    /// The active build/task runner's single-pane session (`SPC t t`),
    /// if any -- see `TaskSession`'s own doc comment.
    task_session: Option<TaskSession>,
    /// Last task per canonical project root, used by `SPC t r`.
    project_last_tasks: HashMap<PathBuf, fenix_tasks::TaskDef>,
    /// The active DAP debug session (`SPC u u`), if any -- see
    /// `DebugSession`'s own doc comment.
    debug_session: Option<DebugSession>,
    /// A debug session waiting for its `preLaunchTask`, by the task's name.
    debug_after_task: Option<(String, PendingLaunch)>,
    /// Debug connections' events with no event loop to bring them (tests):
    /// read by hand.
    #[cfg(test)]
    dap_unread: Vec<(u64, std::sync::mpsc::Receiver<fenix_dap::DapEvent>)>,
    /// Configured breakpoints, by canonicalized file path -> the set of
    /// 1-indexed lines (DAP's own convention, matching `fenix_project::
    /// grep::GrepMatch`'s) with a breakpoint on them -- persists across
    /// buffers being closed/reopened and across debug sessions
    /// (deliberately *not* cleared when a session ends), the same way a
    /// real debugger's breakpoints outlive any one run. Sent to the
    /// adapter as a whole-file `setBreakpoints` list (per DAP spec, one
    /// call replaces every breakpoint in that file, so this is always
    /// the full source of truth, never a delta).
    breakpoints: HashMap<PathBuf, std::collections::BTreeMap<usize, debug_breakpoints::Breakpoint>>,
    /// Values pinned in the code (`SPC u P`), written at their lines at
    /// every stop: kept, like the breakpoints, from session to session.
    debug_pins: Vec<debug_breakpoints::Pin>,
    /// The exceptions you've said to stop on or not, by the adapter's
    /// filter; the adapter's own default for the rest.
    debug_exceptions: HashMap<String, bool>,
    /// Each project's last launch, by its name: what `SPC u u` starts
    /// again rather than asking (`SPC u U` asks).
    debug_last_launch: HashMap<PathBuf, String>,
    /// `SPC u U` is under way: the launches offered even when one was
    /// used last.
    debug_pick_launch: bool,
    /// Esc pressed in DEBUG since the session last stopped: the editor's
    /// in Normal until the next stop, or `SPC u u` / F5 brings DEBUG back.
    debug_mode_left: bool,
    /// Run to cursor's breakpoint (`r` in DEBUG), sent beside
    /// `breakpoints` until the session next stops.
    debug_run_to: Option<(PathBuf, usize)>,
    /// The debug dock is out (`SPC u d` puts it away): shown while there's
    /// a session, under the panes and the terminal, above the modeline.
    debug_dock_open: bool,
    /// The drawer's own state: whether it has the keys, where in it you
    /// are, the evaluate line.
    debug_drawer: debug_drawer::DebugDrawer,
    /// The Ember Trail: the stops so far, kept from one session to the
    /// next (never to disk), and the one gone back to.
    debug_trail: debug_trail::EmberTrail,
    /// The tool status panel's single-pane session (`SPC l m`), if it's
    /// been opened this session -- see `ToolStatusSession`'s own doc
    /// comment.
    tool_status_session: Option<ToolStatusSession>,
    /// Per-line metadata for every real `BufferKind::Git` buffer
    /// currently open (`SPC g g`) -- mirrors `DockerHost::lines` exactly.
    git_lines: HashMap<BufferId, Vec<Option<git_panel::GitLine>>>,
    /// Per-line metadata for every real `BufferKind::Diff` buffer
    /// currently open -- mirrors `git_lines`, with `diff_view`'s own
    /// richer payload (the file/hunk/line each row came from, which is
    /// what every hunk-level action resolves against).
    diff_lines: HashMap<BufferId, Vec<Option<diff_view::DiffViewLine>>>,
    /// The parsed diff each `BufferKind::Diff` buffer is currently
    /// showing, alongside the repo it came from -- kept so a hunk action
    /// can rebuild that exact hunk's patch (`fenix_diff::hunk_patch`)
    /// without re-shelling `git diff` and hoping nothing moved in
    /// between. Same "cache what the panes are showing" reasoning as
    /// `GitSession::files`/`branches`/`commits`.
    diff_models: HashMap<BufferId, DiffModel>,
    /// Per-line metadata for every real `BufferKind::Graph` buffer --
    /// mirrors `diff_lines`, carrying each row's colored spans and the
    /// commit it draws.
    graph_lines: HashMap<BufferId, Vec<Option<graph_view::GraphLine>>>,
    /// Per-line metadata for every real `BufferKind::Merge` buffer --
    /// which conflict each row belongs to, which side its text came
    /// from, and where its two columns split.
    merge_lines: HashMap<BufferId, Vec<Option<merge_view::MergeViewLine>>>,
    /// Armed by `d` on Files/Branches/Stash until the next keypress
    /// confirms or cancels -- mirrors `DockerHost::confirm_remove`, just over
    /// `GitConfirmAction`'s several destructive-action kinds instead of
    /// Docker's single "remove."
    git_confirm: Option<GitConfirmAction>,
    /// The History view (`SPC g l`), if open -- see `HistorySession`.
    history_session: Option<HistorySession>,
    /// What a pathless view of a file at a ref is called (`main:src/a.rs`).
    ref_view_names: HashMap<BufferId, String>,
    /// Each buffer's own indent step for its guides (`file_indent_step`),
    /// as of the edit count it was read at.
    indent_steps: std::cell::RefCell<HashMap<BufferId, (u64, Option<usize>)>>,
    /// Open side-by-side diffs (`SPC g v`) -- see `side_by_side`.
    side_by_sides: Vec<side_by_side::SideBySide>,
    merge_session: Option<MergeSession>,
    compose: Option<ComposeSession>,
    /// Each file-backed buffer's file as Fenix last saw it. Absent for a
    /// buffer whose file hasn't been looked at yet, which the sweep
    /// treats as "check it properly".
    disk_state: HashMap<BufferId, DiskFingerprint>,
    /// Buffers whose file changed on disk while they had unsaved edits.
    /// Saving one of these would silently discard the other version, so
    /// it takes `:w!`; `:e!` goes the other way.
    externally_changed: HashSet<BufferId>,
    /// Where unsaved work is written so a crash doesn't take it. `None`
    /// when the config directory can't be found at all, in which case
    /// the whole feature quietly does nothing rather than failing on
    /// every keystroke.
    session: session::State,
    recovery_dir: Option<PathBuf>,
    /// Each dirty buffer's `edit_count` as of its last snapshot, so an
    /// unchanged buffer isn't rewritten every tick.
    snapshot_state: HashMap<BufferId, u64>,
    unnamed_snapshots: HashMap<BufferId, PathBuf>,
    recovery_failures: HashMap<BufferId, String>,
    /// When the next sweep is due. Driven from `about_to_wait`'s own
    /// `WaitUntil` deadline rather than a poller thread: `stat` on a
    /// handful of open files is far cheaper than a thread and a channel,
    /// and keeping it on the main thread means no locking around the
    /// buffer list it walks.
    next_disk_check: Instant,
    /// Capturing a commit message or new branch name -- mirrors
    /// `explorer_prompt`'s "next keystrokes are text input" shape.
    git_prompt: Option<GitPrompt>,
    /// Set by `x` on a Git pane -- mirrors `DockerHost::menu_open` exactly.
    git_menu_open: bool,
    /// The active Git multi-pane session (`SPC g g`), if any -- see
    /// `GitSession`'s own doc comment.
    git_session: Option<GitSession>,
    /// Pictures pages show, read and uploaded (the reading view's, a
    /// note's, a diagram's drawing).
    page_images: reading_host::PageImages,
    /// One running language server per language, keyed by
    /// `fenix_syntax::LanguageId` -- spawned lazily the first time a
    /// buffer of that language is focused (`ensure_lsp_session`). A
    /// single entry per language for now, not yet the `Vec<LspSession>`
    /// (several attached servers, e.g. pyright for types alongside ruff
    /// for lint/format) the wider design calls for -- deliberately
    /// proven with one server end-to-end first, widened when a second
    /// server actually gets wired in, same "prove it, then generalize"
    /// order the transport itself was already built in.
    lsp_sessions: HashMap<LspKey, LspSession>,
    /// The functions each language server named for its project
    /// (`workspace/symbol`), and when it was last asked: a Tcl call to a
    /// proc defined in another file -- found by the Tcl extension's ctags
    /// -- is colored as one.
    lsp_workspace_names: HashMap<LspKey, (Instant, std::sync::Arc<std::collections::HashSet<String>>)>,
    /// What the symbol picker (Ctrl+T) has asked, and been answered.
    symbol_search: problems::SymbolSearch,
    /// Where the cursors were before their last moves, for Ctrl+U
    /// (`edit.cursor_undo`); and whether it's Ctrl+U moving them now.
    caret_marks: Vec<carets::CaretMark>,
    caret_undoing: bool,
    /// What auto-save (`editor.auto_save`) has seen.
    auto_save: auto_save::AutoSaveState,
    /// What the update check (`updates.check`) knows.
    updates: updates::UpdateState,
    /// Each Tcl buffer's language servers, as last worked out (every few
    /// seconds, not every frame: it can touch the disk) -- whose
    /// `lsp_workspace_names` its calls are colored by.
    tcl_name_servers: HashMap<BufferId, (Instant, Vec<LspKey>)>,
    lsp_unavailable: std::collections::HashSet<LspKey>,
    /// The language servers being started off the UI thread
    /// (`start_lsp_session`), each with the generation its start was
    /// given: a start that comes back to another, or to none -- the
    /// server was stopped or restarted meanwhile -- is let go.
    lsp_starting: HashMap<LspKey, u64>,
    /// Under test, where a start off the UI thread sends what it got,
    /// as the event loop would be sent it; none starts on the UI thread.
    #[cfg(test)]
    lsp_start_sink: Option<std::sync::mpsc::Sender<FenixUserEvent>>,
    /// `SPC q r` asked for Fenix to start again once it has quit
    /// (`prepare_restart`); `main` starts the new one.
    pub(crate) restart_requested: bool,
    /// What the last frame's language-server sync saw
    /// (`sync_lsp_if_changed`): the focused buffer, its edit count, the
    /// servers, those ready, and those given up on.
    lsp_frame_synced: Option<(BufferId, u64, usize, usize, usize)>,
    /// Why each of `lsp_unavailable` stopped, with the last it wrote to
    /// stderr: what Tool Status shows.
    lsp_failures: HashMap<LspKey, String>,
    /// A missing language server offered, asked about, or installing.
    server_install: server_install::InstallState,
    /// What `.editorconfig` says of each file looked at, by path.
    editorconfig_cache: HashMap<PathBuf, fenix_config::editorconfig::Properties>,
    /// Each buffer's inlay hints: which server, what's asked and drawn.
    inlay: HashMap<BufferId, inlay_hints::InlayDoc>,
    /// Each project root's additional servers (`tools.json`'s
    /// `lsp.additional`), read once and again on `:lsp-restart`.
    lsp_additional: HashMap<PathBuf, Vec<fenix_project::tools::AdditionalServer>>,
    /// Requests out to several servers, by id (`PendingLspRequest`'s
    /// `fanout`).
    lsp_fanouts: HashMap<u64, LspFanout>,
    next_lsp_fanout: u64,
    /// The hover request `lsp_hover` is showing answers for; a keypress
    /// clears it, and later answers are dropped.
    lsp_hover_fanout: Option<u64>,
    lsp_completion: Option<LspCompletion>,
    /// The most recent `textDocument/publishDiagnostics` for each file,
    /// keyed by its canonical path (LSP publishes per-URI regardless of
    /// whether a buffer for it is currently open, so this can't be keyed
    /// by `BufferId`). Cleared for a path when a server publishes an
    /// empty list for it (which means "no more diagnostics," not "no
    /// change") or when that path's buffer closes.
    diagnostics: fenix_lsp::fanout::Diagnostics<LspKey>,
    /// `K`'s hover popup text, flattened from whatever `textDocument/
    /// hover` last answered (`hover_contents_text`) -- `Some` shows the
    /// popup (`hover_popup`), cleared on the next keypress `route_
    /// keypress` sees (a transient popup, same posture as the which-key
    /// menu) or if a newer hover request superseded it before this one's
    /// answer arrived (`apply_lsp_response`'s own `buffer` check).
    lsp_hover: Option<String>,

    /// Elastic-column layout for every real `BufferKind::Table` buffer
    /// currently toggled on (`SPC f t`), keyed by `BufferId` -- same
    /// per-buffer-keyed precedent as `dashboard_lines`/`DockerHost::lines`.
    /// See `TableView`'s own doc comment.
    table_views: HashMap<BufferId, TableView>,

    /// The multi-source key-capture tap for `q{register}` macro
    /// recording -- accumulated by `dispatch_keypress` for every key
    /// while `self.vim.is_recording()`, including ones a leader command
    /// or a prompt swallows before `VimState` ever sees them (see
    /// `dispatch_keypress`'s own doc comment for why this can't just
    /// live in `fenix-vim`). Cleared on `VimEvent::MacroRecordStart`,
    /// drained into `VimState::finish_recording` on `MacroRecordStop`.
    macro_capture: Vec<KeyPress>,
    /// Whether the in-progress recording (`macro_capture`) should
    /// append to its target register's existing content instead of
    /// overwriting it -- set from `VimEvent::MacroRecordStart`'s own
    /// case (an uppercase register name), since that event only fires
    /// once, at the start, while `finish_recording` isn't called until
    /// the end.
    macro_recording_append: bool,
    /// Whichever register `@{register}` last actually replayed (not
    /// last *recorded*) -- what `@@` repeats.
    last_played_macro: Option<char>,
    /// `> 0` while inside `play_macro`, at whatever nesting depth a
    /// macro's own `@`-calls have recursed to -- only the outermost
    /// call (`0`) resets `replay_budget`, so the cap applies to the
    /// whole nested replay's total key count, not per level.
    replay_depth: u32,
    /// Keystrokes still allowed this top-level `@` replay (shared
    /// across every nested `@`-call it makes) -- see `MACRO_REPLAY_
    /// BUDGET`'s own doc comment.
    replay_budget: u32,
    /// Raw keys since the last point `VimState::is_idle()` was true --
    /// `.`'s own capture, directly parallel to `macro_capture` (same
    /// "tap every keypress in `dispatch_keypress`, including ones a
    /// prompt swallows before Vim sees them" reasoning) except always-on
    /// rather than opt-in. Reset (and `change_capture_dirty` with it)
    /// every time a fresh span starts; snapshotted into `last_change_
    /// keys` when a completed span turns out to have edited the buffer.
    change_capture: Vec<KeyPress>,
    /// Whether the in-progress `change_capture` span has edited the
    /// buffer yet -- set at the one place `vim.handle_key` is actually
    /// called (`route_keypress`), by comparing `Buffer::edit_count`
    /// before/after. OR-accumulated across every key in the span (a
    /// multi-key command like `dw` only edits on its last key).
    change_capture_dirty: bool,
    /// What `.` replays -- the most recently completed `change_capture`
    /// span that edited the buffer. `None` until the first one happens.
    last_change_keys: Option<Vec<KeyPress>>,
    /// Set for the duration of `repeat_last_change`'s own replay loop --
    /// suppresses `change_capture_dirty`'s bookkeeping (not the edits
    /// themselves) so a bare `.` press's own outer span doesn't get
    /// polluted by the edits its *replayed* keys make, which would
    /// otherwise overwrite `last_change_keys` with just `['.']` on every
    /// repeat instead of leaving it stable. Mirrors how `play_macro`
    /// replays through `route_keypress` directly rather than `dispatch_
    /// keypress`, just at the bookkeeping layer instead of the tap layer
    /// (`.`'s own replay still needs to go through the same call path
    /// the edits themselves happen on, unlike a macro's literal-keys
    /// replay, which is why this is a guard flag and not a different
    /// entry point).
    replaying_change: bool,
    /// `Some` only for a real, `main.rs`-launched `App` (see `App::new`'s
    /// own doc comment) -- the handle background threads (the Docker
    /// stats poller/log follower, the Git status poller) get their own
    /// `Clone` of, to wake the main event loop via `FenixUserEvent`.
    /// `None` in every test.
    event_proxy: Option<winit::event_loop::EventLoopProxy<FenixUserEvent>>,

    /// The autocompletion popup's live state -- `Some` only while Insert
    /// mode and the prefix at the cursor currently justify showing it
    /// (see `sync_completion`, called after every keystroke that reaches
    /// Vim while Insert-relevant, the same "derive fresh from whatever's
    /// current" posture `content_spans`/dashboard centering already use
    /// -- `VimEvent` has no "a character was typed" signal, so this
    /// recomputes instead of trying to track it incrementally). Sourced
    /// from `completion_candidates`: the Tcl-specific keyword/ctags/
    /// symbols-file pool for a Tcl buffer, plus buffer-word candidates
    /// (`completion::buffer_words`) layered on top for *every* buffer
    /// regardless of language -- the one source that isn't Tcl-specific,
    /// so even a buffer with no recognized language at all still gets a
    /// popup once it has something to complete from.
    completion: Option<CompletionState>,
    completion_selected: bool,
    /// The popup was asked for (`Ctrl-Space`), not offered while typing.
    completion_asked: bool,
    completion_generation: u64,
    /// Whether the open popup offers local candidates (buffer words,
    /// keywords, snippets) beside the servers' -- not in a comment or a
    /// string, where only servers are asked.
    completion_local: bool,
    completion_context: Option<(BufferId, fenix_window::WindowId, usize, u64)>,
    /// The focused buffer and its edit count when `sync_completion` last
    /// ran: a key that changed neither doesn't open the popup.
    completion_edit_seen: Option<(BufferId, u64)>,
    snippet: Option<snippets::ActiveSnippet>,
    /// Same role as `picker_scroll`, for the completion popup's own
    /// candidate window.
    completion_scroll: usize,

    vim: VimState,
    /// The OS clipboard, mirrored with Vim's unnamed register around `y`/
    /// `d`/`c`/`p`/`P` (see `sync_clipboard_before_paste`/`sync_clipboard_
    /// after_edit`) so yanking in Fenix and pasting elsewhere (or vice
    /// versa) just works. `None` when the platform clipboard couldn't be
    /// opened -- degrades to the old private-register-only behavior
    /// rather than a hard error, same posture as the optional `rg`/`git`/
    /// `ctags`/`docker` shell-outs.
    clipboard: Option<crate::clipboard::Clipboard>,
    /// What `clipboard` was last set to, so `handle_key` can tell whether
    /// Vim's unnamed register changed this keystroke (yank, delete,
    /// change -- anything that writes it) without re-reading the OS
    /// clipboard just to compare. Updated on every push *and* every pull
    /// so the two stay in lockstep and a pull never immediately reads
    /// back as a spurious push.
    clipboard_mirror: String,
    /// Persists across keystrokes so a `SPC f s` sequence can span several
    /// `handle_key` calls; `'static` because the leader trie is a global
    /// singleton (see `keymap::leader_trie`), which sidesteps
    /// `Matcher` borrowing from a trie `App` would otherwise also own.
    leader_matcher: Matcher<'static, &'static str>,
    /// The open `SPC m` menu, if any -- its trie depends on what was
    /// focused when it opened (see `local_leader`).
    local_matcher: Option<Matcher<'static, &'static str>>,
    /// Same reasoning as `leader_matcher`, for the explorer's own trie --
    /// persists across keystrokes for its one multi-key sequence (`g r`).
    explorer_matcher: Matcher<'static, ExplorerAction>,

    theme: &'static Theme,
    /// The unified settings file (`dirs::config_dir()/fenix/config.ini`)
    /// -- theme name, font size, indent width, and the completion
    /// symbols-file path all live here now, one `load` at startup and one
    /// `save` per change, instead of each setting having its own flat
    /// file and free-function trio. Lives here rather than in
    /// `fenix-vim`/`text.rs`: `VimState`/`TextPipeline` stay free of file
    /// I/O, matching every other pure editing-state type -- `App` applies
    /// each loaded value to the relevant subsystem and saves back into
    /// this one struct on every change (`apply_theme`, `adjust_font_
    /// size`, `VimEvent::IndentWidthChanged`).
    config: fenix_config::Config,
    /// When `settings.toml` last changed, as the disk poll last saw it:
    /// a change it didn't make is read again.
    settings_stamp: Option<std::time::SystemTime>,
    /// `extensions.toml`, yours and the machine's, as last seen: one
    /// changed from outside (`fenix --install-extension`) is put in use
    /// (`extensions_installed_if_changed`).
    extensions_stamp: Option<Vec<Option<std::time::SystemTime>>>,
    /// The focused project's own settings (`.fenix/settings.toml`),
    /// with the root they're for.
    project_settings: Option<(PathBuf, fenix_config::ProjectSettings)>,
    /// Your snippets' folder (a throwaway one in tests).
    snippets_dir: PathBuf,
    /// The buffer and pane the snippets page was opened from, to try a
    /// snippet in.
    snippets_origin: Option<(BufferId, fenix_window::WindowId)>,
    /// Where the tutorial keeps the lessons you've done (a throwaway one
    /// in tests).
    tutorial_progress: PathBuf,
    /// The project of the last file you were in, but the tutorial's
    /// practice repository: where a buffer of no file is.
    last_project: Option<PathBuf>,
    /// Extensions' compose panes' unsent text (`state/compose-drafts.json`).
    compose_drafts_file: PathBuf,
    /// The project the snippets page is showing, while it's in front.
    snippets_project: Option<PathBuf>,

    modifiers: ModifiersState,
    /// Whether the caret is fading toward visible or toward hidden --
    /// the *target* of the current transition, not necessarily what's on
    /// screen right now (see `caret_alpha`).
    blink_visible: bool,
    /// When the current fade started; `caret_alpha` eases from this.
    blink_transition_start: Instant,
    next_blink: Instant,
    /// When the caret was last woken (`wake_caret`: a key, a click):
    /// it blinks for `CARET_BLINKS_FOR` after, then stays lit, so an
    /// idle window stops drawing twice a second (#74).
    caret_woken: Instant,
    /// When an extension's moving layer may next be drawn: at most every
    /// `LAYER_TICK`, however fast the display could go (#74).
    next_layer_frame: Instant,
    pulse: Option<Pulse>,
    /// Everything else that's animating -- see `motion_host`.
    motion: motion_host::MotionState,
    /// The launch splash, fading out over the first frames.
    splash_fade: Option<crate::splash::SplashFade>,
    /// Getting the windows drawing again when they stop: see `gpu_host`.
    gpu_watch: crate::gpu::GpuWatch,
    /// Frames that have reached the screen, and how many times the event
    /// loop has woken: what the live tests watch, and `fenix/state`'s
    /// `stats` reports, so an idle window can be seen to draw nothing (#74).
    frames_presented: u64,
    loop_wakes: u64,
    /// When the editor's first frame is overdue, from the splash's hand-off:
    /// past it with nothing presented, the window is still showing the
    /// splash, so its surface is made again and why it's stuck is said
    /// (#65). Cleared by the first frame presented.
    first_frame_due: Option<Instant>,
    /// The most recent status/error message, if any and not yet expired
    /// -- see `StatusMessage`'s own doc comment. Set via `set_message`/
    /// `set_error`; read (and its expiry checked) by `modeline_pieces`.
    status_message: Option<StatusMessage>,
    /// Armed by `request_quit` when quitting would discard unsaved
    /// changes -- the next keypress is captured by `quit_confirm_key`
    /// (`y` quits, anything else cancels), same "captures the very next
    /// key" shape as `DockerHost::confirm_remove`/`git_confirm`.
    quit_confirm: bool,
}

/// The sidebar/terminal/pane rects a mouse click or wheel notch hit-
/// tests against -- see `App::frame_geometry`.
struct FrameGeometry {
    pane_area: fenix_window::Rect,
    sidebar_rect: Option<fenix_window::Rect>,
    terminal_rect: Option<fenix_window::Rect>,
    panes: Vec<(fenix_window::WindowId, fenix_window::Rect)>,
    /// Which of `panes` render a breadcrumb bar under their title, and
    /// so reserve two rows of chrome instead of one
    /// (`App::pane_has_breadcrumb`). Hit-testing needs it to know where
    /// a pane's chrome stops and its text begins -- that boundary is a
    /// row higher on an informational panel than on an editing pane.
    breadcrumb_panes: Vec<fenix_window::WindowId>,
    /// Every visible tab, across every pane, when the active theme has
    /// `show_tabs` -- empty for every other theme (see `App::frame_
    /// geometry`). Checked by `hit_test`/`cursor_icon_for` before the
    /// coarser `panes` rects, so a tab or its close glyph wins over
    /// "just focus whatever pane this is."
    tabs: Vec<(fenix_window::WindowId, TabRect)>,
    /// Each tab strip's split button, right after its tabs: a click splits
    /// that pane.
    split_buttons: Vec<(fenix_window::WindowId, fenix_window::Rect)>,
}

/// What a click or wheel notch landed on, per `hit_test`.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ScrollTarget {
    Sidebar,
    Terminal,
    Pane(fenix_window::WindowId),
    /// A tab's own body was clicked -- switch that pane to this buffer.
    SwitchTab(fenix_window::WindowId, BufferId),
    /// A tab's close glyph was clicked -- drop it from that pane's tab
    /// list (see `App::handle_click_at`).
    CloseTab(fenix_window::WindowId, BufferId),
    /// A tab strip's split button: split that pane.
    SplitPane(fenix_window::WindowId),
}

/// Point-in-region hit test shared by click-to-focus and wheel-scroll --
/// checked in on-screen draw order (terminal drawn last, so it wins the
/// bottom-left corner it overlaps with the sidebar when both are open;
/// see `App::frame_geometry`'s own doc comment) so a click/scroll always
/// agrees with what's actually visible at that pixel.
fn hit_test(geometry: &FrameGeometry, pos: (f32, f32)) -> Option<ScrollTarget> {
    let (x, y) = pos;
    if let Some(rect) = geometry.terminal_rect {
        if rect.contains_point(x, y) {
            return Some(ScrollTarget::Terminal);
        }
    }
    if let Some(rect) = geometry.sidebar_rect {
        if rect.contains_point(x, y) {
            return Some(ScrollTarget::Sidebar);
        }
    }
    for (pane, rect) in &geometry.split_buttons {
        if rect.contains_point(x, y) {
            return Some(ScrollTarget::SplitPane(*pane));
        }
    }
    for (pane, tab) in &geometry.tabs {
        if tab.close.contains_point(x, y) {
            return Some(ScrollTarget::CloseTab(*pane, tab.buffer));
        }
        if tab.body.contains_point(x, y) {
            return Some(ScrollTarget::SwitchTab(*pane, tab.buffer));
        }
    }
    geometry.panes.iter().find(|(_, rect)| rect.contains_point(x, y)).map(|(id, _)| ScrollTarget::Pane(*id))
}

/// Which OS cursor icon should show at `pos`, given the same `geometry`
/// `hit_test` already checks clicks against -- pure and GPU-free like
/// `hit_test` itself, so hovering and clicking can never disagree about
/// what's under the pointer. A tab/close glyph or the sidebar reads as
/// clickable chrome (`Pointer`); a pane's own *content* sub-rect (below
/// its title/tab strip -- `line_height` is that strip's fixed height, the
/// same value `pane_content_rect` shrinks by) is editable text (`Text`,
/// an I-beam); everything else (a pane's own strip dead space, the
/// terminal, dividers, the modeline) is the plain arrow.
fn cursor_icon_for(geometry: &FrameGeometry, pos: (f32, f32), line_height: f32) -> winit::window::CursorIcon {
    let (x, y) = pos;
    if geometry.tabs.iter().any(|(_, tab)| tab.body.contains_point(x, y)) || geometry.split_buttons.iter().any(|(_, r)| r.contains_point(x, y)) {
        return winit::window::CursorIcon::Pointer;
    }
    if geometry.sidebar_rect.is_some_and(|rect| rect.contains_point(x, y)) {
        return winit::window::CursorIcon::Pointer;
    }
    if let Some((id, rect)) = geometry.panes.iter().find(|(_, rect)| rect.contains_point(x, y)) {
        if y >= rect.y + pane_chrome_height(line_height, geometry.breadcrumb_panes.contains(id)) {
            return winit::window::CursorIcon::Text;
        }
    }
    winit::window::CursorIcon::Default
}

/// One tab's own on-screen layout -- `body` is the whole clickable tab
/// (background + text), `close` its narrower close-glyph sub-rect at the
/// right edge, both sharing `body`'s vertical span.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TabRect {
    buffer: BufferId,
    body: fenix_window::Rect,
    close: fenix_window::Rect,
}

/// Minimum/maximum width, in characters, a tab's body gets for its
/// filename before clamping -- same "stay in a readable range regardless
/// of content length" posture as `WHICH_KEY_MIN_WIDTH`/`_MAX_WIDTH`.
const TAB_MIN_NAME_CHARS: usize = 6;
const TAB_MAX_NAME_CHARS: usize = 20;
/// Extra chars reserved for `" {icon} "` + a worst-case one-char dirty
/// marker + `" × "`, on top of the (clamped) filename itself -- matches
/// `App::redraw`'s own tab-span formatting exactly, so the background
/// rect this produces never clips the text it lays under.
const TAB_CHROME_CHARS: usize = 8;
/// The close glyph's own `" × "` width, in chars, carved out of a tab's
/// right edge.
const TAB_CLOSE_CHARS: usize = 3;

/// Lays out `tabs` left-to-right within `rect` (a pane's own tab-strip
/// rect, `tab_strip_height` tall) -- pure and GPU-free like `pane_content_
/// rect`, so the same geometry backs both hit-testing (`frame_geometry`)
/// and drawing (`redraw`) without the two ever disagreeing about where a
/// tab is. `char_width` must be `TextPipeline::title_char_width()`, not
/// `char_width()` -- tab-strip text renders at the smaller title-scaled
/// font, and laying tabs out with the body font's wider cell measurement
/// drifts every tab's background/hit-test rect right of its own actually-
/// narrower rendered text, worse with each subsequent tab. `tabs` is
/// `(buffer, filename char count)` pairs, left to right; tabs that would
/// run past the strip's right edge are omitted. The strip starts where
/// it keeps the active tab in view (`first_tab_shown`).
#[cfg(test)]
fn pane_tab_layout(rect: fenix_window::Rect, char_width: f32, tabs: &[(BufferId, usize)]) -> Vec<TabRect> {
    pane_tab_layout_iter(rect, char_width, tabs.iter().copied())
}

/// Iterator form of `pane_tab_layout`. The iterator is deliberately
/// consumed only until the strip is full: a pane remembers every buffer it
/// has shown, while a tab strip can display only a handful. In particular,
/// callers must not resolve the display name of every historical buffer on
/// each redraw just to discard almost all of them here.
fn pane_tab_layout_iter<I>(rect: fenix_window::Rect, char_width: f32, tabs: I) -> Vec<TabRect>
where
    I: IntoIterator<Item = (BufferId, usize)>,
{
    let mut out = Vec::new();
    let mut x = rect.x;
    for (buffer, name_chars) in tabs {
        let w = tab_width(name_chars, char_width);
        if x + w > rect.x + rect.w {
            break;
        }
        let close_w = TAB_CLOSE_CHARS as f32 * char_width;
        let body = fenix_window::Rect { x, y: rect.y, w, h: rect.h };
        let close = fenix_window::Rect { x: x + w - close_w, y: rect.y, w: close_w, h: rect.h };
        out.push(TabRect { buffer, body, close });
        x += w;
    }
    out
}

/// How wide a tab whose name is `name_chars` long is drawn.
fn tab_width(name_chars: usize, char_width: f32) -> f32 {
    (name_chars.clamp(TAB_MIN_NAME_CHARS, TAB_MAX_NAME_CHARS) + TAB_CHROME_CHARS) as f32 * char_width
}

/// The first tab a strip `room` wide shows so that tab `active` is in
/// view: 0 while it fits from the start, else as few tabs before it as
/// leave it whole at the right edge. `width` is a tab's width, asked only
/// of the tabs from `active` back to the first shown.
fn first_tab_shown(active: usize, room: f32, width: impl Fn(usize) -> f32) -> usize {
    let mut used = 0.0;
    let mut start = active;
    for i in (0..=active).rev() {
        used += width(i);
        if used > room {
            break;
        }
        start = i;
    }
    start
}

/// Clamps `name` to at most `max_chars`, replacing the tail with `…` when
/// it doesn't fit -- keeps a long filename from running past its tab's own
/// clamped width (`pane_tab_layout`'s own `TAB_MAX_NAME_CHARS`).
fn truncate_tab_name(name: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max_chars {
        return name.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    let mut out: String = chars[..keep].iter().collect();
    out.push('\u{2026}');
    out
}

/// Lines per physical wheel notch -- matches the common "3 lines"
/// default most editors/browsers use.
const WHEEL_LINES_PER_NOTCH: isize = 3;

/// Converts one wheel event into a signed line count -- positive
/// scrolls toward earlier content (up/back), negative toward later.
/// `LineDelta` (a physical wheel notch) always moves a fixed number of
/// lines; `PixelDelta` (a trackpad) is converted through the current
/// line height instead.
fn wheel_delta_lines(delta: MouseScrollDelta, line_height: f32) -> isize {
    match delta {
        MouseScrollDelta::LineDelta(_, y) => y.signum() as isize * WHEEL_LINES_PER_NOTCH,
        MouseScrollDelta::PixelDelta(pos) => (pos.y / line_height as f64).round() as isize,
    }
}

/// What pressed key at the armed `quit_confirm` prompt resolves to --
/// same "wait for exactly one more raw key" shape as `GitConfirmAction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuitConfirmAction {
    Quit,
    ShowDirtyBuffers,
    Cancel,
}

impl App {
    /// `proxy` is what lets background threads (the Docker panel's
    /// stats poller/log follower, Phases 5-6) wake the main event loop
    /// and hand it fresh data via `user_event` -- see `FenixUserEvent`'s
    /// own doc comment. Only the real entry point (`main.rs`) calls
    /// `new`; every test constructs `App` via `with_file` directly and
    /// never sets `event_proxy` (stays `None`), which is exactly why
    /// spawning code checks `event_proxy.is_some()` before spawning
    /// anything -- no fake/mock event loop needed for tests, a session
    /// opened in a test just never has live-updating panes, which is
    /// already true today (nothing there needs a working `docker`
    /// either, per every existing "never fails" test in this file).
    /// `file_arg` is the first *file* this launch was given, already
    /// separated from any flags by `main` -- read there rather than
    /// from `env::args()` here, because `fenix --new-window notes.md`
    /// would otherwise try to open a file called `--new-window`.
    pub fn new(event_proxy: winit::event_loop::EventLoopProxy<FenixUserEvent>, file_arg: Option<String>) -> Self {
        let mut app = Self::with_file(file_arg.clone());
        app.event_proxy = Some(event_proxy);
        app.session.path = fenix_storage::paths::state_file("session.json");
        // From here a project's own programs wait to be allowed: before
        // the session comes back, which opens files and starts servers.
        app.project_trust_begin(fenix_storage::paths::state_file("trusted-projects.json"));
        app.restore_session(file_arg.is_some());
        // What was open on other machines: connected to again.
        app.remote_resume_session();
        // Recording lives here, not inside `with_file` -- `with_file` is
        // what the test suite calls directly to simulate "opened with
        // this file," and recording there would mean every such test
        // writes into the real `~/.config/fenix/recent_files.txt` on
        // whatever machine happens to run the tests. Real launches
        // always go through `new`, so this still fires for actual use.
        if let Some(path) = &file_arg {
            let path = Path::new(path);
            if let Some(open) = app.opener_for(path) {
                // `with_file` can't hand a file to an extension (that
                // needs `self`), so it opened the dashboard; the file
                // goes to its extension now. A dashboard that isn't the
                // workspace's Home would only linger in the buffer list,
                // so it goes.
                let placeholder = app.focused_buffer_id();
                open(&mut app, path, false);
                if app.session.pending.is_none() && !app.is_a_workspace_home(placeholder) {
                    app.buffers.close(placeholder);
                    app.drop_buffer_from_every_strip(placeholder);
                }
            } else {
                app.open_startup_file(path);
            }
        }
        app.announce_recoverable_work();
        if let Some(problem) = app.config.problems.first() {
            let more = match app.config.problems.len() {
                1 => String::new(),
                n => format!(" (and {} more)", n - 1),
            };
            app.set_error(format!("{problem}{more} -- SPC , shows them"));
        }
        // A first run is welcomed.
        app.extensions_at_startup();
        // The extensions that run code, and their projects' markers.
        app.code_extensions_installed();
        // The extensions' project kinds -- before anything is drawn, but
        // after the session came back and a file given was opened, either
        // of which may have named a project by its kind: named again.
        app.project_kinds_now(&crate::installed::kind_extensions());
        // A MIB folder inside another project, on your list from when one
        // was a project of its own: taken off it, once (the packs'
        // markers are known now, so what's a project is too).
        app.drop_mib_folders_from_projects(dirs::home_dir().as_deref());
        // The file opened at start was given its project before the
        // packs' markers were known: again, now they are. Then those that
        // start with Fenix start, in it (or a moment later, after it, each
        // would start again there) -- once the window has drawn its first
        // frame, so forty of them loading don't hold the window up (#33).
        app.refresh_project_root();
        app.wasm.startup_due = Some(Instant::now() + STARTUP_EXTENSIONS_WITHIN);
        // What the keychain said as settings were read (a token too long
        // for it, kept in settings.toml).
        let said = startup::said_later();
        if !said.is_empty() {
            app.set_error(format!("settings.toml: {}", said.join("; ")));
        }
        // Your keybindings, read with the leader map: what's wrong is said.
        let problems = keymap::user_key_problems();
        if !problems.is_empty() {
            app.set_error(format!("keybindings.toml: {}", problems.join("; ")));
        }
        // Your themes and language files, likewise.
        if !theme::problems().is_empty() {
            app.set_error(theme::problems().join("; "));
        }
        if !crate::languages::problems().is_empty() {
            app.set_error(crate::languages::problems().join("; "));
        }
        app
    }

    fn with_file(file_arg: Option<String>) -> Self {
        // Loaded before the initial buffer so a no-argument launch can
        // build the dashboard from them.
        // Tests get their own lists, so running them never edits the
        // project list or recent files of the Fenix you use.
        let known_projects_path = if cfg!(test) {
            isolated_test_path("projects.txt")
        } else {
            fenix_project::KnownProjects::default_path().unwrap_or_else(|| PathBuf::from("fenix-projects.txt"))
        };
        let known_projects = fenix_project::KnownProjects::load_or_default(known_projects_path);
        let recent_files_path = if cfg!(test) {
            isolated_test_path("recent_files.txt")
        } else {
            fenix_project::RecentFiles::default_path().unwrap_or_else(|| PathBuf::from("fenix-recent-files.txt"))
        };
        let recent_files = fenix_project::RecentFiles::load_or_default(recent_files_path);

        let mut buffers = BufferList::new();
        let initial_id = match &file_arg {
            // Recording this path into `recent_files` happens in `new`,
            // not here -- see `new`'s own doc comment for why. A file an
            // extension opens (a PDF) falls through to the same
            // dashboard-placeholder branch a missing arg does: handing it
            // over needs `self`, which doesn't exist yet at this point --
            // `new` recognizes this same placeholder right after
            // construction and gives the file to its extension.
            Some(path) if !startup::claims_file(Path::new(path)) => buffers.open_path(Path::new(path)),
            _ => {
                // Laid out on its first frame, at its pane's real size.
                buffers.open_dashboard("")
            }
        };
        // The file named on the command line is the most common buffer
        // there is, and it's opened here, before `self` exists -- so
        // its fingerprint is taken here too. Without it the first thing
        // to change that file is indistinguishable from the buffer's
        // own unsaved edits, and the warning that should fire doesn't.
        let mut disk_state: HashMap<BufferId, DiskFingerprint> = HashMap::new();
        if let Some(fingerprint) = buffers.get(initial_id).and_then(|ob| ob.buffer.path()).and_then(DiskFingerprint::of) {
            disk_state.insert(initial_id, fingerprint);
        }
        let initial_cursor = buffers.get(initial_id).map(|ob| ob.cursor).unwrap_or(Cursor::at_start());
        let workspaces = WorkspaceList::new(WindowTree::new(initial_id), initial_cursor);

        // `project_root` (used to scope `SPC p f`/`SPC p s`) is still
        // auto-detected from whatever file is open -- only the *known-
        // projects* list (the `SPC p p`/`SPC p d` switch/delete registry)
        // is no longer auto-populated from it. That list is explicitly
        // curated via `SPC p a` now (see `picker_add_project_prompt`'s own
        // doc comment for why): every file ever opened auto-registering
        // its project made the switch-project list an unfiltered history
        // of everywhere you'd been, not a deliberately kept list of
        // projects worth switching between. A dashboard buffer has no
        // path, so this correctly comes out `None`.
        let project_root =
            buffers.get(initial_id).and_then(|ob| ob.buffer.path()).and_then(fenix_project::find_project_root);
        // Tests get their own settings and a token store that forgets,
        // so running them never touches the Fenix you use.
        let (config_path, state_dir) = match fenix_storage::paths::Roots::current() {
            _ if cfg!(test) => (isolated_test_path("settings.toml"), isolated_test_path("state")),
            Some(roots) => (roots.settings_file(), roots.local.join("state")),
            None => (PathBuf::from("settings.toml"), PathBuf::from("state")),
        };
        let config_existed = config_path.exists();
        let mut config = fenix_config::Config::load_at_or_default(config_path, state_dir);
        // `agents.mcp.http`: Fenix's MCP server over HTTP, kept running
        // beside the editor for clients that only speak HTTP.
        if let Some(address) = config.text_value("agents.mcp.http").map(str::trim).filter(|a| !a.is_empty() && !cfg!(test)) {
            let address = address.to_string();
            std::thread::spawn(move || {
                if let Err(why) = crate::mcp_http::serve(&address) {
                    eprintln!("fenix: agents.mcp.http: {why}");
                }
            });
        }
        // Secrets kept as plain text from before move to the keychain
        //, the first time Fenix reads them; the keychain is looked
        // for only when there's one to move.
        let plain = fenix_config::settings().iter().any(|s| matches!(s.kind, fenix_config::Kind::Secret(_)) && matches!(s.get(&config), Some(fenix_config::Value::Text(t)) if !t.trim().is_empty()));
        if plain {
            let (moved, problems) = fenix_config::keychain::migrate(&mut config, wasm_host::system_keychain());
            if !moved.is_empty() {
                eprintln!("fenix: moved {} to the keychain", moved.join(", "));
            }
            for problem in problems {
                eprintln!("fenix: {problem}");
                // Said in the window too, once it's up: a token left in
                // the file is worth knowing about.
                startup::say_later(problem);
            }
        }
        // Whether this is a fresh install's first run is settled now, while
        // `settings.toml` still isn't there: writing it below made every
        // new install look like an upgrade (all the moved extensions
        // installed, and no welcome).
        let _ = startup::first_run();
        // First launch on this machine: write the file straight away, so
        // `settings.toml` is there to find (and hand-edit) from the start.
        if !config_existed {
            if let Err(err) = config.save() {
                eprintln!("fenix: couldn't create settings.toml ({err})");
            }
        }
        let theme = theme::named_or_default(config.theme.as_deref());
        let mut vim = VimState::new();
        vim.set_indent_width(config.indent_width.unwrap_or(fenix_vim::DEFAULT_INDENT_WIDTH));
        vim.set_smart_case(config.search_smart_case != Some(false));
        vim.set_iskeyword_extra(
            config.iskeyword_extra.as_deref().map(|s| s.chars().collect()).unwrap_or_else(|| fenix_vim::DEFAULT_ISKEYWORD_EXTRA.to_vec()),
        );
        Self {
            // One frame, and it's the active one -- so its state is the
            // live set of fields right here rather than a parked
            // `FrameState`. See the field's own doc comment.
            frames: vec![None],
            active_frame: 0,
            focused_frame: 0,
            frame_origin: (0, 0),
            pending_frames: Vec::new(),
            window: None,
            gpu_context: None,
            fonts: None,
            gpu: None,
            text: None,
            bg_rect: None,
            popup_rect: None,
            caret_rect: None,
            motion_gpu: None,
            surfaces: None,
            buffers,
            workspaces,
            line_number_mode: LineNumberMode::from_setting(&config),
            main_view: MainView::Editor,
            explorer: None,
            sidebar: None,
            sidebar_view: explorer_page::View::new(explorer_page::Form::Sidebar),
            sidebar_width: config.explorer_width.map_or(text::SIDEBAR_WIDTH, |w| w as f32),
            explorer_sessions: fenix_storage::state::read(&config.state_dir().join(explorer_host::SESSIONS_FILE), "projects").ok().flatten().unwrap_or_default(),
            explorer_sessions_saved: true,
            sidebar_followed: None,
            explorer_reveal_pending: HashMap::new(),
            sidebar_drag: false,
            mouse: Default::default(),
            explorer_last_click: None,
            explorer_search: 0,
            explorer_preview: None,
            sidebar_open: false,
            sidebar_focused: false,
            terminal: None,
            terminal_shell: 0,
            terminal_root: None,
            parked_terminals: Vec::new(),
            terminal_open: false,
            terminal_focused: false,
            terminal_spawning: false,
            terminal_buffers: HashMap::new(),
            terminal_buffer_focused: None,
            terminal_buffers_spawning: HashSet::new(),
            terminal_buffer_labels: HashMap::new(),
            terminal_buffer_cwds: HashMap::new(),
            term_select: None,
            term_hover: None,
            term_clicks: crate::mouse::ClickCounter::default(),
            jump_view: None,
            reveal_panes: HashSet::new(),
            terminal_buffer_programs: HashMap::new(),
            terminal_buffers_opened: 0,
            cursor_pos: None,
            current_cursor_icon: None,
            tab_geometry: Vec::new(),
            explorer_purpose: ExplorerPurpose::Browse,
            explorer_prompt: None,
            explorer_conflict: None,
            rename_confirm: None,
            explorer_job: None,
            explorer_completions: Vec::new(),
            rename_mode: HashMap::new(),
            volumes: Vec::new(),
            volumes_loading: false,
            recent_dirs: fenix_project::RecentFiles::dirs_or_default(default_recent_dirs_path()),
            explorer_delete_permanently: false,
            picker_scroll: 0,
            project_root,
            project_kinds: RefCell::new(HashMap::new()),
            kinds_generation: 0,
            pages: HashMap::new(),
            project_meta: fenix_project::meta::ProjectMeta::load_or_default(default_project_meta_path()),
            project_health: HashMap::new(),
            active_picker: None,
            palette_preview: RefCell::new(None),
            leader_places: RefCell::new(None),
            jump_back_stack: Vec::new(),
            jump_forward_stack: Vec::new(),
            marks: HashMap::new(),
            quickfix: Vec::new(),
            search: Default::default(),
            search_count_cache: Default::default(),
            quickfix_index: None,
            todo_spans: HashMap::new(),
            known_projects,
            recent_files,
            home_views: HashMap::new(),
            home_data: dashboard::HomeData::default(),
            page_g_pending: false,
            last_tab_click: None,
            tab_drag: None,
            home_project_data: HashMap::new(),
            file_index: Default::default(),
            disk_probe_out: None,
            remote: remote::Remotes::default(),
            trust: trust::ProjectTrust::default(),
            disk_probe_basis: HashMap::new(),
            recent_seen: HashMap::new(),
            recent_probed: None,
            recent_probe_out: None,
            chrome_git_out: None,
            home_todos: HashMap::new(),
            home_todos_pending: HashSet::new(),
            home_logo: None,
            dired_states: HashMap::new(),
            explorer_requests: HashMap::new(),
            explorer_stamps: HashMap::new(),
            explorer_watch: None,
            explorer_dirty: HashSet::new(),
            explorer_dirty_due: None,
            page_dragging: None,
            next_explorer_request: 0,
            pane_titles: HashMap::new(),
            gutter_hunks: HashMap::new(),
            gutter_refreshed: HashMap::new(),
            gutter_diffs: HashMap::new(),
            blames: HashMap::new(),
            chrome_git: None,
            fetch_attempts: HashMap::new(),
            code_folds: HashMap::new(),
            wrap_overrides: HashMap::new(),
            break_cache: std::cell::RefCell::new(HashMap::new()),
            virtual_rows: HashMap::new(),
            semantic_of: HashMap::new(),
            lens_of: HashMap::new(),
            lens_rows: HashMap::new(),
            lens_hover: None,
            own_lenses: HashMap::new(),
            force_row_path: false,
            semantic_protected: HashMap::new(),
            screen_decorations: None,
            pane_text_cols: HashMap::new(),
            fold_levels: HashMap::new(),
            notifications: crate::notify::Center::new(),
            toast_rects: Vec::new(),
            notification_rates: HashMap::new(),
            commands: Rc::new(CommandRegistry::with_builtins().with_code_extensions(&crate::installed::code_extensions())),
            wasm: wasm_host::WasmHost::default(),
            tests: tests_host::TestsHost::default(),
            recent_commands: Vec::new(),
            queued_command: None,
            palette_describes: false,
            describing_keys: None,
            binding_keys: None,
            task_session: None,
            project_last_tasks: HashMap::new(),
            debug_session: None,
            debug_after_task: None,
            #[cfg(test)]
            dap_unread: Vec::new(),
            breakpoints: HashMap::new(),
            debug_pins: Vec::new(),
            debug_exceptions: HashMap::new(),
            debug_last_launch: HashMap::new(),
            debug_pick_launch: false,
            debug_mode_left: false,
            debug_run_to: None,
            debug_dock_open: false,
            debug_drawer: Default::default(),
            debug_trail: Default::default(),
            tool_status_session: None,
            git_lines: HashMap::new(),
            history_session: None,
            ref_view_names: HashMap::new(),
            indent_steps: std::cell::RefCell::new(HashMap::new()),
            side_by_sides: Vec::new(),
            merge_session: None,
            compose: None,
            disk_state,
            externally_changed: HashSet::new(),
            // `None` under test: the recovery directory is a real,
            // shared location in the user's config, and a suite with a
            // thousand buffers in it would otherwise scatter snapshots
            // through it -- which it did, until this was noticed.
            // Tests that exercise recovery point this at a temp
            // directory of their own.
            session: session::State::default(),
            recovery_dir: if cfg!(test) { None } else { fenix_recovery::default_dir() },
            snapshot_state: HashMap::new(),
            unnamed_snapshots: HashMap::new(),
            recovery_failures: HashMap::new(),
            next_disk_check: Instant::now() + DISK_CHECK_INTERVAL,
            diff_lines: HashMap::new(),
            diff_models: HashMap::new(),
            graph_lines: HashMap::new(),
            merge_lines: HashMap::new(),
            git_confirm: None,
            git_prompt: None,
            git_menu_open: false,
            git_session: None,
            page_images: Default::default(),
            lsp_sessions: HashMap::new(),
            lsp_workspace_names: HashMap::new(),
            symbol_search: Default::default(),
            caret_marks: Vec::new(),
            caret_undoing: false,
            auto_save: Default::default(),
            updates: updates::UpdateState { file: Some(if cfg!(test) { isolated_test_path("updates.json") } else { fenix_storage::paths::state_file("updates.json").unwrap_or_else(|| PathBuf::from("updates.json")) }), ..Default::default() },
            tcl_name_servers: HashMap::new(),
            lsp_unavailable: std::collections::HashSet::new(),
            lsp_starting: HashMap::new(),
            #[cfg(test)]
            lsp_start_sink: None,
            restart_requested: false,
            lsp_frame_synced: None,
            lsp_failures: HashMap::new(),
            server_install: Default::default(),
            editorconfig_cache: HashMap::new(),
            inlay: HashMap::new(),
            lsp_additional: HashMap::new(),
            lsp_fanouts: HashMap::new(),
            next_lsp_fanout: 1,
            lsp_hover_fanout: None,
            lsp_completion: None,
            diagnostics: Default::default(),
            lsp_hover: None,
            table_views: HashMap::new(),
            macro_capture: Vec::new(),
            macro_recording_append: false,
            last_played_macro: None,
            replay_depth: 0,
            replay_budget: 0,
            change_capture: Vec::new(),
            change_capture_dirty: false,
            last_change_keys: None,
            replaying_change: false,
            event_proxy: None,
            completion: None,
            completion_selected: false,
            completion_asked: false,
            completion_generation: 0,
            completion_local: true,
            completion_context: None,
            completion_edit_seen: None,
            snippet: None,
            completion_scroll: 0,
            find_file_prompt: None,
            rename_file_prompt: None,
            lsp_rename_prompt: None,
            prompt_prefill_selected: false,
            refactor_preview: None,
            agents: agents::AgentHost::new(),
            agent_calls: agent_calls::AgentCalls::default(),
            agent_lsp: agent_lsp::AgentLsp::default(),
            agent_subscriptions: agent_context::Subscriptions::default(),
            pipelines: agent_pipelines::Pipelines::default(),
            refactor_undo: None,
            workspace_rename_prompt: None,
            goto_line_prompt: None,
            compose_discard_armed: false,
            last_key_at: None,
            leader_uses: 0,
            carets: Default::default(),
            caret_steps: Default::default(),
            switching_recent: false,
            delete_file_confirm: false,
            vim,
            clipboard: crate::clipboard::Clipboard::new().ok(),
            clipboard_mirror: String::new(),
            leader_matcher: keymap::leader_trie().matcher(),
            local_matcher: None,
            explorer_matcher: fenix_explorer::explorer_trie().matcher(),
            theme,
            config,
            settings_stamp: None,
            extensions_stamp: None,
            project_settings: None,
            snippets_dir: if cfg!(test) { isolated_test_path("snippets") } else { fenix_storage::paths::snippets_dir().unwrap_or_else(|| PathBuf::from("snippets")) },
            snippets_origin: None,
            tutorial_progress: if cfg!(test) { isolated_test_path("tutorial.json") } else { fenix_storage::paths::state_file("tutorial.json").unwrap_or_else(|| PathBuf::from("tutorial.json")) },
            last_project: None,
            compose_drafts_file: if cfg!(test) { isolated_test_path("compose-drafts.json") } else { fenix_storage::paths::state_file("compose-drafts.json").unwrap_or_else(|| PathBuf::from("compose-drafts.json")) },
            snippets_project: None,
            modifiers: ModifiersState::empty(),
            blink_visible: true,
            blink_transition_start: Instant::now() - BLINK_FADE,
            next_blink: Instant::now() + BLINK_INTERVAL,
            caret_woken: Instant::now(),
            next_layer_frame: Instant::now(),
            pulse: None,
            motion: motion_host::MotionState::default(),
            splash_fade: None,
            gpu_watch: Default::default(),
            frames_presented: 0,
            loop_wakes: 0,
            first_frame_due: None,
            status_message: None,
            a11y: Default::default(),
            quit_confirm: false,
        }
    }

    /// The active workspace's window tree. Every call site outside this
    /// pair of accessors goes through here (or `windows_mut`) rather than
    /// touching `self.workspaces` directly, mirroring the `open`/
    /// `open_mut` discipline already used for buffers.
    /// `frame`'s winit window, whether that frame is the active one
    /// (its state is live on `App`) or parked. `None` before the frame
    /// has been through `resumed`, and for an out-of-range index.
    fn frame_window(&self, frame: usize) -> Option<&Arc<Window>> {
        match self.frames.get(frame)? {
            Some(parked) => parked.window.as_ref(),
            None => self.window.as_ref(),
        }
    }

    /// `frame`'s split layout and workspaces, parked or live -- the
    /// read-only counterpart to `activate_frame` for the handful of
    /// places (`about_to_wait`'s animation check, cross-frame
    /// navigation) that need to look at a frame without making it the
    /// active one.
    fn frame_workspaces(&self, frame: usize) -> Option<&WorkspaceList> {
        match self.frames.get(frame)? {
            Some(parked) => Some(&parked.workspaces),
            None => Some(&self.workspaces),
        }
    }

    /// Which frame owns the winit window `id` belongs to.
    fn frame_index_of(&self, id: WindowId) -> Option<usize> {
        (0..self.frames.len()).find(|&frame| self.frame_window(frame).map(|w| w.id()) == Some(id))
    }

    /// Makes `frame` the active one: its parked state moves into the
    /// live fields on `App`, and the outgoing frame's live state is
    /// parked in its place. A no-op when `frame` is already active or
    /// out of range.
    ///
    /// Every field is swapped through `mem::replace` against the
    /// incoming frame's own value, so nothing ever needs a placeholder
    /// to stand in for a moved-out `WorkspaceList` -- and the "exactly
    /// one `None` in `frames`, at `active_frame`" invariant holds at
    /// every point this function returns.
    fn activate_frame(&mut self, frame: usize) {
        if frame == self.active_frame {
            return;
        }
        let Some(Some(incoming)) = self.frames.get_mut(frame).map(Option::take) else { return };
        let outgoing = FrameState {
            window: std::mem::replace(&mut self.window, incoming.window),
            gpu: std::mem::replace(&mut self.gpu, incoming.gpu),
            text: std::mem::replace(&mut self.text, incoming.text),
            bg_rect: std::mem::replace(&mut self.bg_rect, incoming.bg_rect),
            popup_rect: std::mem::replace(&mut self.popup_rect, incoming.popup_rect),
            caret_rect: std::mem::replace(&mut self.caret_rect, incoming.caret_rect),
            motion_gpu: std::mem::replace(&mut self.motion_gpu, incoming.motion_gpu),
            workspaces: std::mem::replace(&mut self.workspaces, incoming.workspaces),
            cursor_pos: std::mem::replace(&mut self.cursor_pos, incoming.cursor_pos),
            origin: std::mem::replace(&mut self.frame_origin, incoming.origin),
            sidebar: std::mem::replace(&mut self.sidebar, incoming.sidebar),
            sidebar_open: std::mem::replace(&mut self.sidebar_open, incoming.sidebar_open),
            sidebar_focused: std::mem::replace(&mut self.sidebar_focused, incoming.sidebar_focused),
            terminal_open: std::mem::replace(&mut self.terminal_open, incoming.terminal_open),
            terminal_focused: std::mem::replace(&mut self.terminal_focused, incoming.terminal_focused),
        };
        self.frames[self.active_frame] = Some(outgoing);
        self.active_frame = frame;
    }

    /// Moves editor focus to `frame` for good, as opposed to
    /// `activate_frame`'s temporary switch: this is what a keystroke, a
    /// click, or a frame command does, and the only thing that moves
    /// `focused_frame`. See that field's doc comment for what turns on
    /// the distinction.
    fn focus_frame(&mut self, frame: usize) {
        self.activate_frame(frame);
        self.focused_frame = self.active_frame;
    }

    /// Re-reads the active frame's position from the OS. Called
    /// whenever it could have changed -- the window moved, was resized,
    /// or was just created -- because directional navigation across
    /// frames is decided from these coordinates.
    ///
    /// Reads `inner_position` rather than the outer one a `Moved` event
    /// carries: pane rectangles are relative to the client area, so the
    /// client area's corner is what they have to be offset by.
    fn refresh_frame_origin(&mut self) {
        if let Some(at) = self.window.as_ref().and_then(|window| window.inner_position().ok()) {
            self.frame_origin = (at.x, at.y);
        }
    }

    /// The configured body text size *before* any DPI scaling -- what
    /// `config.ini` holds and what `SPC t =`/`SPC t -` step. Each
    /// frame's pipeline gets this multiplied by its own monitor's
    /// scale factor.
    fn logical_font_size(&self) -> f32 {
        self.config.font_size.unwrap_or(text::FONT_SIZE)
    }

    /// The active frame's DPI scale factor -- 1.0 before its window
    /// exists, which is also the right answer for a headless test.
    fn frame_scale(&self) -> f32 {
        self.window.as_ref().map(|window| window.scale_factor() as f32).unwrap_or(1.0)
    }

    /// Re-applies the configured font size to every frame, each at its
    /// own monitor's scale factor. Every frame, not just the active
    /// one, because `SPC t =` is an editor-wide preference: two windows
    /// on differently-scaled monitors should end up the same apparent
    /// size as each other, not the same pixel size.
    fn apply_font_size(&mut self) {
        let logical = self.logical_font_size();
        let previous = self.active_frame;
        for frame in 0..self.frames.len() {
            self.activate_frame(frame);
            let scale = self.frame_scale();
            if let Some(text) = &mut self.text {
                text.set_font_size(logical, scale);
            }
        }
        self.activate_frame(previous);
    }

    /// Raises the active frame's OS window and asks it to repaint --
    /// what every command that moves editor focus between frames has to
    /// end with, or the keystrokes would keep going to whichever window
    /// the OS still considers foreground.
    fn focus_active_frame_window(&self) {
        if let Some(window) = &self.window {
            // A live test's windows are hidden, and leave the focus alone.
            if !cfg!(test) {
                window.focus_window();
            }
            window.request_redraw();
        }
    }

    /// `SPC w n`: opens another OS window, showing the focused buffer in
    /// a single pane, and moves focus to it. The two windows share
    /// everything but layout -- one buffer list, one undo history, the
    /// same extensions -- so the same file can be open in both and edits
    /// show up in each.
    ///
    /// A silent no-op before `resumed` has built the shared
    /// `GpuContext`: there'd be no device to hang a second swapchain
    /// off. That can't happen from a real keystroke (the window has to
    /// exist for one to arrive) and headless tests use `test_add_frame`.
    pub(crate) fn new_frame(&mut self, event_loop: &ActiveEventLoop) {
        let placement = self.next_frame_placement(event_loop);
        self.open_frame(event_loop, placement);
    }

    /// The half of `new_frame` that takes where to put the window as an
    /// argument, so restoring a saved layout can reuse it (see
    /// `restore_saved_windows`) instead of asking for a free monitor.
    fn open_frame(&mut self, event_loop: &ActiveEventLoop, placement: FramePlacement) {
        let (Some(context), Some(fonts)) = (&self.gpu_context, &self.fonts) else { return };

        let mut attrs = Window::default_attributes().with_title(window_title(None)).with_window_icon(fenix_icon()).with_visible(!cfg!(test));
        if let Some(position) = placement.position {
            attrs = attrs.with_position(position);
        }
        if let Some(size) = placement.size {
            attrs = attrs.with_inner_size(size);
        }
        let window = match event_loop.create_window(attrs) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                self.set_error(format!("couldn't open a new window ({err})"));
                return;
            }
        };

        let gpu = context.attach(Arc::clone(&window));
        let mut text = TextPipeline::new(&gpu, Rc::clone(fonts));
        text.set_font_size(self.logical_font_size(), window.scale_factor() as f32);
        let bg_rect = RectRenderer::new(&gpu);
        let popup_rect = RectRenderer::new(&gpu);
        let caret_rect = RectRenderer::new(&gpu);
        let motion_gpu = motion_host::MotionGpu::new(&gpu);

        let buffer = self.focused_buffer_id();
        let cursor = self.buffers.get(buffer).map(|ob| ob.cursor).unwrap_or(Cursor::at_start());
        self.frames.push(Some(FrameState {
            window: Some(Arc::clone(&window)),
            gpu: Some(gpu),
            text: Some(text),
            bg_rect: Some(bg_rect),
            popup_rect: Some(popup_rect),
            caret_rect: Some(caret_rect),
            motion_gpu: Some(motion_gpu),
            workspaces: WorkspaceList::new(WindowTree::new(buffer), cursor),
            cursor_pos: None,
            origin: (0, 0),
            sidebar: None,
            sidebar_open: false,
            sidebar_focused: false,
            terminal_open: false,
            terminal_focused: false,
        }));
        let opened = self.frames.len() - 1;

        // Maximizing *after* creation, with the window already
        // positioned on the target monitor, is what makes it fill that
        // monitor rather than whichever one the OS considers primary --
        // and it respects the taskbar, which sizing to the raw monitor
        // rectangle wouldn't.
        if placement.maximized {
            window.set_maximized(true);
        }
        self.focus_frame(opened);
        self.refresh_frame_origin();
        self.focus_active_frame_window();
        self.wake_caret();
    }

    /// Where a new frame should open: the first monitor, left to right,
    /// that hasn't got a Fenix frame on it already -- so on a multi-
    /// monitor desk `SPC w n` lands on the empty screen instead of on
    /// top of the window you're already looking at.
    ///
    /// Falls back to a cascade offset from the active frame when every
    /// monitor is occupied (or when the platform reports no monitors at
    /// all), so a second frame on a single screen is still reachable and
    /// doesn't land exactly on top of the first.
    fn next_frame_placement(&self, event_loop: &ActiveEventLoop) -> FramePlacement {
        let occupied: Vec<(i32, i32)> = (0..self.frames.len()).filter_map(|frame| self.frame_center(frame)).collect();
        let mut monitors: Vec<MonitorHandle> = event_loop.available_monitors().collect();
        monitors.sort_by_key(|monitor| monitor.position().x);

        if let Some(free) = monitors.iter().find(|monitor| !occupied.iter().any(|&at| monitor_contains(monitor, at))) {
            let size = free.size();
            return FramePlacement {
                position: Some(free.position()),
                // A sensible restored-down size for a window that's
                // about to be maximized onto this monitor.
                size: Some(PhysicalSize::new(size.width / 2, size.height / 2)),
                maximized: true,
            };
        }

        const CASCADE: i32 = 48;
        let position = self
            .window
            .as_ref()
            .and_then(|window| window.inner_position().ok())
            .map(|at| PhysicalPosition::new(at.x + CASCADE, at.y + CASCADE));
        FramePlacement { position, size: self.window.as_ref().map(|window| window.inner_size()), maximized: false }
    }

    /// Reopens the windows recorded in `config.windows`, called once
    /// from `resumed` after the first window exists. The first saved
    /// entry is applied to that window; each remaining one opens a
    /// frame of its own. Focus ends on the first, wherever it sits.
    ///
    /// A saved rectangle is handed to the OS as-is. If a monitor has
    /// been unplugged since, Windows clamps the window back onto a
    /// visible screen rather than leaving it stranded off the edge --
    /// which is the whole reason this records rectangles instead of
    /// monitor names (see `fenix_config::WindowLayout`).
    fn restore_saved_windows(&mut self, event_loop: &ActiveEventLoop) {
        if self.restore_session_frames(event_loop) { return; }
        if self.config.restore_windows == Some(false) {
            return;
        }
        let layouts = self.config.windows.clone();
        let Some((first, rest)) = layouts.split_first() else { return };

        if let Some(window) = &self.window {
            window.set_outer_position(PhysicalPosition::new(first.x, first.y));
            let _ = window.request_inner_size(PhysicalSize::new(first.width, first.height));
            window.set_maximized(first.maximized);
        }
        self.refresh_frame_origin();

        for layout in rest {
            self.open_frame(event_loop, FramePlacement::from(*layout));
        }
        self.focus_frame(0);
        self.focus_active_frame_window();
    }

    /// Records every open window's geometry into `config`, then writes
    /// it -- called from `exiting`, the one place every quit path
    /// (`SPC q q`, `:q`, the last window's X button, `SPC q Q`) is
    /// guaranteed to pass through.
    ///
    /// A failure to write is reported to stderr and otherwise ignored,
    /// the same posture `save` already has everywhere else it's called:
    /// losing a window arrangement is not worth refusing to exit over.
    fn save_window_layout(&mut self) {
        let layouts: Vec<fenix_config::WindowLayout> = (0..self.frames.len())
            .filter_map(|frame| {
                let window = self.frame_window(frame)?;
                let at = window.outer_position().ok()?;
                let size = window.inner_size();
                Some(fenix_config::WindowLayout {
                    x: at.x,
                    y: at.y,
                    width: size.width,
                    height: size.height,
                    maximized: window.is_maximized(),
                })
            })
            .collect();
        if !layout_worth_saving(&layouts, &self.config.windows) {
            return;
        }
        self.config.windows = layouts;
        if let Err(err) = self.config.save() {
            eprintln!("fenix: couldn't save the window layout ({err})");
        }
    }

    /// The centre of `frame`'s window in desktop coordinates -- how
    /// "which monitor is this frame on" is decided. `None` for a frame
    /// with no window yet, or on a platform that won't report window
    /// positions.
    fn frame_center(&self, frame: usize) -> Option<(i32, i32)> {
        let window = self.frame_window(frame)?;
        let at = window.inner_position().ok()?;
        let size = window.inner_size();
        Some((at.x + size.width as i32 / 2, at.y + size.height as i32 / 2))
    }

    /// `SPC w Q`: closes the focused frame. Closing the *last* frame is
    /// quitting, so it goes through the ordinary unsaved-changes gate
    /// rather than silently dropping your only window -- the same
    /// posture `close_window` takes for the last pane.
    pub(crate) fn close_frame(&mut self, event_loop: &ActiveEventLoop) {
        if self.frames.len() <= 1 {
            self.request_quit(event_loop);
            return;
        }
        self.drop_frame(self.active_frame);
        self.focus_active_frame_window();
        self.wake_caret();
    }

    /// `SPC w O`: closes every frame except the focused one. The
    /// frame-level counterpart of `SPC w o`, and equally permanent.
    pub(crate) fn only_frame(&mut self) {
        while self.frames.len() > 1 {
            // Always a frame other than the active one: with more than
            // one frame open, index 0 and the last index can't both be
            // the active one.
            let victim = if self.active_frame == 0 { self.frames.len() - 1 } else { 0 };
            self.drop_frame(victim);
        }
        self.focus_active_frame_window();
        self.wake_caret();
    }

    /// `SPC w W`: moves focus to the next frame, wrapping -- the
    /// frame-level counterpart of `SPC w w`.
    pub(crate) fn cycle_frame(&mut self) {
        if self.frames.len() <= 1 {
            return;
        }
        let next = (self.active_frame + 1) % self.frames.len();
        self.focus_frame(next);
        self.focus_active_frame_window();
        self.wake_caret();
    }

    /// Removes `frame` for good: tears down the sessions living in it,
    /// then drops its `FrameState`, which drops its `Window` (closing
    /// the OS window) and its swapchain. Refuses on the last frame --
    /// callers that mean "quit" say so explicitly.
    ///
    /// Whichever frame was active stays active, unless it's the one
    /// being dropped; then focus lands on its neighbour. The live
    /// fields are always handed to a frame that will still exist
    /// *before* the removal, because `activate_frame` swaps against a
    /// parked frame and the dying one is about to stop being one.
    fn drop_frame(&mut self, frame: usize) {
        if self.frames.len() <= 1 || frame >= self.frames.len() {
            return;
        }
        let keep = self.active_frame;
        self.activate_frame(frame);
        self.close_sessions_in_active_frame();

        let survivor = if keep == frame {
            if frame == 0 {
                1
            } else {
                frame - 1
            }
        } else {
            keep
        };
        self.focus_frame(survivor);
        self.frames.remove(frame);
        if self.active_frame > frame {
            self.active_frame -= 1;
        }
        self.focused_frame = self.active_frame;
    }

    /// Tears down every session whose panes live in the active frame --
    /// called by `drop_frame` while the frame being dropped is still the
    /// live one, because each session's own close path touches
    /// `workspaces` and `buffers` and has to act on the layout it
    /// actually belongs to.
    ///
    /// Without this, closing a frame would strand whatever it was
    /// holding open: a Git session still polling in the background -- each
    /// pointing at panes that no longer exist.
    fn close_sessions_in_active_frame(&mut self) {
        let panes = self.workspaces.all_panes();

        // One pane apiece is enough to identify these: a panel session
        // builds all of its panes in one workspace, so they're either
        // all in this frame or none of them are.
        if self.git_session.as_ref().is_some_and(|session| panes.contains(&session.status_pane)) {
            self.git_session_close();
        }
    }

    fn windows(&self) -> &WindowTree<BufferId> {
        self.workspaces.active()
    }

    fn windows_mut(&mut self) -> &mut WindowTree<BufferId> {
        self.workspaces.active_mut()
    }

    fn focused_buffer_id(&self) -> BufferId {
        *self.windows().focused_content()
    }

    /// The currently-focused pane's open buffer. Free to use anywhere
    /// that doesn't *also* need another `App` field (like `self.vim`) in
    /// the same expression -- being a method call on `self`, it borrows
    /// opaquely, so those few spots inline `self.buffers.get_mut(id)`
    /// directly instead (see e.g. `handle_key`'s Vim dispatch).
    fn open(&self) -> &OpenBuffer {
        self.buffers.get(self.focused_buffer_id()).expect("focused window always has an open buffer")
    }

    fn open_mut(&mut self) -> &mut OpenBuffer {
        let id = self.focused_buffer_id();
        self.buffers.get_mut(id).expect("focused window always has an open buffer")
    }

    fn focused_pane_id(&self) -> fenix_window::WindowId {
        self.windows().focused_id()
    }

    /// `pane`'s own live cursor/scroll -- every existing pane has one,
    /// seeded when the pane was created (`Workspace::new`/`split_window`/
    /// `set_pane_content`), so this can safely `.expect()` rather than
    /// return `Option`, the same invariant-backed shape `open`/`open_mut`
    /// already use for `focused_buffer_id`.
    fn pane_state(&self, pane: fenix_window::WindowId) -> &PaneState {
        self.workspaces.active_pane_states().get(&pane).expect("every existing pane has a PaneState")
    }

    fn pane_state_mut(&mut self, pane: fenix_window::WindowId) -> &mut PaneState {
        self.workspaces.active_pane_states_mut().get_mut(&pane).expect("every existing pane has a PaneState")
    }

    /// The focused pane's live cursor -- read-only convenience mirroring
    /// `open()`'s role but for pane-owned (not buffer-owned) state. Same
    /// "don't use where another `self` field is also needed in the same
    /// expression" caveat as `open()`'s own doc comment.
    fn cursor(&self) -> Cursor {
        self.pane_state(self.focused_pane_id()).cursor
    }

    /// The focused buffer plus the focused pane's live cursor, as the two
    /// separate mutable borrows most buffer-editing call sites need
    /// simultaneously (`Buffer` methods take `&mut Cursor` as a sibling
    /// argument, not something bundled into one struct anymore now that
    /// cursor is pane-owned rather than buffer-owned). Not usable from a
    /// spot that *also* needs another `self` field (like `self.vim`) in
    /// the same expression -- those inline `self.buffers.get_mut(id)` /
    /// `self.workspaces.active_pane_states_mut().get_mut(&pane)` directly
    /// instead (see `handle_key`'s Vim dispatch), the same reasoning
    /// `open`/`open_mut`'s own doc comment already gives.
    fn focused_buffer_and_cursor_mut(&mut self) -> (&mut Buffer, &mut Cursor) {
        let buffer_id = self.focused_buffer_id();
        let pane = self.focused_pane_id();
        let buffer = &mut self.buffers.get_mut(buffer_id).expect("focused window always has an open buffer").buffer;
        let cursor = &mut self.workspaces.active_pane_states_mut().get_mut(&pane).expect("every existing pane has a PaneState").cursor;
        (buffer, cursor)
    }

    /// Points `pane` at `buffer_id` and resets that pane's live cursor/
    /// scroll to the newly-shown buffer's own remembered position --
    /// every `WindowTree::set_content` call site goes through this
    /// instead of calling it directly, so a pane switching to a
    /// different buffer never keeps stale cursor/scroll state left over
    /// from whatever it was showing before (which could easily be out of
    /// bounds for the new buffer's own length).
    fn set_pane_content(&mut self, pane: fenix_window::WindowId, buffer_id: BufferId) {
        self.set_pane_content_as(pane, buffer_id, false);
    }

    /// `set_pane_content`, with a buffer the pane has no tab for yet
    /// opening in its preview tab when `preview` (see `App::list_tab`).
    fn set_pane_content_as(&mut self, pane: fenix_window::WindowId, buffer_id: BufferId, preview: bool) {
        if self.snippet.as_ref().is_some_and(|s| s.pane == pane) { self.snippet = None; }
        let before = self.windows().content(pane).copied();
        if let Some(before) = before.filter(|&b| b != buffer_id) {
            self.workspaces.active_workspace_mut().last_tab.insert(pane, before);
            // The tab being left keeps its cursor, for coming back to it
            // (and for an insert form aimed at it).
            if let Some(cursor) = self.workspaces.active_pane_states().get(&pane).map(|s| s.cursor) {
                if let Some(ob) = self.buffers.get_mut(before) {
                    ob.cursor = cursor;
                }
            }
        }
        self.windows_mut().set_content(pane, buffer_id);
        let cursor = self.buffers.get(buffer_id).map(|ob| ob.cursor).unwrap_or(Cursor::at_start());
        self.workspaces.active_pane_states_mut().insert(pane, PaneState::seeded_at(cursor));
        self.list_tab(pane, before, buffer_id, preview);
        self.refresh_gutter_hunks(buffer_id);
    }

    /// Recomputes `gutter_hunks` for `buffer_id` from a single synchronous,
    /// one-file `git diff` (fast -- not `GitStatusPoller`'s repo-wide
    /// background poll, a different subsystem this doesn't need), run at
    /// the points a file's on-disk content can meaningfully change under
    /// it: opening/switching to it (`set_pane_content`), right after a
    /// save (`save_inner`), and regaining window focus (`WindowEvent::
    /// Focused(true)`, the same moment `poll_files_changed_on_disk`
    /// already re-syncs against external disk changes). Reflects the
    /// file's last-saved-to-disk content against `HEAD`, not a live per-
    /// keystroke diff against an in-memory git blob -- the explorer's own
    /// git status badges are save-triggered the same way. Clears any
    /// stale entry for a buffer with no backing path, outside a git repo,
    /// or with no changes against `HEAD` -- mirrors how those badges
    /// already no-op there.
    fn refresh_gutter_hunks(&mut self, buffer_id: BufferId) {
        let found = self.compute_gutter_hunks(buffer_id);
        self.set_gutter_hunks(buffer_id, found);
    }

    fn set_gutter_hunks(&mut self, buffer_id: BufferId, found: Option<(PathBuf, fenix_diff::FileDiff)>) {
        self.gutter_refreshed.insert(buffer_id, Instant::now());
        match found {
            Some((root, file)) if !file.hunks.is_empty() => {
                self.gutter_hunks.insert(buffer_id, gutter_marks_from_hunks(&file.hunks));
                self.gutter_diffs.insert(buffer_id, (root, file));
            }
            _ => {
                self.gutter_hunks.remove(&buffer_id);
                self.gutter_diffs.remove(&buffer_id);
            }
        }
    }

    fn compute_gutter_hunks(&self, buffer_id: BufferId) -> Option<(PathBuf, fenix_diff::FileDiff)> {
        let path = self.buffers.get(buffer_id)?.buffer.path()?.to_path_buf();
        Self::gutter_hunks_of(path)
    }

    /// `path`'s changes against its repository's HEAD, for the gutter: the
    /// repository's root and the file's diff. Runs `git`, so it can take a
    /// while -- see `refresh_all_gutter_hunks`.
    fn gutter_hunks_of(path: PathBuf) -> Option<(PathBuf, fenix_diff::FileDiff)> {
        // Resolved from `path` itself, not `self.project_root` -- that
        // field tracks a single global "current project" derived from
        // whichever buffer happens to be *focused* (see `refresh_project_
        // root`), which isn't necessarily this buffer, or any real
        // project at all (a dashboard/scratch buffer has no path, so it's
        // `None` while one of those is focused). Every open buffer's own
        // gutter marks need its own file's real repo root, independent of
        // what else happens to be focused when this runs.
        // The repository's own root, not the project's: a patch built
        // from this diff names paths from the top of the repository,
        // which is where `git apply` has to run.
        let repo_root = git_editor::repository_of(&path)?;
        // A remote file is named to its machine's git from the top of its
        // repository: this side's path for it means nothing there.
        let named = if fenix_vfs::is_remote(&path) { git_editor::relative_to(&path, &repo_root) } else { std::fs::canonicalize(&path).unwrap_or(path).to_string_lossy().into_owned() };
        let diff_text = fenix_git::file_diff(&repo_root, &named, false).ok()?;
        let file = fenix_diff::parse(&diff_text).into_iter().next()?;
        Some((repo_root, file))
    }

    /// `refresh_gutter_hunks` for every currently open buffer -- restoring
    /// a session reconstructs `Workspace`/`WindowTree` state directly
    /// (`app/session.rs`), bypassing `set_pane_content` entirely, so
    /// nothing else would ever compute a restored pane's gutter marks:
    /// merely focusing/clicking into it afterward doesn't either, since
    /// that's a plain focus change, not a buffer switch. Called once
    /// right after a session restore finishes.
    ///
    /// Worked out on a thread of its own: `git` once or twice per buffer
    /// took seconds behind the launch splash (more where a virus scanner
    /// looks at every `git` started). The marks come in when they're
    /// ready; a buffer refreshed meanwhile (a save) keeps its newer ones.
    pub(crate) fn refresh_all_gutter_hunks(&mut self) {
        let paths: Vec<(BufferId, PathBuf)> = self.buffers.mru().iter().filter_map(|&id| Some((id, self.buffers.get(id)?.buffer.path()?.to_path_buf()))).collect();
        let started = Instant::now();
        match self.event_proxy.clone() {
            Some(proxy) => {
                std::thread::spawn(move || {
                    let found = paths.into_iter().map(|(id, path)| (id, Self::gutter_hunks_of(path))).collect();
                    let _ = proxy.send_event(FenixUserEvent::GutterHunks(started, found));
                });
            }
            // No event loop (every test): here and now.
            None => {
                let found = paths.into_iter().map(|(id, path)| (id, Self::gutter_hunks_of(path))).collect();
                self.apply_gutter_hunks(started, found);
            }
        }
    }

    /// `refresh_all_gutter_hunks`' marks, for the buffers still open and
    /// not refreshed since it started.
    fn apply_gutter_hunks(&mut self, started: Instant, found: Vec<(BufferId, Option<(PathBuf, fenix_diff::FileDiff)>)>) {
        for (id, hunks) in found {
            if self.buffers.get(id).is_none() || self.gutter_refreshed.get(&id).is_some_and(|at| *at > started) {
                continue;
            }
            self.set_gutter_hunks(id, hunks);
        }
        self.wake_caret();
    }

    /// Whether the *focused* pane currently belongs to one of the app's
    /// long-lived side sessions (Git/Task/Debug) rather
    /// than just showing an ordinary file/scratch buffer -- checked the
    /// same way each session already answers "is this my pane" for its
    /// own purposes (`git_focused_role`/`jira_focused_
    /// role`/`task_session`'s own single `pane` field), so this adds no
    /// new bookkeeping, just combines what already exists. See `open_
    /// buffer_in_focused_pane`'s doc comment for why this matters --
    /// `TaskSession` hit the exact same class of bug this guard was
    /// built for (`SPC p n` jumping to a diagnostic while the Task
    /// Output pane was focused silently replaced it with the jumped-to
    /// file, so a later `SPC p r`/`SPC p T` rerun kept running -- just
    /// into a buffer no pane displayed anymore).
    fn focused_pane_holds_a_tracked_session(&self) -> bool {
        let focused = self.focused_pane_id();
        // An extension's surface (a remote desktop) is deliberately *not*
        // in this list: it's a page, found by its buffer, so pointing its
        // pane at something else just hides it, and its extension opens
        // it again.
        self.git_focused_role().is_some()
            || self.task_session.as_ref().is_some_and(|s| s.pane == focused)
            || self.tool_status_session.as_ref().is_some_and(|s| s.pane == focused)
            || self
                .history_session
                .as_ref()
                .is_some_and(|s| focused == s.graph_pane || focused == s.refs_pane || focused == s.detail_pane)
            || self.merge_session.as_ref().is_some_and(|s| focused == s.files_pane || focused == s.merge_pane)
            || self.compose.as_ref().is_some_and(|c| focused == c.pane)
    }

    /// Points the focused pane at `id` -- unless the focused pane
    /// currently belongs to a tracked session (Git/Task/Debug),
    /// in which case `id` opens in a brand-new workspace instead
    /// (mirroring `App::new_workspace`'s own "seed a new workspace with
    /// this buffer" logic) and the session's own pane is left completely
    /// untouched.
    ///
    /// Every "open this wherever I'm looking" action (`SPC SPC`, `SPC b
    /// b`, `SPC b X`, opening a document/dashboard/dired listing, ...)
    /// goes through this instead of calling `set_pane_content` on
    /// `focused_pane_id()` directly. Before this existed, none of them
    /// checked what the focused pane already held: reported bug was
    /// "focus a VNC pane, open a file with `SPC SPC`, and the VNC pane
    /// silently starts showing that file instead." The session itself
    /// (socket, reader thread, framebuffer) kept running with no pane
    /// left displaying it, so `SPC v v` on the same host found its own
    /// `session.pane` unchanged and landed right back on that same pane
    /// -- which was still showing the hijacked file, not VNC, making it
    /// look like reopening the session had silently done nothing.
    fn open_buffer_in_focused_pane(&mut self, id: BufferId) {
        self.open_buffer_in_focused_pane_as(id, false);
    }

    /// `open_buffer_in_focused_pane`, in the pane's preview tab when
    /// `preview` -- for a jump rather than a deliberate open.
    fn open_buffer_in_focused_pane_as(&mut self, id: BufferId, preview: bool) {
        if self.focused_pane_holds_a_tracked_session() {
            let cursor = self.buffers.get(id).map(|ob| ob.cursor).unwrap_or(Cursor::at_start());
            self.workspaces.new_workspace(id, cursor);
        } else {
            let focused = self.focused_pane_id();
            self.set_pane_content_as(focused, id, preview);
        }
    }

    /// Re-derives `project_root` for the focused buffer -- called every
    /// time a pane's buffer changes, not just at startup (`with_file`
    /// does the equivalent inline before `self` exists to call this on).
    /// Deliberately does *not* register the result in `known_projects`:
    /// that list is explicitly curated via `SPC p a`/`SPC p d` now, not
    /// auto-populated from wherever you happen to open a file -- see
    /// `picker_add_project_prompt`'s own doc comment for why.
    /// `root`'s kind, detected the first time it's asked for.
    fn project_kind_of(&self, root: &Path) -> fenix_project::ProjectKind {
        *self.project_kinds.borrow_mut().entry(root.to_path_buf()).or_insert_with(|| fenix_project::detect_kind(root))
    }

    /// The focused buffer's project, as (kind, folder name) -- `None` for
    /// a buffer outside any project, which the chrome then doesn't name.
    fn focused_project_identity(&self) -> Option<(fenix_project::ProjectKind, String)> {
        let root = self.project_root.as_deref()?;
        let name = root.file_name()?.to_string_lossy().into_owned();
        Some((self.project_kind_of(root), name))
    }

    /// The project `file` is in, as far as Fenix is concerned: the
    /// active workspace's when the file is inside it -- a crate or a
    /// package nested in the project you opened is still that project,
    /// not one of its own, and a folder you opened as a project is that
    /// project, marker or not -- else the nearest folder with a
    /// project's marker.
    pub(super) fn project_root_of_file(&self, file: &Path) -> Option<PathBuf> {
        match self.workspaces.active_workspace().project.as_deref().filter(|root| path_within(file, root)) {
            Some(workspace) => Some(workspace.to_path_buf()),
            None => fenix_project::find_project_root(file),
        }
    }

    /// The workspace of another project than the active one's that `file`
    /// belongs in: the one whose project holds it most closely. `None`
    /// when the active workspace's project holds it, when no workspace's
    /// does, or with `workspace_per_project` off.
    pub(super) fn workspace_for_file(&self, file: &Path) -> Option<usize> {
        if self.config.workspace_per_project == Some(false) {
            return None;
        }
        let active = self.workspaces.active_index();
        if self.workspaces.workspaces[active].project.as_deref().is_some_and(|root| path_within(file, root)) {
            return None;
        }
        self.workspaces
            .workspaces
            .iter()
            .enumerate()
            .filter_map(|(i, w)| w.project.as_deref().filter(|root| path_within(file, root)).map(|root| (i, root.components().count())))
            .max_by_key(|(_, depth)| *depth)
            .map(|(i, _)| i)
    }

    /// The folder whose `.fenix` speaks for the focused file: the nearest
    /// one on the way up to the project's top that `holds` says has its
    /// own (a nested crate's, a monorepo package's), else the project's.
    pub(super) fn nearest_fenix_root(&self, holds: impl Fn(&Path) -> bool) -> Option<PathBuf> {
        let root = self.project_root.clone()?;
        let file = self.open().buffer.path().filter(|f| !fenix_vfs::is_remote(f) && path_within(f, &root));
        let nearest = file.and_then(Path::parent).and_then(|dir| {
            dir.ancestors().take_while(|d| path_within(d, &root) && place_key(d) != place_key(&root)).find(|d| holds(d)).map(Path::to_path_buf)
        });
        Some(nearest.unwrap_or(root))
    }

    /// Where the focused file's project settings come from
    /// (`.fenix/settings.toml`): `nearest_fenix_root`'s.
    pub(super) fn settings_root(&self) -> Option<PathBuf> {
        self.nearest_fenix_root(|d| d.join(".fenix").join("settings.toml").is_file() || d.join(".fenix").join("project.ini").is_file())
    }

    /// Where the programs that apply to the focused file are named
    /// (`.fenix/tools.json`, `.fenix/lsp`): `nearest_fenix_root`'s.
    pub(super) fn trust_root(&self) -> Option<PathBuf> {
        self.nearest_fenix_root(|d| !fenix_project::trust::files(d).is_empty())
    }

    /// The project of what has the focus: its file's
    /// (`project_root_of_file`), else -- Home, a page -- the active
    /// workspace's.
    pub(super) fn focused_project(&self) -> Option<PathBuf> {
        match self.open().buffer.path() {
            Some(file) => self.project_root_of_file(file),
            None => self.workspaces.active_workspace().project.clone(),
        }
    }

    /// Whether the focus is on the Home of a workspace of no project: a
    /// new one (`SPC TAB n`), or what's left after closing one. It's in
    /// no project, not the last one you were in.
    fn on_a_projectless_home(&self) -> bool {
        let ws = self.workspaces.active_workspace();
        ws.project.is_none() && ws.home == Some(self.focused_buffer_id())
    }

    fn refresh_project_root(&mut self) {
        let path = self.open().buffer.path().map(Path::to_path_buf);
        // Home, and any other buffer that belongs to no project, is in
        // its workspace's project -- `SPC SPC` from a project's Home
        // finds that project's files, not the folder Fenix started in.
        let was = self.project_root.clone();
        self.project_root = path
            .as_deref()
            .and_then(|file| self.project_root_of_file(file))
            .or_else(|| self.workspaces.active_workspace().project.clone())
            // A buffer of no file in a workspace of no project -- a page,
            // a lesson's practice -- stays in the last project you were in
            // a file of: `SPC p t` from the practice runs its tasks. Never
            // the Git lesson's practice repository, which is no project
            // of yours.
            .or_else(|| self.last_project.clone().filter(|_| path.is_none() && !self.on_a_projectless_home()));
        if path.is_some() {
            let practice = self.tutorial_progress.with_file_name("tutorial-repo");
            if let Some(root) = self.project_root.clone().filter(|r| !r.starts_with(&practice)) {
                self.last_project = Some(root);
            }
        }
        if let Some(root) = self.project_root.clone() {
            self.prewarm_file_index(&root);
            if was.as_ref() != Some(&root) {
                self.explorer_project_switched(&root);
            }
        }
        self.refresh_project_settings(false);
        self.apply_editorconfig();
        self.tell_extensions(Happened::FocusChanged);
        // Said before its servers would have started: they won't, yet.
        self.project_trust_notice();
        self.sync_lsp_for_focused_buffer();
    }

    /// Records (or clears) one file's diagnostics -- `textDocument/
    /// publishDiagnostics` is keyed by URI regardless of whether a
    /// buffer for it is currently open, and an *empty* list is the
    /// server's way of saying "no more diagnostics here," not "nothing
    /// changed," so it's stored as an actual removal rather than an
    /// empty `Vec` left behind.
    /// A file's problems: every language server's, and the MIB's.
    fn all_diagnostics(&self, path: &Path) -> Vec<lsp_types::Diagnostic> {
        self.diagnostics.for_path(path)
    }

    /// `]d`/`[d`: moves to the `count`th problem after (or before) the
    /// cursor -- a language server's, or a script's call checked against
    /// the MIB -- and says what it is. Stops at the last one, as `]t` does.
    pub(crate) fn jump_to_diagnostic(&mut self, forward: bool, count: u32) {
        let Some(path) = self.open().buffer.path().map(Path::to_path_buf) else {
            self.set_message("no problems in this buffer");
            return;
        };
        let buffer = &self.open().buffer;
        let at = |d: &lsp_types::Diagnostic| {
            let line = (d.range.start.line as usize).min(buffer.line_count().saturating_sub(1));
            buffer.line_start_char(line) + (d.range.start.character as usize).min(buffer.line_len(line))
        };
        let mut found: Vec<(usize, String)> = self.all_diagnostics(&path).iter().map(|d| (at(d), d.message.clone())).collect();
        found.sort_by_key(|(c, _)| *c);
        found.dedup_by_key(|(c, _)| *c);
        let cursor_idx = self.cursor().char_idx;
        let candidates: Vec<&(usize, String)> = if forward { found.iter().filter(|(c, _)| *c > cursor_idx).collect() } else { found.iter().rev().filter(|(c, _)| *c < cursor_idx).collect() };
        let Some((target, message)) = candidates.get((count.max(1) as usize - 1).min(candidates.len().saturating_sub(1))).map(|(c, m)| (*c, m.clone())) else {
            self.set_message(if found.is_empty() {
                "no problems in this buffer"
            } else if forward {
                "no more problems below"
            } else {
                "no more problems above"
            });
            return;
        };
        let from = JumpEntry { buffer: self.focused_buffer_id(), char_idx: cursor_idx };
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        cursor.char_idx = target;
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
        self.record_jump(from);
        self.wake_caret();
        self.set_message(message);
    }

    /// `key`'s diagnostics for the file on `path` (as the servers spell
    /// it), in place of its own earlier ones: a language server's, or an
    /// extension's (`diagnostics-set`).
    fn publish_diagnostics(&mut self, path: PathBuf, key: LspKey, name: &str, diagnostics: Vec<lsp_types::Diagnostic>) {
        self.diagnostics.publish(path.clone(), key, name, diagnostics);
        self.tell_diagnostic_counts(&path);
    }

    /// The focused file's problem counts, for the extensions that are
    /// told, if `path` is it and they changed.
    fn tell_diagnostic_counts(&mut self, path: &Path) {
        let path = path.to_path_buf();
        // The focused file's count, for the extensions that are told.
        if self.open().buffer.path().is_some_and(|p| fenix_lsp::normalize(p.to_path_buf()) == fenix_lsp::normalize(path.clone())) {
            let all = self.diagnostics.for_path(&path);
            let count = |s: lsp_types::DiagnosticSeverity| all.iter().filter(|d| d.severity == Some(s)).count() as u32;
            let counts = (count(lsp_types::DiagnosticSeverity::ERROR), count(lsp_types::DiagnosticSeverity::WARNING));
            if self.wasm.told_diagnostics != Some(counts) {
                self.wasm.told_diagnostics = Some(counts);
                self.editor_event("diagnostics", fenix_wasm::host::Notice::Diagnostics(counts));
            }
        }
    }
    /// `K` -- requests `textDocument/hover` for the cursor's current
    /// position. With no server attached to the focused buffer it's a
    /// no-op (no error message; this fires on every `K` press, including
    /// ones with genuinely nothing to say) -- Tcl's built-in usage lines
    /// come from the Tcl extension, which is a language server.
    pub(crate) fn request_hover(&mut self) {
        self.request_hover_at(self.cursor().char_idx);
    }

    /// `request_hover`, about char `at` of the focused buffer.
    pub(super) fn request_hover_at(&mut self, at: usize) {
        let Some((keys, text_document, position)) = self.focused_lsp_context_at(at) else {
            self.lsp_hover = None;
            return;
        };
        let buffer = self.focused_buffer_id();
        let targets = self.lsp_route(&keys, fenix_lsp::fanout::Feature::Hover);
        if targets.is_empty() {
            return;
        }
        let order = targets.clone();
        let fanout = self.start_lsp_fanout(|deadline| LspFanout::Hover { buffer, gather: fenix_lsp::fanout::Gather::new(order, deadline) }, targets.len());
        self.lsp_hover_fanout = Some(fanout);
        for key in targets {
            let params = lsp_types::HoverParams { text_document_position_params: lsp_types::TextDocumentPositionParams { text_document: text_document.clone(), position }, work_done_progress_params: Default::default() };
            self.send_lsp_fanout_request::<lsp_types::request::HoverRequest>(&key, params, PendingLspRequest::Hover { fanout });
        }
    }

    /// `gd` -- requests `textDocument/definition`.
    pub(crate) fn request_goto_definition(&mut self) {
        self.request_lsp_locations(false);
    }

    /// `gr` -- requests `textDocument/references` (including the
    /// declaration itself in the results, matching what most editors'
    /// own "find references" shows by default).
    pub(crate) fn request_references(&mut self) {
        self.request_lsp_locations(true);
    }

    /// `SPC c i` -- where the trait, interface or abstract method under the
    /// cursor is implemented, from every server that can say: jumped to
    /// when there's one, listed when there are more (as `gd` does).
    pub(crate) fn request_implementations(&mut self) {
        let Some((keys, text_document, position)) = self.focused_lsp_context() else {
            self.set_error("no language server is running for this file");
            return;
        };
        let buffer = self.focused_buffer_id();
        let targets = self.lsp_route(&keys, fenix_lsp::fanout::Feature::Implementation);
        if targets.is_empty() {
            self.set_error("no language server here finds implementations");
            return;
        }
        let order = targets.clone();
        let fanout = self.start_lsp_fanout(|deadline| LspFanout::Locations { buffer, references: false, gather: fenix_lsp::fanout::Gather::new(order, deadline) }, targets.len());
        let at = lsp_types::TextDocumentPositionParams { text_document, position };
        for key in targets {
            let params = lsp_types::request::GotoImplementationParams { text_document_position_params: at.clone(), work_done_progress_params: Default::default(), partial_result_params: Default::default() };
            // Answered as a definition is: a location, or several.
            self.send_lsp_fanout_request::<lsp_types::request::GotoImplementation>(&key, params, PendingLspRequest::Definition { fanout });
        }
    }

    /// `SPC c a` -- requests `textDocument/codeAction` for the focused
    /// buffer's current line (a single-line range, matching what most
    /// editors send when the caller isn't a Visual selection -- Fenix
    /// has no separate "code action for this selection" entry point yet).
    pub(crate) fn request_code_action(&mut self) {
        self.sync_lsp_for_focused_buffer();
        let path = self.open().buffer.path().map(|p| fenix_lsp::normalize(std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())));
        // The extensions' fixes for problems on this line, each
        // checked against the text as it was when they gave it.
        let line = self.open().buffer.line_col(&self.cursor()).0 as u32;
        let fixes: Vec<(lsp_types::CodeActionOrCommand, refactor::Context, Option<String>)> = match &path {
            Some(p) => self.code_fixes_on(p, line).into_iter().map(|(ext, action)| (lsp_types::CodeActionOrCommand::CodeAction(action), self.code_fix_context(p), Some(ext))).collect(),
            None => Vec::new(),
        };
        let Some((keys, text_document, position)) = self.focused_lsp_context() else {
            if fixes.is_empty() {
                return;
            }
            return self.preview_lsp_actions(fixes);
        };
        let buffer = self.focused_buffer_id();
        let targets = self.lsp_route(&keys, fenix_lsp::fanout::Feature::CodeAction);
        if targets.is_empty() {
            if !fixes.is_empty() {
                return self.preview_lsp_actions(fixes);
            }
            self.set_message("no code actions available");
            return;
        }
        let contexts: HashMap<LspKey, refactor::Context> = targets.iter().map(|key| (key.clone(), self.refactor_context(key))).collect();
        let order = targets.clone();
        let fanout = self.start_lsp_fanout(
            |deadline| LspFanout::CodeActions { buffer, gather: fenix_lsp::fanout::Gather::new(order, deadline), contexts, errors: Vec::new(), fixes },
            targets.len(),
        );
        let line_range = lsp_types::Range { start: lsp_types::Position { line: position.line, character: 0 }, end: lsp_types::Position { line: position.line, character: u32::MAX } };
        for key in targets {
            // Each server is shown only the diagnostics it reported itself.
            let diagnostics = path.as_deref().map(|p| self.diagnostics.from_server(p, &key)).unwrap_or_default();
            let params = lsp_types::CodeActionParams {
                text_document: text_document.clone(),
                range: line_range,
                context: lsp_types::CodeActionContext { diagnostics, only: None, trigger_kind: None },
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            };
            self.send_lsp_fanout_request::<lsp_types::request::CodeActionRequest>(&key, params, PendingLspRequest::CodeAction { fanout });
        }
    }

    /// Records `path` as recently-opened for the dashboard's "Recent
    /// Files" list -- called from every real "read this path off disk"
    /// call site (the picker, the explorer; `with_file`'s own CLI-arg
    /// branch does the equivalent inline before `self` exists). Unlike
    /// `known_projects`, this is automatic, not explicitly curated --
    /// see `RecentFiles`'s own doc comment.
    fn record_recent_file(&mut self, path: &Path) {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.recent_files.add(fenix_core::plain_path(&canonical));
        if let Err(err) = self.recent_files.save() {
            eprintln!("fenix: couldn't save recent files: {err}");
        }
    }

    /// Whether the char immediately before the cursor sits inside a
    /// comment or string literal -- `sync_completion`'s auto-trigger
    /// gate, so typing an ordinary word inside a comment or a string
    /// literal doesn't pop the completion popup (real code identifiers
    /// aren't what's being typed there). Two checks, since neither
    /// alone covers both: `completion::cursor_in_unterminated_string`'s
    /// textual heuristic catches a string still being typed (open
    /// quote, cursor right after it); the focused buffer's own syntax
    /// tree catches a comment (always parses cleanly) and a string
    /// that's already been *closed* elsewhere on the line (a real
    /// capture by then). Deliberately only gates *auto*-trigger: `Ctrl-
    /// Space`'s `force_open_completion` bypasses this, the same "auto-
    /// trigger is context-gated, manual invoke always works" split
    /// most editors make. `false` (never suppress) for any buffer with
    /// no syntax tree at all -- an unrecognized language, or a non-
    /// editor buffer kind -- since there's no way to know the context
    /// without one; that's just today's pre-existing behavior.
    ///
    /// `spell` counts as a comment marker too: Tcl's own `highlights.
    /// scm` captures a comment node as both `@spell @comment` on the
    /// exact same span, and `resolve_overlaps` only ever keeps one
    /// capture of an exact tie (see its own doc comment) -- so either
    /// name showing up here means the same thing, and checking only
    /// `"comment"` would silently miss the tie going the other way.
    fn cursor_in_comment_or_string(&mut self) -> bool {
        let cursor = self.cursor();
        let id = self.focused_buffer_id();
        let Some(ob) = self.buffers.get_mut(id) else { return false };
        // Catches a freshly-typed, still-open string before the syntax
        // tree can (see `cursor_in_unterminated_string`'s own doc
        // comment for why the tree alone can't) -- checked first since
        // it needs no syntax-tree work at all.
        if completion::cursor_in_unterminated_string(&ob.buffer, &cursor) {
            return true;
        }
        // Same "drain this buffer's pending edits, apply them to its
        // syntax tree, then query" shape `content_highlights_for_
        // visible_range` already uses -- draining here and again
        // (with an empty leftover list) at the next redraw is harmless,
        // per `SyntaxState::apply_edits`'s own doc comment.
        let edits: Vec<fenix_syntax::RawEdit> = ob
            .buffer
            .drain_edits()
            .into_iter()
            .map(|d| fenix_syntax::RawEdit { start_char: d.start_char, new_end_char: d.new_end_char, removed: d.removed })
            .collect();
        let Some(syntax) = &mut ob.syntax else { return false };
        let source = ob.buffer.text();
        syntax.apply_edits(&source, &edits);

        let byte_idx = ob.buffer.char_to_byte(cursor.char_idx.min(ob.buffer.len_chars()));
        let Some(check_at) = byte_idx.checked_sub(1) else { return false };
        syntax
            .highlights_in_range(&source, check_at..byte_idx)
            .into_iter()
            .any(|(_, name)| name.starts_with("comment") || name.starts_with("string") || name == "spell")
    }

    /// With the popup up unasked and nothing chosen in it, Enter is the
    /// end of the line: nothing has been typed to complete (it's up after
    /// a server's trigger, `:` say), or the word is already whole.
    fn enter_is_a_new_line(&self) -> bool {
        if self.completion_selected || self.completion_asked { return false; }
        let Some(state) = &self.completion else { return false };
        let at = self.cursor().char_idx;
        at == state.prefix_start || state.picker.selected().is_some_and(|c| c.payload.label == self.open().buffer.text_range(state.prefix_start, at))
    }

    /// The focused buffer's detected language -- a path-extension check
    /// used by `toggle_comment_lines`'s own comment-token lookup, and (to
    /// exclude string/comment spans from the bracket scan, not to gate
    /// whether it runs at all -- `format_buffer`/`format_selection`
    /// work on any buffer regardless) `reindent_skip_ranges`.
    pub(super) fn focused_language(&self) -> Option<fenix_syntax::LanguageId> {
        self.open().buffer.path().and_then(|p| p.extension()).and_then(|e| e.to_str()).and_then(fenix_syntax::detect_language)
    }

    /// `SPC c x`: flips the GFM task checkbox (`- [ ]`/`- [x]`/`- [X]`)
    /// on the focused pane's current line, in place. No per-language
    /// gate -- same "the text shape alone is the signal" posture list
    /// continuation (`fenix_vim::indent::parse_list_item`) already has,
    /// and this is an explicit, deliberate keypress rather than
    /// something that fires on every Enter, so there's even less risk
    /// in leaving it available everywhere. A no-op (with a message) if
    /// the current line has no checkbox to toggle.
    pub(crate) fn toggle_checkbox(&mut self) {
        let line = self.open().buffer.line_col(&self.cursor()).0;
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        if !fenix_vim::toggle_checkbox(buffer, cursor, line) {
            self.set_error("no checkbox on this line to toggle");
        }
    }

    /// `gcc`/`gc{motion}` (`VimEvent::ToggleComment`) -- toggles a
    /// line-comment prefix across `start_line..=end_line`. Blank lines
    /// (after trimming leading whitespace) are never touched either
    /// direction, matching Comment.nvim/vim-commentary. If every non-
    /// blank line in range is already commented, uncomments all of
    /// them; otherwise comments every one that *isn't* already
    /// commented (leaving already-commented lines alone -- the standard
    /// "mixed selection comments the rest" convention those plugins
    /// share). A no-op (with a message, same posture `format_buffer`
    /// already has for "no formatter configured") if the language has
    /// no single-token line comment (`line_comment_token`), or the
    /// range is entirely blank.
    pub(crate) fn toggle_comment_lines(&mut self, start_line: usize, end_line: usize) {
        // One edit per line underneath, one `u` to take them all back.
        self.focused_buffer_and_cursor_mut().0.begin_undo_group();
        self.toggle_comment_lines_inner(start_line, end_line);
        self.focused_buffer_and_cursor_mut().0.end_undo_group();
    }

    fn toggle_comment_lines_inner(&mut self, start_line: usize, end_line: usize) {
        let language = self.focused_language();
        if let Some((open, close)) = language.filter(|l| line_comment_token(*l).is_none()).and_then(block_comment_tokens) {
            self.toggle_block_comment_lines(start_line, end_line, open, close);
            return;
        }
        let Some(token) = language.and_then(line_comment_token) else {
            self.set_error("no comment syntax known for this buffer's language");
            return;
        };
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        let end_line = end_line.min(buffer.line_count().saturating_sub(1));

        // One pass over the *original* text: `first` (a char offset, not
        // byte -- indentation is assumed ASCII, same assumption this
        // codebase's other leading-whitespace helpers already make) and
        // whether this line is already commented. Bottom-to-top
        // processing below never invalidates a not-yet-processed
        // (lower) line's own `line_start_char`, so none of this needs
        // re-reading after an edit.
        let mut lines: Vec<(usize, usize, bool)> = Vec::new();
        let mut all_commented = true;
        for line in start_line..=end_line {
            let start = buffer.line_start_char(line);
            let text = buffer.text_range(start, start + buffer.line_len(line));
            let chars: Vec<char> = text.chars().collect();
            let Some(first) = chars.iter().position(|&c| c != ' ' && c != '\t') else { continue };
            let rest: String = chars[first..].iter().collect();
            let commented = rest.starts_with(token);
            if !commented {
                all_commented = false;
            }
            lines.push((line, first, commented));
        }
        if lines.is_empty() {
            return;
        }

        for &(line, first, commented) in lines.iter().rev() {
            let start = buffer.line_start_char(line);
            if all_commented {
                let mut remove_len = token.chars().count();
                if buffer.char_at(start + first + remove_len) == Some(' ') {
                    remove_len += 1;
                }
                buffer.delete_range(cursor, start + first, start + first + remove_len);
            } else if !commented {
                // Uses the *real* pane cursor here (not a throwaway
                // local `Cursor`) -- a throwaway one would leave the
                // real cursor's `char_idx` stale relative to the just-
                // inserted text, silently pointing at the wrong
                // character for whatever runs next (a follow-up motion,
                // or `.` replaying this same command from a different
                // line).
                cursor.char_idx = start + first;
                buffer.insert_str(cursor, &format!("{token} "));
            }
        }
        // Lands on the first affected line's own start, a stable and
        // predictable spot regardless of which lines above got
        // commented/uncommented (rather than wherever the loop's last
        // per-line edit happened to leave the cursor).
        cursor.char_idx = buffer.line_start_char(start_line).min(buffer.len_chars());
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
        // The mutation happens here, *after* `vim.handle_key` already
        // returned (outside `route_keypress`'s own edit_count comparison
        // window) -- without this, `.` would silently fail to capture
        // `gcc`/`gc{motion}` as a repeatable change. Same guard as the
        // automatic `` `. `` mark write for the same reason.
        if !self.replaying_change {
            self.change_capture_dirty = true;
        }
    }

    /// `toggle_comment_lines` for a block-comment-only language: each
    /// non-blank line is wrapped in its own `open ... close` pair
    /// (`<!-- line -->`) rather than the range in one, so toggling any
    /// sub-range of it later still lines up, and a line that already
    /// contains a comment of its own doesn't end up nesting one inside
    /// another (which XML doesn't allow). Same all-or-rest rule as the
    /// line-comment path.
    fn toggle_block_comment_lines(&mut self, start_line: usize, end_line: usize, open: &str, close: &str) {
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        let end_line = end_line.min(buffer.line_count().saturating_sub(1));
        let mut lines: Vec<(usize, String, String, bool)> = Vec::new();
        let mut all_commented = true;
        for line in start_line..=end_line {
            let start = buffer.line_start_char(line);
            let text = buffer.text_range(start, start + buffer.line_len(line));
            let body = text.trim_start();
            if body.is_empty() {
                continue;
            }
            let indent = text[..text.len() - body.len()].to_string();
            let body = body.trim_end().to_string();
            let commented = body.starts_with(open) && body.ends_with(close) && body.len() >= open.len() + close.len();
            if !commented {
                all_commented = false;
            }
            lines.push((line, indent, body, commented));
        }
        if lines.is_empty() {
            return;
        }
        for (line, indent, body, commented) in lines.into_iter().rev() {
            let replacement = if all_commented {
                let inner = &body[open.len()..body.len() - close.len()];
                let inner = inner.strip_prefix(' ').unwrap_or(inner);
                let inner = inner.strip_suffix(' ').unwrap_or(inner);
                format!("{indent}{inner}")
            } else if !commented {
                format!("{indent}{open} {body} {close}")
            } else {
                continue;
            };
            let start = buffer.line_start_char(line);
            let end = start + buffer.line_len(line);
            buffer.replace_range(cursor, start, end, &replacement);
        }
        cursor.char_idx = buffer.line_start_char(start_line).min(buffer.len_chars());
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
        if !self.replaying_change {
            self.change_capture_dirty = true;
        }
    }

    /// `:{range}!command`, `!{motion}`: lines `first..=last` through
    /// `command` (`cmd /C` on Windows, `sh -c` elsewhere, in the project's
    /// folder), what it writes in their place as one change, the cursor on
    /// the first of them. It runs while you wait, for a few seconds at
    /// most; one that fails says why and leaves the lines.
    pub(crate) fn filter_lines(&mut self, first: usize, last: usize, command: &str) {
        let root = self.project_root.clone().or_else(|| self.open().buffer.path().and_then(Path::parent).map(Path::to_path_buf)).or_else(|| std::env::current_dir().ok()).unwrap_or_default();
        let (program, flag) = if cfg!(windows) { ("cmd", "/C") } else { ("sh", "-c") };
        let spec = fenix_project::tools::CommandSpec::new(program.into(), vec![flag.into(), command.into()]);
        let (input, start, end) = {
            let buffer = &self.open().buffer;
            let last = last.min(buffer.line_count().saturating_sub(1));
            let start = buffer.line_start_char(first.min(last));
            let end = buffer.line_start_char(last) + buffer.line_len(last);
            (format!("{}\n", buffer.text_range(start, end)), start, end)
        };
        let file = root.join("filter");
        match crate::formatter::run(&spec, &input, &file, &root, FILTER_TIMEOUT, true) {
            Ok(output) => {
                let output = output.replace("\r\n", "\n");
                let output = output.strip_suffix('\n').unwrap_or(&output).to_string();
                let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
                buffer.begin_undo_group();
                buffer.replace_range(cursor, start, end, &output);
                buffer.break_undo_run();
                buffer.end_undo_group();
                cursor.char_idx = start.min(buffer.len_chars());
                cursor.sticky_col = 0;
                if !self.replaying_change {
                    self.change_capture_dirty = true;
                }
            }
            Err(why) => self.set_error(why),
        }
    }

    /// The formatter an extension gave for the focused buffer's language
    /// in its project (`formatter-set`), run on it: its text replaced as
    /// one change you can undo, the cursor kept on its line and column.
    /// False when there's none; a formatter that fails says why and
    /// leaves the text.
    pub(crate) fn format_with_extension(&mut self) -> bool {
        match self.run_extension_formatter() {
            Some(Err(why)) => {
                self.set_error(why);
                true
            }
            ran => ran.is_some(),
        }
    }

    /// `format_with_extension`'s work: none when there's no formatter,
    /// else whether it formatted, or why not.
    fn run_extension_formatter(&mut self) -> Option<Result<(), String>> {
        let path = self.open().buffer.path().map(Path::to_path_buf)?;
        let language = self.focused_language()?;
        let root = tool_sessions::root_for_path(&path);
        let (spec, keep_output) = self.wasm.formatter(&root, language)?;
        let source = self.open().buffer.text();
        match crate::formatter::run(&spec, &source, &path, &root, crate::formatter::TIMEOUT, keep_output) {
            Ok(formatted) if formatted == source => Some(Ok(())),
            Ok(formatted) => {
                let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
                let (line, col) = buffer.line_col(cursor);
                replace_text_by_lines(buffer, cursor, &formatted);
                let line = line.min(buffer.line_count().saturating_sub(1));
                cursor.char_idx = buffer.line_start_char(line) + col.min(buffer.line_len(line));
                cursor.sticky_col = col;
                Some(Ok(()))
            }
            Err(why) => Some(Err(why)),
        }
    }

    /// `SPC c F` -- reindents the whole focused buffer in place,
    /// structurally (`fenix_format::reindent`) rather than via a
    /// per-language external tool -- Emacs' `indent-region`, real
    /// Vim's `=` operator. Works on any buffer, not just ones with a
    /// detected language (see `reindent_skip_ranges`'s own doc
    /// comment for what a detected one buys in accuracy). A no-op if
    /// the result is identical to what's already there.
    pub(crate) fn format_buffer(&mut self) {
        let indent_width = self.vim.indent_width();
        let language = self.focused_language();
        let source = self.open().buffer.text();
        let last_line = self.open().buffer.line_count().saturating_sub(1);
        // Tags, not brackets, are what nest in XML -- a bracket-depth
        // reindent would indent after every `(` in element text instead.
        let formatted = if language == Some(fenix_syntax::LanguageId::Xml) {
            fenix_format::reindent_xml(&source, 0, last_line, indent_width)
        } else {
            let skip = reindent_skip_ranges(language, &source);
            fenix_format::reindent(&source, 0, last_line, indent_width, &skip)
        };
        if formatted == source {
            return;
        }
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        replace_text_by_lines(buffer, cursor, &formatted);
    }

    /// `SPC c f` -- reindents the active Visual selection's lines in
    /// place, then returns to Normal mode (mirroring what an ordinary
    /// Visual operator does on completion). The whole buffer is still
    /// walked for nesting context (`fenix_format::reindent_lines`'s own
    /// doc comment on why), but only the selection's own lines become
    /// part of the edit. Only meaningful for Char/Line selections -- a
    /// line-based reindent has no sensible reading of a column
    /// rectangle -- so this is a no-op outside Visual mode or on a
    /// Visual-Block selection.
    pub(crate) fn format_selection(&mut self) {
        // See `request_lsp_format`'s own doc comment: a server-backed
        // format is always whole-document, so this deliberately
        // reformats more than just the selection when one's attached --
        // accepted rather than building a separate `rangeFormatting`
        // path for what's otherwise a rare, capability-gated case.
        if self.request_lsp_format() {
            let pane = self.focused_pane_id();
            if let Some(pane_state) = self.workspaces.active_pane_states_mut().get_mut(&pane) {
                self.vim.exit_visual_mode(&pane_state.cursor);
            }
            return;
        }
        let id = self.focused_buffer_id();
        let pane = self.focused_pane_id();
        let indent_width = self.vim.indent_width();
        let Some(ob) = self.buffers.get_mut(id) else { return };
        let Some(pane_state) = self.workspaces.active_pane_states_mut().get_mut(&pane) else { return };
        let Some((range, _linewise)) = self.vim.visual_selection_range(&ob.buffer, &pane_state.cursor) else {
            // Can't call `self.set_error` here -- `ob`/`pane_state` still
            // hold disjoint mutable borrows of `self.buffers`/`self.
            // workspaces` at this point. Set directly instead; equivalent
            // to `set_error`, just without the (unavailable) method call.
            self.status_message = Some(StatusMessage {
                text: "SPC c f needs an active (non-block) Visual selection".to_string(),
                is_error: true,
                set_at: Instant::now(),
            });
            return;
        };

        // The selection's own char range doesn't need to survive past
        // this point -- only the line numbers it touches do (a
        // structural reindent, like real Vim's `=`, always acts on
        // whole lines regardless of exactly where within them the
        // selection's own endpoints fall). `range.end` is an exclusive
        // upper bound, so the line it names isn't necessarily one the
        // selection actually reaches -- querying at `range.end - 1`
        // (clamped so an empty selection can't underflow) is what
        // avoids pulling in one extra line past a selection that
        // happens to end exactly on a line boundary.
        let start_line_cursor = Cursor { char_idx: range.start, sticky_col: 0 };
        let end_line_cursor = Cursor { char_idx: range.end.saturating_sub(1).max(range.start), sticky_col: 0 };
        let (first_line, _) = ob.buffer.line_col(&start_line_cursor);
        let (last_line, _) = ob.buffer.line_col(&end_line_cursor);

        let source = ob.buffer.text();
        let language = ob.buffer.path().and_then(|p| p.extension()).and_then(|e| e.to_str()).and_then(fenix_syntax::detect_language);
        let replacement = if language == Some(fenix_syntax::LanguageId::Xml) {
            fenix_format::reindent_xml_lines(&source, first_line, last_line, indent_width)
        } else {
            let skip = reindent_skip_ranges(language, &source);
            fenix_format::reindent_lines(&source, first_line, last_line, indent_width, &skip)
        };

        let start = ob.buffer.line_start_char(first_line);
        let end = ob.buffer.line_start_char(last_line) + ob.buffer.line_len(last_line);
        if ob.buffer.text_range(start, end) != replacement {
            ob.buffer.replace_range(&mut pane_state.cursor, start, end, &replacement);
        }
        self.vim.exit_visual_mode(&pane_state.cursor);
    }
    pub(crate) fn save(&mut self) {
        self.save_inner(false);
        self.reload_keybindings_if_saved();
    }

    /// `:w!` -- write even though the file changed underneath us.
    pub(crate) fn save_force(&mut self) {
        self.save_inner(true);
        self.reload_keybindings_if_saved();
    }

    /// Writes the focused buffer, refusing by default if the file has
    /// changed on disk since Fenix last read or wrote it.
    ///
    /// The refusal is the point of the whole file-watching feature. A
    /// plain write is a whole-file overwrite: whatever the other side
    /// did between the buffer being read and this keystroke is gone,
    /// with nothing to recover it from. Both ways out are offered by
    /// name, because "your file changed" is useless without them.
    fn save_unnamed_as(&mut self, input: &str) {
        if self.open().buffer.path().is_some() {
            self.set_error("this buffer already has a path; use :w to save or the file rename command");
            return;
        }
        if !self.open().kind.tracks_unsaved_changes() {
            self.set_error("this panel cannot be saved as a document");
            return;
        }
        let path = PathBuf::from(input);
        let path = if path.is_absolute() {
            path
        } else {
            // In the project, wherever that is: a remote one's paths
            // aren't joined as this machine's are.
            fenix_vfs::join(&self.project_root.clone().unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from("."))), input)
        };
        // Naming a scratch document must never overwrite another document.
        if fenix_vfs::exists(&path) || self.buffers.id_for_path(&path).is_some() {
            self.set_error("destination already exists or is open; choose a new file name");
            return;
        }
        let id = self.focused_buffer_id();
        match self.open_mut().buffer.save_new(&path) {
            Ok(()) => {
                self.buffers.path_changed(id, None);
                self.note_disk_state(id);
                self.discard_recovery_for(id);
                self.refresh_project_root();
                self.set_message(format!("saved {}", self.shown_path(&path)));
            }
            Err(err) => self.set_error(format!("save failed: {err}")),
        }
    }

    fn save_inner(&mut self, force: bool) {
        let id = self.focused_buffer_id();
        if self.open().buffer.path().is_none() {
            self.set_error("no file path to save to yet; use :w <path>");
            return;
        }
        // Saving a preview says you mean to keep it.
        let pane = self.focused_pane_id();
        if self.workspaces.active_workspace().preview.get(&pane) == Some(&id) {
            self.keep_preview(pane);
        }
        if !force {
            // Re-checked here rather than trusting the last sweep: the
            // sweep runs every couple of seconds, and the write is
            // happening now.
            let mut sweep = DiskSweep::default();
            self.check_one_file(id, &mut sweep, false, None);
            if self.externally_changed.contains(&id) {
                let name = self.buffer_display_name(id);
                self.set_error(format!("{name} changed on disk since you opened it -- :w! keeps yours, :e! takes theirs"));
                self.wake_caret();
                return;
            }
            // The check may have reloaded a clean buffer out from under
            // this save, in which case there is nothing left to write
            // and saying so beats silently writing the file back to
            // itself.
            if !sweep.reloaded.is_empty() {
                self.set_message(format!("{} changed on disk and was reloaded -- nothing of yours to save", join_names(&sweep.reloaded)));
                self.wake_caret();
                return;
            }
        }
        // An extension's formatter first, when you've asked for that; one
        // that fails says so, and the text is saved as it is.
        let unformatted = if self.config.format_on_save == Some(true) { self.run_extension_formatter().and_then(Result::err) } else { None };
        // A new file (`SPC f f src/new/mod.rs`) in folders that aren't
        // there yet: they're made.
        if let Some(parent) = self.open().buffer.path().and_then(Path::parent).filter(|p| !p.as_os_str().is_empty() && !fenix_vfs::exists(p)) {
            if let Err(err) = fenix_vfs::create_dir_all(parent) {
                self.set_error(format!("save failed: couldn't create {}: {err}", fenix_core::plain_path(parent).display()));
                self.wake_caret();
                return;
            }
        }
        self.editorconfig_before_save(id, None);
        let ob = self.open_mut();
        match ob.buffer.save() {
            Ok(()) => {
                let path = ob.buffer.path().unwrap().to_path_buf();
                self.after_saved(id, &path);
                match (self.xml_save_problem(&path), Self::snippet_save_note(&path)) {
                    (Some(problem), _) => self.set_error(format!("saved {} -- {problem}", self.shown_path(&path))),
                    (None, Some(Ok(note))) => self.set_message(format!("saved -- {note}")),
                    (None, Some(Err(why))) => self.set_error(format!("saved -- {why}")),
                    (None, None) => self.set_message(format!("saved {}", self.shown_path(&path))),
                }
                if let Some(why) = unformatted {
                    self.set_error(format!("saved {}, unformatted -- {why}", self.shown_path(&path)));
                }
            }
            Err(err) => self.set_error(format!("save failed: {err}")),
        }
        self.wake_caret();
    }

    /// What follows a buffer written to its file, however it was saved
    /// (`:w`, auto-save).
    pub(super) fn after_saved(&mut self, id: BufferId, path: &Path) {
        // Recorded before anything else: this is what stops the next
        // sweep reporting Fenix's own write as somebody else's change.
        self.note_disk_state(id);
        // The work is on disk now, so the snapshot describes nothing
        // that's lost -- and leaving it would offer to "recover" it on
        // the next start.
        self.discard_recovery_for(id);
        self.refresh_gutter_hunks(id);
        // A save of yours settles what auto-save held back.
        self.auto_save_settled(id);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.editor_event("saved", fenix_wasm::host::Notice::Saved(name));
        self.code_file_saved(path);
        self.tests_file_saved(path);
        // A server's settings, edited (`.fenix/lsp/<name>.json`): it's
        // told.
        if let Some(project) = lsp_settings_file(path) {
            let server = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            self.lsp_settings_changed(&project, Some(&server));
        }
        // A `.editorconfig` of yours, saved: read again.
        if path.file_name().is_some_and(|n| n == ".editorconfig") {
            self.editorconfig_changed();
        }
    }

    /// Every open buffer whose kind can actually hold savable content
    /// (`BufferKind::tracks_unsaved_changes`) and is currently dirty,
    /// in `ids_sorted_by_path()` order. Single source of truth for
    /// "what would `:qa` warn about" -- generated, pathless views like
    /// the Jira/Docker/Git panels or the startup dashboard are never
    /// savable in the first place (`App::save` refuses them outright),
    /// so their `is_dirty()` -- which the host itself sets every time
    /// it re-renders them -- would otherwise falsely and permanently
    /// count as "unsaved work."
    fn dirty_tracked_buffer_ids(&self) -> Vec<BufferId> {
        self.buffers
            .ids_sorted_by_path()
            .into_iter()
            .filter(|&id| self.buffers.get(id).is_some_and(|ob| ob.kind.tracks_unsaved_changes() && ob.buffer.is_dirty()))
            .filter(|&id| !self.is_kept_compose(id))
            .collect()
    }

    /// Whether `id` is an extension's compose pane's buffer: what's
    /// written there is kept as you quit (`keep_compose_draft`), so it's no
    /// unsaved work to warn about, nor to keep with the session.
    pub(super) fn is_kept_compose(&self, id: BufferId) -> bool {
        self.compose.as_ref().is_some_and(|c| c.buffer == id && matches!(c.purpose, ComposePurpose::Code { .. }))
    }

    /// Whether any buffer has unsaved changes worth warning about --
    /// extends the modeline's existing single-buffer `[+]` marker
    /// across every open buffer.
    ///
    /// Test-only now: `request_quit` grew past a boolean and works
    /// from `dirty_tracked_buffer_ids` directly, so it can name the
    /// buffers it is blocking on. Kept because it is the readable way
    /// to assert "nothing is unsaved", which several tests want.
    #[cfg(test)]
    fn any_buffer_dirty(&self) -> bool {
        !self.dirty_tracked_buffer_ids().is_empty()
    }

    /// Up to this many dirty buffer names are listed by name in the
    /// quit-confirm message before falling back to "(+N more)".
    const QUIT_CONFIRM_NAME_CAP: usize = 4;

    /// Builds the "N unsaved buffers: a, b, c -- quit anyway?" message
    /// for `dirty`, naming buffers via `buffer_display_name` (the same
    /// path/`*dashboard*`/`*docker*`/etc. fallback the modeline already
    /// uses) so the user knows exactly what `:qa`/`SPC q q` is
    /// blocking on, not just how many.
    fn format_quit_confirm_message(&self, dirty: &[BufferId]) -> String {
        let count = dirty.len();
        let plural = if count == 1 { "" } else { "s" };
        let names: Vec<String> = dirty.iter().take(Self::QUIT_CONFIRM_NAME_CAP).map(|&id| self.buffer_display_name(id)).collect();
        let more = count.saturating_sub(Self::QUIT_CONFIRM_NAME_CAP);
        let suffix = if more > 0 { format!(" (+{more} more)") } else { String::new() };
        format!("{count} unsaved buffer{plural}: {}{suffix} -- quit anyway? (y=yes / n=no / l=list)", names.join(", "))
    }

    /// The event-loop-independent half of `request_quit`: arms `quit_
    /// confirm` and the modeline message when there's unsaved work
    /// (returning `true`), or leaves everything untouched when there
    /// isn't (returning `false`, meaning the caller should exit
    /// immediately). Split out purely so it's directly testable --
    /// `&ActiveEventLoop` can't be constructed in this test harness (see
    /// `test_dispatch_key`'s own doc comment for the same limitation
    /// elsewhere in this file).
    fn arm_quit_confirm_if_dirty(&mut self) -> bool {
        let dirty = self.dirty_tracked_buffer_ids();
        if dirty.is_empty() {
            return false;
        }
        self.quit_confirm = true;
        self.set_error(self.format_quit_confirm_message(&dirty));
        true
    }

    /// `:qa`/`:quitall`/`SPC q q`/the window's close button
    /// (`app.quit`) -- exits immediately if every buffer is saved,
    /// otherwise arms `quit_confirm` and shows a message instead of
    /// exiting, so an accidental (or just habitual, forgetting there's
    /// unsaved work) quit can't silently discard it. `:qa!`/`app.
    /// quit_force` bypasses this entirely (`commands::cmd_quit_force`)
    /// -- real Vim's own `!` convention for "yes, I know, do it
    /// anyway." Note this is no longer reachable from a bare `:q`/
    /// `SPC b k` -- those only close the focused buffer now, see
    /// `App::kill_buffer`.
    /// `SPC q r` (`app.restart`): quits and starts again, the session
    /// restored -- unsaved text and all -- for what only a start reads.
    pub(crate) fn request_restart(&mut self, event_loop: &ActiveEventLoop) {
        match self.prepare_restart() {
            Some(why) => self.set_error(why),
            None => event_loop.exit(),
        }
    }

    pub(crate) fn request_quit(&mut self, event_loop: &ActiveEventLoop) {
        if !self.arm_quit_confirm_if_dirty() {
            event_loop.exit();
        }
    }

    /// Saves every dirty, tracked (`BufferKind::tracks_unsaved_
    /// changes`) buffer that has a path. Refuses the whole operation
    /// up front -- no partial saves -- if any such buffer has no path
    /// (nothing to `:w` to yet), naming which one(s) need `:w <path>`
    /// first. Returns whether it's now safe to quit; callers must not
    /// quit on `false`, an error has already been set.
    fn save_all(&mut self) -> bool {
        let dirty = self.dirty_tracked_buffer_ids();
        let unnamed: Vec<BufferId> = dirty.iter().copied().filter(|&id| self.buffers.get(id).is_some_and(|ob| ob.buffer.path().is_none())).collect();
        if !unnamed.is_empty() {
            let names: Vec<String> = unnamed.iter().map(|&id| self.buffer_display_name(id)).collect();
            self.set_error(format!("no file name for {} -- use :w <path> first", names.join(", ")));
            return false;
        }
        for id in dirty {
            let name = self.buffer_display_name(id);
            self.editorconfig_before_save(id, None);
            let Some(ob) = self.buffers.get_mut(id) else { continue };
            if let Err(err) = ob.buffer.save() {
                self.set_error(format!("save failed for {name}: {err}"));
                return false;
            }
        }
        true
    }

    /// `:wqa`/`:xa` -- saves every unsaved, savable buffer, then quits
    /// unconditionally. Deliberately bypasses the `quit_confirm` y/n
    /// prompt entirely: once `save_all` returns `true` there is by
    /// definition nothing left dirty to warn about, mirroring real
    /// Vim's own `:wqa`, which either succeeds outright or errors, but
    /// never prompts.
    pub(crate) fn request_save_all_and_quit(&mut self, event_loop: &ActiveEventLoop) {
        if self.save_all() {
            event_loop.exit();
        }
    }

    /// The event-loop-independent half of `quit_confirm_key`: clears
    /// `quit_confirm` and reports what the pressed key resolved to --
    /// `y` confirms, `l` asks to see the offending buffers (chosen to
    /// not collide with `y`/`n`), anything else cancels. Split out for
    /// the same testability reason as `arm_quit_confirm_if_dirty`.
    fn resolve_quit_confirm_action(&mut self, keypress: KeyPress) -> QuitConfirmAction {
        self.quit_confirm = false;
        match keypress.code {
            KeyCode::Char('y') => QuitConfirmAction::Quit,
            KeyCode::Char('l') => QuitConfirmAction::ShowDirtyBuffers,
            _ => QuitConfirmAction::Cancel,
        }
    }

    /// Resolves an armed `quit_confirm` -- `y` quits unconditionally
    /// (same as `:qa!`), `l` opens the buffer picker filtered to just
    /// the dirty buffers so the user can jump to and save them instead
    /// of guessing, anything else cancels. Same "captures the very
    /// next key" shape as `DockerHost::confirm_key`/`git_confirm_key`.
    fn quit_confirm_key(&mut self, keypress: KeyPress, event_loop: Option<&ActiveEventLoop>) {
        match self.resolve_quit_confirm_action(keypress) {
            QuitConfirmAction::Quit => { if self.discard_session_edits() { if let Some(event_loop) = event_loop { event_loop.exit(); } } },
            QuitConfirmAction::ShowDirtyBuffers => self.picker_switch_dirty_buffers(),
            QuitConfirmAction::Cancel => {}
        }
        self.wake_caret();
    }

    /// `pane_cols` with the caller's own headless fallback -- the merge
    /// view's columns want a different one from the panels' wrap width,
    /// and a view that renders differently under test than in use is
    /// worse than one that renders wide.
    fn pane_cols_or(&self, pane: fenix_window::WindowId, fallback: usize) -> usize {
        let (Some(gpu), Some(text)) = (&self.gpu, &self.text) else {
            return fallback;
        };
        let (window_width, window_height) = (gpu.size.width as f32, gpu.size.height as f32);
        let (sidebar_px, terminal_h, modeline_top) = self.frame_metrics(window_height);
        let geometry = self.frame_geometry(window_width, sidebar_px, terminal_h, modeline_top);
        let Some((_, rect)) = geometry.panes.iter().find(|(id, _)| *id == pane) else {
            return fallback;
        };
        // Two characters of slack: a pane's own padding, and the
        // rightmost column being flush against the divider reads as
        // text running into the next pane even when it technically
        // fits.
        let cols = ((rect.w - text::PAD_LEFT * 2.0) / text.char_width()).floor() as isize - 2;
        cols.clamp(20, 400) as usize
    }

    /// Points every pane showing `id` at `fallback` -- in every workspace
    /// of every frame, not only the visible one. A pane left showing a
    /// closed buffer in a workspace you aren't looking at is a crash
    /// waiting for the moment you switch to it. Returns the panes changed
    /// in the active workspace, for callers with per-pane state of their
    /// own (titles) to clear.
    fn repoint_panes_showing(&mut self, id: BufferId, fallback: BufferId) -> Vec<fenix_window::WindowId> {
        self.repoint_panes(id, fallback, false)
    }

    /// `repoint_panes_showing`, each pane going back to the tab it showed
    /// before `id` while that's still open (a page closed with `q` gives
    /// its pane back as it was), else to `fallback`.
    fn repoint_panes_back(&mut self, id: BufferId, fallback: BufferId) -> Vec<fenix_window::WindowId> {
        self.repoint_panes(id, fallback, true)
    }

    fn repoint_panes(&mut self, id: BufferId, fallback: BufferId, back: bool) -> Vec<fenix_window::WindowId> {
        let active = self.workspaces.active_index();
        let mut active_panes = Vec::new();
        let mut shown = Vec::new();
        let buffers = &self.buffers;
        let parked = self.frames.iter_mut().flatten().map(|frame| &mut frame.workspaces);
        for (list_index, list) in std::iter::once(&mut self.workspaces).chain(parked).enumerate() {
            for (index, ws) in list.workspaces.iter_mut().enumerate() {
                for pane in ws.windows.windows() {
                    if ws.windows.content(pane) != Some(&id) {
                        continue;
                    }
                    let open = |b: BufferId| b != id && buffers.get(b).is_some();
                    let to = if back {
                        ws.last_tab.get(&pane).copied().filter(|&b| open(b))
                            .or_else(|| ws.pane_tabs.get(&pane).and_then(|t| t.iter().rev().copied().find(|&b| open(b))))
                            .unwrap_or(fallback)
                    } else {
                        fallback
                    };
                    let cursor = buffers.get(to).map(|ob| ob.cursor).unwrap_or(Cursor::at_start());
                    ws.windows.set_content(pane, to);
                    ws.pane_states.insert(pane, PaneState::seeded_at(cursor));
                    if ws.last_tab.get(&pane) == Some(&to) {
                        ws.last_tab.remove(&pane);
                    }
                    let tabs = ws.pane_tabs.entry(pane).or_default();
                    tabs.retain(|&b| b != id);
                    if !tabs.contains(&to) {
                        tabs.push(to);
                    }
                    if !shown.contains(&to) {
                        shown.push(to);
                    }
                    if list_index == 0 && index == active {
                        active_panes.push(pane);
                    }
                }
            }
        }
        if self.snippet.as_ref().is_some_and(|s| active_panes.contains(&s.pane)) {
            self.snippet = None;
        }
        if shown.is_empty() {
            shown.push(fallback);
        }
        for to in shown.into_iter().rev() {
            self.buffers.touch(to);
            self.refresh_gutter_hunks(to);
        }
        active_panes
    }

    /// Builds fuzzy-picker candidates for every file in `root` --
    /// relative path as the label (fuzzy-matched and displayed),
    /// absolute path as the payload (what actually gets opened).
    /// A path relative to `root`, written with forward slashes on
    /// every platform.
    ///
    /// Not cosmetic. Every other path Fenix shows comes from `git`,
    /// which always writes `/` regardless of platform -- so on Windows
    /// a native-separator label made the file picker the one place a
    /// path looked different (`src\\main.rs` in the picker,
    /// `src/main.rs` in the Git panel two keys away), and made the
    /// obvious fuzzy query for a nested file (`src/ma`) match nothing.
    fn relative_label(root: &Path, path: &Path) -> String {
        path.strip_prefix(root).unwrap_or(path).components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
    }

    /// Candidates for both the switch-project (`SPC p p`) and delete-
    /// project (`SPC p d`) pickers -- same list, same label, just a
    /// different picker variant so `picker_confirm` dispatches to a
    /// different action.
    fn known_project_candidates(&self) -> Vec<fenix_picker::Candidate<PathBuf>> {
        self.known_projects
            .roots()
            .iter()
            .map(|root| fenix_picker::Candidate::new(fenix_vfs::display(root), root.clone()))
            .collect()
    }

    /// `S` on the add-project explorer (`ExplorerAction::SelectCwd`):
    /// registers whatever directory is currently being browsed. Already
    /// an absolute, real filesystem path (it came from actually listing
    /// it), so this only needs to canonicalize away any `..`/symlink
    /// noise before persisting -- no typed-input parsing like the old
    /// free-text prompt needed.
    fn register_project_dir(&mut self, dir: &Path) {
        let root = std::fs::canonicalize(dir).map(fenix_lsp::normalize).unwrap_or_else(|_| dir.to_path_buf());
        self.known_projects.add(root);
        if let Err(err) = self.known_projects.save() {
            eprintln!("fenix: couldn't save project history: {err}");
        }
        self.close_folder_picker();
    }

    /// `SPC f e`: opens the full-buffer file explorer starting at the
    /// user's home directory, in `FindFrom` mode -- for the file that
    /// isn't in any project and isn't worth typing an absolute path for
    /// (e.g. something in `~/Downloads`). Falls back to the process's
    /// cwd if the home directory can't be determined. Navigate with the
    /// explorer's usual `j`/`k`/`l`/`h`; `Enter` on a file opens it
    /// straight away (same as ordinary browsing), or `S` fuzzy-searches
    /// recursively from wherever you've navigated to (see
    /// `start_find_from_here`). `q`/`Escape` cancels.
    pub(crate) fn start_explore_from_home(&mut self) {
        let start = dirs::home_dir().unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        self.open_folder_picker(&start, ExplorerPurpose::FindFrom);
    }

    /// `S` on the `FindFrom` explorer (`ExplorerAction::SelectCwd`):
    /// leaves the explorer and opens a `FindFile` picker over every file
    /// under `dir`, recursively -- same candidate-building as `SPC p f`/
    /// `picker_find_file`, just rooted wherever the user navigated
    /// instead of the project root.
    fn start_find_from_here(&mut self, dir: &Path) {
        self.close_folder_picker();
        self.open_find_file(dir, false);
    }

    /// `SPC p n` -- jumps to the next match in `quickfix` (the most
    /// recent `SPC p s` results), without reopening the picker or
    /// re-running the search. Starts at the first match if nothing's
    /// been visited yet. Clamps at either end (a no-op past the last/
    /// before the first match) rather than wrapping -- same "stop
    /// rather than surprise" reasoning `jump_back`/`jump_forward` use
    /// for an exhausted stack. Also a no-op while a Docker/Git panel
    /// session is open (`jumplist_navigation_allowed`) -- this steps
    /// through `open_file_from_picker`/`jump_to_grep_match` exactly like
    /// confirming a Grep picker entry does, so it's exactly as unsafe to
    /// call from there.
    pub(crate) fn quickfix_next(&mut self) {
        self.quickfix_step(1);
    }

    /// `SPC p N` -- the mirror of `quickfix_next`, stepping backward.
    pub(crate) fn quickfix_prev(&mut self) {
        self.quickfix_step(-1);
    }

    fn quickfix_step(&mut self, delta: isize) {
        if self.quickfix.is_empty() || !self.jumplist_navigation_allowed() {
            return;
        }
        let target = match self.quickfix_index {
            None => 0,
            Some(i) => {
                let n = i as isize + delta;
                if n < 0 || n as usize >= self.quickfix.len() {
                    return;
                }
                n as usize
            }
        };
        let entry = self.quickfix[target].clone();
        let from = JumpEntry { buffer: self.focused_buffer_id(), char_idx: self.cursor().char_idx };
        match entry {
            QuickfixEntry::Grep(m) | QuickfixEntry::Task(m) => {
                self.open_file_as_preview(&m.path);
                self.jump_to_grep_match(&m);
            }
            QuickfixEntry::Lsp { path, position } => {
                self.open_file_as_preview(&path);
                self.goto_lsp_position(position);
            }
        }
        self.record_jump(from);
        self.quickfix_index = Some(target);
        self.search_quickfix_moved(target);
    }

    /// The buffer picker's rows for `ids`, in order: a file by its path
    /// in the project (outside it, whole); a page, terminal or Home by
    /// its tab's name; a plain unnamed buffer as the scratch (#50), a
    /// second as `scratch 2`. What reads the same as one already listed
    /// -- each pane's Home, another empty scratch -- is listed once.
    fn buffer_candidates(&self, ids: &[BufferId]) -> Vec<fenix_picker::Candidate<BufferId>> {
        let mut seen = std::collections::HashSet::new();
        let mut scratches = 0;
        let mut out = Vec::new();
        for &id in ids {
            let Some(ob) = self.buffers.get(id) else { continue };
            let label = if let Some(path) = ob.buffer.path() {
                self.project_root.as_deref().and_then(|r| crate::page::path_within(r, path)).unwrap_or_else(|| crate::page::home_relative(path))
            } else if ob.kind == BufferKind::Dashboard {
                // Each pane's Home is the one Home: by the workspace's
                // name, as its tab has it.
                if !seen.insert("home".to_string()) {
                    continue;
                }
                self.workspaces.active_workspace().name.clone()
            } else {
                let shown = self.buffer_display_name(id);
                if shown == "[No Name]" {
                    let empty = ob.buffer.rope().len_chars() == 0 && !ob.buffer.is_dirty();
                    if empty && !seen.insert("empty scratch".to_string()) {
                        continue;
                    }
                    scratches += 1;
                    if scratches == 1 { "scratch".to_string() } else { format!("scratch {scratches}") }
                } else {
                    let name = self.tab_label(id);
                    if !seen.insert(name.clone()) {
                        continue;
                    }
                    name
                }
            };
            out.push(fenix_picker::Candidate::new(label, id));
        }
        out
    }

    /// Points the focused pane at an already-open buffer -- `SPC b b`'s
    /// confirm action, and shared by nothing else (unlike `open_file_from_
    /// picker`, there's no path to resolve/read; the buffer already
    /// exists in the registry).
    fn switch_focused_to_buffer(&mut self, id: BufferId) {
        self.open_buffer_in_focused_pane(id);
        self.buffers.touch(id);
        self.refresh_project_root();
        self.main_view = MainView::Editor;
    }

    /// `SPC b n`/`SPC b p`: cycles the focused pane through every open
    /// buffer in a stable, path-sorted order (not MRU -- repeated `n`/`p`
    /// should walk a fixed list, not bounce between the two most recent).
    fn cycle_buffer(&mut self, delta: isize) {
        let ids = self.buffers.ids_sorted_by_path();
        if ids.len() <= 1 {
            return;
        }
        let current = self.focused_buffer_id();
        let pos = ids.iter().position(|&id| id == current).unwrap_or(0) as isize;
        let next = ids[(pos + delta).rem_euclid(ids.len() as isize) as usize];
        self.switch_focused_to_buffer(next);
        self.wake_caret();
    }

    pub(crate) fn next_buffer(&mut self) {
        self.cycle_buffer(1);
    }

    pub(crate) fn prev_buffer(&mut self) {
        self.cycle_buffer(-1);
    }

    /// The unconditional half of buffer-closing: closes the focused
    /// buffer no matter what state it's in. Any pane (in this window
    /// tree) currently showing it falls back to the MRU-next open buffer,
    /// or a fresh scratch buffer if none remain -- never leaves a pane
    /// pointing at a closed `BufferId`. Closing one of the Docker/Git/
    /// Jira sessions' buffers this way really means "close the whole
    /// session" (they share live background state, not just a buffer
    /// slot) -- routed to `DockerHost::close`/`git_session_close`/
    /// `jira_session_close` instead of the ordinary single-buffer path
    /// below. Used directly (bypassing `kill_buffer`'s dirty guard) by
    /// callers that are cancelling a generated review buffer
    /// (`project_replace_cancel` and its apply-flow counterpart) or
    /// have just deleted the buffer's file on purpose
    /// (`apply_delete_file`) -- none of those are the user-facing
    /// "close" action the guard exists for.
    fn kill_buffer_now(&mut self) {
        let id = self.focused_buffer_id();
        if self.is_a_workspace_home(id) {
            self.set_message("Home stays open -- gt or SPC b b to leave it");
            return;
        }
        if let Some(session) = &self.git_session {
            if [
                session.status_buffer,
                session.staged_buffer,
                session.unstaged_buffer,
                session.branches_buffer,
                session.commits_buffer,
                session.stash_buffer,
                session.main_buffer,
            ]
            .contains(&id)
            {
                self.git_session_close();
                return;
            }
        }

        // Task Output and Tool Status go with the workspace they opened
        // in (#29).
        if self.tool_status_session.as_ref().is_some_and(|s| s.buffer == id) {
            self.tool_status_close();
            return;
        }
        if self.task_session.as_ref().is_some_and(|s| s.buffer == id) {
            self.task_output_close();
            return;
        }
        // A pane-resident terminal's shell dies with its buffer -- see
        // `close_terminal_buffer` for why this, and not navigating away
        // from the pane, is what ends it.
        self.close_terminal_buffer(id);
        // An extension's page: it's told it's gone.
        self.code_page_closed(id);
        self.forget_explorer_page(id);
        self.tests_page_closing(id);
        self.rename_mode.remove(&id);
        self.pages.remove(&id);
        self.buffers.close(id);
        self.table_views.remove(&id);
        if self.refactor_preview.as_ref().is_some_and(|p| p.buffer == id) { self.refactor_preview = None; }
        self.drop_buffer_from_every_strip(id);
        self.refresh_project_root();
        self.wake_caret();
    }

    /// The guarded half both `:q`/`SPC b k` (`force = false`) and `:q!`
    /// (`force = true`) share. Unlike the app-wide `quit_confirm` y/n
    /// prompt, there's no ambiguity about *which* buffer a bare `:q`
    /// means, so an unsaved tracked (`BufferKind::tracks_unsaved_
    /// changes`) buffer is a hard refusal naming it, not a prompt.
    /// `force = true` bypasses this entirely -- same "`!` means I know,
    /// do it anyway" convention as `:qa!`.
    /// Closing a buffer means its snapshot has no owner left. Dropped
    /// either way: a clean close has nothing to recover, and a forced
    /// one is the user saying they don't want it -- offering it back on
    /// the next start would be ignoring that.
    fn close_focused_buffer(&mut self, force: bool) {
        let id = self.focused_buffer_id();
        if !force && self.buffers.get(id).is_some_and(|ob| ob.kind.tracks_unsaved_changes() && ob.buffer.is_dirty()) {
            let name = self.buffer_display_name(id);
            self.set_error(format!("{name} has unsaved changes -- use :q! to discard or :wq to save"));
            return;
        }
        self.discard_recovery_for(id);
        self.kill_buffer_now();
    }

    /// `SPC b k`/`:q`: closes the focused buffer, refusing (with a
    /// named error) if it has unsaved changes -- never quits the
    /// application, even if it's the last buffer open (falls back to a
    /// fresh scratch buffer instead, per `kill_buffer_now`). See
    /// `App::request_quit`/`app.quit` (`:qa`/`SPC q q`) for quitting.
    pub(crate) fn kill_buffer(&mut self) {
        self.close_focused_buffer(false);
    }

    /// `:q!`: closes the focused buffer unconditionally, discarding any
    /// unsaved changes.
    pub(crate) fn force_kill_buffer(&mut self) {
        self.close_focused_buffer(true);
    }

    /// `:wq`/`:x`: saves the focused buffer first if it needs it, then
    /// closes it. Refuses (naming the buffer) if it's dirty but has no
    /// path to save to yet, rather than silently discarding it or
    /// silently succeeding without writing anything.
    pub(crate) fn save_and_close_buffer(&mut self) {
        let id = self.focused_buffer_id();
        let Some(ob) = self.buffers.get(id) else { return };
        if ob.kind.tracks_unsaved_changes() && ob.buffer.is_dirty() {
            if ob.buffer.path().is_none() {
                let name = self.buffer_display_name(id);
                self.set_error(format!("no file name for {name} -- use :w <path> to save before closing"));
                return;
            }
            self.save();
            if self.buffers.get(id).is_some_and(|ob| ob.buffer.is_dirty()) {
                return; // save() already reported the failure; refuse to close
            }
        }
        self.kill_buffer_now();
    }

    /// `SPC b X`: opens a fresh, empty, unnamed buffer in the focused pane.
    pub(crate) fn new_scratch_buffer(&mut self) {
        let id = self.buffers.open_scratch();
        self.open_buffer_in_focused_pane(id);
        self.refresh_project_root();
        self.wake_caret();
    }

    /// `SPC o d`: this workspace's Home, in the focused pane -- the same
    /// as `gh`, not another dashboard.
    pub(crate) fn open_dashboard(&mut self) {
        self.go_home();
    }

    /// `SPC p r`: the test the cursor is in, run in Task Output -- or
    /// through the test explorer where a provider serves the file.
    pub(crate) fn run_test_at_cursor(&mut self) {
        let Some(path) = self.open().buffer.path().map(Path::to_path_buf) else {
            self.set_error("no file here -- SPC p r runs the test the cursor is in");
            return;
        };
        let line = self.open().buffer.line_col(&self.cursor()).0 + 1;
        // A refusal has already been said.
        let _ = self.run_test_at(&path, line);
    }

    /// Runs the test that line `line` (1-based) of the file at `path` is
    /// in, alone, in Task Output, as `SPC p r` does there: a Rust
    /// `#[test]`, a pytest test or a Go `TestXxx` (`fenix_tasks::test_at`),
    /// read from the file's open buffer when it has one, unsaved edits and
    /// all. The task's name (`test ledger::tests::adds`) when it started;
    /// why not when it didn't, also shown in the modeline. `SPC p T` runs
    /// it again. With `tests.enabled` on, a file a test provider serves
    /// goes through the explorer instead (`tests_door`): the test's label
    /// when it's asked.
    pub(crate) fn run_test_at(&mut self, path: &Path, line: usize) -> Result<String, String> {
        // Through the test explorer where a provider serves the file, as
        // `SPC T t` (plan test-explorer.md §2, Q2); else as it always was
        // (Q7).
        match self.tests_door(path, line.saturating_sub(1) as u32) {
            tests_host::Door::Test { root, node } => return self.tests_door_run(path, &root, node).map(|(what, _)| what),
            tests_host::Door::NotATest(why) => {
                self.set_error(why.clone());
                return Err(why);
            }
            tests_host::Door::Fallback => {}
        }
        let text = match self.buffer_id_for_lsp_path(path).and_then(|id| self.buffers.get(id)) {
            Some(ob) => ob.buffer.text(),
            None => match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(err) => {
                    let why = format!("{}: {err}", readable_path(path));
                    self.set_error(why.clone());
                    return Err(why);
                }
            },
        };
        let root = tool_sessions::root_for_path(path);
        let task = match fenix_tasks::test_at(path, &text, line, &root) {
            Ok(task) => task,
            Err(why) => {
                self.set_error(why.clone());
                return Err(why);
            }
        };
        let name = task.name.clone();
        if self.run_task_as(task, root, None) {
            Ok(name)
        } else {
            Err(self.modeline_pieces().1)
        }
    }

    /// The tasks the project at `root` has, as `SPC t t` offers them: the
    /// extensions', those found from its files, and its own
    /// (`tools.json`'s), which win by name.
    fn tasks_for(&self, root: &Path) -> Result<Vec<fenix_tasks::TaskDef>, String> {
        // An extension's first; core's defaults only where they add a
        // name and a command line it doesn't have (#35).
        let mut tasks = fenix_tasks::merge_tasks(self.wasm.tasks(root), fenix_tasks::discover_tasks(root));
        let tools = self.project_tools(root)?;
        for (name, spec) in tools.tasks {
            tasks.retain(|task| task.name != name);
            tasks.push(fenix_tasks::TaskDef { name, command: spec.executable, args: spec.args });
        }
        Ok(tasks)
    }

    /// Overwrites the Task Output buffer's whole content with `header`
    /// (a fresh run's own `$ command args` line) and resets every pane
    /// currently showing it back to the top -- same shape as `DockerHost::
    /// set_buffer`, just without that one's per-line metadata (there
    /// is none for task output; see `content_highlights_for_visible_
    /// range`'s own `BufferKind::TaskOutput` handling, which is exactly
    /// "none" -- plain foreground text throughout).
    fn reset_task_output_buffer(&mut self, id: BufferId, header: &str) {
        if let Some(ob) = self.buffers.get_mut(id) {
            let end = ob.buffer.len_chars();
            let mut scratch_cursor = Cursor::at_start();
            ob.buffer.replace_range(&mut scratch_cursor, 0, end, header);
        }
        for pane in self.windows().windows() {
            if self.windows().content(pane) == Some(&id) {
                let ps = self.pane_state_mut(pane);
                *ps = PaneState::seeded_at(Cursor::at_start());
            }
        }
    }

    /// Uses the same supervisor for GUI and headless tests. Every event is tagged
    /// with a run identity because the output buffer is reused on rerun.
    fn start_task_process(&mut self, task: fenix_tasks::TaskDef, spec: fenix_project::tools::CommandSpec, root: PathBuf, buffer: BufferId) {
        let run_id = fenix_tasks::RunId::next();
        if let Some(session) = self.task_session.as_mut() { session.run_id = Some(run_id); session.started = Some(Instant::now()); }
        let command = spec.command(&root).map_err(|e| format!("couldn't start `{}`: {e}", task.command));
        let command = match command {
            Ok(command) => command,
            Err(error) => { self.apply_task_event(buffer, run_id, fenix_tasks::TaskEvent::Finished(fenix_tasks::TaskOutcome::Failed(error))); return; }
        };
        if let Some(session) = self.task_session.as_mut() { session.cwd = command.get_current_dir().unwrap_or(&root).to_path_buf(); }
        if let Some(proxy) = self.event_proxy.clone() {
            match fenix_tasks::TaskRunner::spawn_command(command, move |event| {
                proxy.send_event(FenixUserEvent::TaskEvent(buffer, run_id, event)).is_ok()
            }) {
                Ok(runner) => if let Some(session) = self.task_session.as_mut() { session.runner = Some(runner); },
                Err(err) => self.apply_task_event(buffer, run_id, fenix_tasks::TaskEvent::Finished(
                    fenix_tasks::TaskOutcome::Failed(format!("couldn't start `{}`: {err}", task.command)))),
            }
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let runner = fenix_tasks::TaskRunner::spawn_command(command, move |event| tx.send(event).is_ok());
        match runner {
            Ok(runner) => {
                let deadline = Instant::now() + Duration::from_secs(10);
                loop {
                    match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                        Ok(event) => {
                            let finished = matches!(event, fenix_tasks::TaskEvent::Finished(_));
                            self.apply_task_event(buffer, run_id, event);
                            if finished { break; }
                        }
                        Err(err) => {
                            runner.cancel();
                            self.apply_task_event(buffer, run_id, fenix_tasks::TaskEvent::Finished(
                                fenix_tasks::TaskOutcome::Failed(format!("headless task deadline: {err}"))));
                            break;
                        }
                    }
                }
            }
            Err(err) => self.apply_task_event(buffer, run_id, fenix_tasks::TaskEvent::Finished(
                fenix_tasks::TaskOutcome::Failed(format!("couldn't start `{}`: {err}", task.command)))),
        }
    }

    fn apply_task_event(&mut self, buffer: BufferId, run_id: fenix_tasks::RunId, event: fenix_tasks::TaskEvent) {
        // An extension's, beside the task pane (`run-task-background`).
        if self.wasm.background.contains_key(&buffer) {
            return self.background_task_event(buffer, run_id, event);
        }
        if !self.task_session.as_ref().is_some_and(|s| s.buffer == buffer && s.run_id == Some(run_id)) { return; }
        match event {
            fenix_tasks::TaskEvent::Output(lines) => {
                for line in lines { self.append_task_output_line(buffer, line.text); }
            }
            fenix_tasks::TaskEvent::Finished(outcome) => {
                match outcome {
                    fenix_tasks::TaskOutcome::Exited { success, .. } => self.finish_task_run(buffer, Some(success)),
                    fenix_tasks::TaskOutcome::Cancelled => {
                        self.append_task_output_text(buffer, "-- task cancelled --");
                        self.server_install_finished(false);
                    }
                    fenix_tasks::TaskOutcome::Failed(error) => {
                        self.append_task_output_text(buffer, &format!("-- task error: {error} --"));
                        self.set_error(format!("Task failed: {error}"));
                        self.server_install_finished(false);
                    }
                }
                if let Some(session) = self.task_session.as_mut() {
                    session.runner = None;
                    session.run_id = None;
                }
                // An agent waiting for it (`run_task`) hears how it went.
                self.agent_task_ended();
            }
        }
    }

    /// Task output handling: parses the line
    /// (`fenix_tasks::parse_line` -- see its own doc comment for why a
    /// cargo `--message-format=json` line decodes into a human-readable
    /// multi-line block plus a recovered location, a recognized-but-
    /// uninteresting cargo JSON line -- `compiler-artifact`, one per
    /// crate compiled -- into nothing at all, and a plain `gcc`/`clang`/
    /// `pytest`/`ctest`-style line is tried against the generic
    /// `file:line:col:` convention instead), appends the display text
    /// (skipped entirely when empty, rather than leaving a blank line
    /// behind for every suppressed cargo JSON message), and -- when a
    /// location was recovered -- feeds it into the generalized quickfix
    /// list (as a path resolved against this session's own `root`,
    /// matching `fenix_project::grep_project`'s own `root.join(...)`
    /// treatment of a tool's root-relative paths) so `SPC p n`/`SPC p N`
    /// can step through this run's diagnostics the same way a project
    /// grep or LSP references already do.
    fn append_task_output_line(&mut self, buffer_id: BufferId, line: String) {
        if self.task_session.as_ref().map(|s| s.buffer) != Some(buffer_id) {
            return;
        }
        let parsed = fenix_tasks::parse_line(&line);
        if !parsed.display.is_empty() {
            self.append_task_output_text(buffer_id, &parsed.display);
        }
        if let Some(loc) = parsed.location {
            let root = self.task_session.as_ref().map(|s| s.cwd.clone()).unwrap_or_default();
            let path = root.join(loc.path);
            self.quickfix.push(QuickfixEntry::Task(fenix_project::GrepMatch { path, line: loc.line, col: loc.col, text: loc.message }));
        }
    }

    /// Appends `text` (verbatim, split on any internal `\n` -- a decoded
    /// cargo compiler message's own `rendered` block is often several
    /// lines) to the Task Output buffer, auto-following the same way
    /// `DockerHost::append_log_line` already does for Docker's own live logs:
    /// any pane currently showing this buffer whose cursor was already
    /// on the last line before this arrived gets moved to the new last
    /// line after, so watching a live build stays pinned to the bottom
    /// unless the user has scrolled up to read something earlier.
    fn append_task_output_text(&mut self, buffer_id: BufferId, text: &str) {
        let panes_at_end: Vec<fenix_window::WindowId> = self
            .windows()
            .windows()
            .into_iter()
            .filter(|&pane| self.windows().content(pane) == Some(&buffer_id))
            .filter(|&pane| {
                let Some(ob) = self.buffers.get(buffer_id) else { return false };
                let (row, _) = ob.buffer.line_col(&self.pane_state(pane).cursor);
                row + 1 >= ob.buffer.visual_line_count()
            })
            .collect();

        let Some(ob) = self.buffers.get_mut(buffer_id) else { return };
        let mut append_cursor = Cursor { char_idx: ob.buffer.len_chars(), sticky_col: 0 };
        ob.buffer.insert_str(&mut append_cursor, &format!("{text}\n"));

        if let Some(ob) = self.buffers.get(buffer_id) {
            let last_line = ob.buffer.visual_line_count().saturating_sub(1);
            let end_char = ob.buffer.line_start_char(last_line);
            for pane in panes_at_end {
                self.pane_state_mut(pane).cursor = Cursor { char_idx: end_char, sticky_col: 0 };
            }
        }
    }

    /// Append the exit marker and release the cancellation handle. The worker
    /// has already drained output; releasing its handle never joins it.
    fn finish_task_run(&mut self, buffer_id: BufferId, success: Option<bool>) {
        if self.task_session.as_ref().map(|s| s.buffer) != Some(buffer_id) {
            return;
        }
        let marker = match success {
            Some(true) => "-- task succeeded --",
            Some(false) => "-- task failed --",
            None => "-- task ended (unknown status) --",
        };
        self.append_task_output_text(buffer_id, marker);
        if let Some(session) = self.task_session.as_mut() {
            session.runner = None;
        }
        // What `last-task` gives: its name, how it went, and its tests'
        // counts when the output says.
        let output = self.buffers.get(buffer_id).map(|ob| ob.buffer.text()).unwrap_or_default();
        self.wasm.last_task = Some(fenix_wasm::host::TaskResult {
            name: self.wasm.task_name.clone().unwrap_or_default(),
            ok: success == Some(true),
            tests: fenix_tasks::test_counts(&output),
        });
        self.server_install_finished(success == Some(true));
        let took = self.task_session.as_mut().and_then(|s| s.started.take()).map(|t| t.elapsed());
        self.task_notify(success == Some(true), took);
        if let Some(root) = self.task_session.as_ref().map(|s| s.root.clone()) {
            self.tell_extensions(Happened::TaskFinished { root: root.clone(), success: success == Some(true) });
            self.debug_task_ended(&root, success == Some(true));
        }
    }

    /// A debug session's adapter (`spec`) started and its panes opened:
    /// told to launch, or `attach`, with `launch_attributes` once it's
    /// initialized. `what` is the program, for its header.
    fn start_debug_session(&mut self, language: fenix_syntax::LanguageId, root: PathBuf, adapter: crate::dap::Adapter, launch_attributes: serde_json::Map<String, serde_json::Value>, attach: bool, what: String) {
        let (client, rx, port) = match adapter.connect(&root) {
            Ok(connected) => connected,
            Err(err) => { self.set_error(format!("couldn't start debug adapter {}: {err}", adapter.describe(adapter.port))); return; }
        };
        let generation = tool_sessions::generation();
        let reader = self.dap_reader(rx, generation);

        // Its dock, under the code you're in: no workspace of its own.
        self.debug_dock_open = true;
        let mut session = DebugSession {
            language,
            generation,
            launch_attributes,
            attach,
            name: what.clone(),
            adapter,
            port,
            children: Vec::new(),
            stopped_on: generation,
            ended: false,
            client,
            reader,
            pending: HashMap::new(),
            root,
            initialized: false,
            stopped_thread: None,
            last_thread: None,
            current_frame_id: None,
            call_stack: Vec::new(),
            frame_ids: Vec::new(),
            selected_frame: 0,
            variables: Vec::new(),
            toggled: Default::default(),
            console: Default::default(),
            watches: Vec::new(),
            watch_values: HashMap::new(),
            end: None,
            output_tail: Default::default(),
            client_closed: false,
            exception_filters: Vec::new(),
            pin_values: HashMap::new(),
            running_on: generation,
        };
        if let Ok(seq) = session.client.request_json("initialize", debug_initialize_arguments()) {
            session.pending.insert(seq, PendingDapRequest::Initialize);
        }
        self.debug_session = Some(session);
        let told = fenix_wasm::host::DebugSession { id: generation, name: what, language: crate::lsp::language_config_name(language), parent: None };
        self.tell_debug(fenix_wasm::host::DebugState::Started(told));
    }

    /// A child session the adapter asked for on connection `on`
    /// (`startDebugging`: js-debug runs each program in one): a connection
    /// of its own to the same adapter, initialized, then launched or
    /// attached with the `configuration` it gave.
    fn start_debug_child(&mut self, on: u64, arguments: serde_json::Value) {
        let Some(session) = self.debug_session.as_ref() else { return };
        let configuration = arguments.get("configuration").and_then(|c| c.as_object()).cloned().unwrap_or_default();
        let attach = arguments.get("request").and_then(|r| r.as_str()) == Some("attach");
        let name = configuration.get("name").and_then(|n| n.as_str()).filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| format!("{} (child)", session.name));
        let (root, language) = (session.root.clone(), session.language);
        let adapter = session.adapter.for_child(session.port);
        let (client, rx, _) = match adapter.connect(&root) {
            Ok(connected) => connected,
            Err(err) => return self.set_error(format!("couldn't start a child debug session {name}: {err}")),
        };
        let generation = tool_sessions::generation();
        let reader = self.dap_reader(rx, generation);
        let mut child = DebugChild { generation, parent: on, name: name.clone(), client, reader, pending: HashMap::new(), configuration, attach, initialized: false };
        if let Ok(seq) = child.client.request_json("initialize", debug_initialize_arguments()) {
            child.pending.insert(seq, PendingDapRequest::Initialize);
        }
        let Some(session) = self.debug_session.as_mut() else { return };
        session.children.push(child);
        debug_drawer::console_push(&mut session.console, format!("{name} started"));
        self.refresh_debug_panes();
        let told = fenix_wasm::host::DebugSession { id: generation, name, language: crate::lsp::language_config_name(language), parent: Some(on) };
        self.tell_debug(fenix_wasm::host::DebugState::Started(told));
    }

    /// Sends a stepping/continuation request (`continue`/`next`/
    /// `stepIn`/`stepOut`/`pause`) -- these don't get their own
    /// response tracked in `pending` (see `PendingDapRequest`'s own doc
    /// comment: the state change they cause arrives via a `stopped`/
    /// `continued` event instead), so this is a thin, un-tracked send
    /// shared by every one of them. It goes to the session that stopped:
    /// a child's, when that's where it is.
    fn send_debug_step(&mut self, req: fenix_dap::requests::Request) {
        if !self.debug_scope_matches() { return; }
        let Some(session) = self.debug_session.as_ref() else { return };
        if let Some(client) = session.client_of(session.stopped_on) {
            let _ = client.request(req);
        }
    }

    /// Sends `setBreakpoints` for `path` from `self.breakpoints`' current
    /// state to every connection past its `initialized` event -- always
    /// the *whole* file's breakpoint list, per DAP spec (one call replaces
    /// every breakpoint the adapter knows about for that source, there's
    /// no incremental add/remove request).
    pub(super) fn send_set_breakpoints_for(&mut self, path: &Path) {
        let Some(session) = self.debug_session.as_ref() else { return };
        let ready: Vec<u64> = std::iter::once((session.generation, session.initialized)).chain(session.children.iter().map(|c| (c.generation, c.initialized))).filter(|(_, ready)| *ready).map(|(on, _)| on).collect();
        for on in ready {
            self.send_set_breakpoints_to(on, path);
        }
    }

    /// `setBreakpoints` for `path` on connection `on` alone.
    fn send_set_breakpoints_to(&mut self, on: u64, path: &Path) {
        let Some(session) = self.debug_session.as_mut() else { return };
        if tool_sessions::root_for_path(path) != session.root { return; }
        let lines = self.breakpoints.get(path).cloned().unwrap_or_default();
        let run_to = self.debug_run_to.as_ref().filter(|(p, _)| p == path).map(|(_, line)| *line);
        let source = fenix_dap::types::Source::builder().path(Some(path.to_string_lossy().into_owned())).build();
        let breakpoints = debug_breakpoints::source_breakpoints(&lines, run_to);
        let args = fenix_dap::requests::SetBreakpointsRequestArguments::builder().source(source).breakpoints(breakpoints).build();
        session.send(on, fenix_dap::requests::Request::SetBreakpoints(args), PendingDapRequest::SetBreakpoints);
    }

    /// What became of a debug session, for the extensions that asked
    /// (`contributes.events` `debug`, `debug-changed`).
    fn tell_debug(&mut self, state: fenix_wasm::host::DebugState) {
        self.editor_call("debug", fenix_wasm::host::Call::DebugChanged(state));
    }

    /// Moves the cursor to `line` (1-indexed) in whichever pane(s)
    /// already show `path`'s buffer, without stealing focus from
    /// wherever it currently is; if `path` isn't open in any pane yet,
    /// opens and focuses it (`open_file_from_picker`), same "visit a
    /// file" semantics a quickfix jump already has. What conveys "this
    /// is the current line while stopped" -- see `DebugSession`'s own
    /// doc comment on why this app relies on the caret's own visibility
    /// rather than a separate persistent highlight for v1.
    fn goto_debug_location(&mut self, path: &Path, line: usize) {
        let Some(buffer_id) = self.buffers.id_for_path(path) else {
            self.open_file_as_preview(path);
            self.jump_to_grep_match(&fenix_project::GrepMatch { path: path.to_path_buf(), line, col: 1, text: String::new() });
            self.beacon_here();
            return;
        };
        // Shown in another workspace (the one the last stop opened): that
        // one is brought forward rather than a new one made at each stop.
        let here = self.windows().windows().into_iter().any(|pane| self.windows().content(pane) == Some(&buffer_id));
        if !here {
            if let Some((index, pane)) = self.workspaces.showing_elsewhere(buffer_id) {
                self.workspaces.switch_to_index(index);
                self.windows_mut().focus(pane);
            }
        }
        let mut shown_somewhere = false;
        for pane in self.windows().windows() {
            if self.windows().content(pane) == Some(&buffer_id) {
                shown_somewhere = true;
                if let Some(ob) = self.buffers.get(buffer_id) {
                    let target_line = line.saturating_sub(1).min(ob.buffer.visual_line_count().saturating_sub(1));
                    let char_idx = ob.buffer.line_start_char(target_line);
                    let (_, sticky) = ob.buffer.line_col(&Cursor { char_idx, sticky_col: 0 });
                    let ps = self.pane_state_mut(pane);
                    ps.cursor = Cursor { char_idx, sticky_col: sticky };
                }
            }
        }
        if !shown_somewhere {
            self.open_file_as_preview(path);
            self.jump_to_grep_match(&fenix_project::GrepMatch { path: path.to_path_buf(), line, col: 1, text: String::new() });
            self.beacon_here();
        } else {
            let prefs = self.motion();
            if prefs.beacon {
                self.start_beacon(buffer_id, line.saturating_sub(1), Instant::now(), prefs.beacon_d);
            }
        }
    }

    /// Re-renders all four Debug panel panes from `self.debug_session`/
    /// `self.breakpoints`' current state -- called after any state
    /// change rather than patching each pane individually; cheap enough
    /// (a handful of stack frames/variables/watches/breakpoints, not
    /// thousands) that rebuilding beats the bug surface of four
    /// separately-patched panes drifting out of sync with each other.
    fn refresh_debug_panes(&mut self) {
        // The values in the code, and the gutter's marks with them; the
        // dock draws from the session itself.
        self.refresh_debug_decorations();
    }

    /// `SPC l m`: opens (or, if already open, refreshes and refocuses)
    /// the single-pane tool status listing -- every `LanguageId` this
    /// app has a built-in LSP default or DAP default for (plus any
    /// `[lsp]`-configured override), whether that command is found on
    /// `PATH`, whether a session for it is running right now, and an
    /// install hint for anything missing. See `tool_status`'s own doc
    /// comment for why this is a synchronous, no-subprocess `PATH` scan
    /// rather than a background poller like `DockerSession`'s.
    ///
    /// `dap_running` only ever checks Python: `dap::default_adapter_
    /// command` is the one DAP adapter this app knows about at all right
    /// now (see its own doc comment), and `DebugSession` doesn't track
    /// which language it was launched for since there's never been a
    /// second one to distinguish from -- revisit this the day a second
    /// DAP adapter actually exists.
    pub(crate) fn open_tool_status_panel(&mut self) {
        let configured = self.config.lsp_servers.clone();
        let lsp_sessions: std::collections::HashSet<fenix_syntax::LanguageId> = self
            .lsp_sessions
            .keys()
            .filter(|key| key.root == self.integration_root())
            .filter_map(|key| match key.server { LspServer::Primary(language) => Some(language), LspServer::Additional(_) => None })
            .collect();
        let root = self.integration_root();
        let tools = match self.project_tools(&root) { Ok(tools) => tools, Err(error) => { self.set_error(error); return; } };
        let dap_language = self.debug_session.as_ref().filter(|s| s.root == root).map(|s| s.language);
        let configured_command = |spec: &fenix_project::tools::CommandSpec| {
            spec.command(&root).ok().map(|command| (command.get_program().to_string_lossy().into_owned(), spec.args.clone()))
        };
        // As the sessions resolve them: the project's own, then an
        // extension's, then Fenix's defaults (#43).
        let entries = crate::tool_status::scan(
            |language| {
                tools
                    .lsp
                    .get(&crate::lsp::language_config_name(language))
                    .and_then(configured_command)
                    .or_else(|| self.wasm.language_server(&root, language).as_ref().and_then(configured_command))
                    .or_else(|| crate::lsp::resolve_server_command(language, &configured))
            },
            |language| {
                tools
                    .dap
                    .get(&crate::lsp::language_config_name(language))
                    .and_then(configured_command)
                    .or_else(|| self.wasm.debug_adapter(&root, language).and_then(|a| a.command).as_ref().and_then(configured_command))
                    .or_else(|| crate::dap::default_adapter_command(language))
            },
            |language| lsp_sessions.contains(&language),
            |language| dap_language == Some(language),
        );
        let additional: Vec<crate::tool_status::AdditionalEntry> = tools
            .lsp
            .additional
            .iter()
            .map(|server| {
                let key = LspKey { server: LspServer::Additional(server.name.clone()), root: root.clone() };
                let command = configured_command(&server.command()).map(|(program, _)| program).unwrap_or_else(|| server.executable.clone());
                crate::tool_status::AdditionalEntry {
                    name: server.name.clone(),
                    found_on_path: crate::tool_status::is_on_path(&command),
                    command,
                    languages: server.languages.clone(),
                    running: self.lsp_sessions.contains_key(&key),
                    failed: self.lsp_unavailable.contains(&key),
                }
            })
            .collect();
        let mut failures: Vec<(String, String)> = self
            .lsp_failures
            .iter()
            .filter(|(key, _)| key.root == root && self.lsp_unavailable.contains(key))
            .map(|(key, why)| (self.lsp_server_name(key), why.clone()))
            .collect();
        failures.sort();
        let text = crate::tool_status::render(&entries) + &crate::tool_status::render_additional(&additional) + &crate::tool_status::render_failures(&failures);

        if let Some(session) = &self.tool_status_session {
            let (workspace_index, pane, buffer) = (session.workspace_index, session.pane, session.buffer);
            self.workspaces.switch_to_index(workspace_index);
            self.windows_mut().focus(pane);
            self.set_tool_status_buffer(buffer, &text);
        } else {
            let buffer = self.buffers.open_tool_status(&text);
            let cursor = Cursor::at_start();
            self.workspaces.new_workspace(buffer, cursor);
            let workspace_index = self.workspaces.active_index();
            let pane = self.focused_pane_id();
            self.pane_titles.insert(pane, "Tool Status".to_string());
            self.tool_status_session = Some(ToolStatusSession { workspace_index, pane, buffer });
        }
        self.wake_caret();
    }

    /// Same shape as `set_debug_buffer`/`DockerHost::set_buffer`.
    fn set_tool_status_buffer(&mut self, id: BufferId, text: &str) {
        if let Some(ob) = self.buffers.get_mut(id) {
            let end = ob.buffer.len_chars();
            let mut scratch_cursor = Cursor::at_start();
            ob.buffer.replace_range(&mut scratch_cursor, 0, end, text);
        }
        for pane in self.windows().windows() {
            if self.windows().content(pane) == Some(&id) {
                let ps = self.pane_state_mut(pane);
                *ps = PaneState::seeded_at(Cursor::at_start());
            }
        }
    }

    /// Sizes the git dashboard's left column so the section you're in
    /// gets room to actually work in, and the other five stay visible as
    /// a compact peek -- lazygit's own accordion behaviour, which this
    /// panel's layout was already modelled on.
    ///
    /// It replaces a cascade that sized sections by how late they were
    /// split off rather than by anything to do with their content: each
    /// `split` halves the pane it lands on, so stacking six of them left
    /// Staged with 42% of the column for its one `Nothing staged` line
    /// while Commits and Stash got ~5% each -- about one row apiece once
    /// the title bar is taken out, which is why a commit subject was
    /// always clipped.
    ///
    /// Reapplied only when focus actually moves between sections (see
    /// `GitSession::accordion_focus`), and skipped entirely while Main
    /// is focused, so reading a diff leaves the left column wherever you
    /// last had it.
    fn apply_git_accordion(&mut self) {
        let Some(session) = &self.git_session else { return };
        if self.workspaces.active_index() != session.workspace_index {
            return;
        }
        let stack = [
            session.status_pane,
            session.staged_pane,
            session.unstaged_pane,
            session.branches_pane,
            session.commits_pane,
            session.stash_pane,
        ];
        let already = session.accordion_focus;
        let focused = self.focused_pane_id();
        if !stack.contains(&focused) || already == Some(focused) {
            return;
        }

        // Bounded by the tree's own `[0.1, 0.9]` ratio clamp: the
        // outermost split's ratio *is* the first section's share of the
        // whole column, so no section can go below 10% without being
        // clamped back -- which would desynchronise every ratio under it
        // in the chain, since each is computed against what the ones
        // above left over.
        //
        // 0.45 rather than the 0.5 that floor nominally permits: at 0.5
        // the unfocused sections land on exactly 0.1, and summing six
        // f32 shares overshoots 1.0 just enough to push the first ratio
        // to 0.09999999 (the clamp test below catches this). Half a
        // section's worth of headroom costs ~2 rows on the focused pane
        // and makes the arithmetic robust. Still ~22 rows focused and
        // ~5 unfocused, against the 1 row Commits used to get.
        const FOCUSED_SHARE: f32 = 0.45;
        let other = (1.0 - FOCUSED_SHARE) / (stack.len() - 1) as f32;

        let shares: Vec<f32> = stack.iter().map(|p| if *p == focused { FOCUSED_SHARE } else { other }).collect();
        for (pane, ratio) in stack.iter().zip(stack_split_ratios(&shares)) {
            self.windows_mut().set_split_near(*pane, ratio);
        }
        if let Some(session) = &mut self.git_session {
            session.accordion_focus = Some(focused);
        }
    }

    /// Rewrites `id`'s buffer text from a freshly-rendered `GitPanel`,
    /// resetting every pane currently showing it back to the top --
    /// mirrors `DockerHost::set_buffer` exactly.
    /// Parses `diff` and renders it into `id` (a `BufferKind::Diff`
    /// buffer), caching both the per-line metadata and the parsed model
    /// so hunk actions can resolve the cursor's row back to a real hunk
    /// without re-shelling `git`. `None`/empty renders the view's own
    /// "(no changes)" placeholder rather than leaving stale content
    /// behind.
    ///
    /// Any fold state for files still present is carried across, so a
    /// refresh (or a hunk being staged out from under the view) doesn't
    /// silently unfold everything the user had collapsed -- same
    /// reasoning as `GitSession::staged_expanded_dirs` persisting across
    /// `git_refresh_session`.
    fn set_diff_buffer(&mut self, id: BufferId, diff: Option<&str>, source: DiffSource, repo_root: &Path) {
        self.set_diff_buffer_with_header(id, &[], diff, source, repo_root);
    }

    /// `set_diff_buffer`, with rows above the diff -- a commit's author,
    /// date and message, which `git show`'s own output carries but a
    /// diff parser has no reason to keep (see `diff_view::
    /// render_with_header`).
    fn set_diff_buffer_with_header(
        &mut self,
        id: BufferId,
        header: &[(String, diff_view::DiffStyle)],
        diff: Option<&str>,
        source: DiffSource,
        repo_root: &Path,
    ) {
        let files = diff.map(fenix_diff::parse).unwrap_or_default();
        // A read-only diff -- a commit, a stash, a comparison -- opens
        // with its files folded, so a change touching a dozen files
        // reads as a scannable list of what it touched rather than a
        // wall of hunks; `Tab` opens the one under the cursor. A
        // single-file diff has no list worth showing, so it stays open.
        // A working-tree diff also stays open: there you're staging
        // hunks, not surveying.
        let collapsed = if source == DiffSource::ReadOnly && files.len() > 1 {
            files.iter().map(|f| f.display_path().to_string()).collect()
        } else {
            self.diff_models.get(&id).map(|m| m.collapsed.clone()).unwrap_or_default()
        };
        let threads = self.diff_models.get(&id).map(|m| m.threads.clone()).unwrap_or_default();
        let view = diff_view::render_with_threads(header, &files, &collapsed, &threads);
        self.diff_lines.insert(id, view.lines);
        self.diff_models.insert(
            id,
            DiffModel {
                repo_root: repo_root.to_path_buf(), files, collapsed, source, header: header.to_vec(), threads },
        );
        if let Some(ob) = self.buffers.get_mut(id) {
            let end = ob.buffer.len_chars();
            let mut scratch_cursor = Cursor::at_start();
            ob.buffer.replace_range(&mut scratch_cursor, 0, end, &view.text);
        }
        for pane in self.windows().windows() {
            if self.windows().content(pane) == Some(&id) {
                let ps = self.pane_state_mut(pane);
                *ps = PaneState::seeded_at(Cursor::at_start());
            }
        }
    }

    /// Re-renders a diff buffer from its already-parsed model, keeping
    /// every pane's cursor on the same line index -- for fold toggles,
    /// where the content above the cursor is unchanged and resetting to
    /// the top would lose the user's place. Mirrors `set_git_buffer_
    /// preserving_line`'s own reasoning for the Files tree.
    fn rerender_diff_buffer(&mut self, id: BufferId) {
        let Some(model) = self.diff_models.get(&id) else { return };
        let view = diff_view::render_with_threads(&model.header, &model.files, &model.collapsed, &model.threads);
        self.diff_lines.insert(id, view.lines);

        let mut old_lines: Vec<(fenix_window::WindowId, usize)> = Vec::new();
        for pane in self.windows().windows() {
            if self.windows().content(pane) == Some(&id) {
                let ps = self.pane_state(pane);
                if let Some(ob) = self.buffers.get(id) {
                    old_lines.push((pane, ob.buffer.line_col(&ps.cursor).0));
                }
            }
        }
        if let Some(ob) = self.buffers.get_mut(id) {
            let end = ob.buffer.len_chars();
            let mut scratch_cursor = Cursor::at_start();
            ob.buffer.replace_range(&mut scratch_cursor, 0, end, &view.text);
        }
        let line_count = self.buffers.get(id).map(|ob| ob.buffer.visual_line_count()).unwrap_or(1);
        for (pane, old_line) in old_lines {
            let line = old_line.min(line_count.saturating_sub(1));
            let Some(ob) = self.buffers.get(id) else { continue };
            let char_idx = ob.buffer.line_start_char(line);
            self.pane_state_mut(pane).cursor = Cursor { char_idx, sticky_col: 0 };
        }
    }

    /// The diff buffer the focused pane is showing, if it is one.
    fn focused_diff_buffer(&self) -> Option<BufferId> {
        let id = self.focused_buffer_id();
        self.diff_models.contains_key(&id).then_some(id)
    }

    /// What the cursor's current row in a diff buffer points at -- the
    /// one lookup every hunk-level action goes through. `None` when the
    /// focused pane isn't a diff, or the row carries no anchor (a
    /// placeholder row).
    fn diff_anchor_at_cursor(&self) -> Option<(BufferId, diff_view::DiffAnchor)> {
        let id = self.focused_diff_buffer()?;
        let line = self.open().buffer.line_col(&self.cursor()).0;
        let anchor = self.diff_lines.get(&id)?.get(line)?.as_ref()?.anchor?;
        Some((id, anchor))
    }

    /// The patch for the one hunk `anchor` names, rebuilt from the same
    /// cached parse the user is looking at. `None` if the anchor points
    /// at a file header rather than a hunk, or if the buffer/file/hunk
    /// is gone because a refresh landed in between -- both of which mean
    /// "don't apply anything," never "apply something else."
    fn hunk_patch_for(&self, buffer: BufferId, anchor: diff_view::DiffAnchor) -> Option<String> {
        let model = self.diff_models.get(&buffer)?;
        let file = model.files.get(anchor.file)?;
        let hunk = file.hunks.get(anchor.hunk?)?;
        Some(fenix_diff::hunk_patch(file, hunk))
    }

    /// `s`/`S`/`d` on a diff row: stages, unstages, or discards the hunk
    /// the cursor is inside. Rebuilds that hunk's patch from the same
    /// parse the user is looking at (`fenix_diff::hunk_patch`) and pipes
    /// it through `git apply` -- see `fenix_git::apply_patch`.
    ///
    /// `target` has to agree with which diff this is: an unstaged hunk
    /// can be staged or discarded, a staged one can only be unstaged.
    /// Asking for the wrong one is a message, not a no-op, since from
    /// the user's side "s did nothing" is indistinguishable from a bug.
    fn diff_apply_hunk(&mut self, target: fenix_git::ApplyTarget) {
        let Some((buffer, anchor)) = self.diff_anchor_at_cursor() else {
            self.set_error("no diff hunk under the cursor".to_string());
            return;
        };
        if anchor.hunk.is_none() {
            self.set_error("put the cursor inside a hunk, not on the file header".to_string());
            return;
        }
        let Some(model) = self.diff_models.get(&buffer) else { return };
        // A conflicted file's diff is a *combined* one, comparing the
        // working tree against both sides at once. There's no combined
        // patch format to feed back to `git apply`, so a hunk from one
        // can't be staged at all -- resolving the conflict is what
        // stages it.
        if model.files.get(anchor.file).is_some_and(|f| f.is_combined) {
            self.set_error("this file is conflicted -- resolve it (SPC g x o/t/b) rather than staging a hunk".to_string());
            return;
        }
        let allowed = matches!(
            (model.source, target),
            (DiffSource::Unstaged, fenix_git::ApplyTarget::Stage | fenix_git::ApplyTarget::Discard) | (DiffSource::Staged, fenix_git::ApplyTarget::Unstage)
        );
        if !allowed {
            let why = match model.source {
                DiffSource::Unstaged => "this hunk isn't staged yet -- `s` stages it, `d` discards it",
                DiffSource::Staged => "this hunk is staged -- `S` unstages it",
                DiffSource::ReadOnly => "this diff isn't editable (a commit, a stash, or a comparison)",
            };
            self.set_error(why.to_string());
            return;
        }
        let repo_root = model.repo_root.clone();
        let Some(patch) = self.hunk_patch_for(buffer, anchor) else {
            self.set_error("that hunk is no longer in the diff -- refresh with `u`".to_string());
            return;
        };

        match fenix_git::apply_patch(&repo_root, &patch, target) {
            Ok(_) => {
                let what = match target {
                    fenix_git::ApplyTarget::Stage => "staged",
                    fenix_git::ApplyTarget::Unstage => "unstaged",
                    fenix_git::ApplyTarget::Discard => "discarded",
                };
                self.set_message(format!("hunk {what}"));
                self.git_refresh_session();
            }
            // git's own "error: patch does not apply" is far more
            // useful than anything this could paraphrase.
            Err(err) => self.set_error(err.trim().to_string()),
        }
    }

    /// `d` on a diff row: asks to discard the hunk under the cursor.
    ///
    /// Discarding is gated behind the panel's confirm-then-act step, but
    /// only a working-tree diff can be discarded at all -- a commit or a
    /// comparison says so rather than arming a confirmation that would
    /// then decline, which would make `y` look broken.
    fn diff_discard_request(&mut self) {
        if self.focused_diff_is_read_only() {
            self.set_error("this diff isn't editable (a commit, a stash, or a comparison)".to_string());
        } else {
            self.git_arm_confirm();
        }
    }

    /// `Tab` on a diff row: folds the file the cursor is in down to its
    /// header row, or unfolds it again.
    fn diff_toggle_fold(&mut self) {
        let Some((buffer, anchor)) = self.diff_anchor_at_cursor() else { return };
        let Some(model) = self.diff_models.get_mut(&buffer) else { return };
        let Some(path) = model.files.get(anchor.file).map(|f| f.display_path().to_string()) else { return };
        if !model.collapsed.remove(&path) {
            model.collapsed.insert(path);
        }
        self.rerender_diff_buffer(buffer);
        self.wake_caret();
    }

    /// `]c`/`[c` on a diff: moves the cursor to the next/previous hunk
    /// header, the same motion pair Vim's own diff mode uses for exactly
    /// this. Stops at the ends rather than wrapping -- a wrap would make
    /// "am I at the last change" impossible to feel.
    fn diff_jump_hunk(&mut self, forward: bool) {
        let Some(buffer) = self.focused_diff_buffer() else { return };
        let Some(lines) = self.diff_lines.get(&buffer) else { return };
        let current = self.open().buffer.line_col(&self.cursor()).0;
        let is_hunk = |i: usize| lines.get(i).and_then(|l| l.as_ref()).is_some_and(|l| l.style == diff_view::DiffStyle::HunkHeader);
        let target = if forward {
            (current + 1..lines.len()).find(|&i| is_hunk(i))
        } else {
            (0..current).rev().find(|&i| is_hunk(i))
        };
        let Some(target) = target else {
            self.set_message(if forward { "no next hunk" } else { "no previous hunk" });
            return;
        };
        let char_idx = self.open().buffer.line_start_char(target);
        let pane = self.focused_pane_id();
        self.pane_state_mut(pane).cursor = Cursor { char_idx, sticky_col: 0 };
        self.wake_caret();
    }

    /// `Enter` on a diff row: opens the real file at the line under the
    /// cursor. Prefers the new-side line number (where the change landed
    /// in the file as it is now); a removed line has only an old-side
    /// number, which still lands in the right neighbourhood, which is
    /// what someone reading a diff actually wants.
    fn diff_open_file_at_cursor(&mut self) {
        let Some((buffer, anchor)) = self.diff_anchor_at_cursor() else { return };
        let Some(model) = self.diff_models.get(&buffer) else { return };
        let Some(file) = model.files.get(anchor.file) else { return };
        let path = model.repo_root.join(file.display_path());
        if !path.is_file() {
            self.set_error(format!("{} isn't a file on disk", file.display_path()));
            return;
        }
        let line = anchor.new_line.or(anchor.old_line).unwrap_or(1);
        self.open_file_from_picker(&path);
        self.jump_to_grep_match(&fenix_project::GrepMatch { path, line, col: 1, text: String::new() });
    }

    fn set_git_buffer(&mut self, id: BufferId, panel: git_panel::GitPanel) {
        self.git_lines.insert(id, panel.lines);
        if let Some(ob) = self.buffers.get_mut(id) {
            let end = ob.buffer.len_chars();
            let mut scratch_cursor = Cursor::at_start();
            ob.buffer.replace_range(&mut scratch_cursor, 0, end, &panel.text);
        }
        for pane in self.windows().windows() {
            if self.windows().content(pane) == Some(&id) {
                let ps = self.pane_state_mut(pane);
                *ps = PaneState::seeded_at(Cursor::at_start());
            }
        }
    }

    /// Like `set_git_buffer` but keeps every pane's cursor on the same
    /// *line index* instead of resetting to the top. Right for `Tab`'s
    /// directory expand/collapse specifically: inserting or removing a
    /// directory's children only ever changes lines *below* the row the
    /// cursor is already sitting on (the toggled directory itself), so
    /// "same line index" lands back on that exact row every time. Not
    /// used for `git_refresh_session`'s own re-renders, which
    /// intentionally reset to the top like every other refresh in this
    /// session -- a real repo-status refresh can reorder or remove rows
    /// anywhere in the list, where this shortcut wouldn't hold.
    fn set_git_buffer_preserving_line(&mut self, id: BufferId, panel: git_panel::GitPanel) {
        self.git_lines.insert(id, panel.lines);
        let mut old_lines: Vec<(fenix_window::WindowId, usize)> = Vec::new();
        for pane in self.windows().windows() {
            if self.windows().content(pane) == Some(&id) {
                let ps = self.pane_state(pane);
                if let Some(ob) = self.buffers.get(id) {
                    let (line, _) = ob.buffer.line_col(&ps.cursor);
                    old_lines.push((pane, line));
                }
            }
        }
        if let Some(ob) = self.buffers.get_mut(id) {
            let end = ob.buffer.len_chars();
            let mut scratch_cursor = Cursor::at_start();
            ob.buffer.replace_range(&mut scratch_cursor, 0, end, &panel.text);
        }
        let new_line_count = self.buffers.get(id).map(|ob| ob.buffer.visual_line_count()).unwrap_or(1);
        for (pane, old_line) in old_lines {
            let clamped = old_line.min(new_line_count.saturating_sub(1));
            let Some(ob) = self.buffers.get(id) else { continue };
            let char_idx = ob.buffer.line_start_char(clamped);
            self.pane_state_mut(pane).cursor = Cursor { char_idx, sticky_col: 0 };
        }
    }

    /// `FenixUserEvent::GitStatusReady` handling: caches the fresh status
    /// and re-renders the Status pane -- mirrors `DockerHost::apply_stats`,
    /// including its "reset to top is harmless, Status is a small read-
    /// only info pane" reasoning.
    /// Whether it changed: the poller asks every couple of seconds, and
    /// the same status isn't written out and drawn again (#74).
    fn apply_git_status(&mut self, status: Option<fenix_git::RepoStatus>) -> bool {
        let Some(session) = self.git_session.as_mut() else { return false };
        if session.status == status {
            return false;
        }
        session.status = status.clone();
        let (staged, unstaged, untracked) = file_counts(&session.files);
        let (status_buffer, conflicts) = (session.status_buffer, conflict_count(&session.files));
        let in_progress = session.in_progress.clone();
        let sides = session.sides.clone();
        self.set_git_buffer(
            status_buffer,
            git_panel::render_status(status.as_ref(), staged, unstaged, untracked, in_progress.as_ref(), conflicts, sides.as_ref()),
        );
        true
    }

    /// `FenixUserEvent::GitRefreshReady` handling: applies a completed
    /// background session refresh (re-renders all six list/status panes,
    /// resets `last_main_entry` and re-syncs Main -- a manual refresh
    /// means the underlying content may have changed even when the
    /// selected path/hash/index hasn't, see `GitSession::last_main_
    /// entry`'s own doc comment), unless a newer refresh has since
    /// superseded it (`request_id` mismatch) or the session's since
    /// closed.
    fn apply_git_refresh(&mut self, request_id: u64, data: GitRefreshData) {
        let Some(session) = self.git_session.as_ref() else { return };
        if session.refresh_request_id != request_id {
            return;
        }
        let (status_buffer, staged_buffer, unstaged_buffer, branches_buffer, commits_buffer, stash_buffer) = (
            session.status_buffer,
            session.staged_buffer,
            session.unstaged_buffer,
            session.branches_buffer,
            session.commits_buffer,
            session.stash_buffer,
        );
        let (staged_expanded, unstaged_expanded) = (session.staged_expanded_dirs.clone(), session.unstaged_expanded_dirs.clone());

        let (staged, unstaged, untracked) = file_counts(&data.files);
        let status_panel = git_panel::render_status(
            data.status.as_ref(),
            staged,
            unstaged,
            untracked,
            data.in_progress.as_ref(),
            conflict_count(&data.files),
            data.sides.as_ref(),
        );
        let staged_panel = git_panel::render_staged(&data.files, &staged_expanded);
        let unstaged_panel = git_panel::render_unstaged(&data.files, &unstaged_expanded);
        let branches_panel = git_panel::render_branches(&data.branches);
        let commits_panel = git_panel::render_commits(&data.commits);
        let stash_panel = git_panel::render_stash(&data.stashes);

        let Some(session) = self.git_session.as_mut() else { return };
        session.status = data.status;
        session.files = data.files;
        session.branches = data.branches;
        session.commits = data.commits;
        session.stashes = data.stashes;
        session.in_progress = data.in_progress;
        session.sides = data.sides;
        session.last_main_entry = None;

        self.set_git_buffer(status_buffer, status_panel);
        self.set_git_buffer(staged_buffer, staged_panel);
        self.set_git_buffer(unstaged_buffer, unstaged_panel);
        self.set_git_buffer(branches_buffer, branches_panel);
        self.set_git_buffer(commits_buffer, commits_panel);
        self.set_git_buffer(stash_buffer, stash_panel);
        self.git_sync_main();
    }

    /// `FenixUserEvent::GitMainReady` handling: applies a completed
    /// background diff fetch, unless a newer request has since
    /// superseded it (`request_id` mismatch -- see `GitSession::main_
    /// request_id`'s own doc comment) or the session's since closed.
    fn apply_git_main(&mut self, request_id: u64, buffer: BufferId, diff: Option<String>) {
        let Some(session) = self.git_session.as_ref() else { return };
        if session.main_request_id != request_id {
            return;
        }
        let (source, repo_root) = (DiffSource::of(&session.last_main_fetch), session.repo_root.clone());
        self.set_diff_buffer(buffer, diff.as_deref(), source, &repo_root);
    }

    /// Runs one git operation and reports what happened.
    ///
    /// A conflict is *not* a failure to recover from: `rebase`/`merge`/
    /// `cherry-pick` all exit non-zero when they stop for one, having
    /// done exactly what they were asked. So a non-zero exit that leaves
    /// an operation in progress reads as "stopped, here's what to do",
    /// while one that doesn't is a real error.
    fn run_git_operation(&mut self, what: &str, result: Result<String, String>) {
        let repo_root = self.git_action_repo_root();
        match result {
            Ok(_) => self.set_message(format!("{what} finished")),
            Err(err) => match fenix_git::in_progress(&repo_root) {
                Some(op) => self.set_error(format!("{} -- resolve the conflicts, then SPC g x c to continue", op.label())),
                None => self.set_error(format!("{what} failed: {}", err.trim().lines().next().unwrap_or("").trim())),
            },
        }
        self.git_refresh_all_views();
    }

    /// Re-reads every open file whose contents changed on disk.
    ///
    /// A rebase, merge, checkout or discard rewrites working-tree files
    /// underneath whatever is already open. Without this, a buffer keeps
    /// showing what the file said before the operation -- and during a
    /// conflicted rebase that's actively dangerous, because saving that
    /// stale buffer writes the *pre-rebase* content back over git's
    /// conflict markers and silently destroys the merge state.
    ///
    /// A buffer with unsaved edits is never touched: those changes are
    /// the user's, and reloading would throw them away. They're counted
    /// and reported instead, so a file that didn't refresh says so
    /// rather than quietly lying about what's on disk.
    fn reload_buffers_changed_on_disk(&mut self) -> DiskSweep {
        // Buffers close from a dozen places; pruning here rather than
        // hooking every one of them keeps the bookkeeping in the one
        // place that already walks the list, and is self-healing if a
        // new close path is added later.
        self.sweep_buffers(None)
    }

    /// The sweep, from what the disk probe saw. A buffer whose file was
    /// saved or reloaded while the probe was out is left for the next
    /// one: what the probe saw of it is older than what Fenix knows.
    pub(super) fn reload_buffers_seen(&mut self, seen: &HashMap<BufferId, Option<DiskFingerprint>>) -> DiskSweep {
        let basis = std::mem::take(&mut self.disk_probe_basis);
        let current: HashMap<BufferId, Option<DiskFingerprint>> =
            seen.iter().filter(|(id, _)| self.disk_state.get(id) == basis.get(id)).map(|(id, f)| (*id, *f)).collect();
        self.sweep_buffers(Some(&current))
    }

    /// Every open file checked: stat'd here (`seen` `None`), or from
    /// what a probe saw -- a buffer it didn't look at is skipped.
    fn sweep_buffers(&mut self, seen: Option<&HashMap<BufferId, Option<DiskFingerprint>>>) -> DiskSweep {
        self.disk_state.retain(|id, _| self.buffers.get(*id).is_some());
        self.externally_changed.retain(|id| self.buffers.get(*id).is_some());

        let mut sweep = DiskSweep::default();
        for id in self.buffers.ids_sorted_by_path() {
            match seen {
                None => self.check_one_file(id, &mut sweep, true, None),
                Some(seen) => {
                    if let Some(fingerprint) = seen.get(&id) {
                        self.check_one_file(id, &mut sweep, true, Some(*fingerprint));
                    }
                }
            }
        }
        sweep
    }

    /// One buffer's file, checked and acted on. The whole sweep in
    /// miniature, split out so the save guard can re-check exactly one
    /// file without walking every open buffer.
    /// `trust_fingerprint` skips the read when mtime and length are
    /// unchanged. The periodic sweep sets it, because it runs on every
    /// open file every couple of seconds and a stat is far cheaper than
    /// a read. The save guard clears it, because the fingerprint can
    /// miss a same-length edit made inside the filesystem's timestamp
    /// granularity -- rare, but a write is the one moment where being
    /// approximately right means losing somebody's work.
    /// `seen`: the file's fingerprint as the disk probe saw it, instead
    /// of a stat here.
    fn check_one_file(&mut self, id: BufferId, sweep: &mut DiskSweep, trust_fingerprint: bool, seen: Option<Option<DiskFingerprint>>) {
        let Some(ob) = self.buffers.get(id) else { return };
        // Generated panels (Git, Docker, diffs, the graph) have no file
        // behind them, and the host rewrites their contents constantly.
        if !ob.kind.tracks_unsaved_changes() {
            return;
        }
        let Some(path) = ob.buffer.path().map(Path::to_path_buf) else { return };
        let name = self.buffer_display_name(id);

        let fingerprint = match seen {
            Some(seen) => seen,
            None => match DiskFingerprint::look(&path) {
                Some(looked) => looked,
                // Its machine isn't answering: nothing is known, and
                // nothing is concluded.
                None => return,
            },
        };
        let Some(fingerprint) = fingerprint else {
            // Gone, or no longer a regular file. Never reloaded to
            // empty: an editor that blanks your buffer because
            // something deleted the file for a moment -- which is
            // exactly what a "safe write" in another editor looks like
            // -- is worse than one that says nothing happened.
            if self.disk_state.remove(&id).is_some() {
                self.externally_changed.insert(id);
                sweep.vanished.push(name);
            }
            return;
        };
        // The fast path: nothing about the file has moved since we last
        // looked, so there is nothing to read.
        let previously_seen = self.disk_state.get(&id).copied();
        if trust_fingerprint && previously_seen == Some(fingerprint) {
            return;
        }

        let Some(Ok((on_disk, encoding))) = self.buffers.get(id).map(|ob| ob.buffer.read_disk_text()) else {
            // Stat'd fine but unreadable (a permission change). Not a
            // change we can act on.
            return;
        };
        let Some(ob) = self.buffers.get(id) else { return };
        if on_disk == ob.buffer.text() {
            // The file moved but its content didn't -- `touch`, or a
            // tool that rewrote it identically. Record the new
            // fingerprint so this doesn't re-read every two seconds,
            // and say nothing: nothing happened.
            self.disk_state.insert(id, fingerprint);
            return;
        }

        if ob.buffer.is_dirty() {
            if previously_seen.is_none() || previously_seen == Some(fingerprint) {
                // Either this file has never been looked at, or it
                // hasn't moved since it was -- so the difference is the
                // buffer's own unsaved edits, which is what "dirty"
                // means, and no evidence at all that anybody else
                // touched it. Recording the fingerprint is all that can
                // honestly be done; a real change from here on moves it.
                self.disk_state.insert(id, fingerprint);
                return;
            }
            // Both sides changed since the last look. Neither can be
            // thrown away without being asked, so the buffer is left
            // exactly as it is and the next save refuses -- see `save`.
            self.externally_changed.insert(id);
            sweep.conflicted.push(name);
            return;
        }

        self.replace_buffer_from_disk(id, &on_disk);
        if let Some(ob) = self.buffers.get_mut(id) {
            ob.buffer.set_encoding(encoding);
        }
        self.disk_state.insert(id, fingerprint);
        self.externally_changed.remove(&id);
        // Its test results are older than the file now, as after a save.
        self.tests_file_changed_on_disk(&path);
        sweep.reloaded.push(name);
    }

    /// Swaps a buffer's text for what's on disk, keeping every pane that
    /// shows it on the same line.
    ///
    /// Every pane showing it, in every workspace and every frame -- not
    /// only the visible ones: a pane parked in another workspace keeps
    /// its cursor too, and would hand it back the moment that workspace
    /// is switched to. Each pane's line is read *before* the text is
    /// swapped: its cursor is a char offset into the old text, and asking
    /// the new, possibly much shorter text where that offset falls is
    /// exactly how a coding agent rewriting the focused file used to
    /// crash the editor (char 20442 of a 37-char file).
    fn replace_buffer_from_disk(&mut self, id: BufferId, on_disk: &str) {
        let Some(ob) = self.buffers.get_mut(id) else { return };

        let mut states: Vec<&mut PaneState> = Vec::new();
        let parked = self.frames.iter_mut().flatten().map(|frame| &mut frame.workspaces);
        for list in std::iter::once(&mut self.workspaces).chain(parked) {
            for Workspace { windows, pane_states, .. } in &mut list.workspaces {
                states.extend(pane_states.iter_mut().filter(|(pane, _)| windows.content(**pane) == Some(&id)).map(|(_, state)| state));
            }
        }
        let old_len = ob.buffer.len_chars();
        let old_lines: Vec<usize> = states
            .iter()
            .map(|state| ob.buffer.line_col(&Cursor { char_idx: state.cursor.char_idx.min(old_len), sticky_col: 0 }).0)
            .collect();

        // Only the lines that changed are replaced, so what's anchored
        // in the rest (extra cursors, an extension's decorations) stays
        // where it was.
        let mut scratch = Cursor::at_start();
        replace_text_by_lines(&mut ob.buffer, &mut scratch, on_disk);
        // A reload isn't user work: leaving it dirty would make every
        // touched file look unsaved and block `:qa`.
        ob.buffer.mark_saved();

        // The file changed, but where you were looking in it is still the
        // best guess at where you want to be -- clamped to the new length.
        let last_line = ob.buffer.visual_line_count().saturating_sub(1);
        for (state, old_line) in states.into_iter().zip(old_lines) {
            let line = old_line.min(last_line);
            state.cursor = Cursor { char_idx: ob.buffer.line_start_char(line), sticky_col: 0 };
            // A scroll position below the new cursor would open on the
            // last line alone with everything above it out of view; from
            // the top, following the cursor scrolls only as far as needed.
            // Snapped rather than eased, so no frame animates through
            // lines the file no longer has.
            if state.scroll_line > line {
                state.scroll_line = 0;
                state.scroll_row = 0;
                state.rendered_scroll = 0.0;
            }
        }

        // A Visual selection's anchor is another offset into the old
        // text, with no line to carry over that would still mean the same
        // selection -- dropping it is the honest answer.
        if self.focused_buffer_id() == id && self.vim.mode() == Mode::Visual {
            let cursor = self.cursor();
            self.vim.exit_visual_mode(&cursor);
        }
    }

    /// Records a buffer's file as Fenix last left it -- called after
    /// every read and every write, so the sweep's fast path stays
    /// accurate and a save Fenix performed itself never looks like
    /// somebody else's change.
    fn note_disk_state(&mut self, id: BufferId) {
        let Some(path) = self.buffers.get(id).and_then(|ob| ob.buffer.path()).map(Path::to_path_buf) else { return };
        match DiskFingerprint::look(&path) {
            Some(Some(fingerprint)) => {
                self.disk_state.insert(id, fingerprint);
            }
            Some(None) => {
                self.disk_state.remove(&id);
            }
            // Not looked at: what was known stands.
            None => return,
        }
        self.externally_changed.remove(&id);
    }

    /// The periodic sweep, plus whatever it found, reported once.
    ///
    /// Called from the event loop's own wait deadline and whenever the
    /// window regains focus. Both are needed: focus catches editing in
    /// another application, and the timer catches Fenix's own terminal
    /// panel, which never takes focus away from the window at all.
    /// Tells each Python language server about its environment again
    /// when it has changed since the last time: `uv add`/`uv sync` (from
    /// the terminal, a task, the wizard or the doctor's fix) installing
    /// or removing a package, or the venv being created after the server
    /// started. pyright doesn't watch `site-packages`, and a new
    /// `workspace/didChangeConfiguration` is what makes it look again.
    fn refresh_python_environments(&mut self) {
        for (key, session) in &mut self.lsp_sessions {
            if !key.is_primary_for(fenix_syntax::LanguageId::Python) || session.capabilities.is_none() {
                continue;
            }
            if session.python_env.as_ref().is_some_and(|last| last.is_stale(&session.root)) {
                push_python_environment(session);
            }
        }
    }

    pub(crate) fn poll_files_changed_on_disk(&mut self) {
        let _profile = crate::profile::Scope::new("poll disk");
        // Advance the maintenance deadline even when watching is disabled.
        // Recovery protects edits independently of external-change detection.
        self.next_disk_check = Instant::now() + DISK_CHECK_INTERVAL;
        // Before the sweep, not after: a sweep that reloads a buffer
        // makes it clean, and snapshotting afterwards would then throw
        // away a snapshot that had never been written.
        self.snapshot_dirty_buffers();
        self.checkpoint_session();
        self.tick_home();
        self.refresh_python_environments();
        self.refresh_git_pages(true);
        self.reload_settings_if_changed();
        self.extensions_installed_if_changed();
        // The files themselves -- open buffers, PDFs, the MIBs, the
        // project's settings -- are looked at off the UI thread.
        self.start_disk_probe();
    }

    /// Writes every dirty buffer that has changed since its last
    /// snapshot, and clears the snapshot of every buffer that no longer
    /// needs one.
    ///
    /// Runs on the same tick as the disk sweep -- so at most a couple of
    /// seconds of typing is ever at risk, and a buffer nobody is
    /// touching costs one integer comparison. Keyed on `edit_count`
    /// rather than on the text, because comparing whole buffers every
    /// two seconds to decide whether to write them would cost more than
    /// writing them.
    fn snapshot_dirty_buffers(&mut self) {
        let Some(dir) = self.recovery_dir.clone() else { return };
        for id in self.buffers.ids_sorted_by_path() {
            let Some(ob) = self.buffers.get(id) else { continue };
            if !ob.kind.tracks_unsaved_changes() {
                continue;
            }
            let path = ob.buffer.path().map(Path::to_path_buf);
            if !ob.buffer.is_dirty() {
                // Saved or reverted since the last tick -- the snapshot
                // is now describing work that isn't lost, and leaving it
                // would offer to "recover" it on the next start.
                if self.snapshot_state.contains_key(&id) || self.unnamed_snapshots.contains_key(&id) || self.recovery_failures.contains_key(&id) {
                    self.discard_recovery_for(id);
                }
                continue;
            }
            let edits = ob.buffer.edit_count();
            if self.snapshot_state.get(&id) == Some(&edits) {
                continue;
            }
            let contents = ob.buffer.text();
            let result = if let Some(path) = &path {
                fenix_recovery::write(&dir, path, &contents).map(|_| ())
            } else {
                let target = self.unnamed_snapshots.entry(id).or_insert_with(|| fenix_recovery::unnamed_path(&dir));
                fenix_recovery::write_unnamed(target, &contents)
            };
            match result {
                Ok(_) => {
                    self.snapshot_state.insert(id, edits);
                    self.recovery_failures.remove(&id);
                }
                Err(err) => {
                    self.recovery_failures.insert(id, err.to_string());
                    self.set_error(format!("Recovery failed: {err}. Save your work to a writable location."));
                }
            }
        }
    }

    /// Drops the recovery snapshot for a buffer whose work is no longer
    /// at risk -- it was saved, or deliberately thrown away.
    fn discard_recovery_for(&mut self, id: BufferId) {
        self.snapshot_state.remove(&id);
        self.recovery_failures.remove(&id);
        if let Some(target) = self.unnamed_snapshots.remove(&id) {
            let _ = std::fs::remove_file(target);
        }
        let Some(dir) = self.recovery_dir.clone() else { return };
        let Some(path) = self.buffers.get(id).and_then(|ob| ob.buffer.path()).map(Path::to_path_buf) else { return };
        let _ = fenix_recovery::discard(&dir, &path);
    }

    /// Says once, at startup, that a previous session left work behind.
    ///
    /// A message rather than a prompt: a modal "recover?" on launch is
    /// the thing everyone hates about swap files, and it fires on every
    /// start until it's answered. This says what's there and where the
    /// key is, and gets out of the way. Old snapshots are pruned first,
    /// so a crash from months ago isn't still being announced.
    fn announce_recoverable_work(&mut self) {
        let Some(dir) = self.recovery_dir.clone() else { return };
        fenix_recovery::prune(&dir, RECOVERY_MAX_AGE);
        let names = self.unclaimed_snapshot_names();
        if names.is_empty() { return; }
        let message = format!("unsaved work from a previous session: {} -- SPC f v recovers it", join_names(&names));
        if let Some(previous) = self.status_message.as_ref().filter(|message| message.is_error) {
            self.set_error(format!("{}; {message}", previous.text));
        } else { self.set_message(message); }
    }

    /// Snapshots a previous session left that no open buffer already
    /// holds unchanged -- the work `SPC f v` would actually give back.
    pub(super) fn unclaimed_snapshot_names(&self) -> Vec<String> {
        let Some(dir) = self.recovery_dir.as_deref() else { return Vec::new() };
        fenix_recovery::list(dir)
            .iter()
            .filter(|snapshot| {
                let owned = snapshot.original.as_ref().and_then(|path| self.buffer_id_for_lsp_path(path))
                    .or_else(|| self.unnamed_snapshots.iter().find_map(|(id, path)| (*path == snapshot.snapshot).then_some(*id)));
                !owned.and_then(|id| self.buffers.get(id)).is_some_and(|ob| ob.buffer.is_dirty() && ob.buffer.text() == snapshot.contents)
            })
            .map(|snapshot| snapshot.label())
            .collect()
    }

    /// Puts a snapshot's text back, as an unsaved edit.
    ///
    /// Deliberately *not* written straight to the file. The recovered
    /// text is what you had, not necessarily what you want -- and the
    /// file may have moved on since the crash. Loading it as a dirty
    /// buffer means `:w` accepts it and `:e!` throws it away, which is
    /// the same pair of answers the file-watching work already
    /// established for two versions that disagree.
    fn recover_snapshot(&mut self, snapshot: &fenix_recovery::Snapshot) {
        self.main_view = MainView::Editor;
        let Some(original) = &snapshot.original else {
            if let Some(id) = self.unnamed_snapshots.iter().find_map(|(id, path)| (path == &snapshot.snapshot).then_some(*id)) {
                self.open_buffer_in_focused_pane(id);
                return;
            }
            self.new_scratch_buffer();
            let id = self.focused_buffer_id();
            self.replace_buffer_from_disk(id, &snapshot.contents);
            self.buffers.get_mut(id).unwrap().buffer.mark_unsaved();
            self.unnamed_snapshots.insert(id, snapshot.snapshot.clone());
            self.set_message("recovered unnamed buffer -- use :w <path> to save it");
            return;
        };
        let existed = original.exists();
        self.open_file_from_picker(original);
        let id = self.focused_buffer_id();
        // Compared against what's on disk *now*: a snapshot identical to
        // the file is work that was saved after all (recovered by hand,
        // or saved by another session), and re-opening it as a dirty
        // buffer would invent a change nobody made.
        let unchanged = self.buffers.get(id).is_some_and(|ob| ob.buffer.text() == snapshot.contents);
        if unchanged {
            self.discard_recovery_for(id);
            self.set_message(format!("{} already matches what was recovered", snapshot.label()));
            self.wake_caret();
            return;
        }
        self.replace_buffer_from_disk(id, &snapshot.contents);
        // `replace_buffer_from_disk` marks the buffer saved, which is
        // right when the text came from the file and wrong here: this
        // text exists nowhere but the snapshot, and letting `:qa` past
        // it would lose it a second time.
        if let Some(ob) = self.buffers.get_mut(id) {
            ob.buffer.mark_unsaved();
        }
        let note = if existed { "" } else { " (the file itself is gone)" };
        self.set_message(format!("recovered {}{note} -- :w keeps it, :e! discards it", snapshot.label()));
        self.wake_caret();
    }

    /// `:e!`: throw away the buffer's own edits and read the file again.
    pub(crate) fn reload_focused_file(&mut self) {
        let id = self.focused_buffer_id();
        let Some(path) = self.buffers.get(id).and_then(|ob| ob.buffer.path()).map(Path::to_path_buf) else {
            self.set_error("this buffer has no file to reload from".to_string());
            return;
        };
        match self.open().buffer.read_disk_text() {
            Ok((on_disk, encoding)) => {
                self.replace_buffer_from_disk(id, &on_disk);
                self.open_mut().buffer.set_encoding(encoding);
                self.note_disk_state(id);
                self.set_message(format!("reloaded {}", path.display()));
            }
            Err(err) => self.set_error(format!("couldn't reload {}: {err}", path.display())),
        }
        self.wake_caret();
    }

    // -- Compose buffer ----------------------------------------------

    /// Opens a compose buffer in a strip under the current pane.
    ///
    /// Replaces any compose buffer already open rather than stacking a
    /// second one: there's only ever one thing being written, and two
    /// would leave no way to tell which `Enter` submits.
    fn open_compose(&mut self, purpose: ComposePurpose) {
        self.open_compose_seeded(purpose, "");
    }

    /// `open_compose`, starting from `seed` -- for editing something that
    /// already has text (a description) rather than writing from blank.
    fn open_compose_seeded(&mut self, purpose: ComposePurpose, seed: &str) {
        // One being written, replaced: what was in it is kept.
        self.keep_compose_draft();
        self.close_compose();
        let returning_to = self.focused_pane_id();
        let buffer = if seed.is_empty() { self.buffers.open_scratch() } else { self.buffers.open_text_view(seed) };
        let pane = self.windows_mut().split(SplitKind::Horizontal, buffer);
        self.workspaces.active_pane_states_mut().insert(pane, PaneState::seeded_at(Cursor::at_start()));
        // A comment is a few lines; the thing being commented on is
        // what needs the room, and is what you're looking at while you
        // write.
        self.windows_mut().resize_focused(0.25);
        let title = self.keys_text(&purpose.title());
        self.pane_titles.insert(pane, title);
        self.windows_mut().focus(pane);
        self.compose = Some(ComposeSession { pane, buffer, purpose, returning_to, seed: seed.to_string(), agent_over_seed: false });
        // A blank one is there to be typed into: it opens in Insert, so
        // the first word written isn't run as commands (`P` pasting).
        // One with text to edit (an amend, a description) opens in Normal.
        if self.standard_keys() {
            // Standard keys type at once, wherever the text is.
            self.settle_keys();
            let agent = self.compose.as_ref().is_some_and(|c| c.purpose.agent_writes());
            self.set_message(if agent { "write, or Ctrl+Shift+A for the agent; Ctrl+Enter sends -- Esc discards" } else { "write, then Ctrl+Enter to send -- Esc discards" });
        } else if seed.is_empty() && self.vim.mode() == Mode::Normal {
            if let (Some(ob), Some(state)) = (self.buffers.get_mut(buffer), self.workspaces.active_pane_states_mut().get_mut(&pane)) {
                self.vim.handle_key(&mut ob.buffer, &mut state.cursor, KeyPress::char('i'));
            }
            let agent = self.compose.as_ref().is_some_and(|c| c.purpose.agent_writes());
            self.set_message(if agent { "write, or Esc then A for the agent; Esc and Enter sends -- q discards" } else { "write, then Esc and Enter to send -- q discards" });
        } else {
            self.set_message("write (i to insert), then Esc and Enter to send -- q discards");
        }
        self.wake_caret();
    }

    /// A compose pane for code extension `ext`'s call `id` -- with what
    /// you wrote in its last one of the same name and didn't send, when
    /// there's that.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn open_code_compose(&mut self, ext: &str, id: u64, what: String, verb: &'static str, allow_empty: bool, seed: &str, ask: Option<String>, over_initial: bool) {
        let kept = self.take_compose_draft(ext, &what);
        let back = kept.is_some();
        self.open_compose_seeded(ComposePurpose::Code { ext: ext.to_string(), id, what, verb, allow_empty, ask }, kept.as_deref().unwrap_or(seed));
        if back {
            self.set_message("what you wrote and didn't send is back -- Esc then Enter keeps it, q discards it");
        } else if over_initial && !seed.is_empty() {
            if let Some(compose) = self.compose.as_mut() {
                compose.agent_over_seed = compose.purpose.agent_writes();
            }
            self.set_message("A has the agent write it over this, i edits it -- Esc and Enter sends, q discards");
        }
    }

    /// The compose pane being written in, unsent -- replaced by another,
    /// or open as Fenix quits: an extension's is kept, by the extension
    /// and the pane's name, for the next one of that name. A pull
    /// request's description half written isn't lost to a closed window.
    fn keep_compose_draft(&mut self) {
        let Some(compose) = self.compose.as_ref() else { return };
        let ComposePurpose::Code { ext, what, .. } = &compose.purpose else { return };
        let text = self.buffers.get(compose.buffer).map(|ob| ob.buffer.text()).unwrap_or_default();
        if text.trim().is_empty() || text.trim_end() == compose.seed.trim_end() {
            return;
        }
        let key = compose_draft_key(ext, what);
        let mut drafts = read_compose_drafts(&self.compose_drafts_file);
        drafts.insert(key, text);
        write_compose_drafts(&self.compose_drafts_file, &drafts);
    }

    /// The unsent text kept for `ext`'s compose pane `what`, taken.
    fn take_compose_draft(&mut self, ext: &str, what: &str) -> Option<String> {
        let mut drafts = read_compose_drafts(&self.compose_drafts_file);
        let text = drafts.remove(&compose_draft_key(ext, what))?;
        write_compose_drafts(&self.compose_drafts_file, &drafts);
        Some(text)
    }

    /// Tears the compose pane down and puts focus back where it was.
    fn close_compose(&mut self) {
        let Some(compose) = self.compose.take() else { return };
        // The agent writing it has nowhere to write any more.
        self.agents_compose_gone(compose.buffer);
        // Closed while still typing (it opens in Insert): back to Normal
        // for wherever focus goes.
        if self.vim.mode() == Mode::Insert {
            if let (Some(ob), Some(state)) = (self.buffers.get_mut(compose.buffer), self.workspaces.active_pane_states_mut().get_mut(&compose.pane)) {
                self.vim.handle_key(&mut ob.buffer, &mut state.cursor, KeyPress::named(FenixNamedKey::Escape));
            }
        }
        self.pane_titles.remove(&compose.pane);
        self.windows_mut().focus(compose.pane);
        if self.windows_mut().close_focused() {
            self.workspaces.active_pane_states_mut().remove(&compose.pane);
            self.workspaces.active_scroll_anims_mut().remove(&compose.pane);
            self.workspaces.active_pane_tabs_mut().remove(&compose.pane);
        }
        self.buffers.close(compose.buffer);
        if self.windows().windows().contains(&compose.returning_to) {
            self.windows_mut().focus(compose.returning_to);
        }
        self.wake_caret();
    }

    /// Whether `A` here has the agent write the compose pane: it's the
    /// one in focus, in Normal mode with nothing half typed, it's a pane
    /// the agent can write, and nothing of yours is in it: it's empty, or
    /// still the template it opened with. Past that `A` is Vim's, appending
    /// to the line, and clearing the box is all it takes to ask again.
    fn compose_agent_offered(&self) -> bool {
        let Some(compose) = self.compose.as_ref() else { return false };
        compose.pane == self.focused_pane_id()
            && (self.vim.mode() == Mode::Normal || (self.standard_keys() && self.vim.is_inserting()))
            && !self.vim.is_pending()
            && compose.purpose.agent_writes()
            && self.buffers.get(compose.buffer).is_some_and(|ob| {
                let text = ob.buffer.text();
                text.trim().is_empty() || (compose.agent_over_seed && text.trim_end() == compose.seed.trim_end())
            })
    }

    /// `A` in an empty compose pane: the agent writes it.
    fn compose_agent_write(&mut self) {
        let Some(compose) = self.compose.as_ref() else { return };
        let buffer = compose.buffer;
        match compose.purpose.clone() {
            ComposePurpose::Code { ext, ask: Some(ask), .. } => self.agents_compose_ask(&ext, &ask, buffer),
            _ => self.agents_commit_message(),
        }
    }

    /// `q` in the compose pane: throw the draft away.
    pub(crate) fn compose_cancel(&mut self) {
        let Some(compose) = self.compose.as_ref() else { return };
        if let ComposePurpose::Code { ext, id, .. } = &compose.purpose {
            let (ext, id) = (ext.clone(), *id);
            self.close_compose();
            return self.code_deliver(&ext, fenix_wasm::host::Notice::Composed((id, None)));
        }
        if let ComposePurpose::PipelineGate { run } = &compose.purpose {
            let run = *run;
            self.close_compose();
            self.set_message("the pipeline stopped at its gate");
            return self.pipeline_gate_edited(run, None);
        }
        self.close_compose();
        self.set_message("comment discarded");
    }

    /// `Enter` in the compose pane (Normal mode): send what's written.
    pub(crate) fn compose_submit(&mut self) {
        let Some(compose) = self.compose.as_ref() else { return };
        let body = self.buffers.get(compose.buffer).map(|ob| ob.buffer.text()).unwrap_or_default();
        let body = body.trim().to_string();
        let purpose = compose.purpose.clone();

        // A pipeline's gate, edited: passed on.
        if let ComposePurpose::PipelineGate { run } = &purpose {
            let run = *run;
            self.close_compose();
            self.pipeline_gate_edited(run, Some(body));
            self.wake_caret();
            return;
        }
        // An extension's text: back to it.
        if let ComposePurpose::Code { ext, id, allow_empty, .. } = &purpose {
            if body.is_empty() && !allow_empty {
                self.set_error("nothing written yet -- q discards this instead".to_string());
                return;
            }
            let (ext, id) = (ext.clone(), *id);
            self.close_compose();
            self.code_deliver(&ext, fenix_wasm::host::Notice::Composed((id, Some(body))));
            self.wake_caret();
            return;
        }
        if body.is_empty() {
            // Posting an empty comment is never what was meant, and the
            // forge would either reject it or publish a blank note.
            self.set_error("nothing written yet -- q discards this instead".to_string());
            return;
        }

        // A commit message goes to `git`, not to a forge -- so it needs
        // none of the client below, and works with no Merge Requests
        // view open at all.
        if let ComposePurpose::RebaseMessage { page, hash } = &purpose {
            self.git_rebase_message(*page, hash.clone(), body);
            self.wake_caret();
            return;
        }
        if let ComposePurpose::GitCommit { repo_root, compose } = &purpose {
            self.git_page_commit(repo_root.clone(), compose.clone(), body);
            self.wake_caret();
            return;
        }
        if let ComposePurpose::CommitMessage { repo_root } = &purpose {
            match fenix_git::commit(repo_root, &body) {
                Ok(_) => {
                    self.close_compose();
                    let summary = body.lines().next().unwrap_or_default().to_string();
                    self.set_message(format!("committed: {summary}"));
                    self.git_refresh_all_views();
                }
                // The draft stays put, same as a failed post: a hook
                // that rejected the message is something you fix in the
                // message.
                Err(err) => self.set_error(format!("commit failed: {err} -- the message is still here")),
            }
            self.wake_caret();
        }
    }

    /// Swaps a Merge buffer's content, keeping every pane showing it on
    /// the same line index -- resolving one conflict re-renders the
    /// whole file, and snapping back to the top after each choice would
    /// make a file with several conflicts miserable to work through.
    fn set_merge_buffer(&mut self, id: BufferId, view: merge_view::MergeView) {
        self.merge_lines.insert(id, view.lines);
        let mut old_lines: Vec<(fenix_window::WindowId, usize)> = Vec::new();
        for pane in self.windows().windows() {
            if self.windows().content(pane) == Some(&id) {
                let ps = self.pane_state(pane);
                if let Some(ob) = self.buffers.get(id) {
                    old_lines.push((pane, ob.buffer.line_col(&ps.cursor).0));
                }
            }
        }
        if let Some(ob) = self.buffers.get_mut(id) {
            let end = ob.buffer.len_chars();
            let mut scratch = Cursor::at_start();
            ob.buffer.replace_range(&mut scratch, 0, end, &view.text);
        }
        let line_count = self.buffers.get(id).map(|ob| ob.buffer.visual_line_count()).unwrap_or(1);
        for (pane, old_line) in old_lines {
            let clamped = old_line.min(line_count.saturating_sub(1));
            let Some(ob) = self.buffers.get(id) else { continue };
            let char_idx = ob.buffer.line_start_char(clamped);
            self.pane_state_mut(pane).cursor = Cursor { char_idx, sticky_col: 0 };
        }
    }

    // -- Conflict resolution in an ordinary file buffer -----------------

    /// The conflicts in the focused buffer, parsed fresh from its text.
    ///
    /// Recomputed on every use rather than cached: the buffer is a real,
    /// editable file that the user may have just hand-edited, and acting
    /// on stale line numbers would resolve the wrong region.
    fn focused_conflicts(&self) -> Vec<fenix_git::Conflict> {
        fenix_git::find_conflicts(&self.open().buffer.text())
    }

    /// `SPC g x j`/`SPC g x k`: move to the next/previous conflict in the
    /// focused file.
    pub(crate) fn goto_conflict(&mut self, forward: bool) {
        if self.merge_goto_conflict(forward) {
            return;
        }
        let conflicts = self.focused_conflicts();
        if conflicts.is_empty() {
            self.set_message("no conflict markers in this file");
            return;
        }
        let line = self.open().buffer.line_col(&self.cursor()).0;
        let target = if forward {
            conflicts.iter().find(|c| c.start > line).or_else(|| conflicts.first())
        } else {
            conflicts.iter().rev().find(|c| c.end < line).or_else(|| conflicts.last())
        };
        let Some(target) = target else { return };
        let char_idx = self.open().buffer.line_start_char(target.start);
        let pane = self.focused_pane_id();
        self.pane_state_mut(pane).cursor = Cursor { char_idx, sticky_col: 0 };
        self.wake_caret();
    }

    /// `SPC g x o`/`t`/`b`: resolve the conflict under the
    /// cursor by keeping ours, theirs, or both.
    ///
    /// Edits the buffer rather than the file on disk, so it lands in the
    /// undo history like any other edit and isn't written until the
    /// buffer is saved -- a mistaken choice is one `u` away.
    pub(crate) fn resolve_conflict_at_cursor(&mut self, resolution: fenix_git::Resolution) {
        // Same keys in the Merge view, where the "buffer" is a rendering
        // of the file rather than the file, so the edit goes to disk.
        // One pair of keys for one decision, wherever you happen to be
        // looking at the conflict from.
        if self.merge_resolve_at_cursor(resolution) {
            return;
        }
        let conflicts = self.focused_conflicts();
        let line = self.open().buffer.line_col(&self.cursor()).0;
        let Some(conflict) = conflicts.iter().find(|c| c.contains(line)).or_else(|| conflicts.iter().find(|c| c.start > line)) else {
            self.set_error("no conflict under the cursor".to_string());
            return;
        };
        let text = self.open().buffer.text();
        let resolved = fenix_git::resolve_conflict(&text, conflict, resolution);
        let start_char = self.open().buffer.line_start_char(conflict.start);

        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        let mut scratch = Cursor::at_start();
        replace_text_by_lines(buffer, &mut scratch, &resolved);
        // Land on where the conflict was, so the next one is a `SPC g j`
        // away rather than needing the file re-navigated.
        cursor.char_idx = start_char.min(buffer.len_chars());
        cursor.sticky_col = 0;

        let kept = match resolution {
            fenix_git::Resolution::Ours => "ours",
            fenix_git::Resolution::Theirs => "theirs",
            fenix_git::Resolution::Both => "both sides",
        };
        let left = fenix_git::find_conflicts(&self.open().buffer.text()).len();
        self.set_message(format!("kept {kept} -- {left} conflict(s) left in this file"));
        self.wake_caret();
    }

    /// `FenixUserEvent::GitHistoryReady` handling: the graph and the refs
    /// drawn, unless a newer read has been asked for since.
    fn apply_history(&mut self, request_id: u64, data: HistoryData) {
        let Some(session) = self.history_session.as_ref() else { return };
        if session.refresh_request_id != request_id {
            return;
        }
        let (graph_buffer, refs_buffer) = (session.graph_buffer, session.refs_buffer);
        let rows = fenix_git::assign_lanes(&data.commits);
        let style = graph_view::GraphStyle::from_config(self.config.git_graph_style.as_deref());
        let panel = graph_view::render_graph(&data.commits, &rows, style);
        self.graph_lines.insert(graph_buffer, panel.lines);
        self.replace_buffer_text(graph_buffer, &panel.text);
        self.set_git_buffer(refs_buffer, graph_view::render_refs(&data.branches, &data.remotes, &data.tags, data.fetch_age));

        if let Some(session) = self.history_session.as_mut() {
            session.commits = data.commits;
            session.last_detail_commit = None; // force the detail pane to re-fetch
        }
        self.history_sync_detail();
    }

    /// Whether the focused diff is one with no index relationship to
    /// change -- a commit, a stash, or a ref comparison.
    fn focused_diff_is_read_only(&self) -> bool {
        self.focused_diff_buffer().and_then(|id| self.diff_models.get(&id)).is_some_and(|m| m.source == DiffSource::ReadOnly)
    }

    /// `FenixUserEvent::GitHistoryDetailReady` handling -- drops a result
    /// a newer request has superseded, exactly as `apply_git_main` does.
    fn apply_history_detail(&mut self, request_id: u64, buffer: BufferId, diff: Option<CommitDetail>) {
        let Some(session) = self.history_session.as_ref() else { return };
        if session.detail_request_id != request_id {
            return;
        }
        let repo_root = session.repo_root.clone();
        let (header, diff) = match diff {
            Some(detail) => (commit_header_rows(&detail.meta), Some(detail.diff)),
            None => (Vec::new(), None),
        };
        self.set_diff_buffer_with_header(buffer, &header, diff.as_deref(), DiffSource::ReadOnly, &repo_root);
    }

    fn apply_git_fetched(&mut self, result: Result<String, String>) {
        self.motion.git_fetching = false;
        match result {
            Ok(_) => self.set_message("fetched"),
            Err(err) => self.set_error(format!("fetch failed: {}", err.trim())),
        }
        if self.history_session.is_some() {
            self.history_refresh();
        }
        if self.git_session.is_some() {
            self.git_refresh_session();
        }
    }

    /// Replaces a generated buffer's whole text, resetting every pane
    /// showing it to the top -- the shared half of `set_git_buffer`/
    /// `set_diff_buffer`, for buffers whose metadata was already stored
    /// by the caller.
    fn replace_buffer_text(&mut self, id: BufferId, text: &str) {
        if let Some(ob) = self.buffers.get_mut(id) {
            let end = ob.buffer.len_chars();
            let mut scratch_cursor = Cursor::at_start();
            ob.buffer.replace_range(&mut scratch_cursor, 0, end, text);
        }
        for pane in self.windows().windows() {
            if self.windows().content(pane) == Some(&id) {
                let ps = self.pane_state_mut(pane);
                *ps = PaneState::seeded_at(Cursor::at_start());
            }
        }
    }

    /// `SPC g q`: closes whichever Git view the focused workspace holds
    /// -- a panel view, or a Git page.
    pub(crate) fn close_git_view(&mut self) {
        if self.side_by_side_close() {
            return;
        }
        let here = self.workspaces.active_index();
        let panes = self.windows().windows();
        if self.history_session.as_ref().is_some_and(|s| s.workspace_index == here) {
            self.history_close();
        } else if self.merge_session.as_ref().is_some_and(|s| s.workspace_index == here) {
            self.merge_close();
        } else if self.git_session.as_ref().is_some_and(|s| panes.contains(&s.status_pane)) {
            self.git_session_close();
        } else if self.is_page_buffer(self.focused_buffer_id()) {
            let id = self.focused_buffer_id();
            self.close_page(id);
        } else {
            self.set_message("no Git view in front to close");
        }
    }

    /// Where a jump lands (`gd`, a search result, a symbol, `Ctrl-O`):
    /// the pane's preview tab, which the next jump reuses.
    /// Whether `path` isn't text, by its first bytes.
    pub(crate) fn looks_binary(path: &Path) -> bool {
        use std::io::Read;
        let Ok(f) = std::fs::File::open(path) else { return false };
        let mut head = Vec::new();
        if f.take(fenix_core::TEXT_SNIFF_BYTES as u64).read_to_end(&mut head).is_err() {
            return false;
        }
        !fenix_core::looks_like_text(&head)
    }

    /// A file that isn't text, in the hex view (`ccsds.hex`) -- or not
    /// opened, when nothing here shows bytes. A PDF says what reads it.
    fn open_binary(&mut self, path: &Path) {
        if Self::looks_like_pdf(path) {
            return self.set_error(format!("{} -- PDFs open in the PDF reader, the fenix.pdf extension: SPC h x installs it", path.display()));
        }
        // The CCSDS extension's hex view, given the file to read.
        let Some(ext) = self.code_server(wasm_host::CCSDS_HEX, None) else {
            return self.set_error(format!("{} isn't text -- the hex view comes with the CCSDS extension (fenix.ccsds): SPC h x installs it", path.display()));
        };
        let full = fenix_lsp::normalize(std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
        self.code_grant(&ext, full.clone());
        let payload = serde_json::json!({ "path": full }).to_string();
        self.code_serve(
            &ext,
            wasm_host::CCSDS_HEX,
            payload,
            fenix_wasm::host::ServiceCaller::Fenix,
            wasm_host::Answer::Fenix(Box::new(|app, answer| {
                if let Err(why) = answer {
                    app.set_error(why);
                }
            })),
        );
    }

    pub(super) fn looks_like_pdf(path: &Path) -> bool {
        path.extension().and_then(|e| e.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
    }

    /// `gf`: opens the file named under the cursor -- relative to the
    /// buffer's folder, then the project -- in the preview tab. A file an
    /// extension opens (a PDF) goes to it, at the page a `#page=38` after
    /// it names.
    /// Whether there's a file name (`spec.pdf#page=3`, `src/a.rs:12`) under
    /// the cursor that `gf` would follow.
    pub(crate) fn link_under_cursor(&self) -> bool {
        let (line, col) = self.open().buffer.line_col(&self.cursor());
        let text = self.open().buffer.line(line).to_string();
        crate::file_link::link_at(text.trim_end_matches(['\n', '\r']), col).is_some()
    }

    pub(crate) fn follow_link_under_cursor(&mut self) {
        let (line, col) = self.open().buffer.line_col(&self.cursor());
        let text = self.open().buffer.line(line).to_string();
        let Some((name, page)) = crate::file_link::link_at(text.trim_end_matches(['\n', '\r']), col) else {
            self.set_message("no file name under the cursor");
            return;
        };
        let here = self.open().buffer.path().and_then(Path::parent).map(Path::to_path_buf);
        let named = PathBuf::from(&name);
        let found = if named.is_absolute() {
            named.exists().then_some(named)
        } else {
            [here, self.project_root.clone(), std::env::current_dir().ok()].into_iter().flatten().map(|dir| dir.join(&named)).find(|p| p.exists())
        };
        let Some(path) = found else {
            self.set_message(format!("no file {name}"));
            return;
        };
        if !self.open_with_extension_at(&path, page, None) {
            self.open_file_as(&path, true);
        }
    }

    fn open_file_as_preview(&mut self, path: &Path) {
        self.open_file_as(path, true);
    }

    fn open_file_as(&mut self, path: &Path, preview: bool) {
        // A file on a remote machine is text to edit: the extensions that
        // open files, and the look at its first bytes, read this
        // machine's disk.
        if fenix_vfs::is_remote(path) {
            return self.open_text_file(path, preview);
        }
        if let Some(open) = self.opener_for(path) {
            open(self, path, preview);
            return;
        }
        if Self::looks_binary(path) {
            return self.open_binary(path);
        }
        self.open_text_file(path, preview);
    }

    /// `path` as text, in the focused pane, though an extension opens it
    /// (`contributes.opens`). False when it isn't text.
    pub(crate) fn open_file_as_text(&mut self, path: &Path) -> bool {
        if path.is_file() && Self::looks_binary(path) {
            self.set_error(format!("{} isn't text", path.display()));
            return false;
        }
        self.open_text_file(path, false);
        true
    }

    fn open_text_file(&mut self, path: &Path, preview: bool) {
        // A remote file that can't be read is not opened as an empty one
        // (which a save would then put in its place): it says why.
        if fenix_vfs::is_remote(path) && self.buffers.id_for_path(path).is_none() {
            match Buffer::from_path(path) {
                Ok(buffer) => {
                    self.buffers.insert_loaded_document(buffer);
                }
                // Not there yet: a new file, named, as for a local one.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    self.set_error(format!("couldn't open {}: {error}", fenix_vfs::display(path)));
                    self.wake_caret();
                    return;
                }
            }
        }
        let was_open = self.buffers.id_for_path(path);
        let id = self.buffers.open_path(path);
        // Opened again, a buffer with edits of its own (or one already
        // found changed on disk) keeps what's known of the disk: looking
        // afresh would take the other side's change for its own and let
        // a save write over it.
        let keep_disk_state = was_open == Some(id) && (self.externally_changed.contains(&id) || self.buffers.get(id).is_some_and(|ob| ob.buffer.is_dirty()));
        if !keep_disk_state {
            self.note_disk_state(id);
        }
        // A file of a project that has a workspace of its own opens there.
        if let Some(i) = self.workspace_for_file(path) {
            self.workspaces.switch_to_index(i);
        }
        self.open_buffer_in_focused_pane_as(id, preview);
        self.refresh_project_root();
        self.record_recent_file(path);
        self.main_view = MainView::Editor;
    }

    /// Opens every file argument this launch was given beyond the
    /// first -- `App::new`/`with_file` already seeded the initial
    /// buffer from `args[0]` itself, but Explorer's "Open With" can
    /// hand a launch multiple selected files at once. Reuses `open_
    /// file_from_picker`'s exact "point the focused pane at it"
    /// behavior; called once per extra path, right after startup, from
    /// `main.rs`.
    pub(crate) fn open_startup_file(&mut self, path: &Path) {
        // Session paths are absolute; CLI paths may be relative or aliases.
        // Reuse the restored document rather than opening a clean duplicate.
        let existing = self.buffer_id_for_lsp_path(path)
            .and_then(|id| self.buffers.get(id))
            .and_then(|ob| ob.buffer.path()).map(Path::to_path_buf);
        self.open_file_from_picker(existing.as_deref().unwrap_or(path));
    }

    /// `SPC f f`: starts the type-a-path find-file prompt.
    pub(crate) fn start_find_file_prompt(&mut self) {
        self.find_file_prompt = Some(String::new());
        self.wake_caret();
    }

    /// Routes one keypress to the in-progress `find_file_prompt` -- same
    /// "next keystrokes are special" shape as `grep_query_key`.
    fn find_file_prompt_key(&mut self, key: KeyPress) {
        if key == KeyPress::char('v').with_ctrl() {
            let pasted = self.clipboard_text();
            if let (Some(input), Some(text)) = (&mut self.find_file_prompt, pasted) {
                input.push_str(&text);
            }
            self.wake_caret();
            return;
        }
        let Some(input) = &mut self.find_file_prompt else { return };
        match key.code {
            KeyCode::Named(FenixNamedKey::Escape) => self.find_file_prompt = None,
            KeyCode::Named(FenixNamedKey::Enter) => {
                let input = self.find_file_prompt.take().unwrap_or_default();
                self.open_typed_path(&input);
            }
            KeyCode::Named(FenixNamedKey::Backspace) if key.mods.ctrl || key.mods.alt => delete_word_back(input),
            KeyCode::Named(FenixNamedKey::Backspace) => {
                input.pop();
            }
            KeyCode::Char(c) if key.mods == Mods::default() => input.push(c),
            _ => {}
        }
        self.wake_caret();
    }

    /// Resolves `typed` (a `~`-expandable, possibly-relative path -- an
    /// absolute one is used as-is) and opens it via `open_file_from_
    /// picker`, which already handles buffer reuse, `recent_files`
    /// tracking, and switching back to `MainView::Editor`. Unlike every
    /// fuzzy picker, this never enumerates anything -- a `.gitignore`'d
    /// (or simply not-yet-existing -- `open_path` already degrades to
    /// an empty, to-be-created buffer, real Emacs `find-file` semantics)
    /// path opens exactly the same as any other.
    fn open_typed_path(&mut self, typed: &str) {
        let typed = typed.trim();
        if typed.is_empty() {
            return;
        }
        let expanded = match typed.strip_prefix('~') {
            Some(rest) => dirs::home_dir()
                .map(|home| home.join(rest.trim_start_matches(['/', '\\'])))
                .unwrap_or_else(|| PathBuf::from(typed)),
            None => PathBuf::from(typed),
        };
        let resolved = if expanded.is_absolute() {
            expanded
        } else {
            let root = self.project_root.clone().unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            root.join(expanded)
        };
        let new = !resolved.exists();
        self.open_file_from_picker(&resolved);
        if new && self.open().buffer.path().is_some() {
            self.set_message(format!("{} is a new file -- SPC f s creates it", fenix_core::plain_path(&resolved).display()));
        }
    }

    /// What to show in place of the modeline while `find_file_prompt` is
    /// active -- mirrors `explorer_prompt_text`.
    fn find_file_prompt_text(&self) -> Option<String> {
        // Where a relative path starts: the project, else Fenix's folder.
        let from = self.project_root.as_deref().and_then(Path::file_name).map(|n| format!(" (in {})", n.to_string_lossy())).unwrap_or_default();
        self.find_file_prompt.as_ref().map(|input| format!("find file{from}: {input}"))
    }

    /// `SPC f y`: copies the focused buffer's file path to the OS
    /// clipboard -- a no-op (clipboard untouched) for an unsaved/
    /// unnamed buffer. Reuses the same `arboard::Clipboard` `y`/`p`
    /// already mirror onto.
    pub(crate) fn yank_file_path(&mut self) {
        let Some(path) = self.open().buffer.path() else { return };
        let text = path.display().to_string();
        if let Some(clipboard) = &mut self.clipboard {
            let _ = clipboard.set_text(text);
        }
        self.wake_caret();
    }

    /// `SPC f R`: starts the rename prompt, pre-seeded with the current
    /// filename -- a no-op for an unsaved/unnamed buffer (there's no
    /// file on disk yet to rename; `:w path` already covers "save this
    /// somewhere new").
    pub(crate) fn start_rename_file_prompt(&mut self) {
        let Some(path) = self.open().buffer.path() else { return };
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        self.prompt_prefill_selected = self.standard_keys() && !name.is_empty();
        self.rename_file_prompt = Some(name);
        self.wake_caret();
    }

    /// Routes one keypress to the in-progress `rename_file_prompt` --
    /// same shape as `find_file_prompt_key`.
    fn rename_file_prompt_key(&mut self, key: KeyPress) {
        if let Some(input) = &mut self.rename_file_prompt {
            if take_selected_prefill(input, &mut self.prompt_prefill_selected, key) {
                self.wake_caret();
                return;
            }
        }
        if key == KeyPress::char('v').with_ctrl() {
            let pasted = self.clipboard_text();
            if let (Some(input), Some(text)) = (&mut self.rename_file_prompt, pasted) {
                input.push_str(&text);
            }
            self.wake_caret();
            return;
        }
        let Some(input) = &mut self.rename_file_prompt else { return };
        match key.code {
            KeyCode::Named(FenixNamedKey::Escape) => self.rename_file_prompt = None,
            KeyCode::Named(FenixNamedKey::Enter) => {
                let input = self.rename_file_prompt.take().unwrap_or_default();
                self.apply_rename_file(&input);
            }
            KeyCode::Named(FenixNamedKey::Backspace) if key.mods.ctrl || key.mods.alt => delete_word_back(input),
            KeyCode::Named(FenixNamedKey::Backspace) => {
                input.pop();
            }
            KeyCode::Char(c) if key.mods == Mods::default() => input.push(c),
            _ => {}
        }
        self.wake_caret();
    }

    /// Renames the focused buffer's file to `new_name` (resolved
    /// against its current parent directory -- a `new_name` containing
    /// its own `/`s lands in a sub/parent path relative to there, same
    /// as typing one into `Path::join` normally would). Writes the
    /// buffer's *current* content to the new path (`Buffer::save_as`,
    /// which also updates the buffer's own `path()`), so unsaved edits
    /// are captured too, not just what was last saved -- then removes
    /// the old file (create-then-remove rather than `std::fs::rename`,
    /// so this can't fail with `EXDEV` across filesystem boundaries) and
    /// tells the registry the path moved (`BufferList::path_changed`,
    /// keeping `open_path`/`id_for_path` correct afterward).
    fn apply_rename_file(&mut self, new_name: &str) {
        let new_name = new_name.trim();
        if new_name.is_empty() {
            return;
        }
        let Some(old_path) = self.open().buffer.path().map(Path::to_path_buf) else { return };
        let new_path = match old_path.parent() {
            Some(parent) => parent.join(new_name),
            None => PathBuf::from(new_name),
        };
        let id = self.focused_buffer_id();
        if let Err(err) = self.open_mut().buffer.save_as(&new_path) {
            self.set_error(format!("rename failed: {err}"));
            return;
        }
        self.buffers.path_changed(id, Some(&old_path));
        if let Err(err) = std::fs::remove_file(&old_path) {
            self.set_error(format!("rename couldn't remove the old file {}: {err}", old_path.display()));
        }
    }

    /// What to show in place of the modeline while `rename_file_prompt`
    /// is active.
    fn rename_file_prompt_text(&self) -> Option<String> {
        self.rename_file_prompt.as_ref().map(|input| format!("rename to: {input}"))
    }

    /// `SPC f D`: arms the delete-current-file confirmation -- a no-op
    /// for an unsaved/unnamed buffer.
    pub(crate) fn start_delete_file_confirm(&mut self) {
        if self.open().buffer.path().is_none() {
            return;
        }
        self.delete_file_confirm = true;
        self.wake_caret();
    }

    /// Routes one keypress to the armed delete confirmation -- same y/n
    /// shape as `DockerHost::confirm_key`.
    fn delete_file_confirm_key(&mut self, key: KeyPress) {
        self.delete_file_confirm = false;
        if key.code == KeyCode::Char('y') {
            self.apply_delete_file();
        }
        self.wake_caret();
    }

    /// Deletes the focused buffer's file from disk and closes the
    /// buffer -- `kill_buffer` already falls back to the MRU-next
    /// buffer (or a fresh scratch one) and removes the stale path from
    /// the registry's own index.
    fn apply_delete_file(&mut self) {
        let Some(path) = self.open().buffer.path().map(Path::to_path_buf) else { return };
        if let Err(err) = std::fs::remove_file(&path) {
            self.set_error(format!("couldn't delete {}: {err}", path.display()));
            return;
        }
        self.kill_buffer_now();
    }

    /// What to show in place of the modeline while the delete
    /// confirmation is armed -- mirrors `DockerHost::confirm_text`.
    fn delete_file_confirm_text(&self) -> Option<String> {
        if !self.delete_file_confirm {
            return None;
        }
        // The question first: a long path is cut at the prompt's edge.
        let name = self.open().buffer.path().map(|p| self.shown_path(p)).unwrap_or_default();
        Some(format!("Delete? y deletes, n keeps -- {name}"))
    }

    /// `path` as a message says it: inside the project from its root
    /// (`src/lib.rs`), else from home (`~/notes.md`).
    fn shown_path(&self, path: &Path) -> String {
        self.project_root.as_deref().and_then(|r| crate::page::path_within(r, path)).unwrap_or_else(|| crate::page::home_relative(path))
    }

    /// What happens after a key has been through Vim, whatever it did:
    /// today, finishing an XML tag the key just typed in Insert mode.
    fn after_vim_key(&mut self, key: KeyPress) {
        self.keep_edited_preview();
        if let KeyCode::Char(c @ ('>' | '/')) = key.code {
            if key.mods == Mods::default() && self.vim.mode() == Mode::Insert {
                self.xml_autoclose(c);
            }
        }
    }

    fn jump_to_grep_match(&mut self, m: &fenix_project::GrepMatch) {
        self.mark_jump();
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        let target_line = m.line.saturating_sub(1).min(buffer.visual_line_count().saturating_sub(1));
        let start = buffer.line_start_char(target_line);
        let col = m.col.saturating_sub(1).min(buffer.line_len(target_line));
        cursor.char_idx = start + col;
        let (_, sticky) = buffer.line_col(cursor);
        cursor.sticky_col = sticky;
    }

    /// Same shape as `jump_to_grep_match`, for a `ctags`-sourced
    /// definition -- `ctags` only ever gives a line, not a column, so
    /// this always lands at the start of it rather than on a specific
    /// character within.
    fn jump_to_tag(&mut self, tag: &fenix_completion::ctags::TagEntry) {
        self.mark_jump();
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        let target_line = tag.line.saturating_sub(1).min(buffer.visual_line_count().saturating_sub(1));
        cursor.char_idx = buffer.line_start_char(target_line);
        let (_, sticky) = buffer.line_col(cursor);
        cursor.sticky_col = sticky;
    }

    /// Pushes `from` onto the `Ctrl-O` back stack and clears the
    /// `Ctrl-I` forward stack -- see `jump_back_stack`'s own doc comment
    /// for why a new jump invalidates old forward history rather than
    /// preserving it the way real Vim's jumplist does.
    fn record_jump(&mut self, from: JumpEntry) {
        // Vim's `'` (and `` ` ``) mark: where the last jump left.
        self.marks.insert('\'', from);
        self.jump_back_stack.push(from);
        self.jump_forward_stack.clear();
        self.mark_jump();
    }

    /// The cursor in the focused pane is jumping, not stepping: the next
    /// scroll centres it if it lands out of view (`scroll_for_cursor`).
    fn mark_jump(&mut self) {
        self.jump_view = Some(self.focused_pane_id());
    }

    /// `Ctrl-O`: jumps to the most recently recorded position, pushing
    /// wherever the cursor currently is onto the forward stack so
    /// `Ctrl-I` can return here. Silently drops (without stopping) any
    /// entry whose buffer was since closed -- see `JumpEntry`'s own doc
    /// comment. A no-op with nothing left to go back to, or while a
    /// Docker/Git panel session is open (see `jumplist_navigation_
    /// allowed`'s own doc comment).
    pub(crate) fn jump_back(&mut self) {
        if !self.jumplist_navigation_allowed() {
            return;
        }
        let current = JumpEntry { buffer: self.focused_buffer_id(), char_idx: self.cursor().char_idx };
        while let Some(entry) = self.jump_back_stack.pop() {
            if self.buffers.get(entry.buffer).is_some() {
                self.jump_forward_stack.push(current);
                self.goto_jump_entry(entry, false);
                return;
            }
        }
    }

    /// `Ctrl-I`: the mirror of `jump_back`, popping the forward stack
    /// instead.
    pub(crate) fn jump_forward(&mut self) {
        if !self.jumplist_navigation_allowed() {
            return;
        }
        let current = JumpEntry { buffer: self.focused_buffer_id(), char_idx: self.cursor().char_idx };
        while let Some(entry) = self.jump_forward_stack.pop() {
            if self.buffers.get(entry.buffer).is_some() {
                self.jump_back_stack.push(current);
                self.goto_jump_entry(entry, false);
                return;
            }
        }
    }

    /// `` `{name} ``/`'{name}` -- looks up `name` in `marks` and jumps
    /// there, recording the position jumped *from* onto the jumplist
    /// first (same as a symbol/grep jump, and real Vim's own mark-jump
    /// behavior) so `Ctrl-O` returns here afterward. A no-op if that
    /// mark was never set, if its buffer's since closed (the stale entry
    /// is dropped from `marks` too, same graceful-degrade posture
    /// `jump_back`/`jump_forward` already have for the jumplist itself),
    /// or while a Docker/Git panel session is open (see `jumplist_
    /// navigation_allowed`'s own doc comment -- `goto_jump_entry` is
    /// exactly as unsafe to call from there for a mark as for a jump).
    fn jump_to_mark(&mut self, name: char, linewise: bool) {
        // `` ` `` and `'` are one mark: the place before the last jump.
        let name = if name == '`' { '\'' } else { name };
        let Some(&entry) = self.marks.get(&name) else { return };
        if self.buffers.get(entry.buffer).is_none() {
            self.marks.remove(&name);
            return;
        }
        if !self.jumplist_navigation_allowed() {
            return;
        }
        let from = JumpEntry { buffer: self.focused_buffer_id(), char_idx: self.cursor().char_idx };
        self.goto_jump_entry(entry, linewise);
        self.record_jump(from);
    }

    /// `goto_jump_entry` repoints the *focused pane's* content outright
    /// (`set_pane_content`), which would silently desync a Docker/Git
    /// panel session's own pane-role bookkeeping (`pane_titles`, `docker_
    /// focused_role`/`git_focused_role`) if it fired on one of that
    /// session's panes -- those track *which pane holds which role* by
    /// pane id, not by buffer, and have no way to notice their pane's
    /// buffer just got swapped out from under them by something outside
    /// the session. Simplest safe rule: no jumplist navigation at all
    /// while either panel is open, rather than trying to reconcile
    /// session state case by case.
    fn jumplist_navigation_allowed(&self) -> bool {
        self.git_session.is_none()
    }

    /// Points the focused pane at `entry.buffer` (already open -- every
    /// `JumpEntry` was recorded from a buffer that existed at the time,
    /// see `JumpEntry`'s own doc comment) and lands the cursor at its
    /// char index (clamped in case the buffer's shrunk since) or, if
    /// `linewise`, the first non-blank of that char index's line --
    /// `'{mark}`'s own landing spot, real Vim's own `` ` ``/`'` split
    /// (`jump_back`/`jump_forward` always pass `false`; only `jump_to_
    /// mark` ever passes `true`). Doesn't touch `project_root`/recent-
    /// files the way `open_file_from_picker` does -- this is "return to
    /// where you were," not "visit a file," so those side effects don't
    /// apply.
    fn goto_jump_entry(&mut self, entry: JumpEntry, linewise: bool) {
        if entry.buffer != self.focused_buffer_id() {
            self.open_buffer_in_focused_pane_as(entry.buffer, true);
            self.refresh_project_root();
        }
        self.mark_jump();
        let normal = self.vim.mode() == Mode::Normal;
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        let target = entry.char_idx.min(buffer.len_chars());
        let (line, col) = buffer.line_col(&Cursor { char_idx: target, sticky_col: 0 });
        cursor.char_idx = if linewise {
            line_first_non_blank_char(buffer, line)
        } else if normal && col > 0 && col >= buffer.line_len(line) {
            // Not past the line's last char in Normal mode (`` `^ `` after
            // typing at a line's end).
            target - 1
        } else {
            target
        };
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
    }

    /// Registers `root` as the current project and immediately chains
    /// into a find-file picker scoped to it -- matches Projectile's own
    /// default "switch project" action rather than leaving you with
    /// nothing to do next. Sets `main_view = Picker` explicitly: a no-op
    /// for the original caller (`picker_confirm`, already mid-picker so
    /// this was already true), but necessary for the dashboard's own
    /// call site (`dashboard_activate_selected`), which isn't already
    /// mid-picker when it calls this.
    fn switch_to_project(&mut self, root: PathBuf) {
        let root = fenix_lsp::normalize(root);
        self.project_root = Some(root.clone());
        self.known_projects.add(root.clone());
        if let Err(err) = self.known_projects.save() {
            eprintln!("fenix: couldn't save project history: {err}");
        }
        self.open_find_file(&root, false);
    }

    /// Whether an edit to `buffer` should be reverted.
    ///
    /// A listing is read-only -- typing into it would desync the text
    /// from the rows it stands for -- *except* while its names are being
    /// edited, which is the one time the text is the point.
    fn buffer_edits_are_reverted(&self, buffer: BufferId, kind: BufferKind) -> bool {
        // A file as it is at a ref: there's nothing to save it to.
        if self.ref_view_names.contains_key(&buffer) {
            return true;
        }
        if kind == BufferKind::Explorer {
            return !self.rename_mode.contains_key(&buffer);
        }
        is_readonly_buffer_kind(kind)
    }

    /// `SPC w v`/`SPC w s`: splits the focused window vertically (side by
    /// side) or horizontally (stacked), Doom's own `splitright`/
    /// `splitbelow` default -- the new pane shows the *same* buffer as the
    /// one it split from (matching Vim's own `:vsplit`/`:split`), not a
    /// blank scratch buffer, so it starts as a second view of what you
    /// were already looking at.
    fn split_window(&mut self, kind: SplitKind) {
        let id = self.focused_buffer_id();
        // The new pane starts as a second, independent view of the same
        // buffer -- same cursor/scroll as the pane it split from (real
        // Vim's own `:split` behavior), not the buffer's stale remembered
        // position. Cloned *before* `split` reassigns focus to the new
        // pane, from the source pane's own `PaneState` (see `PaneState`'s
        // own doc comment for why cursor/scroll are per-pane now).
        let source_state = *self.pane_state(self.focused_pane_id());
        let new_pane = self.windows_mut().split(kind, id);
        self.workspaces.active_pane_states_mut().insert(new_pane, source_state);
        // Bypasses `set_pane_content` (there's no buffer *switch* here, the
        // new pane starts already showing `id`), so it needs its own tab-
        // list seed -- just the one buffer, not the source pane's whole tab
        // list, matching real VS's own "a new split group starts with one
        // tab" behavior.
        self.workspaces.active_pane_tabs_mut().insert(new_pane, vec![id]);
        self.wake_caret();
    }

    pub(crate) fn split_vertical(&mut self) {
        self.split_window(SplitKind::Vertical);
    }

    pub(crate) fn split_horizontal(&mut self) {
        self.split_window(SplitKind::Horizontal);
    }

    /// `SPC w hjkl`: moves focus to the nearest pane in that direction
    /// by real layout geometry, not tree adjacency -- and *across OS
    /// windows*, not just within one. Walking off the left edge of the
    /// window on one monitor continues into the window on the monitor
    /// beside it, landing on whichever of its panes is actually nearest
    /// (its rightmost, coming from the left edge -- not its first).
    ///
    /// One list of rectangles in desktop coordinates, one rule
    /// (`fenix_window::pick`). Panes in the current window are
    /// simply nearer than panes in the next window over, so ordinary
    /// same-window navigation behaves exactly as it always has without
    /// being a special case, and neither is the sidebar (see
    /// `NavTarget::Sidebar`).
    ///
    /// A no-op at the outermost edge of the outermost window.
    pub(crate) fn navigate_window(&mut self, dir: NavDirection) {
        let from = if self.sidebar_focused {
            NavTarget::Sidebar { frame: self.active_frame }
        } else {
            NavTarget::Pane { frame: self.active_frame, pane: self.focused_pane_id() }
        };
        let targets = self.nav_targets();
        let Some(target) = fenix_window::pick(&targets, from, dir) else { return };

        let frame = match target {
            NavTarget::Pane { frame, .. } | NavTarget::Sidebar { frame } => frame,
        };
        if frame != self.active_frame {
            self.focus_frame(frame);
            self.focus_active_frame_window();
        }
        match target {
            NavTarget::Pane { pane, .. } => {
                self.windows_mut().focus(pane);
                // Mirrors `handle_click_at`'s own rule: focusing a pane
                // always clears both panel-focus overrides, since they
                // sit outside the tree and nothing else would.
                self.sidebar_focused = false;
                self.terminal_focused = false;
            }
            NavTarget::Sidebar { .. } => {
                self.sidebar_focused = true;
                self.terminal_focused = false;
            }
        }
        self.wake_caret();
    }

    /// Every pane (and sidebar) of every open frame, as rectangles in
    /// one desktop coordinate space -- each frame's own layout offset by
    /// where its client area sits on the desktop.
    ///
    /// Computed by briefly activating each frame in turn, so the
    /// existing `frame_metrics`/`frame_geometry` pair -- which read the
    /// live sidebar and terminal state, and are what `redraw` and mouse
    /// hit-testing already agree with -- can be reused verbatim rather
    /// than reimplemented against a parked frame. Whichever frame was
    /// active is active again on return.
    fn nav_targets(&mut self) -> Vec<(NavTarget, fenix_window::Rect)> {
        let previous = self.active_frame;
        let mut targets = Vec::new();
        for frame in 0..self.frames.len() {
            self.activate_frame(frame);
            targets.extend(self.active_frame_nav_targets(frame));
        }
        self.activate_frame(previous);
        targets
    }

    /// The active frame's own navigation targets, in desktop
    /// coordinates. `frame` is passed in rather than read from
    /// `active_frame` only so the caller's loop index is what ends up
    /// in the keys.
    fn active_frame_nav_targets(&self, frame: usize) -> Vec<(NavTarget, fenix_window::Rect)> {
        let (width, height) = self
            .gpu
            .as_ref()
            .map(|gpu| (gpu.size.width as f32, gpu.size.height as f32))
            .unwrap_or(NAV_REFERENCE_SIZE);
        let (sidebar_px, terminal_h, modeline_top) = self.frame_metrics(height);
        let geometry = self.frame_geometry(width, sidebar_px, terminal_h, modeline_top);

        let (origin_x, origin_y) = (self.frame_origin.0 as f32, self.frame_origin.1 as f32);
        let to_desktop = |rect: fenix_window::Rect| fenix_window::Rect {
            x: rect.x + origin_x,
            y: rect.y + origin_y,
            w: rect.w,
            h: rect.h,
        };

        let mut targets: Vec<(NavTarget, fenix_window::Rect)> =
            geometry.panes.iter().map(|&(pane, rect)| (NavTarget::Pane { frame, pane }, to_desktop(rect))).collect();
        if let Some(rect) = geometry.sidebar_rect {
            targets.push((NavTarget::Sidebar { frame }, to_desktop(rect)));
        }
        targets
    }

    /// `SPC w w`: cycles focus to the next window in a stable pre-order
    /// traversal, wrapping around.
    pub(crate) fn cycle_window(&mut self) {
        self.windows_mut().cycle_next();
        self.wake_caret();
    }

    /// `SPC w q`: closes the focused window. Refuses (no-op) on the last
    /// window rather than quitting the editor -- silently losing your
    /// only view on a stray keypress is a real data-loss risk this
    /// project has consistently avoided elsewhere, a disclosed deviation
    /// from Doom's own "closes Emacs" behavior. The buffer itself is
    /// never closed here, only the pane showing it -- other panes may
    /// still reference the same `BufferId`.
    pub(crate) fn close_window(&mut self) {
        let closed_pane = self.focused_pane_id();
        if self.windows_mut().close_focused() {
            self.workspaces.active_pane_states_mut().remove(&closed_pane);
            self.workspaces.active_scroll_anims_mut().remove(&closed_pane);
            self.workspaces.active_pane_tabs_mut().remove(&closed_pane);
        }
        self.wake_caret();
    }

    /// `SPC w o`: closes every window except the focused one. A permanent
    /// `only`, not Doom's reversible temporary "enlarge" -- restoring the
    /// closed panes would need layout history this tree doesn't keep yet
    /// (a disclosed cut, see `WindowTree::only`'s own doc comment).
    pub(crate) fn only_window(&mut self) {
        self.windows_mut().only();
        self.wake_caret();
    }

    /// `SPC w =`: resets every split's ratio back to an even 0.5.
    pub(crate) fn balance_windows(&mut self) {
        self.windows_mut().balance();
        self.wake_caret();
    }

    /// `SPC TAB n`: a new workspace, seeded with the focused pane's
    /// current buffer (so it starts on something, not a blank scratch
    /// buffer) -- becomes active immediately.
    pub(crate) fn new_workspace(&mut self) {
        // A new workspace starts on a Home of its own -- not on the
        // buffer it was opened from, which is still in the workspace you
        // left (and the dock comes along by itself).
        let id = self.new_home_buffer();
        self.workspaces.new_workspace(id, Cursor::at_start());
        self.ensure_workspace_home();
        self.refresh_project_root();
        self.wake_caret();
    }

    /// `SPC TAB ]`/`SPC TAB [`: cycles the active workspace, wrapping.
    pub(crate) fn next_workspace(&mut self) {
        self.workspaces.next();
        self.wake_caret();
    }

    pub(crate) fn prev_workspace(&mut self) {
        self.workspaces.prev();
        self.wake_caret();
    }

    /// `SPC TAB d`: removes the active workspace. Refuses (no-op) on the
    /// last one -- same safety posture as `close_window` refusing the
    /// last window.
    pub(crate) fn remove_workspace(&mut self) {
        self.remove_active_workspace();
        self.wake_caret();
    }

    /// Workspace `removed` went: the views that were on it are let go --
    /// Task Output and Tool Status open again in a workspace of their own
    /// next time (#29) -- and those on the workspaces after it are one
    /// further left.
    fn workspace_removed(&mut self, removed: usize) {
        // Whether a view on `index` stays, and where it is now.
        let stays = |index: &mut usize| match (*index).cmp(&removed) {
            std::cmp::Ordering::Less => true,
            std::cmp::Ordering::Equal => false,
            std::cmp::Ordering::Greater => {
                *index -= 1;
                true
            }
        };
        if self.task_session.as_mut().is_some_and(|s| !stays(&mut s.workspace_index)) {
            if let Some(session) = self.task_session.take() {
                self.pane_titles.remove(&session.pane);
            }
        }
        if self.tool_status_session.as_mut().is_some_and(|s| !stays(&mut s.workspace_index)) {
            if let Some(session) = self.tool_status_session.take() {
                self.pane_titles.remove(&session.pane);
            }
        }
        if self.history_session.as_mut().is_some_and(|s| !stays(&mut s.workspace_index)) {
            self.history_session = None;
        }
        if self.merge_session.as_mut().is_some_and(|s| !stays(&mut s.workspace_index)) {
            self.merge_session = None;
        }
        if self.git_session.as_mut().is_some_and(|s| !stays(&mut s.workspace_index)) {
            self.git_session = None;
        }
    }

    /// `q` closes Tool Status and Task Output, as it closes the other
    /// pages, rather than starting a macro in a buffer that's only read
    /// (#29). Whether it did.
    fn view_closed_by_q(&mut self, keypress: KeyPress) -> bool {
        if keypress != KeyPress::char('q') || self.vim.mode() != Mode::Normal || !self.vim.is_idle() || self.leader_matcher.is_pending() || self.vim.is_recording() {
            return false;
        }
        let id = self.focused_buffer_id();
        if self.tool_status_session.as_ref().is_some_and(|s| s.buffer == id) {
            self.tool_status_close();
            return true;
        }
        if self.task_session.as_ref().is_some_and(|s| s.buffer == id) {
            self.task_output_close();
            return true;
        }
        false
    }

    /// Closes Tool Status (`q`, `SPC b d`): its buffer, and the workspace
    /// it opened in, when there's another to go to (#29).
    fn tool_status_close(&mut self) {
        let Some(session) = self.tool_status_session.take() else { return };
        self.close_view_workspace(session.workspace_index, session.pane, session.buffer);
    }

    /// A view's buffer closed, with the workspace it was opened in when
    /// another is left to go to, and its pane's title strip.
    fn close_view_workspace(&mut self, workspace_index: usize, pane: fenix_window::WindowId, buffer: BufferId) {
        self.pane_titles.remove(&pane);
        self.buffers.close(buffer);
        self.drop_buffer_from_every_strip(buffer);
        if workspace_index < self.workspaces.len() {
            self.workspaces.switch_to_index(workspace_index);
            if !self.remove_active_workspace() {
                // The last workspace: Home in the pane instead.
                let home = self.ensure_workspace_home();
                for p in self.windows().windows() {
                    if self.windows().content(p) == Some(&buffer) {
                        self.set_pane_content(p, home);
                    }
                }
            }
        }
        self.refresh_project_root();
        self.wake_caret();
    }

    /// Removes the active workspace; the dock, if it was in it, stays
    /// with you in the one that's left.
    fn remove_active_workspace(&mut self) -> bool {
        let removed = self.workspaces.active_index();
        let dock = self.agents_dock_leaving();
        if !self.workspaces.remove_active() {
            return false;
        }
        self.workspace_removed(removed);
        if let Some(dock) = dock {
            self.agents_dock_arrive(dock);
        }
        true
    }

    /// `SPC TAB r`: starts the rename prompt, pre-seeded with the
    /// active workspace's current name.
    pub(crate) fn start_rename_workspace_prompt(&mut self) {
        self.workspace_rename_prompt = Some(self.workspaces.active_name().to_string());
        self.wake_caret();
    }

    /// Routes one keypress to the in-progress `workspace_rename_prompt`
    /// -- same shape as `rename_file_prompt_key`.
    fn workspace_rename_prompt_key(&mut self, key: KeyPress) {
        if key == KeyPress::char('v').with_ctrl() {
            let pasted = self.clipboard_text();
            if let (Some(input), Some(text)) = (&mut self.workspace_rename_prompt, pasted) {
                input.push_str(&text);
            }
            self.wake_caret();
            return;
        }
        let Some(input) = &mut self.workspace_rename_prompt else { return };
        match key.code {
            KeyCode::Named(FenixNamedKey::Escape) => self.workspace_rename_prompt = None,
            KeyCode::Named(FenixNamedKey::Enter) => {
                let input = self.workspace_rename_prompt.take().unwrap_or_default();
                let trimmed = input.trim();
                if !trimmed.is_empty() {
                    self.workspaces.rename_active(trimmed.to_string());
                }
            }
            KeyCode::Named(FenixNamedKey::Backspace) if key.mods.ctrl || key.mods.alt => delete_word_back(input),
            KeyCode::Named(FenixNamedKey::Backspace) => {
                input.pop();
            }
            KeyCode::Char(c) if key.mods == Mods::default() => input.push(c),
            _ => {}
        }
        self.wake_caret();
    }

    /// What to show in place of the modeline while `workspace_rename_
    /// prompt` is active.
    fn workspace_rename_prompt_text(&self) -> Option<String> {
        self.workspace_rename_prompt.as_ref().map(|input| format!("rename workspace to: {input}"))
    }

    /// Switches to the currently-open workspace named `name`, or else
    /// runs its configured `[workspaces]` action to create one --
    /// `picker_confirm`'s `WorkspaceLauncher` arm.
    ///
    /// Every action already knows how to avoid duplicating itself:
    /// `open_git_panel`/`open_jira_panel`/`DockerHost::open` refocus
    /// their existing session instead of opening a second one, and the
    /// VNC extension does the same by host name -- all independent
    /// of what the *workspace* happens to be named. Only the plain
    /// "no live session" action has nothing else to check against, so
    /// `open_named_workspace` does that matching itself.
    ///
    /// Whichever action ran, the workspace it leaves active gets
    /// renamed to `name` if it isn't already -- a freshly created
    /// session's workspace starts out "workspace-N," and without this
    /// it would show as that instead of the name you picked, both in
    /// `SPC TAB TAB`'s list and the modeline's own indicator.
    fn open_configured_workspace(&mut self, name: &str) {
        let Some((_, action)) = self.config.workspaces.iter().find(|(n, _)| n == name) else { return };
        // `Vnc` is asynchronous: the VNC extension dials in the
        // background and returns at once, having touched no workspace at
        // all (its surface opens once the connection succeeds) -- same
        // for an outright failure (an unconfigured host, no extension),
        // which also leaves the active workspace untouched. Comparing the active *index* before and
        // after, rather than assuming the dispatch always lands
        // somewhere new, is what keeps either of those from renaming
        // whatever workspace merely happened to be active at the time.
        let before = self.workspaces.active_index();
        match parse_workspace_action(action) {
            WorkspaceAction::Git => self.open_git(),
            WorkspaceAction::Jira => {
                let _ = self.run_external_command("jira.open");
            }
            WorkspaceAction::Containers => {
                let _ = self.run_external_command("containers.open");
            }
            WorkspaceAction::Vnc(host) => self.open_vnc_host(&host),
            WorkspaceAction::Project(root) => self.open_project_workspace(name, root),
            WorkspaceAction::Plain => self.open_named_workspace(name),
        }
        if self.workspaces.active_index() != before && self.workspaces.active_name() != name {
            self.workspaces.rename_active(name.to_string());
        }
        self.wake_caret();
    }

    /// `vnc:HOST`: host `HOST` of `vnc.hosts` opened by the VNC
    /// extension (its `vnc.connect` service): the VNC extension.
    fn open_vnc_host(&mut self, host: &str) {
        match self.code_server("vnc.connect", None) {
            Some(server) => {
                let then = |app: &mut App, said: Result<String, String>| {
                    if let Err(e) = said {
                        app.set_error(e);
                    }
                };
                self.code_serve(&server, "vnc.connect", host.to_string(), fenix_wasm::host::ServiceCaller::Fenix, wasm_host::Answer::Fenix(Box::new(then)));
            }
            None => self.set_error(format!("vnc:{host} needs the VNC extension -- SPC h x installs it (fenix.vnc)")),
        }
    }

    /// Switches to an already-open plain workspace named `name`, or
    /// else creates one (on a Home of its own, via the same
    /// `new_workspace` `SPC TAB n` itself uses). The "no live session
    /// to check instead" counterpart of the VNC extension's/`open_
    /// docker_panel`'s own idempotence -- see `open_configured_
    /// workspace`'s own doc comment.
    fn open_named_workspace(&mut self, name: &str) {
        if let Some(idx) = self.workspaces.index_of_name(name) {
            self.workspaces.switch_to_index(idx);
            return;
        }
        self.new_workspace();
    }

    /// Switches to the already-open workspace named `name`, or else
    /// creates one and opens `root` in it via `switch_to_project` --
    /// the same "register, then chain into a find-file picker" behavior
    /// `SPC p p`/the dashboard's own project entries already use, so a
    /// `project:` launcher lands you exactly where picking that project
    /// any other way would. Same idempotence shape as `open_named_
    /// workspace`: identity is the launcher's configured *name*, not the
    /// root, so re-running this while that workspace is still open just
    /// switches to it rather than re-registering/re-picking every time.
    fn open_project_workspace(&mut self, name: &str, root: PathBuf) {
        if let Some(idx) = self.workspaces.index_of_name(name) {
            self.workspaces.switch_to_index(idx);
            return;
        }
        // As any other way of opening it: a workspace of its own (the
        // one it has, or a new one), named as the launcher says.
        let key = place_key(&root);
        self.open_project(root, None, true);
        if self.workspaces.active_workspace().project.as_deref().is_some_and(|p| place_key(p) == key) {
            self.workspaces.rename_active(name.to_string());
        }
    }

    pub(crate) fn undo(&mut self) {
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        let tag = buffer.undo_tag();
        if buffer.undo(cursor) {
            self.carets_after_undo(tag, true);
        }
        self.wake_caret();
    }

    pub(crate) fn redo(&mut self) {
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        let tag = buffer.redo_tag();
        if buffer.redo(cursor) {
            self.carets_after_undo(tag, false);
        }
        self.wake_caret();
    }

    /// Off -> Absolute -> Relative -> Off, until Fenix restarts or
    /// `editor.line_numbers` changes (see `LineNumberMode`).
    pub(crate) fn cycle_line_number_mode(&mut self) {
        self.line_number_mode = match self.line_number_mode {
            LineNumberMode::Off => LineNumberMode::Absolute,
            LineNumberMode::Absolute => LineNumberMode::Relative,
            LineNumberMode::Relative => LineNumberMode::Off,
        };
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Shared by the theme picker's confirm (`SPC t p`):
    /// sets the active theme, persists the choice, and requests a
    /// redraw. Save failure is non-fatal, same posture as `refresh_
    /// project_root`'s `known_projects.save()`.
    fn apply_theme(&mut self, theme: &'static Theme) {
        let old = self.theme;
        self.theme = theme;
        self.fade_theme_from(old);
        self.code_theme_changed();
        self.config.theme = Some(self.theme.name.to_string());
        if let Err(err) = self.config.save() {
            eprintln!("fenix: couldn't save theme choice: {err}");
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// `SPC t f`: toggles borderless fullscreen. `winit::window::Window`
    /// is the source of truth (`window.fullscreen()`), not a mirrored
    /// `App`-owned flag -- nothing else in `App` duplicates window-owned
    /// state either, and this stays correct even if the window somehow
    /// left fullscreen by a means other than this toggle (e.g. the OS).
    pub(crate) fn toggle_fullscreen(&mut self) {
        let Some(window) = &self.window else { return };
        if window.fullscreen().is_some() {
            window.set_fullscreen(None);
        } else {
            window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
        }
    }

    /// The focused file read again as `encoding`.
    pub(crate) fn reopen_with_encoding(&mut self, encoding: fenix_core::TextEncoding) {
        let id = self.focused_buffer_id();
        let Some(ob) = self.buffers.get(id) else { return };
        if ob.buffer.is_dirty() {
            return self.set_error("save or discard your edits first: this reads the file again".to_string());
        }
        let Some(path) = ob.buffer.path().map(Path::to_path_buf) else { return };
        match std::fs::read(&path) {
            Ok(bytes) => {
                self.replace_buffer_from_disk(id, &encoding.decode(&bytes));
                if let Some(ob) = self.buffers.get_mut(id) {
                    ob.buffer.set_encoding(encoding);
                }
                self.note_disk_state(id);
                self.set_message(format!("reading {} as {}", path.display(), encoding.label()));
            }
            Err(err) => self.set_error(format!("couldn't read {}: {err}", path.display())),
        }
        self.wake_caret();
    }

    /// The focused file saved as `encoding`, which stays its encoding. One
    /// that can't hold the text changes nothing, and says why.
    pub(crate) fn save_with_encoding(&mut self, encoding: fenix_core::TextEncoding) {
        let ob = self.open_mut();
        let before = ob.buffer.encoding();
        ob.buffer.set_encoding(encoding);
        ob.buffer.mark_unsaved();
        self.save();
        let ob = self.open_mut();
        if ob.buffer.is_dirty() {
            ob.buffer.set_encoding(before);
        }
    }

    /// `SPC t =`/`Ctrl-=`/`SPC t -`/`Ctrl--`/`SPC t 0`/`Ctrl-0`: grows,
    /// shrinks, or resets the body text size at runtime and persists the
    /// choice, same posture as `apply_theme`. A no-op (nothing to resize,
    /// nothing to persist) before the GPU/`TextPipeline` exist yet --
    /// can't happen for a keybinding a running window is dispatching,
    /// but `App::new`/headless tests can call these before `resumed()`.
    fn adjust_font_size(&mut self, size: f32) {
        if self.text.is_none() {
            return;
        }
        // The *logical* size is what's persisted and what the steps
        // move: each frame's pipeline multiplies it by its own
        // monitor's scale factor (see `apply_font_size`), so storing
        // the already-scaled result would bake one monitor's DPI into
        // the config file.
        self.config.font_size = Some(size.clamp(text::MIN_FONT_SIZE, text::MAX_FONT_SIZE));
        self.apply_font_size();
        if let Err(err) = self.config.save() {
            eprintln!("fenix: couldn't save font size: {err}");
        }
        // `char_width` just changed, so how many columns fit the same
        // window width did too -- same reasoning as the window-resize
        // handler's own call.
        self.resync_terminal_cols();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    pub(crate) fn increase_font_size(&mut self) {
        if self.text.is_none() {
            return;
        }
        self.adjust_font_size(self.logical_font_size() + FONT_SIZE_STEP);
    }

    pub(crate) fn decrease_font_size(&mut self) {
        if self.text.is_none() {
            return;
        }
        self.adjust_font_size(self.logical_font_size() - FONT_SIZE_STEP);
    }

    pub(crate) fn reset_font_size(&mut self) {
        self.adjust_font_size(text::FONT_SIZE);
    }

    /// Whether caret-fade/scroll-ease/pulse animations should play at
    /// all -- `config.animations` unset defaults to on (today's look).
    /// A single flag consulted at every animation's own trigger site
    /// (`ensure_cursor_visible`'s scroll-anim insertion, the `Pulse`
    /// vim event, and `about_to_wait`'s blink-fade cadence check) rather
    /// than one central gate, since each animation is started from a
    /// different place and "just render it instantly instead" reads
    /// more clearly at the call site than a wrapped condition would.
    #[cfg(test)]
    pub(crate) fn animations_enabled(&self) -> bool {
        crate::motion::level(&self.config) != crate::motion::Level::Off
    }

    /// `SPC t a`: flips `config.animations` and persists it -- lets the
    /// user A/B a responsiveness complaint against animation cost
    /// without editing `config.ini` by hand.
    pub(crate) fn toggle_animations(&mut self) {
        self.cycle_motion_level();
    }

    /// Resets the caret blink timer so an edit or navigation always leaves
    /// the caret visible instead of possibly mid-blink.
    fn wake_caret(&mut self) {
        let now = Instant::now();
        self.blink_visible = true;
        // Already-elapsed transition: caret snaps to fully visible right
        // away. A fade-in here would read as input lag, not polish -- the
        // soft fade is for idle blinking, not for the moment right after
        // a keypress.
        self.blink_transition_start = now - BLINK_FADE;
        self.next_blink = now + BLINK_INTERVAL;
        self.caret_woken = now;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// The editor's first frame, if it's overdue: what's in the way is
    /// said on stderr -- no surface, no size, a hidden or minimised
    /// window -- the surface made again and a frame asked for, every
    /// `FIRST_FRAME_WITHIN` until one is presented (#65). When it's next
    /// due, while it's awaited.
    fn first_frame_tick(&mut self, now: Instant) -> Option<Instant> {
        let due = self.first_frame_due?;
        if now < due {
            return Some(due);
        }
        let window = self.window.clone();
        let surface = self.gpu.as_ref().map(|g| (g.surface.is_some(), g.size.width, g.size.height, g.health.failures()));
        let (visible, minimized) = window.as_ref().map_or((None, None), |w| (w.is_visible(), w.is_minimized()));
        let next = now + FIRST_FRAME_WITHIN;
        // Minimised, there's nothing to draw into: wait for it.
        if minimized == Some(true) {
            self.first_frame_due = Some(next);
            return Some(next);
        }
        eprintln!("fenix: no frame drawn {FIRST_FRAME_WITHIN:?} after the splash handed over (surface, width, height, failures: {surface:?}; visible {visible:?}, minimised {minimized:?}); making the window's surface again");
        if let (Some(context), Some(gpu), Some(window)) = (&self.gpu_context, self.gpu.as_mut(), window.as_ref()) {
            if let Err(why) = context.recreate_surface(gpu, Arc::clone(window)) {
                self.gpu_watch.want_rebuild(why);
            }
        }
        for frame in 0..self.frames.len() {
            if let Some(window) = self.frame_window(frame) {
                window.request_redraw();
            }
        }
        self.first_frame_due = Some(next);
        Some(next)
    }

    /// The caret's blink, if it's due: true when it changed and is to be
    /// drawn. Past `CARET_BLINKS_FOR` without a key it stays lit (#74).
    fn blink_tick(&mut self, now: Instant) -> bool {
        if now < self.next_blink || self.caret_settled(now) {
            return false;
        }
        self.blink_visible = !self.blink_visible;
        self.blink_transition_start = now;
        self.next_blink = now + BLINK_INTERVAL;
        true
    }

    /// Whether the caret has stopped blinking: lit, done fading, and long
    /// enough since the last key or click.
    fn caret_settled(&self, now: Instant) -> bool {
        self.blink_visible && now.duration_since(self.caret_woken) >= CARET_BLINKS_FOR && now.duration_since(self.blink_transition_start) >= BLINK_FADE.max(self.motion().blink_fade_d)
    }

    /// When a moving layer is next to be drawn, if one moves: now, at
    /// most every `LAYER_TICK` -- what was due is taken, and the next
    /// time set.
    fn layer_frame_tick(&mut self, now: Instant) -> Option<Instant> {
        if !self.layers_moving(now) {
            return None;
        }
        if now >= self.next_layer_frame {
            self.next_layer_frame = now + LAYER_TICK;
            return Some(now);
        }
        Some(self.next_layer_frame)
    }

    /// 0.0 (hidden) to 1.0 (fully visible), eased across `BLINK_FADE` from
    /// whichever state the caret last toggled into.
    fn caret_alpha(&self) -> f32 {
        let prefs = self.motion();
        if !prefs.caret_fade {
            return if self.blink_visible { 1.0 } else { 0.0 };
        }
        let elapsed = Instant::now().duration_since(self.blink_transition_start);
        let t = ease_out_cubic(elapsed.as_secs_f32() / prefs.blink_fade_d.as_secs_f32().max(1e-3));
        if self.blink_visible { t } else { 1.0 - t }
    }

    /// The actual `FenixUserEvent` handling logic, factored out of the
    /// `ApplicationHandler::user_event` trait method so it's directly
    /// callable from tests without needing a real `ActiveEventLoop`
    /// (which nothing outside winit's own running loop can construct)
    /// -- same reasoning `handle_key`/`test_vim_key` already established
    /// for keyboard input.
    fn handle_user_event(&mut self, event: FenixUserEvent) {
        // What comes back on its own every couple of seconds, and most
        // often finds nothing new, says whether it did: an idle window
        // isn't drawn again for nothing (#74). Everything else is drawn.
        let redraw = match event {
            FenixUserEvent::DiskProbed(result) => self.apply_disk_probe(*result),
            FenixUserEvent::GitStatusReady(status) => self.apply_git_status(status),
            FenixUserEvent::Page(pages::PageEvent::ChromeGit(state)) => self.apply_chrome_git(state.map(|s| *s)),
            event => {
                self.handle_drawn_user_event(event);
                true
            }
        };
        if redraw {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    fn handle_drawn_user_event(&mut self, event: FenixUserEvent) {
        match event {
            FenixUserEvent::Accessibility(event) => self.a11y_event(event),
            FenixUserEvent::Woke(slept) => self.machine_woke(slept, Instant::now()),
            FenixUserEvent::Lsp { key, generation, event } => self.apply_lsp_event(key, generation, event),
            FenixUserEvent::LspStarted(start) => self.apply_lsp_started(*start),
            FenixUserEvent::LspDeadline(fanout) => self.lsp_fanout_progress(fanout),
            FenixUserEvent::TaskEvent(buffer_id, run_id, event) => self.apply_task_event(buffer_id, run_id, event),
            FenixUserEvent::TestsStep(tag, event) => self.tests_step_event(tag, event),
            FenixUserEvent::Dap { generation, event } => {
                if self.debug_session.as_ref().is_some_and(|s| s.generation == generation || s.children.iter().any(|c| c.generation == generation)) { self.apply_dap_event_on(generation, event); }
            },
            // Said whether they changed anything, in `handle_user_event`.
            FenixUserEvent::GitStatusReady(_) | FenixUserEvent::DiskProbed(_) => {}
            FenixUserEvent::ExplorerFoldersChanged(folders) => self.explorer_folders_changed(folders, Instant::now()),
            FenixUserEvent::GitMainReady { request_id, buffer, diff } => self.apply_git_main(request_id, buffer, diff),
            FenixUserEvent::GitHistoryDetailReady { request_id, buffer, diff } => {
                self.apply_history_detail(request_id, buffer, diff)
            }
            FenixUserEvent::GitHistoryReady { request_id, data } => self.apply_history(request_id, *data),
            FenixUserEvent::GitFetched { result } => self.apply_git_fetched(result),
            FenixUserEvent::GitRefreshReady { request_id, data } => self.apply_git_refresh(request_id, data),
            FenixUserEvent::TerminalOutput(target, bytes) => self.apply_terminal_output(target, bytes),
            FenixUserEvent::ExplorerListed { target, request, result } => self.apply_explorer_listed(target, request, result),
            FenixUserEvent::FilesListed { root, include_ignored, files } => self.apply_files_listed(root, include_ignored, files),
            FenixUserEvent::RemoteQuestion(question) => self.remote_question(question),
            FenixUserEvent::RemoteConnected(connected) => self.remote_connected(*connected),
            FenixUserEvent::RemoteStatus(host, status) => self.remote_status(host, status),
            FenixUserEvent::RecentProbed(seen) => self.apply_recent_probed(seen),
            FenixUserEvent::Registry(event) => self.apply_registry_event(event),
            FenixUserEvent::Updates(answer) => self.apply_update_answer(answer),
            FenixUserEvent::Wasm { ext, event } => self.apply_wasm_event(ext, event),
            FenixUserEvent::Acp { conn, generation, event } => self.apply_acp_event(conn, generation, event),
            FenixUserEvent::HomeTodos { root, items } => self.apply_home_todos(root, items),
            FenixUserEvent::ExplorerGitStatus { target, request, statuses } => self.apply_explorer_git_status(target, request, statuses),
            FenixUserEvent::ExplorerFound { target, search, found } => self.apply_explorer_found(target, search, found),
            FenixUserEvent::ExplorerShares { host, result } => self.apply_explorer_shares(host, result),
            FenixUserEvent::ExplorerJobProgress(progress) => {
                if let Some(job) = &mut self.explorer_job {
                    job.progress = progress;
                }
                self.wake_caret();
            }
            FenixUserEvent::ExplorerJobDone(outcomes) => self.finish_explorer_job(outcomes),
            FenixUserEvent::ExplorerMeasured { path, total } => self.apply_explorer_measured(path, total),
            FenixUserEvent::ExplorerVolumes(volumes) => {
                self.volumes_loading = false;
                self.volumes = volumes;
                self.wake_caret();
            }
            FenixUserEvent::GutterHunks(started, found) => self.apply_gutter_hunks(started, found),
            FenixUserEvent::TerminalSpawned(TerminalTarget::Panel | TerminalTarget::PanelShell(_), result) => self.apply_terminal_spawned(result.0),
            FenixUserEvent::TerminalSpawned(TerminalTarget::Buffer(id), result) => self.apply_terminal_buffer_spawned(id, result.0),
            FenixUserEvent::Page(event) => self.apply_page_event(event),
            // A new window asked for here is opened by `about_to_wait`:
            // that needs an `&ActiveEventLoop`, which `handle_user_event`
            // doesn't take (it's deliberately the `ActiveEventLoop`-free
            // half of `user_event`, so it stays directly testable).
            FenixUserEvent::Control { call, reply } => {
                // One an extension answers waits for it; the rest are
                // answered now. The client may have hung up either way.
                if let Err((call, reply)) = self.control_by_extension(call, reply) {
                    let answer = self.apply_control(call);
                    let _ = reply.send(answer);
                }
            }
        }
    }

    /// Text that came with no key of its own: an input method's committed
    /// text, or what a `VK_PACKET` carried. Typed a character at a time
    /// through the real key path, so it lands wherever typing would -- a
    /// buffer in Insert, a prompt, a picker's query, a terminal (#37).
    fn receive_text(&mut self, text: &str, event_loop: Option<&ActiveEventLoop>) {
        self.dispatch_keypresses(keymap::text_keypresses(text), event_loop);
    }

    fn dispatch_keypresses(&mut self, keypresses: Vec<KeyPress>, event_loop: Option<&ActiveEventLoop>) {
        if keypresses.is_empty() {
            return;
        }
        self.code_active();
        for keypress in keypresses {
            self.dispatch_keypress(keypress, event_loop);
        }
    }

    /// Every real keypress passes through here exactly once -- the tap
    /// that makes macro recording see everything, including keys a
    /// leader-triggered command or a prompt swallows before `VimState::
    /// handle_key` ever would (see `VimEvent::MacroRecordStart`'s own
    /// doc comment for why the tap has to live here, not in `fenix-
    /// vim`). `route_keypress` does the actual dispatch -- a macro
    /// replay (`VimEvent::MacroPlay`'s handling, inside `route_keypress`
    /// itself) calls `route_keypress` *directly*, deliberately skipping
    /// this tap: a nested `@b` call's own literal two keystrokes get
    /// captured here (as ordinary top-level input), but `b`'s *expanded*
    /// replay must not also be appended on top of that.
    ///
    /// Also where `.`'s own capture lives (`change_capture`), same
    /// always-tap reasoning: `VimState::is_idle()` marks the span
    /// boundary -- a fresh span starts (clearing `change_capture`/
    /// `change_capture_dirty`) whenever the state was idle *before* this
    /// key, and a completed span (idle again *after*) gets promoted to
    /// `last_change_keys` if it actually edited the buffer along the way
    /// (`change_capture_dirty`, set inside `route_keypress` at the one
    /// place `vim.handle_key` is called). `.` itself replays through
    /// `repeat_last_change` -> `route_keypress` directly, same as a
    /// macro, so its own replayed keys never re-enter this tap.
    fn dispatch_keypress(&mut self, keypress: KeyPress, event_loop: Option<&ActiveEventLoop>) {
        // Extra cursors follow whatever edited their text since.
        self.follow_outside_edits();
        self.sync_key_policy();
        self.settle_keys();
        self.note_typing_pause(Instant::now());
        // The right click's menu, open, has the keyboard.
        if self.mouse.menu.is_some() {
            if let Some(action) = self.menu_key(keypress.legacy()) {
                self.run_menu_action(action);
            }
            return;
        }
        if self.vim.is_recording() {
            self.macro_capture.push(keypress);
        }
        if self.vim.is_idle() {
            self.change_capture.clear();
            self.change_capture_dirty = false;
        }
        self.change_capture.push(keypress);
        let before = self.motion_before_key();
        let carets_before = self.caret_mark();
        self.route_keypress(keypress, event_loop);
        // And whatever the key itself did that wasn't theirs (an undo).
        self.follow_outside_edits();
        self.motion_after_key(before, keypress);
        self.note_caret_move(carets_before);
        // Standard keys: back into Insert (or out of it) for where the key
        // left you.
        self.settle_keys();
        // A lesson of the tutorial's, under way, looks at what came of it.
        self.tutorial_check();
        if self.vim.is_idle() {
            if self.change_capture_dirty {
                self.last_change_keys = Some(std::mem::take(&mut self.change_capture));
            } else {
                self.change_capture.clear();
            }
        }
    }

    /// What Vim's keys need of the focused pane that only the host
    /// knows: its buffer's marks (`d'a`, `gi`, Visual's `'a`) and the
    /// lines it shows (`H`, `M`, `L`).
    ///
    /// The lines are worked out only for the keys that read them, so the
    /// other keys (and a long macro's) don't lay the frame out each time.
    fn sync_vim_view(&mut self, key: KeyPress) {
        let (id, pane) = (self.focused_buffer_id(), self.focused_pane_id());
        self.vim.set_marks(self.marks.iter().filter(|(_, entry)| entry.buffer == id).map(|(&name, entry)| (name, entry.char_idx)));
        if !matches!(key.code, KeyCode::Char('H' | 'M' | 'L')) || key.mods.ctrl || key.mods.alt {
            return;
        }
        let viewport = self.pane_text_rows(pane).and_then(|rows| {
            let ob = self.buffers.get(id)?;
            let top = self.pane_state(pane).scroll_line;
            let last = ob.buffer.line_count().saturating_sub(1);
            // Folds and a side by side's filler rows put other lines on
            // the rows than their numbers say; only then are the rows read
            // line by line.
            let plain = self.code_folds.get(&id).is_none_or(|folds| folds.is_empty()) && !self.is_side_by_side_pane(pane);
            if plain {
                return Some((top.min(last), (top + rows.saturating_sub(1)).min(last)));
            }
            let lines = self.pane_display_lines(pane, id);
            let shown = lines.get(top..(top + rows).min(lines.len()))?;
            Some((*shown.first()?, *shown.last()?))
        });
        self.vim.set_viewport(viewport);
    }

    /// `@{register}` (or `@@`, `register` literally `'@'` -- resolved to
    /// `last_played_macro` here, real Vim's own "repeat whichever
    /// register was last played" convention; a `None` `last_played_
    /// macro` is a silent no-op, matching real Vim's "no previously used
    /// register"). Decodes the register's content (`VimState::decode_
    /// register` -- works the same whether it got there by recording,
    /// yanking, or hand-editing) and replays each key via `route_
    /// keypress` directly, *not* `dispatch_keypress` -- see `dispatch_
    /// keypress`'s own doc comment for why a nested `@b` call must not
    /// double-record its own expansion. `replay_budget` is a defensive
    /// cap (not real Vim's own behavior, which really would hang on a
    /// self-referential macro like `qaa@aq` too) shared across nested
    /// replay calls via a single counter on `self`, so a runaway macro
    /// bails out with a modeline-visible message instead of freezing
    /// the app.
    fn play_macro(&mut self, register: char, count: u32, event_loop: Option<&ActiveEventLoop>) {
        let register = if register == '@' { self.last_played_macro } else { Some(register) };
        let Some(register) = register else { return };
        if self.replay_depth >= MAX_MACRO_REPLAY_DEPTH {
            self.set_error("macro replay stopped -- nested too deep (possible infinite recursion)");
            return;
        }
        let Some(keys) = self.vim.decode_register(register) else { return };
        self.last_played_macro = Some(register);
        // The budget is one shared pool across every nested `@` call
        // this replay makes (only reset at true top-level, tracked via
        // `replay_depth`), not per-level -- a self-referential macro
        // recursing through many levels should still hit the same
        // overall cap, not get `MACRO_REPLAY_BUDGET` keys *per level*.
        if self.replay_depth == 0 {
            self.replay_budget = MACRO_REPLAY_BUDGET;
        }
        self.replay_depth += 1;
        'replay: for _ in 0..count.max(1) {
            for key in &keys {
                if self.replay_budget == 0 {
                    self.set_error("macro replay stopped -- too many keys played (possible infinite recursion)");
                    break 'replay;
                }
                self.replay_budget -= 1;
                self.route_keypress(*key, event_loop);
            }
        }
        self.replay_depth -= 1;
    }

    /// What `.` replays: the last change's keys, with `count` (`3.`) in
    /// place of the count they were typed with -- which `.` then keeps,
    /// as Vim's does, so a later bare `.` repeats with it too.
    fn last_change_keys_with(&mut self, count: Option<u32>) -> Option<Vec<KeyPress>> {
        let keys = self.last_change_keys.as_ref()?;
        if let Some(count) = count {
            let counted = fenix_vim::with_count(keys, count);
            self.last_change_keys = Some(counted.clone());
            return Some(counted);
        }
        Some(keys.clone())
    }

    /// `.` -- replays `last_change_keys` (the most recently completed
    /// change-producing key span, captured by `dispatch_keypress`) the
    /// same way `play_macro` replays a register's decoded content:
    /// through `route_keypress` directly, not `dispatch_keypress`, so
    /// the replay's own keys don't get re-tapped into `change_capture`.
    /// `replaying_change` additionally suppresses `change_capture_
    /// dirty`'s bookkeeping for the duration (see its own doc comment)
    /// -- without it, the replayed edits would make the *outer* `.`
    /// keypress's own span look dirty too, and `last_change_keys` would
    /// get overwritten with just `['.']` on every repeat. A silent no-op
    /// if nothing's been recorded yet.
    fn repeat_last_change(&mut self, count: Option<u32>, event_loop: Option<&ActiveEventLoop>) {
        let Some(keys) = self.last_change_keys_with(count) else { return };
        self.replaying_change = true;
        for key in &keys {
            self.route_keypress(*key, event_loop);
        }
        self.replaying_change = false;
    }

    /// Right before dispatching a bare `p`/`P` in Normal mode (the only
    /// place those keys mean "paste" -- see `fenix_vim::keymaps::normal_
    /// trie`), pulls whatever's currently on the OS clipboard into Vim's
    /// unnamed register, so the paste sees anything copied outside Fenix
    /// since the last sync, not just Fenix's own last yank/delete.
    /// Gated on `!is_pending()` so a count prefix (`3p`) still resolves
    /// through here (counts aren't tracked via the pending-sequence
    /// matcher) while a `g`-prefixed or operator-pending sequence never
    /// misreads its own continuation key as a paste.
    fn pull_clipboard_before_paste(&mut self, keypress: KeyPress) {
        // Insert's `Ctrl-R "` (or `+`, `*`) puts the clipboard in too.
        let ctrl_r = self.vim.is_awaiting_insert_register() && keypress.mods == Mods::default() && matches!(keypress.code, KeyCode::Char('"' | '+' | '*'));
        // Visual too: `p`/`P` there replace the selection with it.
        if !ctrl_r && (!matches!(self.vim.mode(), Mode::Normal | Mode::Visual) || self.vim.is_pending() || keypress.mods != Mods::default()) {
            return;
        }
        if !ctrl_r && !matches!(keypress.code, KeyCode::Char('p') | KeyCode::Char('P')) {
            return;
        }
        let Some(clipboard) = self.clipboard.as_mut() else { return };
        let Ok(text) = clipboard.get_text() else { return };
        // No per-entry regtype metadata on a plain OS clipboard -- a
        // trailing newline is the same linewise signal Neovim's own
        // clipboard provider uses to infer "this was a whole line" from
        // plain text.
        let linewise = text.ends_with('\n');
        self.clipboard_mirror = text.clone();
        self.vim.set_register(text, linewise);
    }

    /// After every dispatched key, mirrors Vim's unnamed register onto
    /// the OS clipboard if this keystroke just changed it (yank, delete,
    /// change -- anything `fenix_vim::VimState` writes it from). Compared
    /// against `clipboard_mirror` rather than gated on `VimEvent::Pulse`,
    /// since delete deliberately doesn't raise a pulse (see `VimEvent::
    /// Pulse`'s own doc comment) but still needs to reach the clipboard --
    /// otherwise `dd` then `p` in Fenix would silently diverge from
    /// whatever's pasted elsewhere.
    fn push_clipboard_after_edit(&mut self) {
        let _profile = crate::profile::Scope::new("write clipboard");
        let (text, _linewise) = self.vim.register();
        if text == self.clipboard_mirror {
            return;
        }
        let text = text.to_string();
        let Some(clipboard) = self.clipboard.as_mut() else { return };
        if clipboard.set_text(text.clone()).is_ok() {
            self.clipboard_mirror = text;
        }
    }

    /// The OS clipboard's current text, if any -- `None` if there's no
    /// clipboard (headless tests), it's empty, or it doesn't hold text.
    /// `Ctrl-V` in every modeline-style prompt (`find_file_prompt_key`
    /// and its siblings) pastes this in; Vim's own `p`/`P` paste has its
    /// own separate path (`pull_clipboard_before_paste`) since it also
    /// needs the linewise/charwise distinction these plain-text prompts
    /// don't have.
    fn clipboard_text(&mut self) -> Option<String> {
        self.clipboard.as_mut()?.get_text().ok()
    }

    /// Keeps the cursor's line within the *focused pane's* visible
    /// window, scrolling as needed. Must be called with the same
    /// `visible_lines` used to render. Every pane owns its own scroll
    /// independently (see `PaneState`'s own doc comment) -- only the
    /// focused one auto-follows the cursor like this; others just render
    /// wherever they were last left.
    fn ensure_cursor_visible(&mut self, visible_lines: usize) {
        let buffer_id = self.focused_buffer_id();
        let pane = self.focused_pane_id();
        let (line, scroll_line, total) = {
            let ob = self.buffers.get(buffer_id).expect("focused window always has an open buffer");
            let cursor = self.pane_state(pane).cursor;
            let document_line = ob.buffer.line_col(&cursor).0;
            let display = self.pane_display_lines(pane, buffer_id);
            let display_line = display.iter().position(|&row| row == document_line).unwrap_or(document_line);
            (display_line, self.pane_state(pane).scroll_line, display.len())
        };
        // A jump into this pane since the last frame: centred if out of view.
        let jumped = self.jump_view == Some(pane);
        if jumped {
            self.jump_view = None;
        }
        // A wrapped pane follows by rows, and snaps.
        if self.follow_cursor_by_rows(pane, buffer_id, visible_lines, jumped) {
            return;
        }
        let target = scroll_for_cursor(scroll_line, line, visible_lines, total, jumped);
        if target != scroll_line {
            let jump = target.abs_diff(scroll_line);
            // A jump bigger than a few screens (`G`, a search landing far
            // away) snaps instantly rather than blurring through an ease.
            // A target arriving while the *previous* ease for this pane
            // hasn't finished yet snaps too: that only happens when scroll
            // targets are coming in faster than the scroll ease can settle
            // (held-key repeat, e.g. holding `j`), and re-triggering a
            // fresh 150ms ease on every one of those keystrokes would
            // otherwise make `rendered_scroll` perpetually chase a few
            // lines behind the cursor instead of ever catching up -- the
            // opposite of the "never sits between a keypress and its
            // effect" goal this animation was built for.
            // A held key keeps snapping; anything else arriving mid-ease
            // (the wheel's next notch) eases on from where the view is.
            let chasing = self.motion.key_repeat && self.workspaces.active_scroll_anims().contains_key(&pane);
            if !self.motion().smooth_scroll || jump > visible_lines.saturating_mul(SCROLL_SNAP_SCREENS) || chasing {
                self.workspaces.active_scroll_anims_mut().remove(&pane);
                self.pane_state_mut(pane).rendered_scroll = target as f32;
            } else {
                let from = self.pane_state(pane).rendered_scroll;
                self.workspaces.active_scroll_anims_mut().insert(pane, ScrollAnim { from, to: target, started: Instant::now() });
            }
            self.pane_state_mut(pane).scroll_line = target;
        }
        self.update_rendered_scroll();
    }

    /// A pane without the focus scrolled to its cursor, centred when out of
    /// view, as a jump in the focused pane is.
    fn reveal_cursor(&mut self, pane: fenix_window::WindowId, buffer_id: BufferId, visible_lines: usize) {
        if self.follow_cursor_by_rows(pane, buffer_id, visible_lines, true) {
            return;
        }
        let Some(ob) = self.buffers.get(buffer_id) else { return };
        let document_line = ob.buffer.line_col(&self.pane_state(pane).cursor).0;
        let display = self.pane_display_lines(pane, buffer_id);
        let line = display.iter().position(|&row| row == document_line).unwrap_or(document_line);
        let target = scroll_for_cursor(self.pane_state(pane).scroll_line, line, visible_lines, display.len(), true);
        self.workspaces.active_scroll_anims_mut().remove(&pane);
        let state = self.pane_state_mut(pane);
        state.scroll_line = target;
        state.rendered_scroll = target as f32;
    }

    /// Horizontal analog of `ensure_cursor_visible` -- keeps the cursor's
    /// *visual* column within the focused pane's visible window, snapping
    /// `scroll_col` instantly (no `rendered_scroll`-style easing on this
    /// axis; see `PaneState.scroll_col`'s own doc comment for why).
    /// `visual_cols` needs the cursor's own line's tab-expanded column,
    /// not its raw character column -- `content_spans` slices the tab-
    /// *expanded* display text, where "1 char = 1 visual column" holds
    /// (the same invariant `remap_col`'s own doc comment relies on), so
    /// comparing a raw column against `scroll_col` would drift on any
    /// line with a tab before the cursor.
    fn ensure_cursor_visible_horizontally(&mut self, visible_cols: usize) {
        let pane = self.focused_pane_id();
        let buffer_id = self.focused_buffer_id();
        let cursor_col = {
            let ob = self.buffers.get(buffer_id).expect("focused window always has an open buffer");
            let cursor = self.pane_state(pane).cursor;
            let (line, col) = ob.buffer.line_col(&cursor);
            let start = ob.buffer.line_start_char(line);
            let len = ob.buffer.line_len(line);
            let line_text = ob.buffer.text_range(start, start + len);
            if line_text.contains('\t') {
                let tab_stops = self.tab_stops_for(buffer_id);
                let (_, col_map) = tabstops::expand_line(&line_text, &tab_stops);
                tabstops::visual_col(&col_map, col)
            } else {
                col
            }
        };
        let pane_state = self.pane_state_mut(pane);
        pane_state.scroll_col = scroll_to_include(pane_state.scroll_col, cursor_col, visible_cols);
    }

    /// Advances the focused pane's `rendered_scroll` toward its
    /// `scroll_line` if a transition is in flight, clearing it once
    /// settled.
    fn update_rendered_scroll(&mut self) {
        let pane = self.focused_pane_id();
        let scroll_line = self.pane_state(pane).scroll_line;
        let Some(anim) = self.workspaces.active_scroll_anims().get(&pane) else {
            self.pane_state_mut(pane).rendered_scroll = scroll_line as f32;
            return;
        };
        let (from, to, started) = (anim.from, anim.to, anim.started);
        let t = Instant::now().duration_since(started).as_secs_f32() / self.motion().scroll_d.as_secs_f32().max(1e-3);
        if t >= 1.0 {
            self.pane_state_mut(pane).rendered_scroll = to as f32;
            self.workspaces.active_scroll_anims_mut().remove(&pane);
        } else {
            let to = to as f32;
            self.pane_state_mut(pane).rendered_scroll = from + (to - from) * ease_out_cubic(t);
        }
    }

    /// The focused pane's display line rendering starts from --
    /// `rendered_scroll` rounded down (not `scroll_line`, which is only
    /// the *target* `rendered_scroll` is easing toward).
    #[cfg(test)]
    fn render_base_line(&self) -> usize {
        self.pane_state(self.focused_pane_id()).rendered_scroll.floor().max(0.0) as usize
    }

    /// The display name for `buffer_id` -- its filename if it has a
    /// path, else a kind-specific placeholder (`*dashboard*`, `*docker*`,
    /// `*rename*` for names being edited, or `[No Name]`). Shared by the
    /// modeline (which only ever names the *focused* pane) and the
    /// per-pane title strips `redraw()` draws above every visible pane
    /// -- with a split open, two different files need their own labels
    /// to tell them apart at a glance, not just the one the modeline
    /// shows for whichever pane happens to be focused.
    fn buffer_display_name(&self, buffer_id: BufferId) -> String {
        let Some(ob) = self.buffers.get(buffer_id) else { return "[No Name]".to_string() };
        ob.buffer.path().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| {
            if ob.kind == BufferKind::WorkspaceEdit {
                "*refactor*".to_string()
            } else if ob.kind == BufferKind::Dashboard {
                "home".to_string()
            } else if ob.kind == BufferKind::Explorer {
                "*rename*".to_string()
            } else if ob.kind == BufferKind::Git {
                "*git*".to_string()
            } else if ob.kind == BufferKind::TaskOutput {
                // An extension's task beside the task pane: by its name.
                self.ref_view_names.get(&buffer_id).cloned().unwrap_or_else(|| "*task output*".to_string())
            } else if ob.kind == BufferKind::Debug {
                "*debug*".to_string()
            } else if ob.kind == BufferKind::Prompt {
                "*agent prompt*".to_string()
            } else if ob.kind == BufferKind::ToolStatus {
                "*tool status*".to_string()
            } else if ob.kind == BufferKind::Diff {
                "*diff*".to_string()
            } else if ob.kind == BufferKind::Graph {
                "*graph*".to_string()
            } else if ob.kind == BufferKind::Merge {
                "*merge*".to_string()
            } else if ob.kind == BufferKind::Terminal {
                // Numbered on creation (`terminal_buffer_labels`) so
                // several open at once are tellable apart here and in
                // `SPC b b`; the bare fallback only covers the instant
                // between the buffer existing and being labelled.
                self.terminal_buffer_labels.get(&buffer_id).cloned().unwrap_or_else(|| "*terminal*".to_string())
            } else if ob.kind == BufferKind::Page {
                self.page_title(buffer_id)
            } else if let Some(name) = self.ref_view_names.get(&buffer_id) {
                name.clone()
            } else if let Some(name) = self.code_scratch_name(buffer_id) {
                name
            } else if let Some(name) = self.tutorial_buffer_name(buffer_id) {
                name
            } else {
                "[No Name]".to_string()
            }
        })
    }

    /// Sets a plain (non-error) status message -- shown in the modeline
    /// for `MESSAGE_DURATION` (`modeline_pieces`). For routine
    /// confirmations ("saved", "tags refreshed"), not failures -- use
    /// `set_error` for those, which also tints the text.
    fn set_message(&mut self, text: impl Into<String>) {
        let text = self.keys_text(&text.into());
        self.status_message = Some(StatusMessage { text, is_error: false, set_at: Instant::now() });
    }

    /// Sets an error status message -- same display as `set_message`,
    /// tinted red (`git_conflicted`'s accent). This is the replacement
    /// for what used to be a silent `eprintln!` at most of this file's
    /// failure sites: invisible in a normally-launched GUI window, so
    /// the user had no way to know a search/save/format/etc. failed.
    fn set_error(&mut self, text: impl Into<String>) {
        let text = self.keys_text(&text.into());
        self.status_message = Some(StatusMessage { text, is_error: true, set_at: Instant::now() });
    }

    fn mode_badge_label(&self) -> &'static str {
        if self.wasm.captured.is_some() {
            return "CAPTURE";
        }
        // Keys go to a shell: Vim's mode means nothing meanwhile (#42).
        if self.terminal_has_keyboard() {
            return "TERMINAL";
        }
        if self.debug_mode_active() {
            return "DEBUG";
        }
        if let Some(badge) = self.standard_badge() {
            return badge;
        }
        if self.vim.mode() == Mode::Visual { self.vim.visual_kind().label() } else { self.vim.mode().label() }
    }

    /// The text currently occupying the app's one shared "capturing
    /// prompt" slot -- whichever of Vim's own `:command`/`/`-`?`-search,
    /// the project-wide grep query, or one of this app's many single-
    /// line prompts/confirmations is active right now. `None` when
    /// nothing is capturing input, the common case. These are mutually
    /// exclusive states (`route_keypress`'s own capturing-prompt
    /// dispatch already treats them that way), so this priority order
    /// only matters for readability, not correctness. Feeds `prompt_
    /// popup` -- the only thing that changed here from this function's
    /// own predecessor (the old `modeline_command_text` local `redraw`
    /// used to compute inline) is *where* this text ends up shown, not
    /// what it is or how it's computed; every individual `*_text()`
    /// function this calls is untouched.
    fn active_prompt_text(&self) -> Option<String> {
        if let Some(question) = self.vim.substitute_confirm_prompt() {
            return Some(question);
        }
        if self.vim.mode() == Mode::Search {
            let prefix = if self.vim.search_forward() { "/" } else { "?" };
            let count = match self.search_count() {
                Some((_, 0)) if !self.vim.search_query().is_empty() => "   no match".to_string(),
                Some((i, n)) => format!("   [{i}/{n}]"),
                None => String::new(),
            };
            return Some(format!("{prefix}{}{count}", self.vim.search_query()));
        }
        if self.vim.mode() == Mode::Command {
            let preview = match self.vim.substitute_preview(&self.open().buffer, &self.cursor()) {
                Some(Ok(p)) if p.count == 0 => "   no match".to_string(),
                Some(Ok(p)) => {
                    let plural = if p.count == 1 { "" } else { "es" };
                    match &p.first {
                        Some((line, before, after)) => format!("   {} match{plural} · {}: {} → {}", p.count, line + 1, before.trim(), after.trim()),
                        None => format!("   {} match{plural}", p.count),
                    }
                }
                Some(Err(why)) => format!("   ✗ {}", why.lines().last().unwrap_or("")),
                None => String::new(),
            };
            return Some(format!(":{}{preview}", self.vim.command_line()));
        }
        self.explorer_job_text().or_else(|| self.rename_confirm_text()).or_else(|| self.explorer_prompt_text())
            .or_else(|| self.code_prompt_text())
            .or_else(|| self.git_confirm_text())
            .or_else(|| self.git_prompt_text())
            .or_else(|| self.find_file_prompt_text())
            .or_else(|| self.rename_file_prompt_text())
            .or_else(|| self.lsp_rename_prompt_text())
            .or_else(|| self.workspace_rename_prompt_text())
            .or_else(|| self.goto_line_prompt_text())
            .or_else(|| self.delete_file_confirm_text())
            .or_else(|| self.server_install_confirm_text())
    }

    /// (badge background, badge text color) for the current mode. Visual's
    /// Whether the caret should render as a full-cell block (Normal,
    /// Visual, Replace, Command) rather than the thin Insert-mode bar --
    /// matches real Vim's own cursor-shape convention (block outside
    /// Insert, thin bar while actively typing).
    fn caret_is_block(&self) -> bool {
        // Standard keys' caret is a bar, always.
        !self.standard_keys() && self.vim.mode() != Mode::Insert
    }

    /// three kinds all share one accent (matching orbit-emacs's own
    /// evil-state table, which has a single "Visual" entry) -- only the
    /// badge's label text differs between them.
    fn mode_colors(&self) -> ([f32; 4], glyphon::Color) {
        let theme = self.theme;
        if self.active_explorer_target().is_some() {
            return (theme.mode_explorer, theme.mode_text_dark);
        }
        if self.main_view == MainView::Picker {
            return (theme.mode_picker, theme.mode_text_dark);
        }
        // The stop's own colour, the gutter's bar.
        if self.debug_mode_active() {
            return (glyphon_to_rgba(theme.git_modified), theme.mode_text_dark);
        }
        // Standard keys: one quiet colour, but for a selection.
        if self.standard_keys() && matches!(self.vim.mode(), Mode::Normal | Mode::Insert) {
            return (theme.mode_normal, theme.mode_text_dark);
        }
        match self.vim.mode() {
            Mode::Normal => (theme.mode_normal, theme.mode_text_dark),
            Mode::Insert => (theme.mode_insert, theme.mode_text_dark),
            Mode::Visual => (theme.mode_visual, theme.mode_text_light),
            Mode::Replace => (theme.mode_replace, theme.mode_text_dark),
            Mode::Command => (theme.mode_command, theme.mode_text_dark),
            // Same "prompt-like input" family as `:` -- not a new theme
            // color, just reusing the command one.
            Mode::Search => (theme.mode_command, theme.mode_text_dark),
        }
    }

    /// `Custom` stops for a `BufferKind::Table` buffer, else the buffer-
    /// wide `Fixed` default from `config.tab_width` (real Vim's own
    /// `:set tabstop`, `DEFAULT_TAB_WIDTH` if unset).
    fn tab_stops_for(&self, id: BufferId) -> TabStops {
        match self.table_views.get(&id) {
            Some(view) => TabStops::Custom(view.stops.clone()),
            None => TabStops::Fixed(self.effective_tab_width()),
        }
    }

    /// Recomputes `id`'s `TableView` from its *current* buffer text if
    /// it doesn't have one yet, or if `edit_count()` has moved since the
    /// last computation -- the auto-realign every edit gets for free,
    /// mirroring the `edit_count`-diff idiom `syntax_highlights_for_
    /// visible_range` already uses to decide "did anything change." A
    /// no-op for a buffer that isn't (or is no longer) `BufferKind::
    /// Table`. Column names are synthesized (`Column 1`, `Column 2`,
    /// ...) -- the source data's real tab-separated content has no
    /// header row `parse` could reliably distinguish from an ordinary
    /// data row.
    fn sync_table_view(&mut self, id: BufferId) {
        let Some(ob) = self.buffers.get(id) else { return };
        if ob.kind != BufferKind::Table {
            return;
        }
        let edit_count = ob.buffer.edit_count();
        if self.table_views.get(&id).is_some_and(|v| v.last_edit_count == edit_count) {
            return;
        }
        let rows = fenix_table::parse(&ob.buffer.text(), '\t');
        let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
        let columns: Vec<String> = (0..column_count).map(|i| format!("Column {}", i + 1)).collect();
        let widths = fenix_table::column_widths(&columns, &rows);
        let stops = fenix_table::tab_stops(&widths, TABLE_COLUMN_GAP);
        self.table_views.insert(id, TableView { columns, stops, last_edit_count: edit_count });
    }

    /// This pane's title-bar text for a `BufferKind::Table` buffer --
    /// column names, space-padded to line up with the same `stops` the
    /// renderer expands the content's real tabs to, so header and
    /// content land in identical pixel columns (both ultimately paid out
    /// in the same `char_width`). `None` for anything else, so the
    /// caller falls back to the ordinary `buffer_display_name`.
    fn table_view_header(&self, id: BufferId) -> Option<String> {
        let view = self.table_views.get(&id)?;
        let mut header = String::new();
        let mut col_start = 0usize;
        for (name, &stop) in view.columns.iter().zip(&view.stops) {
            header.push_str(name);
            let pad = stop.saturating_sub(col_start + name.chars().count());
            header.push_str(&" ".repeat(pad));
            col_start = stop;
        }
        Some(header)
    }

    /// `SPC f t`: toggles the focused buffer between `Text` and `Table`
    /// -- no synthetic buffer, no distinct copy: the *same* buffer, same
    /// path, same undo history, just retagged in place, since the file's
    /// real tab-separated content is already exactly what the elastic-
    /// column renderer needs to align it. A no-op on anything but
    /// `Text`/`Table` (Dashboard/Explorer/Docker/Git buffers have no
    /// meaningful table view).
    pub(crate) fn toggle_table_view(&mut self) {
        let id = self.focused_buffer_id();
        let Some(ob) = self.buffers.get_mut(id) else { return };
        match ob.kind {
            BufferKind::Table => {
                ob.kind = BufferKind::Text;
                self.table_views.remove(&id);
            }
            BufferKind::Text => {
                ob.kind = BufferKind::Table;
                self.sync_table_view(id);
            }
            _ => {}
        }
    }

    /// 0-indexed tab-occurrence column the cursor currently sits in on
    /// its own line, for a `BufferKind::Table` buffer's `]`/`[`/`j`/`k`
    /// key interception -- counts real `\t` characters before the
    /// cursor. A cursor past every tab on its line (the last column, or
    /// a short ragged row) reports one past the last tab, same convention
    /// `tab_index` uses in `tabstops::expand_line`.
    fn table_current_column(&self) -> usize {
        let buffer = &self.open().buffer;
        let cursor_idx = self.cursor().char_idx;
        let line = buffer.line_col(&self.cursor()).0;
        let line_start = buffer.line_start_char(line);
        buffer.text_range(line_start, cursor_idx).matches('\t').count()
    }

    /// The char index just after `line`'s `column`-th tab (0-indexed),
    /// or the line's own end if it has fewer than `column` tabs -- the
    /// landing spot for a table view's column navigation, both `]`/`[`
    /// (same row) and `j`/`k` (adjacent row, same column).
    fn table_column_start(&self, line: usize, column: usize) -> usize {
        let buffer = &self.open().buffer;
        let line_start = buffer.line_start_char(line);
        if column == 0 {
            return line_start; // field 0 starts at the line itself, no preceding tab
        }
        let len = buffer.line_len(line);
        let line_text = buffer.text_range(line_start, line_start + len);
        // Field `column` (>= 1) starts just after tab occurrence `column
        // - 1` (0-indexed): tab 0 separates field 0 from field 1, tab 1
        // separates field 1 from field 2, and so on.
        match line_text.match_indices('\t').nth(column - 1) {
            Some((byte_idx, _)) => line_start + line_text[..byte_idx].chars().count() + 1,
            None => line_start + len,
        }
    }

    /// `]`/`[` on a `BufferKind::Table` buffer: moves to the start of
    /// the next/previous column on the current row.
    fn table_move_column(&mut self, delta: isize) {
        let current = self.table_current_column() as isize;
        let target = (current + delta).max(0) as usize;
        let line = self.open().buffer.line_col(&self.cursor()).0;
        let idx = self.table_column_start(line, target);
        let pane = self.focused_pane_id();
        let cursor = &mut self.pane_state_mut(pane).cursor;
        cursor.char_idx = idx;
        cursor.sticky_col = 0;
    }

    /// `j`/`k` on a `BufferKind::Table` buffer: moves a row while
    /// staying in the *same elastic column* -- plain char-based `sticky_
    /// col` doesn't work here, since rows have different raw lengths up
    /// to the same visual column (that's the whole point of elastic
    /// alignment), so this counts tabs instead of chars to find "same
    /// column."
    fn table_move_row(&mut self, delta: isize) {
        let column = self.table_current_column();
        let line = self.open().buffer.line_col(&self.cursor()).0;
        // `visual_line_count`, not `line_count` -- a buffer ending in
        // `\n` (every table view's real file content, per convention)
        // would otherwise let this land on ropey's phantom trailing
        // empty line past the real last row.
        let line_count = self.open().buffer.visual_line_count();
        let Some(target_line) = line.checked_add_signed(delta).filter(|&l| l < line_count) else { return };
        let idx = self.table_column_start(target_line, column);
        let pane = self.focused_pane_id();
        let cursor = &mut self.pane_state_mut(pane).cursor;
        cursor.char_idx = idx;
        cursor.sticky_col = 0;
    }

    /// Character width of the line-number gutter, including one trailing
    /// column of padding before the text starts; 0 when the gutter is off.
    /// Sized to the buffer's total line count regardless of Absolute vs.
    /// Relative, so the gutter doesn't resize if that mode is ever toggled
    /// at runtime -- matching how Vim shares one `numberwidth` between
    /// `number` and `relativenumber` instead of sizing them separately.
    /// `ob` is passed explicitly (rather than always reading the focused
    /// buffer) so a split's every visible pane gets a gutter sized to
    /// *its own* buffer's line count, not just the focused one's.
    fn gutter_chars(&self, ob: &OpenBuffer) -> usize {
        if ob.kind == BufferKind::Dashboard
            || ob.kind == BufferKind::Explorer
            || ob.kind == BufferKind::Git
            || ob.kind == BufferKind::Table
            || ob.kind == BufferKind::TaskOutput
            || ob.kind == BufferKind::Debug
            || ob.kind == BufferKind::ToolStatus
            // The diff view draws its own old/new line-number columns
            // (see `diff_view::content_row`), which are the real line
            // numbers here -- a buffer-line gutter next to them would
            // be a third, meaningless column.
            || ob.kind == BufferKind::Diff
            || ob.kind == BufferKind::Graph
            // The merge view's own two columns are the layout; a line
            // gutter beside them would shift one column and not the
            // other, which is exactly the alignment it exists to keep.
            || ob.kind == BufferKind::Merge
            // A terminal's columns are the shell's own; a gutter would
            // shift every one of them out from under what the shell
            // thinks it drew.
            || ob.kind == BufferKind::Terminal
            || ob.kind == BufferKind::Page
            // A prompt (the agents' composer) is a box you type in: what
            // you wrote, with no line numbers beside it.
            || ob.kind == BufferKind::Prompt
        {
            return 0;
        }
        let blame = self.blame_width(ob);
        if self.line_number_mode == LineNumberMode::Off {
            return blame;
        }
        // The test column (§9.1, step 5): a cell on the left of the
        // numbers in a file a provider found tests in.
        ob.buffer.visual_line_count().max(1).to_string().len() + 1 + blame + usize::from(self.fold_column(ob)) + usize::from(self.tests_column(ob))
    }

    /// Rich-text spans for one pane's content area covering `rows` screen
    /// rows starting at `render_base_line`: each row that maps to a real
    /// buffer line gets an optional gutter prefix (its line number, right-
    /// aligned, brighter on the cursor's own line) followed by that line's
    /// text; rows past the end of the buffer get a `~` marker instead,
    /// matching Vim's convention for "there's no line here" -- shown even
    /// with the gutter off, same as Vim shows it regardless of `number`.
    /// `syntax_highlights` (document-wide byte ranges, already resolved to
    /// colors) splits each line's text into colored sub-spans instead of
    /// one flat `theme.fg` span when non-empty. `ob` (rather than always
    /// the focused buffer) is what makes every visible pane -- not just
    /// the focused one -- show its own buffer's real content.
    ///
    /// `dashboard_pad`, when `Some`, replaces the real-gutter/tilde logic
    /// entirely: index `buffer_line` gives that specific row's own
    /// horizontal-centering pad (in characters, from `dashboard_center_
    /// offset` -- different rows can want different padding, e.g. the
    /// banner block and the content block below it center independently)
    /// written as literal leading blank characters, since `TextArea.left`
    /// has no geometric offset to hook a shift into.
    ///
    /// `cursor_line` is `ob`'s *pane's* own cursor row (relative-gutter
    /// numbering needs it) -- passed in rather than read off `ob.cursor`
    /// now that cursor is pane-owned, not buffer-owned (see `PaneState`'s
    /// own doc comment), so this stays correct for any pane, not just
    /// the focused one.
    ///
    /// `tab_stops` governs how a literal `\t` in the line's own text
    /// expands (see the `tabstops` module) -- a line with no tab at all
    /// never consults it and renders exactly as before. A line that
    /// *does* contain one still gets full `syntax_highlights` coloring,
    /// via `remap_highlights_for_display` translating the highlight
    /// ranges (byte offsets against the real buffer text) onto the
    /// tab-expanded display string's own offsets -- tab-indented C/
    /// Bash/Python files (all have a tree-sitter grammar registered in
    /// this project) keep working, not just delimited data files with
    /// no grammar in the first place.
    ///
    /// `scroll_col` (visual columns, post-tab-expansion -- see `remap_
    /// col`'s own doc comment for why) drops that many leading
    /// characters from each row's *code* text only, via `skip_chars` --
    /// the gutter prefix above is built and pushed before this runs, so
    /// it's never touched. A line shorter than `scroll_col` renders
    /// with no code text at all for that row, same as a real editor
    /// showing nothing once you've scrolled past a short line's end.
    #[allow(clippy::too_many_arguments)] // a plain data parameter list, not a design smell worth a struct for one private call site
    fn content_spans(
        &self,
        ob: &OpenBuffer,
        render_base_line: usize,
        rows: usize,
        gutter_chars: usize,
        dashboard_pad: Option<&[usize]>,
        syntax_highlights: &[(std::ops::Range<usize>, glyphon::Color)],
        cursor_line: usize,
        tab_stops: &TabStops,
        scroll_col: usize,
    ) -> Vec<(String, glyphon::Color)> {
        self.content_spans_decorated(ob, render_base_line, rows, gutter_chars, dashboard_pad, syntax_highlights, cursor_line, tab_stops, scroll_col, &HashMap::new())
    }

    /// `content_spans`, with the lines an extension decorated laid out as
    /// `decorated` has them (by buffer line): their inlays and
    /// replacements in their own colours, the syntax's moved along.
    #[allow(clippy::too_many_arguments)]
    fn content_spans_decorated(
        &self,
        ob: &OpenBuffer,
        render_base_line: usize,
        rows: usize,
        gutter_chars: usize,
        dashboard_pad: Option<&[usize]>,
        syntax_highlights: &[(std::ops::Range<usize>, glyphon::Color)],
        cursor_line: usize,
        tab_stops: &TabStops,
        scroll_col: usize,
        decorated: &HashMap<usize, crate::decorations::LineLayout>,
    ) -> Vec<(String, glyphon::Color)> {
        let theme = self.theme;
        let visual_lines = ob.buffer.visual_line_count();
        let mut spans = Vec::new();
        // The fold markers' column: ▾ on a fold's first line, ▸ when it's
        // closed.
        let fold_marks = if dashboard_pad.is_none() && gutter_chars > 0 { self.fold_marks(ob) } else { None };
        // The test column's marks, for these rows' lines only.
        let test_cells = if dashboard_pad.is_none() && gutter_chars > 0 && rows > 0 { self.tests_cells(ob, render_base_line..=render_base_line + rows - 1) } else { None };
        for r in 0..rows {
            let buffer_line = render_base_line + r;
            let has_line = buffer_line < visual_lines;
            self.push_gutter(ob, buffer_line, has_line, gutter_chars, dashboard_pad, cursor_line, fold_marks.as_ref(), test_cells.as_ref(), &mut spans);
            if has_line {
                self.push_line_text(ob, buffer_line, scroll_col..scroll_col.saturating_add(MAX_ROW_CHARS), syntax_highlights, tab_stops, decorated, &mut spans);
            }
            if r + 1 < rows {
                spans.push(("\n".to_string(), theme.fg));
            }
        }
        spans
    }

    /// The start of a content row: `dashboard_pad`'s pad, or the gutter
    /// -- `buffer_line`'s blame, test mark (`test_cells`, with the test
    /// column), number (`~` past the end, `has_line` false) and fold
    /// marker -- or, with no gutter, `~` past the end.
    #[allow(clippy::too_many_arguments)]
    fn push_gutter(
        &self,
        ob: &OpenBuffer,
        buffer_line: usize,
        has_line: bool,
        gutter_chars: usize,
        dashboard_pad: Option<&[usize]>,
        cursor_line: usize,
        fold_marks: Option<&(HashMap<usize, usize>, HashSet<usize>)>,
        test_cells: Option<&tests_host::TestCells>,
        spans: &mut Vec<(String, glyphon::Color)>,
    ) {
        let theme = self.theme;
        if let Some(pad) = dashboard_pad {
            let n = pad.get(buffer_line).copied().unwrap_or(0);
            if n > 0 {
                spans.push((" ".repeat(n), theme.fg));
            }
        } else if gutter_chars > 0 {
            let blame = self.blame_width(ob);
            if blame > 0 {
                spans.push(self.blame_cell(ob, buffer_line, blame));
            }
            let numbers = gutter_chars - blame;
            if numbers > 0 {
                let marker_cells = usize::from(fold_marks.is_some());
                let test_cell = usize::from(test_cells.is_some());
                // The test column's cell, left of the number; past the end
                // the `~` takes it, as it takes the numbers'.
                if let (Some(cells), true) = (test_cells, has_line) {
                    let (glyph, color) = cells.at(buffer_line).map_or((' ', theme.gutter_fg), |(g, c, _)| (g, c));
                    spans.push((glyph.to_string(), color));
                }
                let gutter = if has_line {
                    let n = match self.line_number_mode {
                        LineNumberMode::Relative => buffer_line.abs_diff(cursor_line),
                        _ => buffer_line + 1,
                    };
                    format!("{:>width$}", n, width = numbers.saturating_sub(1 + marker_cells + test_cell))
                } else {
                    format!("{:<width$}", "~", width = numbers.saturating_sub(marker_cells))
                };
                let color = if has_line && buffer_line == cursor_line { theme.fg } else { theme.gutter_fg };
                spans.push((gutter, color));
                if let Some((starts, closed)) = fold_marks {
                    let marker = match (has_line && starts.contains_key(&buffer_line), closed.contains(&buffer_line)) {
                        (true, true) => "▸",
                        (true, false) => "▾",
                        _ => " ",
                    };
                    let marker_color = if closed.contains(&buffer_line) { rgba_to_glyphon(theme.caret) } else { theme.gutter_fg };
                    spans.push((marker.to_string(), marker_color));
                }
                if has_line {
                    spans.push((" ".to_string(), color));
                }
            }
        } else if !has_line && ob.kind != BufferKind::Prompt {
            // A prompt's rows past what was written are just the box.
            spans.push(("~".to_string(), theme.gutter_fg));
        }
    }

    /// Visual columns `cols` of `buffer_line` as drawn -- tabs expanded,
    /// the decorations in `decorated` laid out -- in its syntax's colours.
    #[allow(clippy::too_many_arguments)]
    fn push_line_text(
        &self,
        ob: &OpenBuffer,
        buffer_line: usize,
        cols: std::ops::Range<usize>,
        syntax_highlights: &[(std::ops::Range<usize>, glyphon::Color)],
        tab_stops: &TabStops,
        decorated: &HashMap<usize, crate::decorations::LineLayout>,
        spans: &mut Vec<(String, glyphon::Color)>,
    ) {
        let drawn = self.drawn_line(ob, buffer_line, syntax_highlights, tab_stops, decorated);
        let (sliced, from) = skip_chars(&drawn.text, cols.start);
        let to = from + keep_chars(sliced, cols.end.saturating_sub(cols.start)).len();
        self.push_drawn(&drawn, from..to, spans);
    }

    /// `buffer_line` as drawn, once for all the rows it's on: its text
    /// with tabs expanded and decorations laid out, and the colours of
    /// its bytes.
    pub(super) fn drawn_line<'a>(
        &self,
        ob: &OpenBuffer,
        buffer_line: usize,
        syntax_highlights: &'a [(std::ops::Range<usize>, glyphon::Color)],
        tab_stops: &TabStops,
        decorated: &HashMap<usize, crate::decorations::LineLayout>,
    ) -> DrawnLine<'a> {
        let theme = self.theme;
        let start = ob.buffer.line_start_char(buffer_line);
        let len = ob.buffer.line_len(buffer_line);
        let line_byte_start = ob.buffer.char_to_byte(start);
        let line_byte_end = ob.buffer.char_to_byte(start + len);
        if let Some(layout) = decorated.get(&buffer_line) {
            let display = &layout.display;
            let remapped = remap_highlights_for_display(&ob.buffer, start, &layout.col_map, display, line_byte_start, line_byte_end, syntax_highlights);
            let ranges = decorated_ranges(remapped, &layout.runs, |role| pages::role_color(theme, role));
            return DrawnLine { text: display.clone(), ranges: std::borrow::Cow::Owned(ranges), base: 0 };
        }
        let line_text = ob.buffer.text_range(start, start + len);
        if line_text.contains('\t') {
            let (display, col_map) = tabstops::expand_line(&line_text, tab_stops);
            // Local to `display` (0-based), as `remapped` is -- not the
            // real buffer's byte offsets, unlike a line without a tab.
            let ranges = if syntax_highlights.is_empty() {
                Vec::new()
            } else {
                remap_highlights_for_display(&ob.buffer, start, &col_map, &display, line_byte_start, line_byte_end, syntax_highlights)
            };
            return DrawnLine { text: display, ranges: std::borrow::Cow::Owned(ranges), base: 0 };
        }
        DrawnLine { text: line_text, ranges: std::borrow::Cow::Borrowed(syntax_highlights), base: line_byte_start }
    }

    /// Bytes `bytes` of a line as drawn, in their colours.
    pub(super) fn push_drawn(&self, drawn: &DrawnLine, bytes: std::ops::Range<usize>, spans: &mut Vec<(String, glyphon::Color)>) {
        let sliced = &drawn.text[bytes.clone()];
        if drawn.ranges.is_empty() {
            spans.push((sliced.to_string(), self.theme.fg));
        } else {
            spans.extend(split_line_by_highlights(sliced, drawn.base + bytes.start, drawn.base + bytes.end, &drawn.ranges, self.theme.fg));
        }
    }

    /// For a Tcl buffer, the words its `function` capture may color as
    /// calls: `tcl.scm`'s `(command name: (_) @function)` captures every
    /// word in command position, since a query can't know which are
    /// commands. Tcl's built-ins, the procs the file itself defines (a
    /// call to one twenty lines up is the most ordinary thing in a
    /// script), the project's procs its language servers name (the Tcl
    /// extension's, from ctags) and your `completion.symbols_file`'s
    /// words. A proc `myns::greet` is called that, `::myns::greet`, or
    /// `greet` inside its namespace: each is known. `None` for any other
    /// buffer.
    fn known_commands(&self, id: BufferId) -> Option<std::collections::HashSet<String>> {
        let ob = self.buffers.get(id)?;
        let path = ob.buffer.path()?;
        if !is_tcl_path(path) {
            return None;
        }
        let mut known: std::collections::HashSet<String> = fenix_completion::tcl::KEYWORDS.iter().map(|k| k.to_string()).collect();
        let mut add = |name: &str| {
            let name = name.trim().trim_start_matches("::");
            if name.is_empty() {
                return;
            }
            if let Some((_, tail)) = name.rsplit_once("::") {
                known.insert(tail.to_string());
            }
            known.insert(name.to_string());
        };
        for name in fenix_completion::tcl::procs_defined_in(&ob.buffer.text()) {
            add(&name);
        }
        for key in self.tcl_name_servers.get(&id).map(|(_, keys)| keys.as_slice()).unwrap_or_default() {
            if let Some((_, names)) = self.lsp_workspace_names.get(key) {
                names.iter().for_each(|n| add(n));
            }
        }
        if let Some(file) = &self.config.completion_symbols_file {
            completion::symbols_file(file).iter().for_each(|item| add(&item.label));
        }
        Some(known)
    }

    /// Asks a Tcl buffer's language servers that answer `workspace/symbol`
    /// for the project's functions, every few seconds while it's drawn:
    /// the Tcl extension's ctags scan lands after the file opens, and
    /// files change.
    fn ask_workspace_names(&mut self, id: BufferId) {
        const EVERY: Duration = Duration::from_secs(5);
        let Some(path) = self.buffers.get(id).and_then(|ob| ob.buffer.path()).filter(|p| is_tcl_path(p)).map(Path::to_path_buf) else { return };
        let now = Instant::now();
        if self.tcl_name_servers.get(&id).is_some_and(|(at, _)| now.duration_since(*at) < EVERY) {
            return;
        }
        let keys = self.cached_lsp_keys(&path);
        self.tcl_name_servers.retain(|b, _| self.buffers.get(*b).is_some());
        self.tcl_name_servers.insert(id, (now, keys.clone()));
        for key in keys {
            if self.lsp_workspace_names.get(&key).is_some_and(|(at, _)| now.duration_since(*at) < EVERY) {
                continue;
            }
            let Some(session) = self.lsp_sessions.get_mut(&key) else { continue };
            let answers = session.capabilities.as_ref().and_then(|c| c.workspace_symbol_provider.as_ref()).is_some_and(|p| !matches!(p, lsp_types::OneOf::Left(false)));
            if !answers {
                continue;
            }
            let params = lsp_types::WorkspaceSymbolParams { query: String::new(), work_done_progress_params: Default::default(), partial_result_params: Default::default() };
            if let Ok(request) = session.client.request::<lsp_types::request::WorkspaceSymbolRequest>(params) {
                session.pending.insert(request, PendingLspRequest::WorkspaceNames);
                let had = self.lsp_workspace_names.remove(&key).map(|(_, names)| names).unwrap_or_default();
                self.lsp_workspace_names.insert(key, (now, had));
            }
        }
    }

    /// The (line, col_start, col_end) segments of the active Visual-mode
    /// selection on `lines`, for highlighting. Empty outside Visual
    /// mode. Shape depends on `visual_kind()`: Char is a single charwise
    /// span, Line highlights whole lines regardless of column, Block is a
    /// column-range rectangle across lines (clamped per ragged line, like
    /// the Block operators themselves).
    fn visual_selection_segments(&self, lines: std::ops::Range<usize>) -> LineSegments {
        if self.snippet_is_valid() {
            if let Some(range) = self.snippet.as_ref().and_then(|s| s.session.selection()) {
                return self.range_to_segments(range, lines);
            }
        }
        if self.vim.mode() != Mode::Visual {
            return Vec::new();
        }
        let anchor = self.vim.visual_anchor();
        let (first_visible, last_visible) = (lines.start, lines.end.min(self.open().buffer.line_count()));
        let mut segments = Vec::new();

        match self.vim.visual_kind() {
            VisualKind::Char => {
                let cursor_idx = self.cursor().char_idx;
                // Standard keys select between the anchor and the cursor,
                // the cursor's character not in it -- drawn so, as the
                // extra cursors' selections are.
                let (lo, hi) = if self.vim.policy() == fenix_vim::EditPolicy::Standard {
                    (anchor.min(cursor_idx), anchor.max(cursor_idx))
                } else if anchor <= cursor_idx {
                    (anchor, cursor_idx + 1)
                } else {
                    (cursor_idx, anchor + 1)
                };
                let hi = hi.min(self.open().buffer.len_chars());
                segments = self.range_to_segments(lo..hi, lines);
            }
            VisualKind::Line => {
                let (line_lo, line_hi) = self.anchor_cursor_line_range(anchor);
                for line in first_visible.max(line_lo)..last_visible.min(line_hi + 1) {
                    // at least 1 col wide so an empty line still shows a sliver
                    let width = self.open().buffer.line_len(line).max(1);
                    segments.push((line, 0, width));
                }
            }
            VisualKind::Block => {
                let (line_lo, line_hi) = self.anchor_cursor_line_range(anchor);
                let anchor_cursor = Cursor { char_idx: anchor, sticky_col: 0 };
                let (_, anchor_col) = self.open().buffer.line_col(&anchor_cursor);
                let (_, cursor_col) = self.open().buffer.line_col(&self.cursor());
                let (col_lo, col_hi) = if anchor_col <= cursor_col {
                    (anchor_col, cursor_col + 1)
                } else {
                    (cursor_col, anchor_col + 1)
                };
                for line in first_visible.max(line_lo)..last_visible.min(line_hi + 1) {
                    let len = self.open().buffer.line_len(line);
                    let seg_start = col_lo.min(len);
                    let seg_end = col_hi.min(len);
                    if seg_start < seg_end {
                        segments.push((line, seg_start, seg_end));
                    }
                }
            }
        }
        segments
    }

    /// (min, max) line index spanned by the visual anchor and the cursor.
    fn anchor_cursor_line_range(&self, anchor: usize) -> (usize, usize) {
        let anchor_cursor = Cursor { char_idx: anchor, sticky_col: 0 };
        let (anchor_line, _) = self.open().buffer.line_col(&anchor_cursor);
        let (cursor_line, _) = self.open().buffer.line_col(&self.cursor());
        if anchor_line <= cursor_line { (anchor_line, cursor_line) } else { (cursor_line, anchor_line) }
    }

    /// The (line, col_start, col_end) segments a plain char range covers
    /// on `lines` -- shared by Char-kind Visual selection and the
    /// yank/paste pulse, which are both "highlight this contiguous span"
    /// even though they come from different sources. Only the lines the
    /// range is on are looked at.
    fn range_to_segments(&self, range: std::ops::Range<usize>, lines: std::ops::Range<usize>) -> LineSegments {
        let buffer = &self.open().buffer;
        let len = buffer.len_chars();
        let mut segments = Vec::new();
        if range.start >= range.end {
            return segments;
        }
        let first = buffer.rope().char_to_line(range.start.min(len)).max(lines.start);
        let last = (buffer.rope().char_to_line(range.end.min(len)) + 1).min(lines.end).min(buffer.line_count());
        for line in first..last {
            let line_start = buffer.line_start_char(line);
            let line_end = line_start + buffer.line_len(line);
            let seg_start = range.start.max(line_start);
            let seg_end = range.end.min(line_end);
            if seg_start < seg_end {
                segments.push((line, seg_start - line_start, seg_end - line_start));
            }
        }
        segments
    }

    /// Segments for the bracket under the cursor and its match, when the
    /// cursor sits exactly on one of `(){}[]` and a match exists --
    /// empty otherwise (unmatched brackets, or the cursor isn't on a
    /// bracket at all). Reuses `range_to_segments` for the "char range
    /// -> view segments" conversion, called once per bracket rather
    /// than over a combined range since the two are rarely adjacent.
    fn bracket_match_segments(&self, lines: std::ops::Range<usize>) -> LineSegments {
        let cursor_idx = self.cursor().char_idx;
        let Some(match_idx) = fenix_vim::find_matching_bracket(&self.open().buffer, cursor_idx) else {
            return Vec::new();
        };
        let mut segments = self.range_to_segments(cursor_idx..cursor_idx + 1, lines.clone());
        segments.extend(self.range_to_segments(match_idx..match_idx + 1, lines));
        segments
    }

    /// `hlsearch`'s persistent match-highlight segments -- every
    /// occurrence of the last confirmed search pattern on `lines`,
    /// empty whenever `VimState::hlsearch_active` is
    /// false. Converts the visible line range to a byte range (`Buffer::
    /// char_to_byte` at each line boundary) for `VimState::hlsearch_
    /// matches`, then each match's byte range back to view segments via
    /// `range_to_segments`, the same "windowed, computed fresh each
    /// frame" discipline `bracket_match_segments`/`syntax_highlights_
    /// for_visible_range` already use.
    #[cfg(test)]
    fn hlsearch_segments(&self, lines: std::ops::Range<usize>) -> LineSegments {
        let mut segments = self.search_match_segments(lines.clone());
        segments.extend(self.substitute_segments(lines));
        segments
    }

    /// `hlsearch_segments`' search matches alone: what a frame asks of
    /// each run of lines shown.
    fn search_match_segments(&self, lines: std::ops::Range<usize>) -> LineSegments {
        let buffer = &self.open().buffer;
        let last_visible = lines.end.min(buffer.line_count());
        let start_char = buffer.line_start_char(lines.start.min(last_visible));
        let end_char = if last_visible < buffer.line_count() {
            buffer.line_start_char(last_visible)
        } else {
            buffer.len_chars()
        };
        let byte_range = buffer.char_to_byte(start_char)..buffer.char_to_byte(end_char);

        let mut segments = LineSegments::new();
        for m in self.vim.hlsearch_matches(buffer, byte_range) {
            let start = buffer.byte_to_char(m.start);
            let end = buffer.byte_to_char(m.end);
            segments.extend(self.range_to_segments(start..end, lines.clone()));
        }
        segments
    }

    /// What a `:s` being typed would replace on `lines`, and the match
    /// its `c` flag is asking about: previewed once a frame, the command's
    /// whole range at a time.
    fn substitute_segments(&self, lines: std::ops::Range<usize>) -> LineSegments {
        let buffer = &self.open().buffer;
        let mut segments = LineSegments::new();
        let last_visible = lines.end.min(buffer.line_count());
        let start_char = buffer.line_start_char(lines.start.min(last_visible));
        let end_char = if last_visible < buffer.line_count() { buffer.line_start_char(last_visible) } else { buffer.len_chars() };
        if let Some(Ok(preview)) = self.vim.substitute_preview(buffer, &self.cursor()) {
            for m in preview.matches.into_iter().filter(|m| m.end >= start_char && m.start <= end_char) {
                segments.extend(self.range_to_segments(m, lines.clone()));
            }
        }
        if let Some(m) = self.vim.substitute_confirm_match() {
            segments.extend(self.range_to_segments(m, lines));
        }
        segments
    }

    /// `[3/12]`: which match of the lit search the cursor is on, of how
    /// many -- counted once per change of buffer, pattern or position.
    fn search_count(&self) -> Option<(usize, usize)> {
        let pattern = self.vim.active_search_pattern()?;
        let id = self.focused_buffer_id();
        let ob = self.open();
        let key = (id, ob.buffer.edit_count(), pattern.clone());
        let mut cache = self.search_count_cache.borrow_mut();
        if cache.as_ref().is_none_or(|(k, _)| *k != key) {
            let text_len = ob.buffer.char_to_byte(ob.buffer.len_chars());
            let starts = fenix_vim::all_matches_in_range(&ob.buffer, &pattern, 0..text_len).unwrap_or_default().into_iter().map(|m| m.start).collect();
            *cache = Some((key, starts));
        }
        let (_, starts) = cache.as_ref()?;
        if starts.is_empty() {
            return Some((0, 0));
        }
        // While typing, the match the search would land on.
        let at = match self.vim.preview_match(&ob.buffer, &self.cursor()) {
            Some(idx) => ob.buffer.char_to_byte(idx),
            None => ob.buffer.char_to_byte(self.cursor().char_idx),
        };
        let current = starts.iter().filter(|&&s| s <= at).count().max(1);
        Some((current, starts.len()))
    }

    /// The focused pane's caret position for this frame, as (line,
    /// col): normally the real cursor's (`line`, `col`), but during
    /// `Mode::Search` where the in-progress query would jump to instead
    /// (incsearch) -- the real `Cursor`/`last_search` are untouched
    /// until Enter confirms (see `VimState::preview_match`'s own doc
    /// comment). A preview on a line not shown draws no caret this
    /// frame rather than auto-scrolling to reveal it (a disclosed
    /// simplification -- real incsearch also pans the viewport).
    fn focused_caret(&self, line: usize, col: usize) -> Option<(usize, usize)> {
        if self.vim.mode() != Mode::Search {
            return Some((line, col));
        }
        self.vim.preview_match(&self.open().buffer, &self.cursor()).map(|idx| {
            let preview_cursor = Cursor { char_idx: idx, sticky_col: 0 };
            self.open().buffer.line_col(&preview_cursor)
        })
    }

    /// Segments and current alpha for an active yank/paste pulse, or
    /// `None` when there isn't one. Fades quickly at first then lingers
    /// faintly, via the same ease-out curve as the caret (inverted: pulses
    /// start bright and fade, blink fades in toward its target instead).
    fn pulse_overlay(&self, lines: std::ops::Range<usize>) -> Option<(LineSegments, f32)> {
        let pulse = self.pulse.as_ref()?;
        let t = Instant::now().duration_since(pulse.started).as_secs_f32() / self.motion().pulse_d.as_secs_f32().max(1e-3);
        let alpha = PULSE_PEAK_ALPHA * (1.0 - ease_out_cubic(t));
        if alpha <= 0.0 {
            return None;
        }
        Some((self.range_to_segments(pulse.range.clone(), lines), alpha))
    }

    /// Whichever prompt/confirmation `active_prompt_text` says is
    /// currently active, as a one-row floating popup just above the
    /// modeline -- `None` when nothing is. Shares the modeline's own
    /// background/text color (`bg_modeline`/`fg_modeline`), since this
    /// is exactly the text the modeline itself used to show inline
    /// before this popup existed. Unlike `which_key_popup`, not capped
    /// at `WHICH_KEY_MAX_WIDTH` (480px, tuned for short which-key
    /// labels) -- prompt text routinely needs to be longer (a file
    /// path, a commit message), so the only ceiling here is the same
    /// window-width-based one `which_key_popup` itself computes before
    /// *additionally* clamping to `WHICH_KEY_MAX_WIDTH`.
    /// The chars of the prompt's text drawn selected: a rename prompt's
    /// prefilled name, until a key.
    fn prompt_selection(&self) -> Option<std::ops::Range<usize>> {
        if !self.prompt_prefill_selected {
            return None;
        }
        let text = self.active_prompt_text()?;
        let input = self.rename_file_prompt.as_ref().or(self.lsp_rename_prompt.as_ref())?;
        let len = input.chars().count();
        let total = text.chars().count();
        text.ends_with(input.as_str()).then(|| total - len..total)
    }

    /// How the panels are drawn (`appearance.panels`).
    pub(super) fn panel_look(&self) -> panels::Look {
        panels::Look::from_setting(self.config.text_value("appearance.panels"))
    }

    /// Whether `pane` has a breadcrumb row under its tabs: a file's, or
    /// a text buffer's, does -- where in it the cursor is; a page or a
    /// tool's buffer doesn't, its tab already says what it is, and starts
    /// right under the tabs.
    fn pane_has_breadcrumb(&self, pane: fenix_window::WindowId) -> bool {
        if self.tab_style().is_none() || self.pane_titles.contains_key(&pane) {
            return false;
        }
        let Some(ob) = self.windows().content(pane).and_then(|b| self.buffers.get(*b)) else { return true };
        ob.buffer.path().is_some() || matches!(ob.kind, BufferKind::Text | BufferKind::Table)
    }

    fn frame_geometry(&self, window_width: f32, sidebar_px: f32, terminal_h: f32, modeline_top: f32) -> FrameGeometry {
        // The debug dock sits right above the modeline; the terminal and
        // the panes above it.
        let dock_h = self.debug_dock_px();
        let modeline_top = modeline_top - dock_h;
        let pane_area = fenix_window::Rect {
            x: sidebar_px,
            y: 0.0,
            w: (window_width - sidebar_px).max(0.0),
            h: (modeline_top - terminal_h).max(0.0),
        };
        // Each pane is drawn as a panel on the tray, a gap from the next:
        // its rect is where that panel is, so the text, the mouse and the
        // drawing all agree. The split's dividers stay where the layout
        // put them, in the gaps, for dragging.
        // Flat (`appearance.panels`): edge to edge, as before 1.4.
        let outer = panels::Edges { left: sidebar_px <= 0.0, top: true, right: true };
        let framed = self.panel_look() == panels::Look::Framed;
        let panes: Vec<_> = self.windows().layout(pane_area).into_iter().map(|(id, rect)| (id, if framed { panels::framed(rect, pane_area, outer) } else { rect })).collect();
        // The tabs the last frame measured and drew, not a fresh estimate.
        // Re-deriving them here from character counts is what used to make
        // a click land on a different tab than the one under the cursor:
        // that estimate can't account for the icon font's advance (much
        // wider than the body font's) and so drifted further right with
        // every tab. `redraw` publishes the real, shaped bounds into
        // `tab_geometry`, so hit-testing and pixels now come from one
        // measurement instead of two disagreeing calculations. Cheaper
        // too -- this used to resolve display names on every mouse move.
        let tabs = if self.tab_style().is_some() { self.tab_geometry.clone() } else { Vec::new() };
        // Right after each strip's last tab, as wide as a close glyph.
        let title_char_width = self.text.as_ref().map(|t| t.title_char_width()).unwrap_or(text::CHAR_WIDTH * text::TITLE_FONT_SCALE);
        let mut split_buttons: Vec<(fenix_window::WindowId, fenix_window::Rect)> = Vec::new();
        for (pane, tab) in &tabs {
            let right = tab.body.x + tab.body.w;
            match split_buttons.iter_mut().find(|(p, _)| p == pane) {
                Some((_, r)) if r.x < right => r.x = right,
                Some(_) => {}
                None => split_buttons.push((*pane, fenix_window::Rect { x: right, y: tab.body.y, w: TAB_CLOSE_CHARS as f32 * title_char_width, h: tab.body.h })),
            }
        }
        // Only where it fits in its pane.
        split_buttons.retain(|(pane, r)| panes.iter().any(|(p, pr)| p == pane && r.x + r.w <= pr.x + pr.w));
        let breadcrumb_panes = panes.iter().map(|(id, _)| *id).filter(|id| self.pane_has_breadcrumb(*id)).collect();
        FrameGeometry {
            panes,
            breadcrumb_panes,
            pane_area,
            sidebar_rect: (sidebar_px > 0.0).then_some(fenix_window::Rect { x: 0.0, y: 0.0, w: sidebar_px, h: modeline_top }),
            terminal_rect: (terminal_h > 0.0)
                .then_some(fenix_window::Rect { x: 0.0, y: modeline_top - terminal_h, w: window_width, h: terminal_h }),
            tabs,
            split_buttons,
        }
    }

    /// `(sidebar_px, terminal_h, modeline_top)` -- `frame_geometry`'s
    /// three inputs, derived the same way `redraw`'s own copies are.
    /// Not shared with `redraw` itself: that function also needs
    /// `char_width`/`line_height` as their own long-lived locals for
    /// everything else it does, and mutates `self.text` along the way
    /// (`set_font_family`) where this only reads it -- kept separate
    /// rather than reshaping `redraw`'s well-exercised metrics block
    /// just to share five one-line computations.
    fn frame_metrics(&self, window_height: f32) -> (f32, f32, f32) {
        let line_height = self.text.as_ref().map(|t| t.line_height()).unwrap_or(text::LINE_HEIGHT);
        let modeline_height = self.text.as_ref().map(|t| t.modeline_height()).unwrap_or(text::LINE_HEIGHT);
        // The palette floats over the window; the sidebar stays.
        let show_sidebar = self.sidebar_open;
        let sidebar_px = if show_sidebar { self.sidebar_width } else { 0.0 };
        let show_terminal = self.terminal_open;
        let terminal_h = if show_terminal { text::terminal_height(line_height) } else { 0.0 };
        let modeline_top = window_height - modeline_height;
        (sidebar_px, terminal_h, modeline_top)
    }

    /// `zz`/`zt`/`zb` -- scrolls the focused pane so the cursor's line
    /// lands centered/top/bottom. Thin `self.gpu`-reading wrapper around
    /// `scroll_window_at`, mirroring `handle_click`/`handle_click_at`'s
    /// own split -- a silent no-op before the window/GPU exists, same
    /// posture `handle_click` already has.
    fn scroll_window(&mut self, target: VimScrollTarget) {
        let Some((window_width, window_height)) = self.gpu.as_ref().map(|gpu| (gpu.size.width as f32, gpu.size.height as f32))
        else {
            return;
        };
        self.scroll_window_at(target, window_width, window_height);
    }

    /// The GPU-independent core of `scroll_window` -- computes the
    /// focused pane's visible-line count the same way `redraw`'s own
    /// `pane_visible_lines` is derived (`frame_metrics`/`frame_geometry`/
    /// `pane_content_rect`/`text::lines_that_fit`), then snaps `scroll_
    /// line`/`rendered_scroll` directly (no ease -- these are deliberate
    /// one-shot jumps, not a "keep the cursor in view" nudge, the same
    /// reason `ensure_cursor_visible`'s own "huge jump" branch snaps
    /// instead of animating) to land the cursor's line where requested.
    /// Clears any in-flight scroll animation for this pane first so it
    /// can't fight the snap on the next frame.
    fn scroll_window_at(&mut self, target: VimScrollTarget, window_width: f32, window_height: f32) {
        let pane = self.focused_pane_id();
        let buffer_id = self.focused_buffer_id();
        let Some(ob) = self.buffers.get(buffer_id) else { return };
        let cursor = self.pane_state(pane).cursor;
        let (document_line, _) = ob.buffer.line_col(&cursor);
        let line = self.pane_display_lines(pane, buffer_id).iter().position(|&row| row == document_line).unwrap_or(document_line);

        let line_height = self.text.as_ref().map(|t| t.line_height()).unwrap_or(text::LINE_HEIGHT);
        let (sidebar_px, terminal_h, modeline_top) = self.frame_metrics(window_height);
        let geometry = self.frame_geometry(window_width, sidebar_px, terminal_h, modeline_top);
        let Some((_, rect)) = geometry.panes.iter().find(|(id, _)| *id == pane) else { return };
        let content_rect = pane_content_rect(*rect, line_height, self.pane_has_breadcrumb(pane));
        let visible_lines = text::lines_that_fit(content_rect.h, line_height);
        // A wrapped pane: the cursor's row, not its line's first.
        if self.scroll_rows_to_cursor(pane, buffer_id, target, visible_lines) {
            return;
        }

        let scroll_line = match target {
            VimScrollTarget::Top => line,
            VimScrollTarget::Bottom => line.saturating_sub(visible_lines.saturating_sub(1)),
            VimScrollTarget::Center => line.saturating_sub(visible_lines / 2),
        };
        self.workspaces.active_scroll_anims_mut().remove(&pane);
        let pane_state = self.pane_state_mut(pane);
        pane_state.scroll_line = scroll_line;
        pane_state.rendered_scroll = scroll_line as f32;
    }

    /// Updates the OS cursor icon for wherever the mouse is now, via
    /// `cursor_icon_for` -- called on every `WindowEvent::CursorMoved`.
    /// Only actually calls `window
    /// .set_cursor` when the icon changed since last time (`self.current_
    /// cursor_icon`) -- `CursorMoved` can fire hundreds of times a second,
    /// and the OS call isn't free.
    fn update_hover_cursor(&mut self, pos: (f32, f32), window_width: f32, window_height: f32) {

        let (sidebar_px, terminal_h, modeline_top) = self.frame_metrics(window_height);
        let geometry = self.frame_geometry(window_width, sidebar_px, terminal_h, modeline_top);
        let line_height = self.text.as_ref().map(|t| t.line_height()).unwrap_or(text::LINE_HEIGHT);
        // A lens under the pointer: drawn as the one a click runs.
        let over = hit_test(&geometry, pos).and_then(|t| match t {
            ScrollTarget::Pane(p) => geometry.panes.iter().find(|(id, _)| *id == p).copied(),
            _ => None,
        });
        if self.hover_lens(over, pos) {
            self.request_redraw_all();
        }
        let icon = if self.sidebar_drag || self.on_sidebar_edge(pos) {
            winit::window::CursorIcon::ColResize
        } else if let Some(icon) = self.divider_icon(&geometry, pos) {
            icon
        } else if self.tab_drag.is_some() {
            winit::window::CursorIcon::Grabbing
        } else if self.term_hover.is_some() || self.lens_hover.is_some() {
            // Ctrl over a terminal's link, or over a code lens: a click
            // follows it.
            winit::window::CursorIcon::Pointer
        } else {
            cursor_icon_for(&geometry, pos, line_height)
        };
        if self.current_cursor_icon == Some(icon) {
            return;
        }
        self.current_cursor_icon = Some(icon);
        if let Some(window) = &self.window {
            window.set_cursor(icon);
        }
    }

    /// The text cell (line, column) under `pos`, for text whose top-left
    /// is `origin` (before padding) scrolled down `scroll` lines.
    fn cell_at(&self, pos: (f32, f32), origin: (f32, f32), scroll: usize) -> (usize, usize) {
        let line_height = self.text.as_ref().map(|t| t.line_height()).unwrap_or(text::LINE_HEIGHT);
        let char_width = self.text.as_ref().map(|t| t.char_width()).unwrap_or(text::CHAR_WIDTH);
        let line = ((pos.1 - origin.1 - text::PAD_TOP) / line_height).max(0.0) as usize;
        let col = ((pos.0 - origin.0 - text::PAD_LEFT) / char_width).max(0.0) as usize;
        (scroll + line, col)
    }

    /// Drops `buffer` from `pane`'s own tab strip (a `show_tabs` theme's
    /// tab × click) -- never calls `BufferList::close` (its own doc
    /// comment makes retargeting/closing the caller's job, and the same
    /// buffer may still be shown in another split or listed in another
    /// pane's own tab strip), so this only ever hides it from *this*
    /// pane's strip, same as navigating away from a buffer already works
    /// today (`SPC b b` still reaches it). If `buffer` was the pane's
    /// active tab, retargets to its neighbour (`tab_after_closing`), or
    /// Home once no other tab is left. Home itself never closes.
    fn close_pane_tab(&mut self, pane: fenix_window::WindowId, buffer: BufferId) {
        if self.workspaces.active_workspace().home == Some(buffer) {
            return;
        }
        let was_active = self.windows().content(pane) == Some(&buffer);
        let order = self.pane_tab_order(pane);
        let ws = self.workspaces.active_workspace_mut();
        let Some(tabs) = ws.pane_tabs.get_mut(&pane) else { return };
        let Some(at) = tabs.iter().position(|&id| id == buffer) else { return };
        tabs.remove(at);
        let closed = ws.closed_tabs.entry(pane).or_default();
        closed.push((buffer, at));
        if closed.len() > tabs::CLOSED_TABS_KEPT {
            closed.remove(0);
        }
        if ws.preview.get(&pane) == Some(&buffer) {
            ws.preview.remove(&pane);
        }
        if was_active {
            let target = tabs::tab_after_closing(&order, buffer).unwrap_or_else(|| self.ensure_workspace_home());
            self.set_pane_content(pane, target);
        }
    }

    /// Wheel-scrolls a pane that ISN'T focused: moves its own
    /// `PaneState.scroll_line` directly, bounded so at least one
    /// screenful of the buffer always stays in view. Never touches the
    /// cursor -- a non-focused pane never auto-follows its cursor
    /// anyway (`ensure_cursor_visible` only runs `if is_focused`), so
    /// there's nothing to fight the next frame the way there would be
    /// for the focused pane (see `scroll_focused_pane`).
    fn scroll_unfocused_pane(&mut self, pane: fenix_window::WindowId, lines: isize, rect_h: f32, line_height: f32) {
        let Some(&buffer_id) = self.windows().content(pane) else { return };
        if self.buffers.get(buffer_id).is_none() { return; }
        let visible = text::lines_that_fit(rect_h, line_height);
        // A wrapped pane: by rows.
        if self.scroll_by_rows(pane, buffer_id, -lines, visible) {
            return;
        }
        let total = self.pane_display_lines(pane, buffer_id).len();
        let max_scroll = total.saturating_sub(visible);
        let state = self.pane_state_mut(pane);
        state.scroll_line = state.scroll_line.saturating_add_signed(-lines).min(max_scroll);
        // What's drawn: only the focused pane's follows `scroll_line`.
        state.rendered_scroll = state.scroll_line as f32;
    }

    /// Wheel-scrolls the focused pane by moving its cursor/selection --
    /// poking its `scroll_line` directly would just get overwritten by
    /// `ensure_cursor_visible` on the very next frame, since that runs
    /// unconditionally whenever a pane is focused. Moving the cursor
    /// instead drives scrolling through the exact same path a real
    /// `j`/`k` keypress would. `Buffer::move_page` is a direct, mode-
    /// independent cursor primitive (not routed through key dispatch),
    /// so it can't be misread as text input while in Insert mode.
    fn scroll_focused_pane(&mut self, lines: isize) {
        let down = lines < 0;
        let count = lines.unsigned_abs();
        match self.main_view {
            MainView::Editor => {
                let pane = self.focused_pane_id();
                let buffer_id = self.focused_buffer_id();
                let mut cursor = self.pane_state(pane).cursor;
                let Some(ob) = self.buffers.get_mut(buffer_id) else { return };
                ob.buffer.move_page(&mut cursor, count, down);
                self.pane_state_mut(pane).cursor = cursor;
            }
            MainView::Picker => {
                if let Some(picker) = &mut self.active_picker {
                    picker_move_selection(picker, -lines);
                }
            }
        }
    }

    /// The pane (if any) at `pos` and its on-screen pixel rect -- what a
    /// surface's pointer and wheel are placed by.
    fn pane_rect_at(&self, pos: (f32, f32)) -> Option<(fenix_window::WindowId, fenix_window::Rect)> {
        let (window_width, window_height) = self.gpu.as_ref().map(|gpu| (gpu.size.width as f32, gpu.size.height as f32))?;
        let (sidebar_px, terminal_h, modeline_top) = self.frame_metrics(window_height);
        let geometry = self.frame_geometry(window_width, sidebar_px, terminal_h, modeline_top);
        let Some(ScrollTarget::Pane(pane)) = hit_test(&geometry, pos) else { return None };
        geometry.panes.iter().find(|(id, _)| *id == pane).copied()
    }

    /// How far apart `buffer`'s indent guides are: its own lines' step,
    /// else the indent width setting. Read again only after an edit.
    fn guide_indent_width(&self, buffer: BufferId) -> usize {
        let setting = self.vim.indent_width();
        let Some(ob) = self.buffers.get(buffer) else { return setting };
        let edits = ob.buffer.edit_count();
        if let Some((at, step)) = self.indent_steps.borrow().get(&buffer) {
            if *at == edits {
                return step.unwrap_or(setting);
            }
        }
        let lines: Vec<String> = (0..ob.buffer.line_count().min(2000))
            .map(|l| {
                let start = ob.buffer.line_start_char(l);
                ob.buffer.text_range(start, start + ob.buffer.line_len(l))
            })
            .collect();
        let step = file_indent_step(lines.iter().map(String::as_str));
        self.indent_steps.borrow_mut().insert(buffer, (edits, step));
        step.unwrap_or(setting)
    }
}

/// The app icon as a `winit::window::Icon` for the title bar/taskbar,
/// drawn from `fenix-brand` -- the same geometry `build.rs` embeds into
/// the `.exe` as its resource icon (what Explorer and a taskbar pin read
/// before the process is even running), so the two can't disagree.
pub(crate) fn fenix_icon() -> Option<Icon> {
    let icon = fenix_brand::app_icon(256);
    Icon::from_rgba(icon.rgba, icon.width, icon.height).ok()
}

impl App {
    /// Takes the window `window`'s surface back from the launch splash
    /// and lets the splash fade out over the first frames. With no
    /// splash back (it was off, or its thread failed), the surface is
    /// made afresh if it has to be.
    pub(crate) fn end_splash(&mut self, window: WindowId, splash: Option<crate::splash::SplashReturn>) {
        let active = self.window.as_ref().is_some_and(|w| w.id() == window);
        let frame = if active { None } else { (0..self.frames.len()).find(|&f| self.frame_window(f).is_some_and(|w| w.id() == window)) };
        let window = if active { self.window.clone() } else { frame.and_then(|f| self.frame_window(f).cloned()) };
        let gpu = match frame {
            None => self.gpu.as_mut(),
            Some(f) => self.frames[f].as_mut().and_then(|state| state.gpu.as_mut()),
        };
        self.first_frame_due = Some(Instant::now() + FIRST_FRAME_WITHIN);
        let Some(gpu) = gpu else { return };
        match splash {
            Some(back) => {
                gpu.restore_surface(back.surface);
                gpu.resize(PhysicalSize::new(back.config.width, back.config.height));
                gpu.apply_pending_resize();
                self.splash_fade = Some(crate::splash::SplashFade {
                    sprites: back.sprites,
                    scene: back.scene,
                    started: back.started,
                    progress: back.progress,
                    fade_from: Instant::now(),
                    still: back.still,
                });
            }
            // The splash's thread failed, and its surface went with it.
            None if gpu.surface.is_none() => {
                if let (Some(context), Some(window)) = (&self.gpu_context, window) {
                    if let Err(why) = context.recreate_surface(gpu, window) {
                        self.gpu_watch.want_rebuild(why);
                    }
                }
            }
            None => {}
        }
    }

    /// Everything `resumed` does once the first window and its GPU exist:
    /// fonts, the text and rect pipelines, then the saved windows.
    /// `progress` hears how far along it is (0 to 1), for the launch
    /// splash; the window's surface may be on loan to the splash while
    /// this runs, which nothing here needs.
    pub(crate) fn start_in(&mut self, event_loop: &ActiveEventLoop, window: Arc<Window>, gpu_context: GpuContext, gpu: GpuState, progress: &dyn Fn(f32)) {
        let fonts = Rc::new(RefCell::new(text::FontContext::new(&gpu)));
        crate::profile::launch_mark("fonts");
        progress(0.75);
        // No priming call needed for pane content -- `redraw()` populates
        // every visible pane's `GlyphBuffer` fresh (creating it lazily)
        // on the first real frame, which winit already requests
        // immediately after this.
        let mut text = TextPipeline::new(&gpu, Rc::clone(&fonts));
        text.set_font_size(self.logical_font_size(), window.scale_factor() as f32);
        let bg_rect = RectRenderer::new(&gpu);
        let popup_rect = RectRenderer::new(&gpu);
        let caret_rect = RectRenderer::new(&gpu);
        let surfaces = SurfacePipeline::new(&gpu);
        self.motion_gpu = Some(motion_host::MotionGpu::new(&gpu));
        crate::profile::launch_mark("pipelines");
        progress(0.85);

        self.window = Some(window);
        self.gpu_context = Some(gpu_context);
        self.fonts = Some(fonts);
        self.gpu = Some(gpu);
        self.text = Some(text);
        self.bg_rect = Some(bg_rect);
        self.popup_rect = Some(popup_rect);
        self.caret_rect = Some(caret_rect);
        self.surfaces = Some(surfaces);
        self.refresh_frame_origin();
        crate::profile::launch_mark("frame origin");
        self.restore_saved_windows(event_loop);
        progress(0.95);
        crate::profile::launch_mark("resumed");
    }
}

impl ApplicationHandler<FenixUserEvent> for App {
    /// Thin trait-required wrapper around `handle_user_event` -- see
    /// that method's own doc comment for why the actual logic lives
    /// there instead of here (same "extract the `ActiveEventLoop`-free
    /// part so it's directly testable" reasoning `handle_key`/
    /// `test_vim_key` already established for keyboard input).
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: FenixUserEvent) {
        // Timed with FENIX_PROFILE, and named in the hang log while it runs.
        let _busy = crate::profile::Scope::new(event.label());
        self.handle_user_event(event);
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attrs = Window::default_attributes().with_title(window_title(None)).with_window_icon(fenix_icon());
        let window =
            Arc::new(event_loop.create_window(attrs).expect("failed to create window"));
        crate::profile::launch_mark("window");

        let (gpu_context, gpu) = pollster::block_on(GpuContext::new(window.clone()));
        crate::profile::launch_mark("gpu");
        self.start_in(event_loop, window, gpu_context, gpu, &|_| {});
    }

    /// The one place every quit path is guaranteed to pass through --
    /// `SPC q q`, `:q`, `SPC q Q`, and the last window's X button all
    /// end in `event_loop.exit()`, which lands here.
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // An extension's text being written, kept for next time.
        self.keep_compose_draft();
        // Agents' threads saved, their processes stopped.
        self.agents_shutdown();
        // Connections to remote machines ended, and what ran over them.
        self.remote_close_all();
        // Test runs' trees killed, their results written.
        self.tests_shutdown();
        self.checkpoint_session();
        self.save_window_layout();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        self.follow_outside_edits();
        let _busy = crate::watchdog::enter(match &event {
            WindowEvent::RedrawRequested => "redraw",
            WindowEvent::KeyboardInput { .. } | WindowEvent::ModifiersChanged(_) | WindowEvent::Ime(_) => "a key",
            WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. } | WindowEvent::CursorMoved { .. } => "the mouse",
            WindowEvent::Focused(_) => "the window's focus",
            WindowEvent::Resized(_) | WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. } => "the window's size",
            _ => "a window event",
        });
        self.a11y_window_event(id, &event);
        let Some(frame) = self.frame_index_of(id) else { return };
        // Every event is handled with the frame it arrived from active,
        // so the handlers below -- all written against one implicit
        // window -- resolve to the right frame's layout, renderers and
        // swapchain without any of them knowing frames exist.
        //
        // Whether that frame then *keeps* editor focus depends on what
        // the event was. Real input moves focus with it: a keystroke, a
        // click, or the OS telling us this window was focused. Anything
        // else -- a resize, a plain hover (Windows delivers mouse moves
        // to whatever window is under the pointer, focused or not), a
        // background frame asking to repaint -- is handled in that
        // frame's context and then hands focus straight back.
        let keeps_focus = matches!(
            &event,
            WindowEvent::KeyboardInput { .. } | WindowEvent::MouseInput { .. } | WindowEvent::Focused(true)
        );
        let previous = self.active_frame;
        if keeps_focus {
            self.focus_frame(frame);
        } else {
            self.activate_frame(frame);
        }
        match event {
            // The X button closes that one frame, exactly like
            // `SPC w Q`. On the *last* frame that is quitting, so
            // `close_frame` routes it through the same unsaved-changes
            // check as `:q`/`SPC q q` -- the X button shouldn't be able
            // to silently discard work either.
            WindowEvent::CloseRequested => self.close_frame(event_loop),
            // A window moving between monitors changes nothing about
            // how it draws, but it does change where it sits relative
            // to the other frames -- which is what `SPC w h`/`l`
            // crossing a window boundary is decided from.
            WindowEvent::Moved(_) => self.refresh_frame_origin(),
            // Dragged onto a monitor with different scaling: the same
            // configured size now needs a different number of physical
            // pixels to look the same. Uses the event's factor rather
            // than the window's, which may not have caught up yet.
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let logical = self.logical_font_size();
                if let Some(text) = &mut self.text {
                    text.set_font_size(logical, scale_factor as f32);
                }
                self.resync_terminal_cols();
            }
            WindowEvent::Resized(size) => {
                // Maximizing (and being snapped, and un-maximizing)
                // moves the client area's corner as well as its size.
                self.refresh_frame_origin();
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size);
                }
                if let Some(text) = &mut self.text {
                    text.resize(size.width as f32, size.height as f32);
                }
                self.resync_terminal_cols();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                // Ctrl let go with the recent-buffer switcher up: its row.
                if !self.modifiers.control_key() {
                    self.recent_switch_released();
                }
                // Ctrl pressed or let go over a terminal's link.
                self.terminal_hover(self.cursor_pos);
                if let (Some(pos), Some((w, h))) = (self.cursor_pos, self.gpu.as_ref().map(|gpu| (gpu.size.width as f32, gpu.size.height as f32))) {
                    self.update_hover_cursor(pos, w, h);
                }
            }
            // Alt-Tabbing (or otherwise switching) away from the window
            // while a surface has the keyboard releases it: keys typed
            // into another application aren't the surface's, and its
            // hidden pointer must show again there.
            // Coming back from another application is the other half
            // of the answer: the timer catches Fenix's own terminal
            // panel (which never takes focus away), and this catches
            // everything else, without waiting out the interval.
            WindowEvent::Focused(true) => {
                self.poll_files_changed_on_disk();
                self.refresh_gutter_hunks(self.focused_buffer_id());
            }
            WindowEvent::Focused(false) => {
                self.code_surface_release();
                self.auto_save_all();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let pos = (position.x as f32, position.y as f32);
                self.cursor_pos = Some(pos);
                if self.sidebar_drag {
                    if let Some(width) = self.gpu.as_ref().map(|gpu| gpu.size.width as f32) {
                        self.drag_sidebar_to(pos.0, width);
                    }
                }
                if let Some((window_width, window_height)) = self.gpu.as_ref().map(|gpu| (gpu.size.width as f32, gpu.size.height as f32)) {
                    // A selection or a divider following the pointer; the
                    // menu's row under it; the hover's wait.
                    self.mouse_drag_to(pos, window_width, window_height);
                    self.page_select_drag(pos);
                    self.menu_hover_at(pos, window_width, window_height);
                    self.pointer_rest_at(pos, Instant::now(), window_width, window_height);
                }
                self.code_surface_pointer(pos, None);
                self.code_page_pointer(pos, None);
                self.terminal_hover(Some(pos));
                if let Some((window_width, window_height)) = self.gpu.as_ref().map(|gpu| (gpu.size.width as f32, gpu.size.height as f32)) {
                    self.update_hover_cursor(pos, window_width, window_height);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if state == ElementState::Pressed { self.snippet = None; }
                let size = self.gpu.as_ref().map(|gpu| (gpu.size.width as f32, gpu.size.height as f32));
                // The right click's menu takes the click on it; one off it
                // closes it and goes on.
                if let (ElementState::Pressed, Some(pos), Some((w, h))) = (state, self.cursor_pos, size) {
                    if let Some(chosen) = self.menu_press_at(pos, w, h) {
                        if let Some(action) = chosen {
                            self.run_menu_action(action);
                        }
                        return;
                    }
                }
                if state == ElementState::Pressed {
                    self.close_mouse_hover();
                }
                if button == MouseButton::Left && state == ElementState::Released {
                    self.mouse_release();
                }
                // The sidebar's edge, dragged to resize it.
                if button == MouseButton::Left {
                    if state == ElementState::Pressed && self.cursor_pos.is_some_and(|pos| self.on_sidebar_edge(pos)) {
                        self.sidebar_drag = true;
                        return;
                    }
                    if state == ElementState::Released && std::mem::take(&mut self.sidebar_drag) {
                        self.sidebar_width_dragged();
                        return;
                    }
                }
                // A divider between two panes, dragged to resize them.
                if let (MouseButton::Left, ElementState::Pressed, Some(pos), Some((w, h))) = (button, state, self.cursor_pos, size) {
                    if self.divider_press_at(pos, w, h) {
                        return;
                    }
                }
                // A click on a toast: its action, or dismissed.
                if state == ElementState::Pressed && self.cursor_pos.is_some_and(|pos| self.toast_click(pos, button)) {
                    return;
                }
                // A click on an extension's clickable item is its alone.
                if state == ElementState::Pressed && button == MouseButton::Left && self.cursor_pos.is_some_and(|pos| self.layer_click(pos) || self.decoration_click(pos)) {
                    return;
                }
                if state == ElementState::Pressed && button == MouseButton::Left {
                    if let Some(pos) = self.cursor_pos {
                        // A terminal's text: a selection starts, or a
                        // Ctrl-click follows the link there.
                        if self.terminal_mouse_press(pos) {
                            return;
                        }
                        self.handle_click(pos);
                    }
                }
                // A right click on a terminal copies or pastes.
                if let (MouseButton::Right, ElementState::Pressed, Some(pos)) = (button, state, self.cursor_pos) {
                    if self.terminal_right_click(pos) {
                        return;
                    }
                }
                // A right click in text or on a tab: its menu.
                if let (MouseButton::Right, ElementState::Pressed, Some(pos), Some((w, h))) = (button, state, self.cursor_pos, size) {
                    if self.right_click_at(pos, w, h) {
                        return;
                    }
                }
                // A click on a code page that takes the mouse is its too.
                if let Some(pos) = self.cursor_pos {
                    self.code_page_pointer(pos, Some((button, state == ElementState::Pressed)));
                }
                if let Some(pos) = self.cursor_pos {
                    self.tab_mouse(pos, button, state == ElementState::Pressed);
                }
                if let Some(pos) = self.cursor_pos {
                    self.code_surface_pointer(pos, Some((button, state == ElementState::Pressed)));
                }
            }
            // A file dropped on the window: a picture goes to the agent
            // when its thread is out (a screenshot, to look at a bug);
            // anything else, or with no thread out, opens.
            WindowEvent::DroppedFile(path) => {
                let picture = agents::image_mime(&path).is_some() && self.agents.shown(&self.pages).is_some();
                if picture {
                    if let Err(why) = self.agents_attach_image(&path) {
                        self.set_error(why);
                    }
                } else if path.is_file() {
                    self.open_file_from_picker(&path);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.mouse.menu = None;
                self.close_mouse_hover();
                if let Some(pos) = self.cursor_pos {
                    self.handle_wheel(pos, delta);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.handle_key(&event, event_loop);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            // Fenix doesn't turn the input method on, but text one
            // commits anyway is typed like any other.
            WindowEvent::Ime(winit::event::Ime::Commit(text)) => {
                self.receive_text(&text, Some(event_loop));
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
                // The window is up: the extensions that start with Fenix,
                // now (#33).
                if self.frames_presented > 0 {
                    self.code_startup_now();
                }
                // Code extensions' pages that came into view or left it;
                // their layers' glides that ended, and the layout.
                let sized = std::mem::take(&mut self.wasm.sized);
                self.code_surfaces_drawn(sized);
                self.code_pages_drawn();
                self.layers_after_frame();
            }
            _ => {}
        }
        if !keeps_focus {
            self.activate_frame(previous);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let _profile = crate::profile::Scope::new("about_to_wait");
        self.loop_wakes += 1;
        // A screen reader hears what the events just handled changed.
        self.a11y_update();
        // Frames requested by a `fenix --new-window` hand-off, which
        // arrived somewhere without an `&ActiveEventLoop` to open a
        // window with. See `App::pending_frames`.
        let mut opened = false;
        for (files, focus) in std::mem::take(&mut self.pending_frames) {
            self.new_frame(event_loop);
            self.open_positions(&files, focus);
            opened = true;
        }
        let plan = self.plan_wake(Instant::now());
        if plan.redraw || opened {
            for frame in 0..self.frames.len() {
                if let Some(window) = self.frame_window(frame) {
                    window.request_redraw();
                }
            }
        }
        event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(plan.wait_until));
    }
}

/// What the event loop does before it sleeps: draw a frame or not, and
/// when to wake with nothing else to wake it (`App::plan_wake`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WakePlan {
    redraw: bool,
    wait_until: Instant,
}

impl App {
    /// `about_to_wait`'s work, without the event loop: what's due now
    /// done, whether the windows need a frame, and when the next thing is
    /// due. An idle editor plans no frame and a long sleep (#74).
    fn plan_wake(&mut self, now: Instant) -> WakePlan {
        let mut needs_redraw = false;
        if self.blink_tick(now) {
            needs_redraw = true;
        }

        // Semantic tokens due to be asked for (typing paused, an ask that
        // timed out): asked now, not on whatever frame comes next.
        self.ask_due_semantic_tokens();
        self.ask_due_lenses();
        self.ask_due_inlay_hints();

        // Keep redrawing at animation cadence only while something is
        // actually transitioning (blink fade or an active pulse); once
        // everything settles, fall back to the long, efficient wait for
        // the next blink toggle -- an idle editor still does no per-frame
        // work between blinks.
        let prefs = self.motion();
        let blink_transitioning = prefs.caret_fade && now.duration_since(self.blink_transition_start) < prefs.blink_fade_d;
        let pulse_active = match &self.pulse {
            Some(p) if now.duration_since(p.started) < prefs.pulse_d => true,
            Some(_) => {
                self.pulse = None; // expired -- drop it so redraw stops drawing it
                needs_redraw = true;
                false
            }
            None => false,
        };
        // Any frame's focused pane mid-scroll keeps the clock running,
        // not just the active frame's -- a smooth scroll started on one
        // monitor has to keep easing after focus moves to another.
        let scroll_animating = (0..self.frames.len())
            .filter_map(|frame| self.frame_workspaces(frame))
            .any(|workspaces| workspaces.active_scroll_anims().contains_key(&workspaces.active().focused_id()));
        self.prune_motion(now);
        let animating = blink_transitioning || pulse_active || scroll_animating || self.motion_animating(now);
        if animating {
            needs_redraw = true;
        }
        // A layer moving on its own (a pet walking) is drawn at most every
        // `LAYER_TICK`, not every frame the display can show.
        let layer_wake = self.layer_frame_tick(now);
        if layer_wake.is_some_and(|due| due <= now) {
            needs_redraw = true;
        }

        // Folders whose names changed, once the burst has settled.
        let relist_due = self.explorer_relist_dirty(now);
        if now >= self.next_disk_check {
            self.poll_files_changed_on_disk();
        }
        // Buffers changed and still a moment, told to the extensions.
        let changes_told_at = self.code_buffers_changed(now);
        // And the lines the panes show, still a moment.
        let changes_told_at = match (changes_told_at, self.code_views_changed(now)) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        // Extensions' service calls past their deadline (an agent's tool
        // call), given up on.
        let changes_told_at = match (changes_told_at, self.code_serve_deadlines(now)) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        // What has the focus, when it moved -- the explorer, the terminal
        // panel, a picker too -- told to those that ask.
        self.code_focus_moved();
        // A tutorial step that's waited long enough shows its hint.
        if self.tutorial_tick(now) {
            needs_redraw = true;
        }

        // The disk check has to be one of the deadlines the loop waits
        // on, not just something that happens when a redraw was already
        // due -- an idle editor whose terminal panel just rewrote a file
        // has no other reason to wake up.
        // A caret that has stopped blinking has nothing to wake for.
        let wait_until = if animating { now + ANIM_TICK } else if self.caret_settled(now) { now + IDLE_WAIT } else { self.next_blink };
        let wait_until = if layer_wake.is_some() { wait_until.min(self.next_layer_frame) } else { wait_until };
        let wait_until = wait_until.min(self.next_disk_check);
        let wait_until = changes_told_at.map_or(wait_until, |at| wait_until.min(at));
        let wait_until = relist_due.map_or(wait_until, |at| wait_until.min(at));
        // An extension's layer whose next frame the loop just woke for.
        let motion_on = crate::motion::level(&self.config) != crate::motion::Level::Off;
        if self.wasm.layers.frame_due(now, motion_on) {
            needs_redraw = true;
        }
        // A message about to fade, or the spinner's next blade.
        let wait_until = match self.motion_next_wake(now) {
            Some(wake) if wake <= now => {
                needs_redraw = true;
                now + ANIM_TICK
            }
            Some(wake) => wait_until.min(wake),
            None => wait_until,
        };
        // A pending listing has no other reason to wake the loop: nothing
        // is animating and no key was pressed, so without this the "still
        // reading" line would never appear and a slow share would look
        // exactly like a frozen pane.
        let wait_until = self
            .explorer_requests
            .values()
            .filter(|r| !r.done)
            .map(|r| r.started + SLOW_LISTING_AFTER)
            .fold(wait_until, |acc, deadline| acc.min(deadline));
        // A language server that never answered `initialize`, given up.
        let wait_until = match self.lsp_initialize_tick(now) {
            Some(due) => wait_until.min(due),
            None => wait_until,
        };
        // The Search page's typing, once it pauses.
        let wait_until = match self.search_fire_due(now) {
            Some(due) => wait_until.min(due),
            None => wait_until,
        };
        // The test explorer: scans once typing pauses, runs on save, a
        // run's timeout, a project's first scan.
        let wait_until = match self.tests_tick(now) {
            Some(due) => wait_until.min(due),
            None => wait_until,
        };
        // Its page and the modeline's item, at most ten times a second
        // while its model changes.
        let (tests_drawn, tests_due) = self.tests_views_due(now);
        needs_redraw |= tests_drawn;
        let wait_until = match tests_due {
            Some(due) => wait_until.min(due),
            None => wait_until,
        };
        // The agents: cancels that never ended, restarts after a crash,
        // idle agents stopped, files they're changing looked at.
        let wait_until = match self.agents_tick(now) {
            Some(due) => wait_until.min(due),
            None => wait_until,
        };
        // A drag held past a pane's edge scrolls on; the pointer resting
        // on a symbol shows its hover.
        let wait_until = match self.drag_scroll_tick(now) {
            Some(due) => wait_until.min(due),
            None => wait_until,
        };
        let wait_until = match self.hover_tick(now) {
            Some(due) => wait_until.min(due),
            None => wait_until,
        };
        // Auto-save: edits that have rested, or a buffer just left.
        let wait_until = match self.auto_save_tick(now) {
            Some(due) => wait_until.min(due),
            None => wait_until,
        };
        // The daily look for a newer Fenix, when it's on.
        let wait_until = match self.update_tick(now) {
            Some(due) => wait_until.min(due),
            None => wait_until,
        };
        // Notifications: the next toast to time out.
        let wait_until = match self.notifications_tick(now) {
            Some(due) => wait_until.min(due),
            None => wait_until,
        };
        // The extensions that start with Fenix, should no first frame
        // have come to start them (#33).
        let wait_until = match self.code_startup_tick(now) {
            Some(due) => wait_until.min(due),
            None => wait_until,
        };
        // The first frame, still not drawn: the window is stuck on the
        // splash (#65).
        let wait_until = match self.first_frame_tick(now) {
            Some(due) => wait_until.min(due),
            None => wait_until,
        };
        // A window that stopped drawing, and the device behind it.
        let wait_until = match self.gpu_tick(now) {
            Some(due) => wait_until.min(due),
            None => wait_until,
        };
        WakePlan { redraw: needs_redraw, wait_until }
    }
}

/// Test-only conveniences for driving the focused buffer directly --
/// mirrors the exact inline-field-access pattern the equivalent
/// production code paths use (see e.g. `handle_key`'s Vim dispatch),
/// just named so test setup isn't repeating the borrow-splitting dance
/// at every call site.
#[cfg(test)]
impl App {
    fn test_insert(&mut self, ch: char) {
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        buffer.insert_char(cursor, ch);
    }

    fn test_insert_str(&mut self, s: &str) {
        for ch in s.chars() {
            self.test_insert(ch);
        }
    }

    /// Lays the focused Home buffer out as if its pane were 140 x 45
    /// cells -- what a frame does -- and returns the layout.
    fn test_home(&mut self) -> dashboard::HomeView {
        let (id, pane) = (self.focused_buffer_id(), self.focused_pane_id());
        self.refresh_home_data(true);
        self.home_views.remove(&id);
        self.ensure_home_layout(id, pane, 140, 45);
        self.home_views[&id].clone()
    }

    /// Puts the cursor on the slot showing `entry` in the focused Home.
    fn test_select_home(&mut self, entry: &dashboard::HomeEntry) {
        let view = self.test_home();
        let slot = &view.slots[view.find(entry).unwrap_or_else(|| panic!("no {entry:?} on Home:\n{}", view.text))];
        let (line, col) = slot.cursor();
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        cursor.char_idx = buffer.line_start_char(line) + col;
    }

    fn test_vim_key(&mut self, key: KeyPress) -> VimEvent {
        let id = self.focused_buffer_id();
        let pane = self.focused_pane_id();
        let ob = self.buffers.get_mut(id).expect("focused window always has an open buffer");
        let pane_state = self.workspaces.active_pane_states_mut().get_mut(&pane).expect("every existing pane has a PaneState");
        self.vim.handle_key(&mut ob.buffer, &mut pane_state.cursor, key)
    }

    /// Test-only mirror of `dispatch_keypress` -- the real one needs a
    /// live `&ActiveEventLoop`, which nothing in this test harness can
    /// construct (confirmed: no existing test calls `handle_key`/
    /// `dispatch_keypress`/`route_keypress` at all, for exactly this
    /// reason). This validates the same recording-tap/finalize/replay
    /// bookkeeping for ordinary Vim-reaching keys -- the realistic bulk
    /// of any macro's content -- but, like every other App-level test
    /// helper in this file, doesn't go through the leader trie or any
    /// prompt intercept, so it can't claim coverage of "a macro that
    /// replays a leader command."
    fn test_dispatch_key(&mut self, kp: KeyPress) {
        if self.vim.is_recording() {
            self.macro_capture.push(kp);
        }
        // Mirrors `dispatch_keypress`'s own `.`-capture span tracking.
        if self.vim.is_idle() {
            self.change_capture.clear();
            self.change_capture_dirty = false;
        }
        self.change_capture.push(kp);
        self.test_route_key(kp);
        if self.vim.is_idle() {
            if self.change_capture_dirty {
                self.last_change_keys = Some(std::mem::take(&mut self.change_capture));
            } else {
                self.change_capture.clear();
            }
        }
    }

    /// The non-tapping half of `test_dispatch_key`, mirroring `route_
    /// keypress` -- `test_play_macro`'s replay loop calls this directly,
    /// skipping the tap, for the same reason `play_macro` calls `route_
    /// keypress` (not `dispatch_keypress`) in production: a nested `@b`
    /// call's own literal keystrokes are tapped by `test_dispatch_key`
    /// as ordinary top-level input, but `b`'s *expanded* replay must not
    /// also be appended on top of that.
    fn test_route_key(&mut self, kp: KeyPress) {
        self.sync_vim_view(kp);
        let id = self.focused_buffer_id();
        let pane = self.focused_pane_id();
        let vim_event = {
            let reverts_edits = self.buffers.get(id).is_some_and(|ob| self.buffer_edits_are_reverted(id, ob.kind));
            let ob = self.buffers.get_mut(id).expect("focused window always has an open buffer");
            let pane_state = self.workspaces.active_pane_states_mut().get_mut(&pane).expect("every existing pane has a PaneState");
            let mode_before = self.vim.mode();
            let edit_count_before = ob.buffer.edit_count();
            let event = self.vim.handle_key(&mut ob.buffer, &mut pane_state.cursor, kp);
            // Mirrors `route_keypress`'s own instrumentation -- see
            // `dispatch_keypress`'s doc comment.
            if ob.buffer.edit_count() != edit_count_before {
                if !self.replaying_change {
                    self.change_capture_dirty = true;
                }
                self.marks.insert('.', JumpEntry { buffer: id, char_idx: pane_state.cursor.char_idx });
                // Mirrors `route_keypress`'s own read-only enforcement --
                // see `is_readonly_buffer_kind`'s doc comment.
                if reverts_edits {
                    ob.buffer.undo(&mut pane_state.cursor);
                }
            }
            if matches!(mode_before, Mode::Insert | Mode::Replace) && !self.vim.is_inserting() {
                let char_idx = self.vim.last_insert_exit().unwrap_or(pane_state.cursor.char_idx);
                self.marks.insert('^', JumpEntry { buffer: id, char_idx });
            }
            event
        };
        match vim_event {
            VimEvent::MacroRecordStart(register) => {
                self.macro_capture.clear();
                self.macro_recording_append = register.is_ascii_uppercase();
            }
            VimEvent::MacroRecordStop(register) => {
                self.macro_capture.pop();
                let keys = std::mem::take(&mut self.macro_capture);
                self.vim.finish_recording(register, &keys, self.macro_recording_append);
            }
            VimEvent::MacroPlay { register, count } => self.test_play_macro(register, count),
            VimEvent::RepeatLastChange { count } => self.test_repeat_last_change(count),
            VimEvent::MarkSet(name, char_idx) => {
                self.marks.insert(name, JumpEntry { buffer: id, char_idx });
            }
            VimEvent::Error(msg) => self.set_error(msg),
            VimEvent::Message(msg) => self.set_message(msg),
            VimEvent::ToggleComment { start_line, end_line } => self.toggle_comment_lines(start_line, end_line),
            VimEvent::FilterLines { first, last, command } => self.filter_lines(first, last, &command),
            // Along the Ember Trail while there's one here; the TODOs
            // otherwise.
            VimEvent::BracketJump { target: fenix_vim::BracketTarget::Todo, forward, count } if self.debug_trail_here() => self.debug_trail_scrub(forward, count as usize),
            VimEvent::BracketJump { target: fenix_vim::BracketTarget::Todo, forward, count } => self.jump_to_todo(forward, count),
            VimEvent::BracketJump { target: fenix_vim::BracketTarget::Hunk, forward, count } => self.jump_to_hunk(forward, count),
            VimEvent::BracketJump { target: fenix_vim::BracketTarget::Diagnostic, forward, count } => self.jump_to_diagnostic(forward, count),
            VimEvent::BracketJump { target: fenix_vim::BracketTarget::Change, forward, count } => self.side_by_side_jump(forward, count),
            VimEvent::BracketJump { target: fenix_vim::BracketTarget::AgentHunk, forward, count } => self.agents_jump_to_hunk(forward, count),
            VimEvent::Diff { put } => self.side_by_side_take(put),
            VimEvent::Tab(mv) => self.move_tab(mv),
            _ => {}
        }
        self.after_vim_key(kp);
    }

    /// Test-only mirror of `route_keypress`'s own `%`-on-an-XML-tag claim.
    fn test_percent(&mut self) {
        if !(self.vim.is_idle() && self.xml_jump_matching_tag()) {
            self.test_route_key(KeyPress::char('%'));
        }
    }

    /// Test-only mirror of the completion tier of `route_keypress`:
    /// offer the key to the popup first, and only otherwise let it reach
    /// Vim and re-derive the popup from the result. Both halves are the
    /// production functions -- only the tier ordering is restated here,
    /// since `route_keypress` itself needs a live `&ActiveEventLoop`
    /// (see `test_dispatch_key`).
    fn test_completion_key(&mut self, kp: KeyPress) {
        if self.completion_key(kp) {
            return;
        }
        self.test_route_key(kp);
        self.sync_completion();
    }

    /// Test-only mirror of `repeat_last_change` -- identical logic,
    /// replaying through `test_route_key` instead of the real `route_
    /// keypress`.
    fn test_repeat_last_change(&mut self, count: Option<u32>) {
        let Some(keys) = self.last_change_keys_with(count) else { return };
        self.replaying_change = true;
        for key in &keys {
            self.test_route_key(*key);
        }
        self.replaying_change = false;
    }

    /// Test-only mirror of `play_macro` -- identical logic, replaying
    /// through `test_route_key` instead of the real `route_keypress`.
    fn test_play_macro(&mut self, register: char, count: u32) {
        let register = if register == '@' { self.last_played_macro } else { Some(register) };
        let Some(register) = register else { return };
        if self.replay_depth >= MAX_MACRO_REPLAY_DEPTH {
            return;
        }
        let Some(keys) = self.vim.decode_register(register) else { return };
        self.last_played_macro = Some(register);
        if self.replay_depth == 0 {
            self.replay_budget = MACRO_REPLAY_BUDGET;
        }
        self.replay_depth += 1;
        'replay: for _ in 0..count.max(1) {
            for key in &keys {
                if self.replay_budget == 0 {
                    break 'replay;
                }
                self.replay_budget -= 1;
                self.test_route_key(*key);
            }
        }
        self.replay_depth -= 1;
    }

    /// Points the focused pane at `path`, opening it fresh in the
    /// registry -- test-only equivalent of what `open_file_from_picker`/
    /// `explorer_open_selected` do in production.
    fn test_open_path(&mut self, path: &Path) {
        let id = self.buffers.open_path(path);
        self.note_disk_state(id);
        let focused = self.focused_pane_id();
        self.set_pane_content(focused, id);
    }

    /// Adds a parked second (third, ...) frame showing `buffer` in a
    /// single pane, and returns its index. The headless equivalent of
    /// what the `frame.new` command does for real -- minus the winit
    /// window and GPU resources, which a test can't have and which
    /// nothing about frame *bookkeeping* depends on (every render field
    /// is already `Option` for exactly this reason).
    #[cfg(test)]
    fn test_add_frame(&mut self, buffer: BufferId) -> usize {
        let cursor = self.buffers.get(buffer).map(|ob| ob.cursor).unwrap_or(Cursor::at_start());
        self.frames.push(Some(FrameState {
            window: None,
            gpu: None,
            text: None,
            bg_rect: None,
            popup_rect: None,
            caret_rect: None,
            motion_gpu: None,
            workspaces: WorkspaceList::new(WindowTree::new(buffer), cursor),
            cursor_pos: None,
            origin: (0, 0),
            sidebar: None,
            sidebar_open: false,
            sidebar_focused: false,
            terminal_open: false,
            terminal_focused: false,
        }));
        self.frames.len() - 1
    }

    /// Puts `frame`'s client area at `origin` in desktop coordinates --
    /// the headless stand-in for `refresh_frame_origin`, which needs a
    /// real window to ask. Cross-window navigation is decided entirely
    /// from these, so this is what lets it be tested without monitors.
    #[cfg(test)]
    fn test_place_frame(&mut self, frame: usize, origin: (i32, i32)) {
        match self.frames.get_mut(frame) {
            Some(Some(parked)) => parked.origin = origin,
            // The active frame's state is the live set on `App`.
            Some(None) => self.frame_origin = origin,
            None => {}
        }
    }

    /// The focused pane's live cursor -- test-only convenience wrapping
    /// `pane_state`, since tests can't borrow two fields at once the way
    /// production code inlines it (`focused_buffer_and_cursor_mut`, etc.).
    #[cfg(test)]
    fn test_cursor(&self) -> Cursor {
        self.pane_state(self.focused_pane_id()).cursor
    }

    /// Overwrites the focused pane's live cursor -- test-only convenience
    /// replacing the old `app.open_mut().cursor = ...` idiom, now that
    /// cursor lives on the pane, not the buffer.
    #[cfg(test)]
    fn test_set_cursor(&mut self, cursor: Cursor) {
        let pane = self.focused_pane_id();
        self.pane_state_mut(pane).cursor = cursor;
    }
}

#[cfg(test)]
mod tests;
