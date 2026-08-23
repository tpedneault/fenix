use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fenix_buffers::{BufferId, BufferList, OpenBuffer};
use fenix_explorer::{ExplorerAction, ExplorerState};
use fenix_keymap::{KeyCode, KeyPress, Matcher, NamedKey as FenixNamedKey, Step};
use fenix_vim::{Mode, VimEvent, VimState, VisualKind};
use fenix_window::{NavDirection, SplitKind, WindowTree};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, ModifiersState};
use winit::window::{Window, WindowId};

use fenix_core::Cursor;

use crate::commands::CommandRegistry;
use crate::gpu::GpuState;
use crate::icon;
use crate::keymap;
use crate::popup;
use crate::rect::RectRenderer;
use crate::text::{self, TextPipeline};
use crate::theme::{self, Theme};

const BLINK_INTERVAL: Duration = Duration::from_millis(500);
/// How long the caret takes to fade in/out at each blink toggle, instead
/// of flipping instantly.
const BLINK_FADE: Duration = Duration::from_millis(120);
/// Redraw cadence while an animation (blink fade, later scroll/pulse) is
/// actively transitioning. Idle time uses the much longer `WaitUntil`s
/// below instead -- see the plan's "animations are short bursts, not
/// continuous idle work" rationale.
const ANIM_TICK: Duration = Duration::from_millis(16);
/// How long a yank/paste pulse stays visible before fully fading out.
/// Modeled on orbit-emacs's own yank/paste pulse feature.
const PULSE_DURATION: Duration = Duration::from_millis(300);
/// Alpha the pulse starts at before fading -- brighter than the steady
/// Visual-selection overlay so it reads as a distinct flash, not a
/// held selection.
const PULSE_PEAK_ALPHA: f32 = 0.45;
/// How long the viewport takes to ease to a new scroll position.
const SCROLL_DURATION: Duration = Duration::from_millis(150);
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

/// Vertical padding inside the which-key popup, above its first row and
/// below its last -- factored into both its own height and `popup::
/// max_rows`'s "how many rows actually fit" calculation, so the two stay
/// consistent with each other.
const WHICH_KEY_PADDING: f32 = 8.0;

/// An active yank/paste highlight, fading out over `PULSE_DURATION`.
struct Pulse {
    range: std::ops::Range<usize>,
    started: Instant,
}

/// An in-flight scroll transition: eases `rendered_scroll` from `from`
/// to the (now-current) `scroll_line` target.
struct ScrollAnim {
    from: f32,
    to: usize,
    started: Instant,
}

/// Per-visible-line highlight segments: (view_row, col_start, col_end).
type Segments = Vec<(usize, usize, usize)>;

/// A content or sidebar row's rich-text spans: (text, color, use_icon_font).
type RowSpans = Vec<(String, glyphon::Color, bool)>;
/// `explorer_row_spans`'s result: the spans, which rendered row (if any)
/// is the selected entry, and which rows are marked.
type ExplorerRowsResult = (RowSpans, Option<usize>, Vec<usize>);

/// Line-number gutter display. Not wired to a config file yet -- there
/// isn't one yet (`App::with_file` just picks a hardcoded default) -- but
/// the enum exists now so hooking that up later is a matter of reading
/// this field from somewhere else, not redesigning the rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineNumberMode {
    Off,
    Absolute,
    Relative,
}

/// What occupies the main content area: the editor buffer, a full-buffer
/// directory listing, or a fuzzy-filtered picker (find-file/grep/switch-
/// project) -- all three "visit something else instead of the buffer for
/// a moment," the same reasoning that lets `Picker` reuse `Explorer`'s
/// stash/restore machinery unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainView {
    Editor,
    Explorer,
    Picker,
}

/// A picker's payload type varies by what it's picking between, but
/// `App` only ever has one active at a time -- an enum, not three
/// separate `Option` fields, the same reasoning `ExplorerPrompt` already
/// uses for "one thing, a few kinds."
enum ActivePicker {
    FindFile(fenix_picker::PickerState<PathBuf>),
    Grep(fenix_picker::PickerState<fenix_project::GrepMatch>),
    SwitchProject(fenix_picker::PickerState<PathBuf>),
    SwitchBuffer(fenix_picker::PickerState<BufferId>),
}

// The three `ActivePicker` variants wrap `PickerState<T>` for different
// `T`, so there's no single method call that works across all of them --
// these free functions do the one-line match each caller would otherwise
// repeat.
fn picker_push_char(picker: &mut ActivePicker, c: char) {
    match picker {
        ActivePicker::FindFile(s) => s.push_char(c),
        ActivePicker::Grep(s) => s.push_char(c),
        ActivePicker::SwitchProject(s) => s.push_char(c),
        ActivePicker::SwitchBuffer(s) => s.push_char(c),
    }
}

fn picker_backspace(picker: &mut ActivePicker) {
    match picker {
        ActivePicker::FindFile(s) => s.backspace(),
        ActivePicker::Grep(s) => s.backspace(),
        ActivePicker::SwitchProject(s) => s.backspace(),
        ActivePicker::SwitchBuffer(s) => s.backspace(),
    }
}

fn picker_move_selection(picker: &mut ActivePicker, delta: isize) {
    match picker {
        ActivePicker::FindFile(s) => s.move_selection(delta),
        ActivePicker::Grep(s) => s.move_selection(delta),
        ActivePicker::SwitchProject(s) => s.move_selection(delta),
        ActivePicker::SwitchBuffer(s) => s.move_selection(delta),
    }
}

fn picker_query(picker: &ActivePicker) -> &str {
    match picker {
        ActivePicker::FindFile(s) => s.query(),
        ActivePicker::Grep(s) => s.query(),
        ActivePicker::SwitchProject(s) => s.query(),
        ActivePicker::SwitchBuffer(s) => s.query(),
    }
}

fn picker_len(picker: &ActivePicker) -> usize {
    match picker {
        ActivePicker::FindFile(s) => s.len(),
        ActivePicker::Grep(s) => s.len(),
        ActivePicker::SwitchProject(s) => s.len(),
        ActivePicker::SwitchBuffer(s) => s.len(),
    }
}

fn picker_selected_row(picker: &ActivePicker) -> usize {
    match picker {
        ActivePicker::FindFile(s) => s.selected_row(),
        ActivePicker::Grep(s) => s.selected_row(),
        ActivePicker::SwitchProject(s) => s.selected_row(),
        ActivePicker::SwitchBuffer(s) => s.selected_row(),
    }
}

/// Labels only (rendering doesn't need the payload) for the windowed
/// slice `[offset, offset + count)`, paired with whether each is the
/// current selection -- same windowing shape `ExplorerState`'s rendering
/// already uses for a directory listing.
fn picker_visible_labels(picker: &ActivePicker, offset: usize, count: usize) -> Vec<(bool, String)> {
    match picker {
        ActivePicker::FindFile(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::Grep(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::SwitchProject(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::SwitchBuffer(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
    }
}

/// Which explorer-mode prompt (if any) is capturing the next keystrokes --
/// same "next key(s) mean something special" pattern `fenix-vim`'s
/// `Mode::Command`/`pending_replace` already use, applied to file-manager
/// text input instead of Vim's own command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptKind {
    ConfirmDelete,
    Rename,
    CreateFile,
    CreateDir,
    CopyTo,
    MoveTo,
}

struct ExplorerPrompt {
    kind: PromptKind,
    input: String,
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

/// Ease-out cubic: fast start, gentle settle. Used for every short
/// transition animation (caret fade now; scroll/pulse reuse it too).
fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
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

/// One-letter badge for an explorer row's git status.
fn git_status_marker(status: fenix_explorer::GitStatus) -> &'static str {
    match status {
        fenix_explorer::GitStatus::Modified => "M",
        fenix_explorer::GitStatus::Staged => "S",
        fenix_explorer::GitStatus::Untracked => "?",
        fenix_explorer::GitStatus::Ignored => "I",
        fenix_explorer::GitStatus::Conflicted => "U",
    }
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
fn format_age(modified: std::time::SystemTime) -> String {
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
}

/// A non-empty, ordered list of workspaces with one active at a time.
/// Switching workspaces is just moving `active` -- the `WindowTree`s
/// themselves are untouched, so a workspace's layout is exactly as it
/// was left.
struct WorkspaceList {
    workspaces: Vec<Workspace>,
    active: usize,
}

impl WorkspaceList {
    fn new(initial_windows: WindowTree<BufferId>) -> Self {
        Self { workspaces: vec![Workspace { name: "workspace-1".to_string(), windows: initial_windows }], active: 0 }
    }

    fn active(&self) -> &WindowTree<BufferId> {
        &self.workspaces[self.active].windows
    }

    fn active_mut(&mut self) -> &mut WindowTree<BufferId> {
        &mut self.workspaces[self.active].windows
    }

    fn active_name(&self) -> &str {
        &self.workspaces[self.active].name
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
    /// at rather than nothing. Becomes active.
    fn new_workspace(&mut self, content: BufferId) {
        let name = format!("workspace-{}", self.workspaces.len() + 1);
        self.workspaces.push(Workspace { name, windows: WindowTree::new(content) });
        self.active = self.workspaces.len() - 1;
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

pub struct App {
    window: Option<Arc<Window>>,
    gpu: Option<GpuState>,
    text: Option<TextPipeline>,
    /// Opaque panel backgrounds (modeline bar, which-key popup) and the
    /// selection highlight -- drawn *before* text, so they sit behind it
    /// instead of covering it. A single alpha-blended draw call can't do
    /// both "opaque background behind this text" and "opaque background
    /// in front of that text" at once, so the caret gets its own renderer
    /// drawn after text instead of sharing this one.
    bg_rect: Option<RectRenderer>,
    caret_rect: Option<RectRenderer>,

    /// Every open buffer, keyed by `BufferId` -- cursor, scroll position,
    /// and syntax state all live on each buffer's own `OpenBuffer`
    /// (buffer-local, not per-window; see the type's own doc comment for
    /// why). `App` never touches a buffer directly, only through this and
    /// `windows` -- use the `open`/`open_mut`/`focused_buffer_id` helpers.
    buffers: BufferList,
    /// Every workspace's own split layout -- `windows()`/`windows_mut()`
    /// (right after the constructor) are the accessors to use everywhere
    /// else; they read/write whichever workspace is currently active, the
    /// same "always go through the helper, not the field" discipline
    /// `open`/`open_mut` already establish for buffers.
    workspaces: WorkspaceList,
    /// In-flight scroll transitions, keyed by which buffer they're
    /// easing -- kept here rather than on `OpenBuffer` since `ScrollAnim`
    /// needs `Instant`, an animation/rendering-layer concern `fenix-
    /// buffers` (host-agnostic, no such concept) shouldn't need to know
    /// about.
    scroll_anims: HashMap<BufferId, ScrollAnim>,

    /// See `LineNumberMode` doc comment -- hardcoded for now.
    line_number_mode: LineNumberMode,

    main_view: MainView,
    /// The full-buffer listing (`main_view == Explorer`). A separate
    /// field from `sidebar` -- they were one shared `Option` early on,
    /// but that aliased the two uses together: jumping to a full-buffer
    /// listing while a sidebar was already open would silently clobber
    /// the sidebar's state out from under it. Independent fields mean
    /// each mode's state survives the other being invoked.
    explorer: Option<ExplorerState>,
    /// The sidebar's own listing, independent of `explorer`. `Some` for
    /// as long as the sidebar is open, regardless of whether it's
    /// currently focused.
    sidebar: Option<ExplorerState>,
    sidebar_open: bool,
    /// Whether keys currently route to the sidebar's trie instead of Vim
    /// -- only meaningful while `sidebar_open`. The sidebar stays visible
    /// but unfocused while editing, like a persistent project tree.
    sidebar_focused: bool,
    explorer_prompt: Option<ExplorerPrompt>,
    /// Topmost visible row of the full-buffer/sidebar listings. Plain
    /// integers, not eased like `rendered_scroll` -- a directory listing
    /// doesn't get the smooth-scroll treatment the editor buffer does,
    /// just enough to keep the selection on screen.
    explorer_scroll: usize,
    sidebar_scroll: usize,
    /// Same role as `explorer_scroll`, for the picker's candidate list.
    /// Reset to 0 whenever a new picker is entered.
    picker_scroll: usize,

    /// The current buffer's project root (re-derived whenever a file is
    /// opened -- same "always fresh, never stale" posture as everything
    /// else computed from buffer state). `None` outside any recognized
    /// project.
    project_root: Option<PathBuf>,
    active_picker: Option<ActivePicker>,
    /// The grep search-term prompt, when in progress -- `Some` only
    /// between `SPC p s` and the term being submitted (or cancelled),
    /// not while an `ActivePicker::Grep` is already showing results.
    pending_grep_query: Option<String>,
    /// Loaded once at startup; saved back to disk every time a new
    /// project root is visited. Empty (silently) on a platform with no
    /// config-directory concept, or if the file can't be read for some
    /// other reason -- a picker just starting with no history isn't
    /// worth failing over.
    known_projects: fenix_project::KnownProjects,

    vim: VimState,
    /// Persists across keystrokes so a `SPC f s` sequence can span several
    /// `handle_key` calls; `'static` because the leader trie is a global
    /// singleton (see `keymap::leader_trie`), which sidesteps
    /// `Matcher` borrowing from a trie `App` would otherwise also own.
    leader_matcher: Matcher<'static, &'static str>,
    /// Same reasoning as `leader_matcher`, for the explorer's own trie --
    /// persists across keystrokes for its one multi-key sequence (`g r`).
    explorer_matcher: Matcher<'static, ExplorerAction>,

    theme: &'static Theme,
    /// Where the current theme choice is persisted -- resolved once at
    /// startup (`theme::default_path()`, falling back to a relative path
    /// on the rare platform with no config-directory concept, same
    /// fallback `known_projects_path` already uses) and reused by every
    /// `cycle_theme` save.
    theme_path: PathBuf,

    modifiers: ModifiersState,
    /// Whether the caret is fading toward visible or toward hidden --
    /// the *target* of the current transition, not necessarily what's on
    /// screen right now (see `caret_alpha`).
    blink_visible: bool,
    /// When the current fade started; `caret_alpha` eases from this.
    blink_transition_start: Instant,
    next_blink: Instant,
    pulse: Option<Pulse>,
}

impl App {
    pub fn new() -> Self {
        Self::with_file(env::args().nth(1))
    }

    fn with_file(file_arg: Option<String>) -> Self {
        let mut buffers = BufferList::new();
        let initial_id = match file_arg {
            Some(path) => buffers.open_path(Path::new(&path)),
            None => buffers.open_scratch(),
        };
        let workspaces = WorkspaceList::new(WindowTree::new(initial_id));

        let project_root =
            buffers.get(initial_id).and_then(|ob| ob.buffer.path()).and_then(fenix_project::find_project_root);
        let known_projects_path =
            fenix_project::KnownProjects::default_path().unwrap_or_else(|| PathBuf::from("fenix-projects.txt"));
        let mut known_projects = fenix_project::KnownProjects::load_or_default(known_projects_path);
        if let Some(root) = &project_root {
            known_projects.add(root.clone());
            let _ = known_projects.save();
        }
        let theme_path = theme::default_path().unwrap_or_else(|| PathBuf::from("fenix-theme.txt"));
        let theme = theme::load_from(&theme_path);

        Self {
            window: None,
            gpu: None,
            text: None,
            bg_rect: None,
            caret_rect: None,
            buffers,
            workspaces,
            scroll_anims: HashMap::new(),
            line_number_mode: LineNumberMode::Absolute,
            main_view: MainView::Editor,
            explorer: None,
            sidebar: None,
            sidebar_open: false,
            sidebar_focused: false,
            explorer_prompt: None,
            explorer_scroll: 0,
            sidebar_scroll: 0,
            picker_scroll: 0,
            project_root,
            active_picker: None,
            known_projects,
            pending_grep_query: None,
            vim: VimState::new(),
            leader_matcher: keymap::leader_trie().matcher(),
            explorer_matcher: fenix_explorer::explorer_trie().matcher(),
            theme,
            theme_path,
            modifiers: ModifiersState::empty(),
            blink_visible: true,
            blink_transition_start: Instant::now() - BLINK_FADE,
            next_blink: Instant::now() + BLINK_INTERVAL,
            pulse: None,
        }
    }

    /// The active workspace's window tree. Every call site outside this
    /// pair of accessors goes through here (or `windows_mut`) rather than
    /// touching `self.workspaces` directly, mirroring the `open`/
    /// `open_mut` discipline already used for buffers.
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

    /// Re-derives `project_root` for the focused buffer and registers it
    /// in `known_projects` (auto-add-on-visit, matching Projectile's own
    /// behavior) -- called every time a pane's buffer changes, not just
    /// at startup (`with_file` does the equivalent inline before `self`
    /// exists to call this on).
    fn refresh_project_root(&mut self) {
        self.project_root = self.open().buffer.path().and_then(fenix_project::find_project_root);
        if let Some(root) = self.project_root.clone() {
            self.known_projects.add(root);
            if let Err(err) = self.known_projects.save() {
                eprintln!("fenix: couldn't save project history: {err}");
            }
        }
    }

    pub(crate) fn save(&mut self) {
        if self.open().buffer.path().is_none() {
            eprintln!("fenix: no file path to save to yet; pass a file path as the first argument");
            return;
        }
        let ob = self.open_mut();
        match ob.buffer.save() {
            Ok(()) => println!("fenix: saved {:?}", ob.buffer.path().unwrap()),
            Err(err) => eprintln!("fenix: save failed: {err}"),
        }
        self.wake_caret();
    }

    /// Directory the explorer opens in when asked to "start somewhere
    /// sensible": the focused buffer's own directory if it has a path,
    /// else the process's cwd.
    fn explorer_start_dir(&self) -> PathBuf {
        self.open()
            .buffer
            .path()
            .and_then(|p| p.parent())
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    /// `SPC f j` (dired-jump): replaces the main view with a full-buffer
    /// directory listing at the current file's directory. No stashing
    /// needed -- the focused pane's `BufferId` is left untouched (the
    /// buffer itself stays safely owned by `self.buffers` the whole
    /// time), `main_view` alone controls whether it's rendered as buffer
    /// content or the explorer/picker UI.
    pub(crate) fn explorer_jump(&mut self) {
        let dir = self.explorer_start_dir();
        let explorer = match ExplorerState::open(&dir) {
            Ok(e) => e,
            Err(err) => {
                eprintln!("fenix: couldn't list {} ({err})", dir.display());
                return;
            }
        };
        self.explorer = Some(explorer);
        self.main_view = MainView::Explorer;
        self.wake_caret();
    }

    /// `SPC e t`: toggles the sidebar open/closed. Opening focuses it
    /// immediately (you just asked to browse); closing always returns
    /// focus to the editor.
    pub(crate) fn toggle_sidebar(&mut self) {
        if self.sidebar_open {
            self.sidebar_open = false;
            self.sidebar_focused = false;
            self.sidebar = None;
        } else {
            let dir = self.explorer_start_dir();
            match ExplorerState::open(&dir) {
                Ok(explorer) => {
                    self.sidebar = Some(explorer);
                    self.sidebar_open = true;
                    self.sidebar_focused = true;
                }
                Err(err) => eprintln!("fenix: couldn't list {} ({err})", dir.display()),
            }
        }
        self.wake_caret();
    }

    /// Builds fuzzy-picker candidates for every file in `root` --
    /// relative path as the label (fuzzy-matched and displayed),
    /// absolute path as the payload (what actually gets opened).
    fn find_file_candidates(root: &Path) -> Vec<fenix_picker::Candidate<PathBuf>> {
        fenix_project::list_project_files(root)
            .into_iter()
            .map(|path| {
                let label = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().into_owned();
                fenix_picker::Candidate::new(label, path)
            })
            .collect()
    }

    /// `SPC p f`: a fuzzy file picker scoped to the current project (or
    /// the process's cwd, if no project was detected -- still useful,
    /// just not project-scoped).
    pub(crate) fn picker_find_file(&mut self) {
        let root = self.project_root.clone().unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let candidates = Self::find_file_candidates(&root);
        self.enter_picker(ActivePicker::FindFile(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC p p`: a fuzzy picker over the persisted, MRU-ordered known-
    /// projects list.
    pub(crate) fn picker_switch_project(&mut self) {
        let candidates = self
            .known_projects
            .roots()
            .iter()
            .map(|root| fenix_picker::Candidate::new(root.to_string_lossy().into_owned(), root.clone()))
            .collect();
        self.enter_picker(ActivePicker::SwitchProject(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC p s`: unlike find-file/switch-project (which already have a
    /// full candidate list to fuzzy-filter locally), grep needs a search
    /// term *before* there's anything to show -- this starts a short
    /// modeline-level prompt for it (mirroring Vim's own `:` command
    /// line), not the full picker view yet. `run_grep` opens the actual
    /// picker once results come back.
    pub(crate) fn picker_grep_prompt(&mut self) {
        self.pending_grep_query = Some(String::new());
        self.wake_caret();
    }

    /// Routes one keypress to the in-progress grep search-term prompt --
    /// the same "next keystrokes are special" shape as `explorer_prompt_key`,
    /// scoped to this one always-plain-text case.
    fn grep_query_key(&mut self, key: KeyPress) {
        let Some(query) = &mut self.pending_grep_query else { return };
        match key.code {
            KeyCode::Named(FenixNamedKey::Escape) => self.pending_grep_query = None,
            KeyCode::Named(FenixNamedKey::Enter) => {
                let query = self.pending_grep_query.take().unwrap_or_default();
                self.run_grep(&query);
            }
            KeyCode::Named(FenixNamedKey::Backspace) => {
                query.pop();
            }
            KeyCode::Char(c) => query.push(c),
            _ => {}
        }
        self.wake_caret();
    }

    fn run_grep(&mut self, query: &str) {
        let root = self.project_root.clone().unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        match fenix_project::grep_project(&root, query) {
            Ok(matches) => {
                let candidates = matches
                    .into_iter()
                    .map(|m| {
                        let rel = m.path.strip_prefix(&root).unwrap_or(&m.path).to_string_lossy().into_owned();
                        let label = format!("{rel}:{}: {}", m.line, m.text.trim());
                        fenix_picker::Candidate::new(label, m)
                    })
                    .collect();
                self.enter_picker(ActivePicker::Grep(fenix_picker::PickerState::new(candidates)));
            }
            Err(err) => eprintln!("fenix: search failed: {err}"),
        }
    }

    /// `SPC b b`: a fuzzy picker over every open buffer, MRU-ordered
    /// (most-recently-touched first, matching Doom's own buffer switcher),
    /// each labeled with its path (or `*scratch*` for an unnamed buffer)
    /// and a leading `+` marker for unsaved changes.
    pub(crate) fn picker_switch_buffer(&mut self) {
        let candidates = self
            .buffers
            .mru()
            .iter()
            .map(|&id| {
                let ob = self.buffers.get(id).expect("mru only lists open buffers");
                let name = ob.buffer.path().map(|p| p.display().to_string()).unwrap_or_else(|| "*scratch*".to_string());
                let marker = if ob.buffer.is_dirty() { "+ " } else { "" };
                fenix_picker::Candidate::new(format!("{marker}{name}"), id)
            })
            .collect();
        self.enter_picker(ActivePicker::SwitchBuffer(fenix_picker::PickerState::new(candidates)));
    }

    /// Points the focused pane at an already-open buffer -- `SPC b b`'s
    /// confirm action, and shared by nothing else (unlike `open_file_from_
    /// picker`, there's no path to resolve/read; the buffer already
    /// exists in the registry).
    fn switch_focused_to_buffer(&mut self, id: BufferId) {
        let focused = self.windows().focused_id();
        self.windows_mut().set_content(focused, id);
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

    /// `SPC b k`: closes the focused buffer. Any pane (in this window
    /// tree) currently showing it falls back to the MRU-next open buffer,
    /// or a fresh scratch buffer if none remain -- never leaves a pane
    /// pointing at a closed `BufferId`.
    pub(crate) fn kill_buffer(&mut self) {
        let id = self.focused_buffer_id();
        self.buffers.close(id);
        let fallback = self.buffers.mru().first().copied().unwrap_or_else(|| self.buffers.open_scratch());
        for pane in self.windows().windows() {
            if self.windows().content(pane) == Some(&id) {
                self.windows_mut().set_content(pane, fallback);
            }
        }
        self.buffers.touch(fallback);
        self.refresh_project_root();
        self.wake_caret();
    }

    /// `SPC b X`: opens a fresh, empty, unnamed buffer in the focused pane.
    pub(crate) fn new_scratch_buffer(&mut self) {
        let id = self.buffers.open_scratch();
        let focused = self.windows().focused_id();
        self.windows_mut().set_content(focused, id);
        self.refresh_project_root();
        self.wake_caret();
    }

    /// No stashing needed -- same reasoning as `explorer_jump`, now that
    /// buffers live in `self.buffers` rather than direct `App` fields.
    fn enter_picker(&mut self, picker: ActivePicker) {
        self.active_picker = Some(picker);
        self.picker_scroll = 0;
        self.main_view = MainView::Picker;
        self.wake_caret();
    }

    /// `Escape`: cancels the active picker -- no file/project opened, the
    /// focused pane's buffer was never touched, so returning to the
    /// editor just means switching `main_view` back.
    fn picker_cancel(&mut self) {
        self.active_picker = None;
        self.main_view = MainView::Editor;
    }

    /// `Enter`: confirms the selected candidate, dispatching differently
    /// per picker kind. A no-op (stays open) if the filtered list is
    /// currently empty -- nothing to confirm.
    fn picker_confirm(&mut self) {
        match &self.active_picker {
            Some(ActivePicker::FindFile(state)) => {
                let Some(path) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                self.open_file_from_picker(&path);
            }
            Some(ActivePicker::SwitchProject(state)) => {
                let Some(root) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                self.switch_to_project(root);
            }
            Some(ActivePicker::Grep(state)) => {
                let Some(m) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                self.open_file_from_picker(&m.path);
                self.jump_to_grep_match(&m);
            }
            Some(ActivePicker::SwitchBuffer(state)) => {
                let Some(id) = state.selected().map(|c| c.payload) else { return };
                self.active_picker = None;
                self.switch_focused_to_buffer(id);
            }
            None => {}
        }
        self.wake_caret();
    }

    /// Points the focused pane at `path` (opening it in the registry --
    /// reusing an already-open buffer for the same path, per
    /// `BufferList::open_path`), exactly like opening a file from the
    /// explorer. A picker always fully took over `main_view` to get
    /// here, so confirming always means "this is current now, back to
    /// the editor."
    fn open_file_from_picker(&mut self, path: &Path) {
        let id = self.buffers.open_path(path);
        let focused = self.windows().focused_id();
        self.windows_mut().set_content(focused, id);
        self.refresh_project_root();
        self.main_view = MainView::Editor;
    }

    fn jump_to_grep_match(&mut self, m: &fenix_project::GrepMatch) {
        let ob = self.open_mut();
        let target_line = m.line.saturating_sub(1).min(ob.buffer.visual_line_count().saturating_sub(1));
        let start = ob.buffer.line_start_char(target_line);
        let col = m.col.saturating_sub(1).min(ob.buffer.line_len(target_line));
        ob.cursor.char_idx = start + col;
        let (_, sticky) = ob.buffer.line_col(&ob.cursor);
        ob.cursor.sticky_col = sticky;
    }

    /// Registers `root` as the current project and immediately chains
    /// into a find-file picker scoped to it -- matches Projectile's own
    /// default "switch project" action rather than leaving you with
    /// nothing to do next. `main_view`/the stash are untouched: we're
    /// already mid-picker (this runs from `picker_confirm`), just
    /// swapping which picker is active.
    fn switch_to_project(&mut self, root: PathBuf) {
        self.project_root = Some(root.clone());
        self.known_projects.add(root.clone());
        if let Err(err) = self.known_projects.save() {
            eprintln!("fenix: couldn't save project history: {err}");
        }
        let candidates = Self::find_file_candidates(&root);
        self.active_picker = Some(ActivePicker::FindFile(fenix_picker::PickerState::new(candidates)));
        self.picker_scroll = 0;
    }

    /// Routes one keypress to the active picker: plain characters edit
    /// the query and re-filter, Up/Down or Ctrl-N/Ctrl-P move the
    /// selection, Enter confirms, Escape cancels -- everything else is
    /// ignored (the picker is a text-input mode, not an action trie like
    /// the explorer's, so unrecognized keys just don't do anything
    /// rather than falling through to something else).
    fn picker_key(&mut self, key: KeyPress) {
        let Some(picker) = &mut self.active_picker else { return };
        match key.code {
            KeyCode::Named(FenixNamedKey::Escape) => self.picker_cancel(),
            KeyCode::Named(FenixNamedKey::Enter) => self.picker_confirm(),
            KeyCode::Named(FenixNamedKey::Backspace) => picker_backspace(picker),
            KeyCode::Named(FenixNamedKey::Down) => picker_move_selection(picker, 1),
            KeyCode::Named(FenixNamedKey::Up) => picker_move_selection(picker, -1),
            KeyCode::Char('n') if key.mods.ctrl => picker_move_selection(picker, 1),
            KeyCode::Char('p') if key.mods.ctrl => picker_move_selection(picker, -1),
            KeyCode::Char(c) if !key.mods.ctrl => picker_push_char(picker, c),
            _ => {}
        }
        self.wake_caret();
    }

    /// The explorer currently receiving input: the full-buffer listing
    /// when `main_view == Explorer`, else the sidebar's when it's
    /// focused, else `None` (keys go to Vim instead).
    fn active_explorer(&self) -> Option<&ExplorerState> {
        if self.main_view == MainView::Explorer {
            self.explorer.as_ref()
        } else if self.sidebar_focused {
            self.sidebar.as_ref()
        } else {
            None
        }
    }

    fn active_explorer_mut(&mut self) -> Option<&mut ExplorerState> {
        if self.main_view == MainView::Explorer {
            self.explorer.as_mut()
        } else if self.sidebar_focused {
            self.sidebar.as_mut()
        } else {
            None
        }
    }

    /// Stores a freshly re-opened `ExplorerState` back into whichever
    /// slot is currently active -- used after navigating into a
    /// directory or up to a parent, which replace the whole listing.
    fn set_active_explorer(&mut self, state: ExplorerState) {
        if self.main_view == MainView::Explorer {
            self.explorer = Some(state);
        } else {
            self.sidebar = Some(state);
        }
    }

    /// Dispatches one explorer command against whichever listing is
    /// active. Re-fetches `active_explorer_mut()` fresh per arm rather
    /// than binding it once, since a few arms (`Open`, `ParentDir`,
    /// `Quit`) need to touch other `self` fields too (replacing the
    /// buffer, restoring the stash) and can't do that while still
    /// holding it borrowed.
    fn explorer_handle_action(&mut self, action: ExplorerAction) {
        if self.active_explorer().is_none() {
            return;
        }
        match action {
            ExplorerAction::Down => {
                self.active_explorer_mut().unwrap().move_selection(1);
            }
            ExplorerAction::Up => {
                self.active_explorer_mut().unwrap().move_selection(-1);
            }
            ExplorerAction::ToggleExpand => {
                if let Err(err) = self.active_explorer_mut().unwrap().toggle_expand() {
                    eprintln!("fenix: couldn't expand ({err})");
                }
            }
            ExplorerAction::ToggleMark => self.active_explorer_mut().unwrap().toggle_mark(),
            ExplorerAction::MarkAll => self.active_explorer_mut().unwrap().mark_all(),
            ExplorerAction::UnmarkAll => self.active_explorer_mut().unwrap().unmark_all(),
            ExplorerAction::ToggleAllMarks => self.active_explorer_mut().unwrap().toggle_all_marks(),
            ExplorerAction::ToggleHidden => {
                if let Err(err) = self.active_explorer_mut().unwrap().toggle_hidden() {
                    eprintln!("fenix: couldn't refresh ({err})");
                }
            }
            ExplorerAction::Refresh => {
                if let Err(err) = self.active_explorer_mut().unwrap().refresh() {
                    eprintln!("fenix: couldn't refresh ({err})");
                }
            }
            ExplorerAction::ParentDir => {
                let parent = self.active_explorer().unwrap().cwd.parent().map(Path::to_path_buf);
                if let Some(parent) = parent {
                    match ExplorerState::open(&parent) {
                        Ok(new_state) => self.set_active_explorer(new_state),
                        Err(err) => eprintln!("fenix: couldn't list {} ({err})", parent.display()),
                    }
                }
            }
            ExplorerAction::Open => self.explorer_open_selected(),
            ExplorerAction::BeginDelete => {
                if !self.active_explorer().unwrap().targets().is_empty() {
                    self.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::ConfirmDelete, input: String::new() });
                }
            }
            ExplorerAction::BeginRename => {
                if let Some(name) = self.active_explorer().unwrap().selected_entry().map(|e| e.name.clone()) {
                    self.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::Rename, input: name });
                }
            }
            ExplorerAction::BeginCreateFile => {
                self.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::CreateFile, input: String::new() });
            }
            ExplorerAction::BeginCreateDir => {
                self.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::CreateDir, input: String::new() });
            }
            ExplorerAction::BeginCopy => {
                if !self.active_explorer().unwrap().targets().is_empty() {
                    self.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::CopyTo, input: String::new() });
                }
            }
            ExplorerAction::BeginMove => {
                if !self.active_explorer().unwrap().targets().is_empty() {
                    self.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::MoveTo, input: String::new() });
                }
            }
            ExplorerAction::Quit => self.explorer_quit(),
        }
        self.wake_caret();
    }

    /// `Enter`/`l` on the entry at point: navigates into a directory
    /// (replacing the listing), or visits a file -- replacing the editor
    /// buffer and, depending on how the explorer got here, either
    /// returning to the editor (full-buffer mode, dropping the stash --
    /// the new file is now current) or just handing focus back to it
    /// (sidebar mode, which stays open).
    fn explorer_open_selected(&mut self) {
        let Some(explorer) = self.active_explorer() else { return };
        let Some(entry) = explorer.selected_entry() else { return };
        let path = entry.path.clone();
        let is_dir = entry.is_dir;

        if is_dir {
            match ExplorerState::open(&path) {
                Ok(new_state) => self.set_active_explorer(new_state),
                Err(err) => eprintln!("fenix: couldn't list {} ({err})", path.display()),
            }
            return;
        }

        let id = self.buffers.open_path(&path);
        let focused = self.windows().focused_id();
        self.windows_mut().set_content(focused, id);
        self.refresh_project_root();

        if self.main_view == MainView::Explorer {
            self.main_view = MainView::Editor;
            self.explorer = None;
        } else {
            self.sidebar_focused = false;
        }
    }

    /// `q`/`Escape`: in full-buffer mode, leaves the explorer entirely --
    /// the focused pane's buffer was never touched by entering it, so
    /// there's nothing to restore, just switch `main_view` back; in the
    /// sidebar, just hands focus back to the editor without closing it.
    fn explorer_quit(&mut self) {
        if self.main_view == MainView::Explorer {
            self.main_view = MainView::Editor;
            self.explorer = None;
        } else if self.sidebar_focused {
            self.sidebar_focused = false;
        }
    }

    /// Routes one keypress to whichever explorer prompt is active: a
    /// bare y/n for delete confirmation, or free-text input (with
    /// Backspace/Enter/Escape) for rename/create/copy/move.
    fn explorer_prompt_key(&mut self, key: KeyPress) {
        let Some(prompt) = &mut self.explorer_prompt else { return };

        if prompt.kind == PromptKind::ConfirmDelete {
            if key.code == KeyCode::Char('y') || key.code == KeyCode::Char('Y') {
                self.explorer_prompt = None;
                if let Some(explorer) = self.active_explorer_mut() {
                    if let Err(err) = explorer.delete_targets() {
                        eprintln!("fenix: delete failed: {err}");
                    }
                }
            } else {
                self.explorer_prompt = None;
            }
            self.wake_caret();
            return;
        }

        match key.code {
            KeyCode::Named(FenixNamedKey::Escape) => self.explorer_prompt = None,
            KeyCode::Named(FenixNamedKey::Enter) => {
                let ExplorerPrompt { kind, input } = self.explorer_prompt.take().unwrap();
                self.explorer_prompt_submit(kind, &input);
            }
            KeyCode::Named(FenixNamedKey::Backspace) => {
                prompt.input.pop();
            }
            KeyCode::Char(c) => prompt.input.push(c),
            _ => {}
        }
        self.wake_caret();
    }

    fn explorer_prompt_submit(&mut self, kind: PromptKind, input: &str) {
        let Some(explorer) = self.active_explorer_mut() else { return };
        let result = match kind {
            PromptKind::Rename => explorer.rename_selected(input),
            PromptKind::CreateFile => explorer.create_file(input),
            PromptKind::CreateDir => explorer.create_dir(input),
            PromptKind::CopyTo => explorer.copy_targets_to(Path::new(input)),
            PromptKind::MoveTo => explorer.move_targets_to(Path::new(input)),
            PromptKind::ConfirmDelete => return, // handled in explorer_prompt_key via y/n, not Enter
        };
        if let Err(err) = result {
            eprintln!("fenix: operation failed: {err}");
        }
    }

    /// `SPC w v`/`SPC w s`: splits the focused window vertically (side by
    /// side) or horizontally (stacked), Doom's own `splitright`/
    /// `splitbelow` default -- the new pane shows the *same* buffer as the
    /// one it split from (matching Vim's own `:vsplit`/`:split`), not a
    /// blank scratch buffer, so it starts as a second view of what you
    /// were already looking at.
    fn split_window(&mut self, kind: SplitKind) {
        let id = self.focused_buffer_id();
        self.windows_mut().split(kind, id);
        self.wake_caret();
    }

    pub(crate) fn split_vertical(&mut self) {
        self.split_window(SplitKind::Vertical);
    }

    pub(crate) fn split_horizontal(&mut self) {
        self.split_window(SplitKind::Horizontal);
    }

    /// `SPC w hjkl`: moves focus to the nearest window in that direction
    /// by real layout geometry, not tree adjacency -- a no-op at the
    /// grid's edge (`WindowTree::navigate`'s own contract).
    pub(crate) fn navigate_window(&mut self, dir: NavDirection) {
        self.windows_mut().navigate(dir);
        self.wake_caret();
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
        self.windows_mut().close_focused();
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
        let id = self.focused_buffer_id();
        self.workspaces.new_workspace(id);
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
        self.workspaces.remove_active();
        self.wake_caret();
    }

    pub(crate) fn undo(&mut self) {
        let ob = self.open_mut();
        ob.buffer.undo(&mut ob.cursor);
        self.wake_caret();
    }

    pub(crate) fn redo(&mut self) {
        let ob = self.open_mut();
        ob.buffer.redo(&mut ob.cursor);
        self.wake_caret();
    }

    /// Off -> Absolute -> Relative -> Off. Stands in for a config-file
    /// setting that doesn't exist yet (see `LineNumberMode`) -- a keybound
    /// cycle is how you'd want to flip this at runtime anyway, so it's not
    /// wasted work once config loading lands.
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

    /// `SPC t t`: cycles to the next theme in `theme::ALL` (wrapping) and
    /// persists the choice -- matched by `name`, not pointer identity,
    /// which Rust doesn't guarantee is stable across separate `&SOME_CONST`
    /// expressions. Save failure is non-fatal, same posture as
    /// `refresh_project_root`'s `known_projects.save()`.
    pub(crate) fn cycle_theme(&mut self) {
        let current = theme::ALL.iter().position(|t| t.name == self.theme.name).unwrap_or(0);
        let next = (current + 1) % theme::ALL.len();
        self.theme = theme::ALL[next];
        if let Err(err) = theme::save_to(self.theme, &self.theme_path) {
            eprintln!("fenix: couldn't save theme choice: {err}");
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
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
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// 0.0 (hidden) to 1.0 (fully visible), eased across `BLINK_FADE` from
    /// whichever state the caret last toggled into.
    fn caret_alpha(&self) -> f32 {
        let elapsed = Instant::now().duration_since(self.blink_transition_start);
        let t = ease_out_cubic(elapsed.as_secs_f32() / BLINK_FADE.as_secs_f32());
        if self.blink_visible { t } else { 1.0 - t }
    }

    fn handle_key(&mut self, event: &KeyEvent, event_loop: &ActiveEventLoop) {
        if event.state != ElementState::Pressed {
            return;
        }

        let Some(keypress) = keymap::to_keypress(event, self.modifiers) else { return };

        // An active prompt (rename/create-name/confirm-delete) captures
        // every keystroke until it resolves -- takes priority over
        // everything else, including global Ctrl-chords, so e.g. Ctrl-S
        // can't accidentally interrupt an in-progress rename.
        if self.explorer_prompt.is_some() {
            self.explorer_prompt_key(keypress);
            return;
        }

        // The grep search-term prompt and an open picker both capture all
        // input the same way -- checked ahead of the explorer/sidebar-focus
        // check below since a picker can be opened while the sidebar is
        // focused (switch-project's find-file chain, for instance).
        if self.pending_grep_query.is_some() {
            self.grep_query_key(keypress);
            return;
        }
        if self.active_picker.is_some() {
            self.picker_key(keypress);
            return;
        }

        // The explorer (full-buffer or a focused sidebar) owns all input
        // while it has focus -- its own trie, not Vim's, and not the
        // global Ctrl-chords below (browsing is a distinct modal UI, the
        // same reasoning that already keeps Insert/Command mode out of
        // Normal's trie).
        if self.active_explorer().is_some() {
            if let Step::Matched(&action) = self.explorer_matcher.feed(keypress) {
                self.explorer_handle_action(action);
            }
            self.wake_caret();
            return;
        }

        if self.modifiers.control_key() {
            if let Key::Character(s) = &event.logical_key {
                let id = if s.eq_ignore_ascii_case("s") {
                    Some("file.save")
                } else if s.eq_ignore_ascii_case("z") && self.modifiers.shift_key() {
                    Some("edit.redo")
                } else if s.eq_ignore_ascii_case("z") {
                    Some("edit.undo")
                } else if s.eq_ignore_ascii_case("y") {
                    Some("edit.redo")
                } else if s.eq_ignore_ascii_case("q") {
                    Some("app.quit")
                } else {
                    None
                };
                if let Some(id) = id {
                    // Built fresh per chord rather than stored on App: it's
                    // just a few fn-pointer entries, and this sidesteps
                    // borrowing `self` both as receiver and argument.
                    CommandRegistry::with_builtins().run(self, event_loop, id);
                    self.wake_caret();
                    return;
                }
            }
            // Not a recognized global chord (e.g. Ctrl-r is Vim's redo):
            // fall through instead of swallowing it.
        }

        // Window-size-aware paging is a GUI concern, handled the same way
        // regardless of Vim mode -- fenix-vim doesn't know about viewport size.
        if keypress == KeyPress::named(FenixNamedKey::PageUp)
            || keypress == KeyPress::named(FenixNamedKey::PageDown)
        {
            let page_size = self
                .gpu
                .as_ref()
                .map(|gpu| text::visible_line_count(gpu.size.height as f32))
                .unwrap_or(20);
            let down = keypress == KeyPress::named(FenixNamedKey::PageDown);
            let ob = self.open_mut();
            ob.buffer.move_page(&mut ob.cursor, page_size, down);
            self.wake_caret();
            return;
        }

        // Leader sequences span multiple keystrokes: stay routed here while
        // one is already in progress, or start one on SPC from Normal mode
        // (matching orbit-emacs, where SPC is a Normal-mode-only leader --
        // in Insert mode it should just insert a space).
        if self.leader_matcher.is_pending()
            || (self.vim.mode() == Mode::Normal && keypress == KeyPress::char(' '))
        {
            if let Step::Matched(&id) = self.leader_matcher.feed(keypress) {
                CommandRegistry::with_builtins().run(self, event_loop, id);
            }
            self.wake_caret();
            return;
        }

        let id = self.focused_buffer_id();
        let vim_event = {
            let Some(ob) = self.buffers.get_mut(id) else { return };
            self.vim.handle_key(&mut ob.buffer, &mut ob.cursor, keypress)
        };
        match vim_event {
            VimEvent::RequestSave => {
                CommandRegistry::with_builtins().run(self, event_loop, "file.save");
            }
            VimEvent::RequestQuit => {
                CommandRegistry::with_builtins().run(self, event_loop, "app.quit");
            }
            VimEvent::RequestSaveAndQuit => {
                CommandRegistry::with_builtins().run(self, event_loop, "file.save");
                CommandRegistry::with_builtins().run(self, event_loop, "app.quit");
            }
            VimEvent::Pulse(range) => {
                self.pulse = Some(Pulse { range, started: Instant::now() });
            }
            VimEvent::None => {}
        }
        self.wake_caret();
    }

    /// Keeps the cursor's line within the visible window, scrolling as
    /// needed. Must be called with the same `visible_lines` used to render.
    fn ensure_cursor_visible(&mut self, visible_lines: usize) {
        let id = self.focused_buffer_id();
        let (line, scroll_line) = {
            let ob = self.buffers.get(id).expect("focused window always has an open buffer");
            (ob.buffer.line_col(&ob.cursor).0, ob.scroll_line)
        };
        let target = scroll_to_include(scroll_line, line, visible_lines);
        if target != scroll_line {
            let jump = target.abs_diff(scroll_line);
            if jump > visible_lines.saturating_mul(SCROLL_SNAP_SCREENS) {
                self.scroll_anims.remove(&id);
                self.buffers.get_mut(id).unwrap().rendered_scroll = target as f32;
            } else {
                let from = self.buffers.get(id).unwrap().rendered_scroll;
                self.scroll_anims.insert(id, ScrollAnim { from, to: target, started: Instant::now() });
            }
            self.buffers.get_mut(id).unwrap().scroll_line = target;
        }
        self.update_rendered_scroll();
    }

    /// Advances the focused buffer's `rendered_scroll` toward its
    /// `scroll_line` if a transition is in flight, clearing it once
    /// settled.
    fn update_rendered_scroll(&mut self) {
        let id = self.focused_buffer_id();
        let scroll_line = self.buffers.get(id).unwrap().scroll_line;
        let Some(anim) = self.scroll_anims.get(&id) else {
            self.buffers.get_mut(id).unwrap().rendered_scroll = scroll_line as f32;
            return;
        };
        let t = Instant::now().duration_since(anim.started).as_secs_f32() / SCROLL_DURATION.as_secs_f32();
        if t >= 1.0 {
            self.buffers.get_mut(id).unwrap().rendered_scroll = anim.to as f32;
            self.scroll_anims.remove(&id);
        } else {
            let (from, to) = (anim.from, anim.to as f32);
            self.buffers.get_mut(id).unwrap().rendered_scroll = from + (to - from) * ease_out_cubic(t);
        }
    }

    /// The buffer line rendering starts from -- `rendered_scroll` rounded
    /// down. Content, caret, hl-line, selection, and pulse all anchor
    /// their row math to this (not `scroll_line`, which is only the
    /// *target* `rendered_scroll` is easing toward).
    fn render_base_line(&self) -> usize {
        self.open().rendered_scroll.floor().max(0.0) as usize
    }

    /// (mode label, rest-of-modeline suffix) -- `None` while typing a `:`
    /// command, since that replaces the whole modeline with raw command
    /// text instead of the usual badge + filename + position layout.
    fn modeline_pieces(&self) -> Option<(&'static str, String)> {
        if self.vim.mode() == Mode::Command || self.pending_grep_query.is_some() {
            return None;
        }
        if self.main_view == MainView::Explorer {
            let suffix = match &self.explorer {
                Some(explorer) => {
                    let marked =
                        if explorer.marks.is_empty() { String::new() } else { format!(" [{} marked]", explorer.marks.len()) };
                    format!("│ {}{marked}   {} items ", explorer.cwd.display(), explorer.entries.len())
                }
                None => String::new(),
            };
            return Some(("EXPLORE", suffix));
        }
        if self.main_view == MainView::Picker {
            let (label, count) = match &self.active_picker {
                Some(picker @ ActivePicker::FindFile(_)) => ("FINDFILE", picker_len(picker)),
                Some(picker @ ActivePicker::Grep(_)) => ("GREP", picker_len(picker)),
                Some(picker @ ActivePicker::SwitchProject(_)) => ("SWPROJ", picker_len(picker)),
                Some(picker @ ActivePicker::SwitchBuffer(_)) => ("SWBUF", picker_len(picker)),
                None => ("PICKER", 0),
            };
            return Some((label, format!("│ {count} matches ")));
        }
        let ob = self.open();
        let filename = ob
            .buffer
            .path()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "[No Name]".to_string());
        let modified = if ob.buffer.is_dirty() { " [+]" } else { "" };
        let (line, col) = ob.buffer.line_col(&ob.cursor);
        let mode_label =
            if self.vim.mode() == Mode::Visual { self.vim.visual_kind().label() } else { self.vim.mode().label() };
        // Only shown once there's more than one workspace to distinguish --
        // stays out of the way for the common single-workspace case.
        let workspace_indicator = if self.workspaces.len() > 1 {
            format!("   [{}/{} {}]", self.workspaces.active_index() + 1, self.workspaces.len(), self.workspaces.active_name())
        } else {
            String::new()
        };
        let suffix = format!("│ {filename}{modified}{workspace_indicator}   Ln {}, Col {} ", line + 1, col + 1);
        Some((mode_label, suffix))
    }

    /// Test-only: the modeline as one plain string, for assertions --
    /// `redraw` renders it as separately-colored spans instead (see
    /// `modeline_pieces`/`mode_colors`), so nothing else needs this.
    #[cfg(test)]
    fn modeline_text(&self) -> String {
        if self.vim.mode() == Mode::Command {
            return format!(":{}", self.vim.command_line());
        }
        if let Some(query) = &self.pending_grep_query {
            return format!("rg: {query}");
        }
        let (mode_label, suffix) = self.modeline_pieces().unwrap();
        format!(" {mode_label:^width$}{suffix}", width = text::MODE_BADGE_CHARS)
    }

    /// (badge background, badge text color) for the current mode. Visual's
    /// Whether the caret should render as a full-cell block (Normal,
    /// Visual, Replace, Command) rather than the thin Insert-mode bar --
    /// matches real Vim's own cursor-shape convention (block outside
    /// Insert, thin bar while actively typing).
    fn caret_is_block(&self) -> bool {
        self.vim.mode() != Mode::Insert
    }

    /// three kinds all share one accent (matching orbit-emacs's own
    /// evil-state table, which has a single "Visual" entry) -- only the
    /// badge's label text differs between them.
    fn mode_colors(&self) -> ([f32; 4], glyphon::Color) {
        let theme = self.theme;
        if self.main_view == MainView::Explorer {
            return (theme.mode_explorer, theme.mode_text_dark);
        }
        if self.main_view == MainView::Picker {
            return (theme.mode_picker, theme.mode_text_dark);
        }
        match self.vim.mode() {
            Mode::Normal => (theme.mode_normal, theme.mode_text_dark),
            Mode::Insert => (theme.mode_insert, theme.mode_text_dark),
            Mode::Visual => (theme.mode_visual, theme.mode_text_light),
            Mode::Replace => (theme.mode_replace, theme.mode_text_dark),
            Mode::Command => (theme.mode_command, theme.mode_text_dark),
        }
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
        if self.line_number_mode == LineNumberMode::Off {
            return 0;
        }
        ob.buffer.visual_line_count().max(1).to_string().len() + 1
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
    fn content_spans(
        &self,
        ob: &OpenBuffer,
        render_base_line: usize,
        rows: usize,
        gutter_chars: usize,
        syntax_highlights: &[(std::ops::Range<usize>, glyphon::Color)],
    ) -> Vec<(String, glyphon::Color)> {
        let theme = self.theme;
        let visual_lines = ob.buffer.visual_line_count();
        let (cursor_line, _) = ob.buffer.line_col(&ob.cursor);
        let mut spans = Vec::new();
        for r in 0..rows {
            let buffer_line = render_base_line + r;
            let has_line = buffer_line < visual_lines;
            if gutter_chars > 0 {
                let gutter = if has_line {
                    let n = match self.line_number_mode {
                        LineNumberMode::Relative => buffer_line.abs_diff(cursor_line),
                        _ => buffer_line + 1,
                    };
                    format!("{:>width$} ", n, width = gutter_chars - 1)
                } else {
                    format!("{:<width$}", "~", width = gutter_chars)
                };
                let color = if has_line && buffer_line == cursor_line { theme.fg } else { theme.gutter_fg };
                spans.push((gutter, color));
            } else if !has_line {
                spans.push(("~".to_string(), theme.gutter_fg));
            }
            if has_line {
                let start = ob.buffer.line_start_char(buffer_line);
                let len = ob.buffer.line_len(buffer_line);
                let line_text = ob.buffer.text_range(start, start + len);
                if syntax_highlights.is_empty() {
                    spans.push((line_text, theme.fg));
                } else {
                    let line_byte_start = ob.buffer.char_to_byte(start);
                    let line_byte_end = ob.buffer.char_to_byte(start + len);
                    spans.extend(split_line_by_highlights(
                        &line_text,
                        line_byte_start,
                        line_byte_end,
                        syntax_highlights,
                        theme.fg,
                    ));
                }
            }
            if r + 1 < rows {
                spans.push(("\n".to_string(), theme.fg));
            }
        }
        spans
    }

    /// Drains buffer `id`'s edits since the last frame, feeds them through
    /// its active language's incremental parser (if any), and returns
    /// resolved syntax-highlight colors for the visible byte range only --
    /// same windowing discipline `Buffer::visible_text` already uses.
    /// Always drains, even with no active language, so the buffer's edit
    /// log doesn't grow unbounded for files nothing is consuming it for.
    /// Takes an explicit `id` (rather than always the focused buffer) so
    /// every visible pane's buffer gets reparsed, not just the focused
    /// one's -- `self.buffers.get_mut(id)` is a direct field projection,
    /// so this can still read `self.theme` first in the same call without
    /// the opaque-method-call borrow conflict `open_mut()` would risk.
    fn syntax_highlights_for_visible_range(
        &mut self,
        id: BufferId,
        render_base_line: usize,
        rows: usize,
    ) -> Vec<(std::ops::Range<usize>, glyphon::Color)> {
        let theme = self.theme;
        let Some(ob) = self.buffers.get_mut(id) else { return Vec::new() };
        let deltas = ob.buffer.drain_edits();
        let Some(syntax) = &mut ob.syntax else { return Vec::new() };

        let edits: Vec<fenix_syntax::RawEdit> = deltas
            .into_iter()
            .map(|d| fenix_syntax::RawEdit { start_char: d.start_char, new_end_char: d.new_end_char, removed: d.removed })
            .collect();
        let source = ob.buffer.text();
        syntax.apply_edits(&source, &edits);

        let visual_lines = ob.buffer.visual_line_count();
        if render_base_line >= visual_lines {
            return Vec::new();
        }
        let last_line = (render_base_line + rows).min(visual_lines) - 1;
        let start_char = ob.buffer.line_start_char(render_base_line);
        let end_char = ob.buffer.line_start_char(last_line) + ob.buffer.line_len(last_line);
        let byte_range = ob.buffer.char_to_byte(start_char)..ob.buffer.char_to_byte(end_char);

        syntax
            .highlights_in_range(&source, byte_range)
            .into_iter()
            .map(|(range, name)| (range, theme.syntax_color(name)))
            .collect()
    }

    /// Per-visible-line (view_row, col_start, col_end) segments of the
    /// active Visual-mode selection, for highlighting. Empty outside Visual
    /// mode. Shape depends on `visual_kind()`: Char is a single charwise
    /// span, Line highlights whole lines regardless of column, Block is a
    /// column-range rectangle across lines (clamped per ragged line, like
    /// the Block operators themselves).
    fn visual_selection_segments(&self, visible_lines: usize) -> Segments {
        if self.vim.mode() != Mode::Visual {
            return Vec::new();
        }
        let anchor = self.vim.visual_anchor();
        let last_visible = (self.render_base_line() + visible_lines).min(self.open().buffer.line_count());
        let mut segments = Vec::new();

        match self.vim.visual_kind() {
            VisualKind::Char => {
                let cursor_idx = self.open().cursor.char_idx;
                let (lo, hi) =
                    if anchor <= cursor_idx { (anchor, cursor_idx + 1) } else { (cursor_idx, anchor + 1) };
                let hi = hi.min(self.open().buffer.len_chars());
                segments = self.range_to_segments(lo..hi, visible_lines);
            }
            VisualKind::Line => {
                let (line_lo, line_hi) = self.anchor_cursor_line_range(anchor);
                for line in self.render_base_line().max(line_lo)..last_visible.min(line_hi + 1) {
                    // at least 1 col wide so an empty line still shows a sliver
                    let width = self.open().buffer.line_len(line).max(1);
                    segments.push((line - self.render_base_line(), 0, width));
                }
            }
            VisualKind::Block => {
                let (line_lo, line_hi) = self.anchor_cursor_line_range(anchor);
                let anchor_cursor = Cursor { char_idx: anchor, sticky_col: 0 };
                let (_, anchor_col) = self.open().buffer.line_col(&anchor_cursor);
                let (_, cursor_col) = { let ob = self.open(); ob.buffer.line_col(&ob.cursor) };
                let (col_lo, col_hi) = if anchor_col <= cursor_col {
                    (anchor_col, cursor_col + 1)
                } else {
                    (cursor_col, anchor_col + 1)
                };
                for line in self.render_base_line().max(line_lo)..last_visible.min(line_hi + 1) {
                    let len = self.open().buffer.line_len(line);
                    let seg_start = col_lo.min(len);
                    let seg_end = col_hi.min(len);
                    if seg_start < seg_end {
                        segments.push((line - self.render_base_line(), seg_start, seg_end));
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
        let (cursor_line, _) = { let ob = self.open(); ob.buffer.line_col(&ob.cursor) };
        if anchor_line <= cursor_line { (anchor_line, cursor_line) } else { (cursor_line, anchor_line) }
    }

    /// Per-visible-line (view_row, col_start, col_end) segments a plain
    /// char range covers -- shared by Char-kind Visual selection and the
    /// yank/paste pulse, which are both "highlight this contiguous span"
    /// even though they come from different sources.
    fn range_to_segments(&self, range: std::ops::Range<usize>, visible_lines: usize) -> Segments {
        let last_visible = (self.render_base_line() + visible_lines).min(self.open().buffer.line_count());
        let mut segments = Vec::new();
        for line in self.render_base_line()..last_visible {
            let line_start = self.open().buffer.line_start_char(line);
            let line_end = line_start + self.open().buffer.line_len(line);
            let seg_start = range.start.max(line_start);
            let seg_end = range.end.min(line_end);
            if seg_start < seg_end {
                segments.push((line - self.render_base_line(), seg_start - line_start, seg_end - line_start));
            }
        }
        segments
    }

    /// Key/label pairs for whichever pending sequence is currently active
    /// (the leader menu takes priority, since it's the outermost one --
    /// Vim can't be mid-sequence while a leader sequence is in progress).
    /// Empty when nothing is pending.
    fn pending_hints(&self) -> Vec<(KeyPress, &'static str)> {
        if self.leader_matcher.is_pending() {
            self.leader_matcher.pending_children()
        } else {
            self.vim.pending_children()
        }
    }

    /// Segments and current alpha for an active yank/paste pulse, or
    /// `None` when there isn't one. Fades quickly at first then lingers
    /// faintly, via the same ease-out curve as the caret (inverted: pulses
    /// start bright and fade, blink fades in toward its target instead).
    fn pulse_overlay(&self, visible_lines: usize) -> Option<(Segments, f32)> {
        let pulse = self.pulse.as_ref()?;
        let t = Instant::now().duration_since(pulse.started).as_secs_f32() / PULSE_DURATION.as_secs_f32();
        let alpha = PULSE_PEAK_ALPHA * (1.0 - ease_out_cubic(t));
        if alpha <= 0.0 {
            return None;
        }
        Some((self.range_to_segments(pulse.range.clone(), visible_lines), alpha))
    }

    /// The which-key popup's rich-text spans (key column in the theme's
    /// accent color, label in the modeline's own text color, sorted
    /// alphabetically by label for scannability) and its resolved
    /// on-screen rect, or `None` when nothing is pending. Truncates to
    /// whatever `popup::max_rows` says actually fits above the modeline,
    /// with a trailing "+N more" summary row instead of letting the
    /// panel run under it -- previously unbounded, this is what could
    /// make the popup draw over/past the modeline once enough leader
    /// groups existed to make its content taller than the window.
    fn which_key_popup(&self, window_width: f32, modeline_top: f32) -> Option<(fenix_window::Rect, RowSpans)> {
        let mut hints = self.pending_hints();
        if hints.is_empty() {
            return None;
        }
        hints.sort_by(|a, b| a.1.cmp(b.1));

        let max_rows = popup::max_rows(modeline_top, text::WHICH_KEY_MARGIN, text::LINE_HEIGHT, WHICH_KEY_PADDING);
        let shown_count = if hints.len() > max_rows { max_rows.saturating_sub(1).max(1) } else { hints.len() };
        let truncated = hints.len() - shown_count;

        let theme = self.theme;
        let mut spans = Vec::new();
        for (i, (key, label)) in hints[..shown_count].iter().enumerate() {
            if i > 0 {
                spans.push(("\n".to_string(), theme.fg_modeline, false));
            }
            spans.push((format!("{:<6}", keymap::describe_keypress(key)), theme.syntax_keyword, false));
            spans.push(((*label).to_string(), theme.fg_modeline, false));
        }
        if truncated > 0 {
            spans.push(("\n".to_string(), theme.fg_modeline, false));
            spans.push((format!("+{truncated} more"), theme.gutter_fg, false));
        }

        let row_count = shown_count + usize::from(truncated > 0);
        let height = row_count as f32 * text::LINE_HEIGHT + WHICH_KEY_PADDING;
        let rect =
            popup::resolve(popup::Anchor::TopRight { margin: text::WHICH_KEY_MARGIN }, text::WHICH_KEY_WIDTH, height, window_width, modeline_top);
        Some((rect, spans))
    }

    /// Builds the visible rows of a directory listing as rich-text spans
    /// (indent, icon glyph, name, git-status marker, and -- for the
    /// full-width explorer only, not the narrow sidebar -- size/age
    /// attributes), the same shape `content_spans` builds for editor
    /// text so both flow through the same `set_*_rich` machinery.
    /// Returns the spans plus which rendered row (if any) is the
    /// selected entry and which rows are marked, for the caller to draw
    /// highlight rects for.
    fn explorer_row_spans(&self, explorer: &ExplorerState, scroll: usize, rows: usize, show_attrs: bool) -> ExplorerRowsResult {
        let theme = self.theme;
        let mut spans = Vec::new();
        let mut selected_row = None;
        let mut marked_rows = Vec::new();

        let end = (scroll + rows).min(explorer.entries.len());
        let visible_count = end.saturating_sub(scroll);
        for (i, idx) in (scroll..end).enumerate() {
            let entry = &explorer.entries[idx];
            if idx == explorer.selected {
                selected_row = Some(i);
            }
            if explorer.marks.contains(&entry.path) {
                marked_rows.push(i);
            }

            if entry.depth > 0 {
                spans.push(("  ".repeat(entry.depth), theme.fg, false));
            }

            let expanded =
                entry.is_dir && explorer.entries.get(idx + 1).is_some_and(|next| next.depth > entry.depth);
            let icon_color = if entry.is_dir { theme.icon_folder } else { theme.icon_file };
            spans.push((icon::icon_for(&entry.name, entry.is_dir, expanded).to_string(), icon_color, true));
            spans.push((format!(" {}", entry.name), theme.fg, false));

            if let Some(status) = entry.git_status {
                spans.push((format!("  {}", git_status_marker(status)), theme.git_status_color(status), false));
            }
            if show_attrs && !entry.is_dir {
                spans.push((format!("  {}  {}", format_size(entry.size), format_age(entry.modified)), theme.gutter_fg, false));
            }

            if i + 1 < visible_count {
                spans.push(("\n".to_string(), theme.fg, false));
            }
        }
        (spans, selected_row, marked_rows)
    }

    /// Builds the picker's visible rows as rich-text spans: row 0 is the
    /// query prompt (`"> {query}"`), the rows below are the filtered/
    /// ranked candidates windowed by `self.picker_scroll` -- same shape
    /// `explorer_row_spans` builds for a directory listing, so both flow
    /// through the same `set_content_rich` pipeline. Returns the spans
    /// plus which rendered row (if any) is the current selection, for
    /// the caller to draw a highlight rect over.
    fn picker_row_spans(&self, picker: &ActivePicker, rows: usize) -> (RowSpans, Option<usize>) {
        let theme = self.theme;
        let mut spans: RowSpans = vec![(format!("> {}", picker_query(picker)), theme.fg, false)];

        let candidate_rows = rows.saturating_sub(1);
        let visible = picker_visible_labels(picker, self.picker_scroll, candidate_rows);
        if !visible.is_empty() {
            spans.push(("\n".to_string(), theme.fg, false));
        }
        let count = visible.len();
        let mut hl_row = None;
        for (i, (is_selected, label)) in visible.into_iter().enumerate() {
            if is_selected {
                hl_row = Some(i + 1);
            }
            spans.push((label, theme.fg, false));
            if i + 1 < count {
                spans.push(("\n".to_string(), theme.fg, false));
            }
        }
        (spans, hl_row)
    }

    fn redraw(&mut self) {
        let Some((window_width, window_height)) = self.gpu.as_ref().map(|gpu| (gpu.size.width as f32, gpu.size.height as f32))
        else {
            return;
        };
        let visible_lines = text::visible_line_count(window_height);

        // Resolved once, up front: the active theme's font (so
        // `char_width` below reflects it) and the real measured advance
        // width for that font, used for every per-column pixel
        // computation below instead of the fixed-ratio `text::
        // CHAR_WIDTH` constant, which broke the moment a second font
        // (the bundled TempleOS bitmap font, a ~1.0x-em advance vs. the
        // constant's assumed ~0.6x) entered the mix.
        let theme = self.theme;
        let char_width = match &mut self.text {
            Some(text) => {
                text.set_theme(theme);
                text.char_width()
            }
            None => text::CHAR_WIDTH,
        };
        let caret_is_block = self.caret_is_block();

        // Sidebar is independent of window splits *and* `main_view` --
        // kept alive (and its own scroll adjusted) even while a full-
        // buffer explorer is showing in the focused pane, but only
        // actually rendered in Editor mode (see `show_sidebar` below);
        // a file listing on top of an explorer/picker overlay would just
        // be confusing. It's a single frame-level strip, not per-pane --
        // matches Doom's own treemacs sidebar, which is frame-local too.
        if let Some(sidebar) = &self.sidebar {
            self.sidebar_scroll = scroll_to_include(self.sidebar_scroll, sidebar.selected, visible_lines);
        }
        let show_sidebar = self.sidebar_open && self.main_view == MainView::Editor;
        let sidebar_px = if show_sidebar { text::SIDEBAR_WIDTH } else { 0.0 };
        let sidebar_render = if show_sidebar {
            self.sidebar.as_ref().map(|s| self.explorer_row_spans(s, self.sidebar_scroll, visible_lines, false))
        } else {
            None
        };

        let modeline_top = window_height - text::MODELINE_HEIGHT;
        let pane_area =
            fenix_window::Rect { x: sidebar_px, y: 0.0, w: (window_width - sidebar_px).max(0.0), h: modeline_top.max(0.0) };
        let layout = self.windows().layout(pane_area);
        let focused_pane = self.windows().focused_id();

        // One entry per visible window pane -- built fresh every frame
        // from whatever `WindowTree::layout` currently reports (splits/
        // closes/resizes take effect immediately, no separate relayout
        // step needed). Only the *focused* pane can show the explorer/
        // picker overlay (`main_view`/`active_picker`/`explorer` are a
        // single global "what's the current editing context" concept,
        // same as before splits existed) or Vim-mode decoration
        // (selection highlight, pulse, blinking caret) -- every other
        // pane always renders its own buffer's plain content, still with
        // its own gutter/syntax highlighting/current-line highlight, so
        // splitting to see two files is genuinely useful, just without
        // the "actively being edited" polish on the one you're not in.
        struct PaneRender {
            pane: fenix_window::WindowId,
            rect: fenix_window::Rect,
            spans: RowSpans,
            hl_row: Option<usize>,
            marked_rows: Vec<usize>,
            selection_segments: Segments,
            pulse_overlay: Option<(Segments, f32)>,
            caret: Option<(usize, usize)>,
            content_frac: f32,
            gutter_px: f32,
        }

        let mut panes_render: Vec<PaneRender> = Vec::with_capacity(layout.len());
        for (pane, rect) in &layout {
            let (pane, rect) = (*pane, *rect);
            let is_focused = pane == focused_pane;
            let pane_visible_lines = text::lines_that_fit(rect.h);

            if is_focused && self.main_view == MainView::Explorer {
                let rows = pane_visible_lines + 1;
                if let Some(explorer) = &self.explorer {
                    self.explorer_scroll = scroll_to_include(self.explorer_scroll, explorer.selected, pane_visible_lines);
                }
                let (spans, selected_row, marks) = match &self.explorer {
                    Some(explorer) => self.explorer_row_spans(explorer, self.explorer_scroll, rows, true),
                    None => (Vec::new(), None, Vec::new()),
                };
                panes_render.push(PaneRender {
                    pane,
                    rect,
                    spans,
                    hl_row: selected_row,
                    marked_rows: marks,
                    selection_segments: Segments::new(),
                    pulse_overlay: None,
                    caret: None,
                    content_frac: 0.0,
                    gutter_px: 0.0,
                });
                continue;
            }
            if is_focused && self.main_view == MainView::Picker {
                let rows = pane_visible_lines + 1;
                if let Some(picker) = &self.active_picker {
                    self.picker_scroll = scroll_to_include(self.picker_scroll, picker_selected_row(picker), pane_visible_lines);
                }
                let (spans, selected_row) = match &self.active_picker {
                    Some(picker) => self.picker_row_spans(picker, rows),
                    None => (Vec::new(), None),
                };
                panes_render.push(PaneRender {
                    pane,
                    rect,
                    spans,
                    hl_row: selected_row,
                    marked_rows: Vec::new(),
                    selection_segments: Segments::new(),
                    pulse_overlay: None,
                    caret: None,
                    content_frac: 0.0,
                    gutter_px: 0.0,
                });
                continue;
            }

            // Plain buffer content -- every pane not currently showing an
            // overlay, focused or not.
            let buffer_id = *self.windows().content(pane).expect("every pane has a buffer");
            if is_focused {
                self.ensure_cursor_visible(pane_visible_lines);
            }
            let rendered_scroll = self.buffers.get(buffer_id).map(|ob| ob.rendered_scroll).unwrap_or(0.0);
            let render_base_line = rendered_scroll.floor().max(0.0) as usize;
            let render_frac = rendered_scroll - rendered_scroll.floor();
            let gutter_chars = self.buffers.get(buffer_id).map(|ob| self.gutter_chars(ob)).unwrap_or(0);
            let gutter_px = gutter_chars as f32 * char_width;
            let syntax_highlights = self.syntax_highlights_for_visible_range(buffer_id, render_base_line, pane_visible_lines + 1);
            let content_spans = match self.buffers.get(buffer_id) {
                Some(ob) => self.content_spans(ob, render_base_line, pane_visible_lines + 1, gutter_chars, &syntax_highlights),
                None => Vec::new(),
            };
            let spans: RowSpans = content_spans.into_iter().map(|(s, c)| (s, c, false)).collect();

            let (line, col) = self
                .buffers
                .get(buffer_id)
                .map(|ob| ob.buffer.line_col(&ob.cursor))
                .unwrap_or((0, 0));
            // During a large animated pan the cursor's actual line can
            // legitimately be outside the currently-fetched window for
            // part of the transition (it hasn't panned into view yet) --
            // `None` means "don't draw the hl-line/caret this frame," not
            // a bug.
            let hl_row = line.checked_sub(render_base_line).filter(|&row| row <= pane_visible_lines);

            let (selection_segments, pulse_overlay, caret) = if is_focused {
                (
                    self.visual_selection_segments(pane_visible_lines + 1),
                    self.pulse_overlay(pane_visible_lines + 1),
                    hl_row.map(|row| (row, col)),
                )
            } else {
                (Segments::new(), None, None)
            };

            panes_render.push(PaneRender {
                pane,
                rect,
                spans,
                hl_row,
                marked_rows: Vec::new(),
                selection_segments,
                pulse_overlay,
                caret,
                content_frac: render_frac,
                gutter_px,
            });
        }

        let modeline_pieces = self.modeline_pieces();
        let modeline_command_text = if let Some(query) = &self.pending_grep_query {
            Some(format!("rg: {query}"))
        } else if modeline_pieces.is_none() {
            Some(format!(":{}", self.vim.command_line()))
        } else {
            None
        };
        let (badge_bg, badge_fg) = self.mode_colors();
        // Top-right corner, clear of both the content the user is actively
        // editing (top-left, where the cursor usually is) and the modeline
        // (bottom) -- least likely to sit under whatever they're looking
        // at. Resolved (and, if needed, row-truncated) here rather than
        // left to `text.rs`/`prepare` so its position is known before the
        // `bg_rect` panel-background push below.
        let which_key_popup = self.which_key_popup(window_width, modeline_top);
        let caret_alpha = self.caret_alpha();

        let (Some(window), Some(gpu), Some(text), Some(bg_rect), Some(caret_rect)) =
            (&self.window, &mut self.gpu, &mut self.text, &mut self.bg_rect, &mut self.caret_rect)
        else {
            return;
        };

        let live_panes: Vec<fenix_window::WindowId> = panes_render.iter().map(|p| p.pane).collect();
        for pane in &panes_render {
            let content_refs: Vec<(&str, glyphon::Color, bool)> =
                pane.spans.iter().map(|(s, c, i)| (s.as_str(), *c, *i)).collect();
            text.set_pane_rich(pane.pane, pane.rect.w, pane.rect.h, &content_refs);
        }
        text.retain_panes(&live_panes);
        if let Some((sidebar_spans, _, _)) = &sidebar_render {
            let sidebar_refs: Vec<(&str, glyphon::Color, bool)> =
                sidebar_spans.iter().map(|(s, c, i)| (s.as_str(), *c, *i)).collect();
            text.set_sidebar_rich(&sidebar_refs);
        }
        match &modeline_pieces {
            Some((mode_label, suffix)) => {
                let badge = format!(" {:^width$}", mode_label, width = text::MODE_BADGE_CHARS);
                text.set_modeline_text(&[(badge.as_str(), badge_fg), (suffix.as_str(), theme.fg_modeline)]);
            }
            None => {
                let cmd = modeline_command_text.as_deref().unwrap_or("");
                text.set_modeline_text(&[(cmd, theme.fg_modeline)]);
            }
        }

        let popup_rects: Vec<(popup::PopupId, fenix_window::Rect)> = if let Some((rect, spans)) = &which_key_popup {
            let refs: Vec<(&str, glyphon::Color, bool)> = spans.iter().map(|(s, c, i)| (s.as_str(), *c, *i)).collect();
            text.set_popup_rich(popup::PopupId::WhichKey, text::WHICH_KEY_WIDTH, &refs);
            vec![(popup::PopupId::WhichKey, *rect)]
        } else {
            Vec::new()
        };
        text.retain_popups(&popup_rects.iter().map(|(id, _)| *id).collect::<Vec<_>>());

        let sidebar_row_y = |row: usize| text::PAD_TOP + row as f32 * text::LINE_HEIGHT;

        bg_rect.clear();
        for pane in &panes_render {
            // Row index (relative to that pane's own render_base_line, or
            // its explorer/picker listing's own scroll) -> pixel y within
            // the pane, shifted up by its own mid-scroll fractional
            // offset so it pans in step with its text (always 0 outside
            // the focused pane's Editor-mode smooth scroll).
            let row_y = |row: usize| pane.rect.y + text::PAD_TOP + row as f32 * text::LINE_HEIGHT - pane.content_frac * text::LINE_HEIGHT;
            if let Some(row) = pane.hl_row {
                let y = row_y(row);
                bg_rect.push_rect(gpu, pane.rect.x, y, pane.rect.w, text::LINE_HEIGHT, theme.hl_line);
            }
            for row in &pane.marked_rows {
                let y = row_y(*row);
                bg_rect.push_rect(gpu, pane.rect.x, y, pane.rect.w, text::LINE_HEIGHT, theme.selection);
            }
            // Column-math x-offset for this pane's caret/selection/pulse
            // rects: its own left edge, plus `PAD_LEFT`, plus its own
            // line-number gutter -- has to match exactly where `text.
            // prepare`'s `TextArea` for this pane actually renders the
            // text, or these rects drift out of alignment with it.
            let content_x = pane.rect.x + text::PAD_LEFT + pane.gutter_px;
            for &(row, col_start, col_end) in &pane.selection_segments {
                let x = content_x + col_start as f32 * char_width;
                let y = row_y(row);
                let w = (col_end - col_start) as f32 * char_width;
                bg_rect.push_rect(gpu, x, y, w, text::LINE_HEIGHT, theme.selection);
            }
            if let Some((segments, alpha)) = &pane.pulse_overlay {
                let [r, g, b, _] = theme.caret;
                for &(row, col_start, col_end) in segments {
                    let x = content_x + col_start as f32 * char_width;
                    let y = row_y(row);
                    let w = (col_end - col_start) as f32 * char_width;
                    bg_rect.push_rect(gpu, x, y, w, text::LINE_HEIGHT, [r, g, b, *alpha]);
                }
            }
        }
        bg_rect.push_rect(gpu, 0.0, modeline_top, window_width, text::MODELINE_HEIGHT, theme.bg_modeline);
        if modeline_pieces.is_some() {
            // Starts at PAD_LEFT, matching where the badge text itself
            // starts rendering (`text.rs`'s modeline TextArea uses the same
            // left inset) -- starting this at the window edge instead left
            // the rendered label overflowing past the badge's right edge,
            // throwing off how centered it looked inside the colored badge.
            let badge_width = (1.0 + text::MODE_BADGE_CHARS as f32) * char_width;
            bg_rect.push_rect(gpu, text::PAD_LEFT, modeline_top, badge_width, text::MODELINE_HEIGHT, badge_bg);
        }
        for &(_, rect) in &popup_rects {
            bg_rect.push_rect(gpu, rect.x, rect.y, rect.w, rect.h, theme.bg_modeline);
        }
        if show_sidebar {
            bg_rect.push_rect(gpu, 0.0, 0.0, text::SIDEBAR_WIDTH, modeline_top, theme.bg_modeline);
            if let Some((_, Some(selected_row), _)) = &sidebar_render {
                let y = sidebar_row_y(*selected_row);
                bg_rect.push_rect(gpu, 0.0, y, text::SIDEBAR_WIDTH, text::LINE_HEIGHT, theme.hl_line);
            }
        }
        // Divider lines along every split boundary the layout computed --
        // drawn from each pane's own right/bottom edge, so two adjacent
        // panes each contribute one line and they land on top of each
        // other exactly on the boundary. Skipped for the last column/row
        // (nothing to divide from past the window edge).
        for pane in &panes_render {
            if pane.rect.x + pane.rect.w < pane_area.x + pane_area.w - 0.5 {
                bg_rect.push_rect(gpu, pane.rect.x + pane.rect.w - 1.0, pane.rect.y, 2.0, pane.rect.h, theme.divider);
            }
            if pane.rect.y + pane.rect.h < pane_area.y + pane_area.h - 0.5 {
                bg_rect.push_rect(gpu, pane.rect.x, pane.rect.y + pane.rect.h - 1.0, pane.rect.w, 2.0, theme.divider);
            }
        }
        // Layered last -- on top of everything else pushed to `bg_rect`
        // this frame (modeline bar, hl-line, selection, which-key/sidebar
        // backgrounds) -- so a themed border reads as the outermost frame
        // around the whole window rather than something those cover.
        // `BORDER_WIDTH` sits comfortably inside `PAD_TOP`/`PAD_LEFT`, so
        // it can't clip into body text.
        if let Some(border) = theme.border {
            bg_rect.push_rect(gpu, 0.0, 0.0, window_width, BORDER_WIDTH, border);
            bg_rect.push_rect(gpu, 0.0, window_height - BORDER_WIDTH, window_width, BORDER_WIDTH, border);
            bg_rect.push_rect(gpu, 0.0, 0.0, BORDER_WIDTH, window_height, border);
            bg_rect.push_rect(gpu, window_width - BORDER_WIDTH, 0.0, BORDER_WIDTH, window_height, border);
        }
        bg_rect.flush(gpu);

        caret_rect.clear();
        if let Some(focused) = panes_render.iter().find(|p| p.pane == focused_pane) {
            if let Some((row, col)) = focused.caret {
                if caret_alpha > 0.0 {
                    let content_x = focused.rect.x + text::PAD_LEFT + focused.gutter_px;
                    let caret_x = content_x + col as f32 * char_width;
                    let caret_y = focused.rect.y + text::PAD_TOP + row as f32 * text::LINE_HEIGHT
                        - focused.content_frac * text::LINE_HEIGHT;
                    let [r, g, b, a] = theme.caret;
                    // Insert keeps the thin bar (an I-beam-style "about to
                    // type here" marker); every other mode (Normal, Visual,
                    // Replace, Command) gets a full-cell block, matching real
                    // Vim's own cursor-shape convention. The block is drawn
                    // at reduced opacity -- caret_rect is composited *after*
                    // text (so the caret always shows on top, no glyph-recolor
                    // trick needed), and a fully opaque block would otherwise
                    // completely hide the character underneath it instead of
                    // just marking its position.
                    let (width, block_alpha) = if caret_is_block { (char_width, 0.6) } else { (2.0, 1.0) };
                    caret_rect.push_rect(gpu, caret_x, caret_y, width, text::LINE_HEIGHT, [r, g, b, a * caret_alpha * block_alpha]);
                }
            }
        }
        caret_rect.flush(gpu);

        let prepare_panes: Vec<(fenix_window::WindowId, fenix_window::Rect, f32)> =
            panes_render.iter().map(|p| (p.pane, p.rect, p.content_frac)).collect();
        text.prepare(gpu, theme, &prepare_panes, &popup_rects, show_sidebar);

        let frame = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                gpu.surface.configure(&gpu.device, &gpu.config);
                return;
            }
            status @ (wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation) => {
                eprintln!("fenix: surface acquire failed: {status:?}");
                return;
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            let [r, g, b, a] = theme.bg;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: r as f64,
                            g: g as f64,
                            b: b as f64,
                            a: a as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            bg_rect.render(&mut pass);
            text.render(&mut pass);
            caret_rect.render(&mut pass);
        }
        gpu.queue.submit(Some(encoder.finish()));
        window.pre_present_notify();
        gpu.queue.present(frame);
        text.trim();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attrs = Window::default_attributes().with_title("Fenix");
        let window =
            Arc::new(event_loop.create_window(attrs).expect("failed to create window"));

        let gpu = pollster::block_on(GpuState::new(window.clone()));
        // No priming call needed for pane content -- `redraw()` populates
        // every visible pane's `GlyphBuffer` fresh (creating it lazily)
        // on the first real frame, which winit already requests
        // immediately after this.
        let text = TextPipeline::new(&gpu);
        let bg_rect = RectRenderer::new(&gpu);
        let caret_rect = RectRenderer::new(&gpu);

        self.window = Some(window);
        self.gpu = Some(gpu);
        self.text = Some(text);
        self.bg_rect = Some(bg_rect);
        self.caret_rect = Some(caret_rect);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size);
                }
                if let Some(text) = &mut self.text {
                    text.resize(size.width as f32, size.height as f32);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.handle_key(&event, event_loop);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let mut needs_redraw = false;

        if now >= self.next_blink {
            self.blink_visible = !self.blink_visible;
            self.blink_transition_start = now;
            self.next_blink = now + BLINK_INTERVAL;
            needs_redraw = true;
        }

        // Keep redrawing at animation cadence only while something is
        // actually transitioning (blink fade or an active pulse); once
        // everything settles, fall back to the long, efficient wait for
        // the next blink toggle -- an idle editor still does no per-frame
        // work between blinks.
        let blink_transitioning = now.duration_since(self.blink_transition_start) < BLINK_FADE;
        let pulse_active = match &self.pulse {
            Some(p) if now.duration_since(p.started) < PULSE_DURATION => true,
            Some(_) => {
                self.pulse = None; // expired -- drop it so redraw stops drawing it
                needs_redraw = true;
                false
            }
            None => false,
        };
        let animating = blink_transitioning || pulse_active || self.scroll_anims.contains_key(&self.focused_buffer_id());
        if animating {
            needs_redraw = true;
        }

        if needs_redraw {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }

        let wait_until = if animating { now + ANIM_TICK } else { self.next_blink };
        event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(wait_until));
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
        let ob = self.open_mut();
        ob.buffer.insert_char(&mut ob.cursor, ch);
    }

    fn test_insert_str(&mut self, s: &str) {
        for ch in s.chars() {
            self.test_insert(ch);
        }
    }

    fn test_vim_key(&mut self, key: KeyPress) -> VimEvent {
        let id = self.focused_buffer_id();
        let ob = self.buffers.get_mut(id).expect("focused window always has an open buffer");
        self.vim.handle_key(&mut ob.buffer, &mut ob.cursor, key)
    }

    /// Points the focused pane at `path`, opening it fresh in the
    /// registry -- test-only equivalent of what `open_file_from_picker`/
    /// `explorer_open_selected` do in production.
    fn test_open_path(&mut self, path: &Path) {
        let id = self.buffers.open_path(path);
        let focused = self.windows().focused_id();
        self.windows_mut().set_content(focused, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_stays_put_when_cursor_already_visible() {
        assert_eq!(scroll_to_include(5, 7, 10), 5);
    }

    #[test]
    fn scroll_jumps_up_when_cursor_is_above_the_window() {
        assert_eq!(scroll_to_include(10, 3, 10), 3);
    }

    #[test]
    fn scroll_advances_when_cursor_is_below_the_window() {
        // window [10, 20); cursor at line 20 is the first line below it
        assert_eq!(scroll_to_include(10, 20, 10), 11);
    }

    #[test]
    fn single_visible_line_tracks_cursor_exactly() {
        assert_eq!(scroll_to_include(0, 42, 1), 42);
    }

    #[test]
    fn app_auto_scrolls_to_keep_cursor_in_view() {
        let mut app = App::with_file(None);
        for _ in 0..30 {
            app.test_insert('\n');
        }
        // cursor is now on line 30; a 10-line viewport starting at 0 doesn't include it
        app.ensure_cursor_visible(10);
        assert_eq!(app.open().scroll_line, 21);
    }

    #[test]
    fn small_scroll_change_starts_an_animation_not_an_instant_jump() {
        let mut app = App::with_file(None);
        for _ in 0..5 {
            app.test_insert('\n');
        }
        app.ensure_cursor_visible(3); // 6 lines, 3-line viewport -> scrolls a bit
        assert!(app.scroll_anims.contains_key(&app.focused_buffer_id()));
        let ob = app.open();
        assert_ne!(ob.rendered_scroll, ob.scroll_line as f32); // still mid-ease, not snapped
    }

    #[test]
    fn huge_scroll_jump_snaps_instantly_without_animating() {
        let mut app = App::with_file(None);
        for _ in 0..500 {
            app.test_insert('\n');
        }
        app.ensure_cursor_visible(10); // jump of ~490 lines, way past the snap threshold
        assert!(!app.scroll_anims.contains_key(&app.focused_buffer_id()));
        let ob = app.open();
        assert_eq!(ob.rendered_scroll, ob.scroll_line as f32);
    }

    #[test]
    fn rendered_scroll_eases_toward_target_and_settles() {
        let mut app = App::with_file(None);
        let id = app.focused_buffer_id();
        {
            let ob = app.open_mut();
            ob.scroll_line = 10;
            ob.rendered_scroll = 0.0;
        }
        app.scroll_anims.insert(id, ScrollAnim { from: 0.0, to: 10, started: Instant::now() - SCROLL_DURATION / 2 });
        app.update_rendered_scroll();
        let r = app.open().rendered_scroll;
        assert!(r > 0.0 && r < 10.0, "should be partway there");
        assert!(app.scroll_anims.contains_key(&id));

        app.scroll_anims.insert(id, ScrollAnim { from: 0.0, to: 10, started: Instant::now() - SCROLL_DURATION * 2 });
        app.update_rendered_scroll();
        assert_eq!(app.open().rendered_scroll, 10.0);
        assert!(!app.scroll_anims.contains_key(&id)); // settled, animation cleared
    }

    #[test]
    fn render_base_line_splits_a_fractional_scroll_position() {
        let mut app = App::with_file(None);
        app.open_mut().rendered_scroll = 4.25;
        assert_eq!(app.render_base_line(), 4);
        assert!((app.open().rendered_scroll.fract() - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn modeline_reflects_filename_dirty_state_mode_and_position() {
        let mut app = App::with_file(None);
        assert_eq!(app.modeline_text(), "  NORMAL │ [No Name]   Ln 1, Col 1 ");

        app.test_insert('a');
        app.test_insert('b');
        assert_eq!(app.modeline_text(), "  NORMAL │ [No Name] [+]   Ln 1, Col 3 ");
    }

    #[test]
    fn modeline_shows_command_line_while_typing_an_ex_command() {
        let mut app = App::with_file(None);
        for ch in [':', 'w', 'q'] {
            app.test_vim_key(KeyPress::char(ch));
        }
        assert_eq!(app.modeline_text(), ":wq");
    }

    #[test]
    fn visual_selection_segments_cover_the_selected_range() {
        let mut app = App::with_file(None);
        for ch in "hello world".chars() {
            app.test_insert(ch);
        }
        app.open_mut().cursor = Cursor::at_start();
        app.test_vim_key(KeyPress::char('v'));
        for _ in 0..4 {
            app.test_vim_key(KeyPress::char('l'));
        }
        // anchor 0, cursor now at 4 ("hello"[0..5))
        assert_eq!(app.visual_selection_segments(10), vec![(0, 0, 5)]);
    }

    #[test]
    fn visual_selection_segments_empty_outside_visual_mode() {
        let app = App::with_file(None);
        assert!(app.visual_selection_segments(10).is_empty());
    }

    #[test]
    fn modeline_shows_visual_kind_not_just_visual() {
        let mut app = App::with_file(None);
        for ch in "one\ntwo\nthree".chars() {
            app.test_insert(ch);
        }
        app.open_mut().cursor = Cursor::at_start();

        app.test_vim_key(KeyPress::char('v'));
        assert!(app.modeline_text().contains("VISUAL"));

        app.test_vim_key(KeyPress::char('V'));
        assert!(app.modeline_text().contains("V-LINE"));

        app.test_vim_key(KeyPress::char('v').with_ctrl());
        assert!(app.modeline_text().contains("V-BLOCK"));
    }

    #[test]
    fn mode_colors_differ_per_mode_and_visual_kinds_share_one_accent() {
        let mut app = App::with_file(None);
        let (normal_bg, _) = app.mode_colors();

        app.test_vim_key(KeyPress::char('i'));
        let (insert_bg, _) = app.mode_colors();
        assert_ne!(normal_bg, insert_bg);
        app.test_vim_key(KeyPress::named(FenixNamedKey::Escape));

        app.test_vim_key(KeyPress::char('v'));
        let (char_visual_bg, _) = app.mode_colors();
        app.test_vim_key(KeyPress::char('V'));
        let (line_visual_bg, _) = app.mode_colors();
        assert_eq!(char_visual_bg, line_visual_bg); // one accent for all Visual kinds
        assert_ne!(char_visual_bg, normal_bg);
    }

    #[test]
    fn caret_is_block_everywhere_except_insert_mode() {
        let mut app = App::with_file(None);
        assert!(app.caret_is_block()); // starts in Normal

        app.test_vim_key(KeyPress::char('i'));
        assert!(!app.caret_is_block()); // Insert -- thin bar
        app.test_vim_key(KeyPress::named(FenixNamedKey::Escape));
        assert!(app.caret_is_block()); // back to Normal

        app.test_vim_key(KeyPress::char('v'));
        assert!(app.caret_is_block()); // Visual
    }

    #[test]
    fn visual_line_segments_cover_whole_lines_regardless_of_column() {
        let mut app = App::with_file(None);
        for ch in "one\ntwo\nthree".chars() {
            app.test_insert(ch);
        }
        app.open_mut().cursor = Cursor { char_idx: 5, sticky_col: 1 }; // column 1 of "two"
        app.test_vim_key(KeyPress::char('V'));
        assert_eq!(app.visual_selection_segments(10), vec![(1, 0, 3)]);
    }

    #[test]
    fn visual_block_segments_form_a_column_rectangle() {
        let mut app = App::with_file(None);
        for ch in "abc\ndef\nghi".chars() {
            app.test_insert(ch);
        }
        app.open_mut().cursor = Cursor::at_start();
        app.test_vim_key(KeyPress::char('v').with_ctrl());
        for ch in ['j', 'j', 'l'] {
            app.test_vim_key(KeyPress::char(ch));
        }
        assert_eq!(app.visual_selection_segments(10), vec![(0, 0, 2), (1, 0, 2), (2, 0, 2)]);
    }

    #[test]
    fn ease_out_cubic_starts_at_zero_ends_at_one_and_is_monotonic() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert_eq!(ease_out_cubic(2.0), 1.0); // clamps past the end
        assert_eq!(ease_out_cubic(-1.0), 0.0); // clamps before the start

        let mut prev = 0.0;
        for i in 1..=10 {
            let v = ease_out_cubic(i as f32 / 10.0);
            assert!(v >= prev, "not monotonic at step {i}");
            prev = v;
        }
    }

    #[test]
    fn ease_out_cubic_front_loads_motion_more_than_linear() {
        // "ease-out": faster at the start than a linear ramp would be
        assert!(ease_out_cubic(0.25) > 0.25);
    }

    #[test]
    fn fresh_app_caret_is_immediately_fully_visible_no_fade_in_delay() {
        let app = App::with_file(None);
        assert_eq!(app.caret_alpha(), 1.0);
    }

    #[test]
    fn wake_caret_snaps_to_visible_without_fading_in() {
        let mut app = App::with_file(None);
        app.blink_visible = false;
        app.blink_transition_start = Instant::now(); // mid fade-out
        app.wake_caret();
        assert_eq!(app.caret_alpha(), 1.0);
    }

    #[test]
    fn no_pulse_by_default() {
        let app = App::with_file(None);
        assert!(app.pulse_overlay(10).is_none());
    }

    #[test]
    fn fresh_pulse_is_near_peak_alpha_and_covers_its_range() {
        let mut app = App::with_file(None);
        for ch in "hello world".chars() {
            app.test_insert(ch);
        }
        app.pulse = Some(Pulse { range: 0..5, started: Instant::now() });
        let (segments, alpha) = app.pulse_overlay(10).expect("pulse should be active");
        assert_eq!(segments, vec![(0, 0, 5)]);
        assert!(alpha > 0.0 && alpha <= PULSE_PEAK_ALPHA);
    }

    #[test]
    fn expired_pulse_produces_no_overlay() {
        let mut app = App::with_file(None);
        for ch in "hello".chars() {
            app.test_insert(ch);
        }
        app.pulse = Some(Pulse { range: 0..5, started: Instant::now() - PULSE_DURATION * 2 });
        assert!(app.pulse_overlay(10).is_none());
    }

    // App::handle_key itself needs a real winit KeyEvent/ActiveEventLoop,
    // which aren't constructible in a unit test (same constraint as the
    // rest of the winit-facing integration, verified by code review
    // instead -- see the plan). This exercises the layer below it: that a
    // VimEvent::Pulse from VimState turns into a renderable pulse_overlay,
    // which is the actual logic worth covering here; App's own arm that
    // wires the two together is two lines and reviewed by eye.
    #[test]
    fn vim_pulse_event_yields_a_renderable_pulse_overlay() {
        let mut app = App::with_file(None);
        for ch in "hello world".chars() {
            app.test_insert(ch);
        }
        app.open_mut().cursor = Cursor::at_start();
        assert!(app.pulse.is_none());

        app.test_vim_key(KeyPress::char('y'));
        let event = app.test_vim_key(KeyPress::char('w'));
        let fenix_vim::VimEvent::Pulse(range) = event else { panic!("expected a Pulse event from yw") };
        app.pulse = Some(Pulse { range, started: Instant::now() });
        assert!(app.pulse_overlay(10).is_some());
    }

    #[test]
    fn gutter_chars_zero_when_off() {
        let mut app = App::with_file(None);
        app.line_number_mode = LineNumberMode::Off;
        assert_eq!(app.gutter_chars(app.open()), 0);
    }

    #[test]
    fn gutter_chars_defaults_to_absolute_and_sizes_for_line_count() {
        let app = App::with_file(None); // empty buffer: one visual line
        assert_eq!(app.gutter_chars(app.open()), 2); // 1-digit number + 1 padding column
    }

    #[test]
    fn content_spans_marks_rows_past_buffer_end_with_tilde() {
        let app = App::with_file(None); // single empty line, cursor on it
        let gutter = app.gutter_chars(app.open());
        let spans = app.content_spans(app.open(), 0, 3, gutter, &[]);
        let joined: String = spans.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(joined, "1 \n~ \n~ ");
    }

    #[test]
    fn content_spans_off_mode_still_shows_tilde_for_rows_past_end() {
        let mut app = App::with_file(None);
        app.line_number_mode = LineNumberMode::Off;
        let gutter = app.gutter_chars(app.open());
        let spans = app.content_spans(app.open(), 0, 2, gutter, &[]);
        let joined: String = spans.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(joined, "\n~");
    }

    #[test]
    fn content_spans_relative_mode_shows_distance_from_cursor() {
        let mut app = App::with_file(None);
        app.line_number_mode = LineNumberMode::Relative;
        app.test_insert_str("a\nb\nc\nd");
        app.open_mut().cursor = Cursor { char_idx: 2, sticky_col: 0 }; // line 1, 'b'
        let gutter = app.gutter_chars(app.open());
        let spans = app.content_spans(app.open(), 0, 4, gutter, &[]);
        let joined: String = spans.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(joined, "1 a\n0 b\n1 c\n2 d");
    }

    #[test]
    fn content_spans_current_line_number_uses_fg_not_gutter_fg() {
        let mut app = App::with_file(None);
        app.test_insert_str("a\nb");
        app.open_mut().cursor = Cursor { char_idx: 2, sticky_col: 0 }; // line 1
        let gutter = app.gutter_chars(app.open());
        let spans = app.content_spans(app.open(), 0, 2, gutter, &[]);
        assert_eq!(spans[0].1, app.theme.gutter_fg); // line 0: not current
        assert_eq!(spans[2].1, app.theme.fg); // line 1: current line's gutter
    }

    #[test]
    fn cycle_line_number_mode_goes_off_absolute_relative_off() {
        let mut app = App::with_file(None);
        app.line_number_mode = LineNumberMode::Off;
        app.cycle_line_number_mode();
        assert_eq!(app.line_number_mode, LineNumberMode::Absolute);
        app.cycle_line_number_mode();
        assert_eq!(app.line_number_mode, LineNumberMode::Relative);
        app.cycle_line_number_mode();
        assert_eq!(app.line_number_mode, LineNumberMode::Off);
    }

    #[test]
    fn cycle_theme_wraps_through_all_themes_and_persists() {
        let dir = TempDir::new("cycle_theme");
        let mut app = App::with_file(None);
        app.theme_path = dir.path().join("theme.txt");
        // Fixed starting point -- not asserted from `with_file`'s own
        // load, since that reads the *real* config path and would be
        // flaky against whatever's actually persisted on this machine.
        app.theme = &theme::ORBIT_DARK;

        app.cycle_theme();
        assert_eq!(app.theme.name, "TempleOS");
        assert_eq!(theme::load_from(&app.theme_path).name, "TempleOS"); // persisted

        app.cycle_theme();
        assert_eq!(app.theme.name, "Orbit Dark"); // wrapped back around
        assert_eq!(theme::load_from(&app.theme_path).name, "Orbit Dark");
    }

    #[test]
    fn split_line_by_highlights_covers_a_middle_span() {
        let theme = &theme::ORBIT_DARK;
        let spans = split_line_by_highlights("let x = 1;", 0, 10, &[(4..5, theme.syntax_variable)], theme.fg);
        let joined: String = spans.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(joined, "let x = 1;");
        assert_eq!(spans[1], ("x".to_string(), theme.syntax_variable));
        assert_eq!(spans[0].1, theme.fg);
        assert_eq!(spans[2].1, theme.fg);
    }

    #[test]
    fn split_line_by_highlights_clips_ranges_extending_past_the_line() {
        let theme = &theme::ORBIT_DARK;
        // A highlight spanning into the next line should only color this
        // line's portion of it.
        let spans = split_line_by_highlights("abc", 10, 13, &[(8..15, theme.syntax_string)], theme.fg);
        assert_eq!(spans, vec![("abc".to_string(), theme.syntax_string)]);
    }

    #[test]
    fn split_line_by_highlights_ignores_ranges_outside_the_line() {
        let theme = &theme::ORBIT_DARK;
        let spans = split_line_by_highlights("abc", 10, 13, &[(0..5, theme.syntax_string)], theme.fg);
        assert_eq!(spans, vec![("abc".to_string(), theme.fg)]);
    }

    #[test]
    fn syntax_state_seeds_from_the_detected_language() {
        let mut app = App::with_file(None);
        assert!(app.open().syntax.is_none()); // no path -> no language

        app.open_mut().syntax = Some(fenix_syntax::SyntaxState::new(fenix_syntax::LanguageId::Rust, ""));
        app.test_insert_str("fn main() {}");

        let id = app.focused_buffer_id();
        let highlights = app.syntax_highlights_for_visible_range(id, 0, 1);
        assert!(!highlights.is_empty(), "expected highlights for a Rust buffer, got none");

        let spans = app.content_spans(app.open(), 0, 1, 0, &highlights);
        let fn_span = spans.iter().find(|(s, _)| s == "fn");
        assert_eq!(
            fn_span.map(|(_, c)| *c),
            Some(app.theme.syntax_color("keyword")),
            "expected \"fn\" colored as a keyword, got {spans:?}"
        );
    }

    #[test]
    fn syntax_highlights_drain_edits_even_without_an_active_language() {
        let mut app = App::with_file(None);
        assert!(app.open().syntax.is_none());
        app.test_insert_str("hello");
        let id = app.focused_buffer_id();
        assert!(app.syntax_highlights_for_visible_range(id, 0, 1).is_empty());
        // The edit log should have been drained regardless, not left to grow.
        assert!(app.open_mut().buffer.drain_edits().is_empty());
    }

    /// A real, uniquely-named temp directory, removed on drop -- these
    /// tests exercise the real filesystem via `fenix_explorer::
    /// ExplorerState`, same reasoning as that crate's own `TempDir`.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("fenix-gui-test-{name}-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
        fn touch(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, b"hello\n").unwrap();
            path
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

    #[test]
    fn explorer_jump_switches_to_explorer_view_without_touching_the_buffer() {
        let dir = TempDir::new("jump_stashes");
        let file = dir.touch("a.txt");
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        app.test_insert_str(" extra"); // dirty the buffer so "untouched" is checkable
        let focused_before = app.focused_buffer_id();

        app.explorer_jump();
        assert_eq!(app.main_view, MainView::Explorer);
        assert!(app.explorer.is_some());
        // No stashing needed -- the focused pane's buffer id (and its
        // content) is left exactly as it was; only `main_view` changed.
        assert_eq!(app.focused_buffer_id(), focused_before);
        assert_eq!(app.open().buffer.text(), " extrahello\n");
        assert_eq!(app.explorer.as_ref().unwrap().cwd, dir.path());
    }

    #[test]
    fn full_buffer_jump_does_not_clobber_an_already_open_sidebar() {
        // Regression test: `explorer` and `sidebar` used to be one shared
        // field, which meant jumping to a full-buffer listing while a
        // sidebar was open silently overwrote the sidebar's state.
        let sidebar_dir = TempDir::new("jump_no_clobber_sidebar");
        let jump_dir = TempDir::new("jump_no_clobber_jump");
        let file = jump_dir.touch("a.txt");

        let mut app = App::with_file(None);
        app.sidebar = Some(ExplorerState::open(sidebar_dir.path()).unwrap());
        app.sidebar_open = true;
        app.sidebar_focused = false;

        app.test_open_path(&file);
        app.explorer_jump();

        assert_eq!(app.main_view, MainView::Explorer);
        assert_eq!(app.explorer.as_ref().unwrap().cwd, jump_dir.path());
        // The sidebar's own listing must be untouched by the jump.
        assert!(app.sidebar_open);
        assert_eq!(app.sidebar.as_ref().unwrap().cwd, sidebar_dir.path());
    }

    #[test]
    fn explorer_row_spans_marks_the_icon_span_and_flags_the_selected_row() {
        let dir = TempDir::new("row_spans_icon_selected");
        dir.touch("main.rs");
        let app = App::with_file(None);
        let explorer = ExplorerState::open(dir.path()).unwrap(); // selected = "main.rs" (only entry)

        let (spans, selected_row, marked_rows) = app.explorer_row_spans(&explorer, 0, 5, true);
        assert_eq!(selected_row, Some(0));
        assert!(marked_rows.is_empty());

        // First span is the icon glyph, flagged to render in the icon font.
        assert!(spans[0].2, "expected the icon span to be flagged is_icon, got {spans:?}");
        // Somewhere in the row, the plain filename shows up in the body font.
        assert!(spans.iter().any(|(s, _, is_icon)| !is_icon && s.contains("main.rs")));
    }

    #[test]
    fn explorer_row_spans_includes_marked_rows() {
        let dir = TempDir::new("row_spans_marked");
        dir.touch("a");
        dir.touch("b");
        let app = App::with_file(None);
        let mut explorer = ExplorerState::open(dir.path()).unwrap();
        explorer.marks.insert(dir.path().join("b"));

        let (_, _, marked_rows) = app.explorer_row_spans(&explorer, 0, 5, true);
        assert_eq!(marked_rows, vec![1]); // "b" sorts second, alphabetically after "a"
    }

    #[test]
    fn explorer_row_spans_respects_the_scroll_window() {
        let dir = TempDir::new("row_spans_scroll");
        for name in ["a", "b", "c", "d"] {
            dir.touch(name);
        }
        let app = App::with_file(None);
        let explorer = ExplorerState::open(dir.path()).unwrap();

        // Scrolled to start at index 2 ("c"), showing only 2 rows.
        let (spans, _, _) = app.explorer_row_spans(&explorer, 2, 2, true);
        let joined: String = spans.iter().map(|(s, _, _)| s.as_str()).collect();
        assert!(joined.contains('c'));
        assert!(joined.contains('d'));
        assert!(!joined.contains('a'));
        assert!(!joined.contains('b'));
    }

    #[test]
    fn explorer_row_spans_shows_attrs_only_when_requested() {
        let dir = TempDir::new("row_spans_attrs");
        dir.touch("a.txt");
        let app = App::with_file(None);
        let explorer = ExplorerState::open(dir.path()).unwrap();

        let (with_attrs, _, _) = app.explorer_row_spans(&explorer, 0, 5, true);
        let (without_attrs, _, _) = app.explorer_row_spans(&explorer, 0, 5, false);
        let joined_with: String = with_attrs.iter().map(|(s, _, _)| s.as_str()).collect();
        let joined_without: String = without_attrs.iter().map(|(s, _, _)| s.as_str()).collect();
        assert!(joined_with.contains('B') || joined_with.contains('K')); // size suffix present
        assert!(!joined_without.contains('B') && !joined_without.contains('K'));
    }

    #[test]
    fn quitting_full_buffer_explorer_leaves_the_buffer_exactly_as_it_was() {
        let dir = TempDir::new("jump_quit_restores");
        let file = dir.touch("a.txt");
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        app.test_insert_str(" extra");
        let dirty_text = app.open().buffer.text();

        app.explorer_jump();
        app.explorer_quit();

        assert_eq!(app.main_view, MainView::Editor);
        assert!(app.explorer.is_none());
        assert_eq!(app.open().buffer.text(), dirty_text);
    }

    #[test]
    fn toggle_sidebar_opens_then_closes() {
        let dir = TempDir::new("toggle_sidebar");
        dir.touch("a.txt");
        let mut app = App::with_file(None);
        app.test_open_path(&dir.path().join("a.txt"));

        app.toggle_sidebar();
        assert!(app.sidebar_open);
        assert!(app.sidebar_focused);
        assert!(app.sidebar.is_some());

        app.toggle_sidebar();
        assert!(!app.sidebar_open);
        assert!(!app.sidebar_focused);
        assert!(app.sidebar.is_none());
    }

    #[test]
    fn split_vertical_adds_a_focused_pane_showing_the_same_buffer() {
        let mut app = App::with_file(None);
        let original = app.windows().focused_id();
        let buffer_id = app.focused_buffer_id();

        app.split_vertical();

        assert_eq!(app.windows().window_count(), 2);
        assert_ne!(app.windows().focused_id(), original);
        assert_eq!(app.focused_buffer_id(), buffer_id); // new pane shows the same buffer
    }

    #[test]
    fn split_horizontal_also_adds_a_focused_pane() {
        let mut app = App::with_file(None);
        app.split_horizontal();
        assert_eq!(app.windows().window_count(), 2);
    }

    #[test]
    fn close_window_is_a_no_op_on_the_last_window() {
        let mut app = App::with_file(None);
        app.close_window();
        assert_eq!(app.windows().window_count(), 1);
    }

    #[test]
    fn close_window_collapses_back_to_one_pane() {
        let mut app = App::with_file(None);
        app.split_vertical();
        app.close_window();
        assert_eq!(app.windows().window_count(), 1);
    }

    #[test]
    fn cycle_window_wraps_between_the_two_panes() {
        let mut app = App::with_file(None);
        let first = app.windows().focused_id();
        app.split_vertical();
        let second = app.windows().focused_id();
        assert_ne!(first, second);

        app.cycle_window();
        assert_eq!(app.windows().focused_id(), first);
        app.cycle_window();
        assert_eq!(app.windows().focused_id(), second);
    }

    #[test]
    fn navigate_window_moves_focus_left_to_the_original_pane() {
        let mut app = App::with_file(None);
        let left = app.windows().focused_id();
        app.split_vertical(); // new pane appears to the right, becomes focused
        assert_ne!(app.windows().focused_id(), left);

        app.navigate_window(fenix_window::NavDirection::Left);
        assert_eq!(app.windows().focused_id(), left);
    }

    #[test]
    fn only_window_closes_every_other_pane() {
        let mut app = App::with_file(None);
        app.split_vertical();
        app.split_horizontal();
        assert_eq!(app.windows().window_count(), 3);

        app.only_window();
        assert_eq!(app.windows().window_count(), 1);
    }

    #[test]
    fn balance_windows_resets_a_skewed_ratio_back_to_half() {
        let mut app = App::with_file(None);
        app.split_vertical();
        app.windows_mut().resize_focused(0.3); // skew away from 0.5
        app.balance_windows();

        let rects = app.windows().layout(fenix_window::Rect { x: 0.0, y: 0.0, w: 200.0, h: 100.0 });
        for (_, r) in rects {
            assert!((r.w - 100.0).abs() < 0.01);
        }
    }

    #[test]
    fn new_workspace_becomes_active_and_is_seeded_with_the_current_buffer() {
        let mut app = App::with_file(None);
        let buffer_id = app.focused_buffer_id();

        app.new_workspace();

        assert_eq!(app.workspaces.len(), 2);
        assert_eq!(app.workspaces.active_index(), 1);
        assert_eq!(app.workspaces.active_name(), "workspace-2");
        assert_eq!(app.focused_buffer_id(), buffer_id); // seeded, not a blank scratch
        assert_eq!(app.windows().window_count(), 1); // new workspace starts unsplit
    }

    #[test]
    fn next_and_prev_workspace_cycle_and_wrap() {
        let mut app = App::with_file(None);
        app.new_workspace();
        app.new_workspace(); // 3 workspaces total, active = 2

        app.next_workspace(); // wraps to 0
        assert_eq!(app.workspaces.active_index(), 0);
        app.prev_workspace(); // wraps back to 2
        assert_eq!(app.workspaces.active_index(), 2);
        app.prev_workspace();
        assert_eq!(app.workspaces.active_index(), 1);
    }

    #[test]
    fn remove_workspace_is_a_no_op_on_the_last_one() {
        let mut app = App::with_file(None);
        app.remove_workspace();
        assert_eq!(app.workspaces.len(), 1);
    }

    #[test]
    fn remove_workspace_removes_the_active_one() {
        let mut app = App::with_file(None);
        app.new_workspace(); // active: workspace-2

        app.remove_workspace();

        assert_eq!(app.workspaces.len(), 1);
        assert_eq!(app.workspaces.active_name(), "workspace-1");
    }

    #[test]
    fn each_workspace_keeps_its_own_split_layout() {
        let mut app = App::with_file(None);
        app.split_vertical();
        app.split_horizontal();
        assert_eq!(app.windows().window_count(), 3);

        app.new_workspace(); // fresh, unsplit workspace
        assert_eq!(app.windows().window_count(), 1);

        app.prev_workspace(); // back to workspace-1
        assert_eq!(app.windows().window_count(), 3); // layout untouched
    }

    #[test]
    fn modeline_shows_the_workspace_indicator_only_once_there_are_several() {
        let mut app = App::with_file(None);
        assert!(!app.modeline_text().contains("workspace"));

        app.new_workspace();
        assert!(app.modeline_text().contains("[2/2 workspace-2]"));
    }

    #[test]
    fn which_key_popup_is_none_when_nothing_is_pending() {
        let app = App::with_file(None);
        assert!(app.which_key_popup(800.0, 580.0).is_none());
    }

    #[test]
    fn which_key_popup_lists_pending_leader_children_sorted_by_label() {
        let mut app = App::with_file(None);
        app.leader_matcher.feed(KeyPress::char(' '));
        app.leader_matcher.feed(KeyPress::char('t')); // SPC t: "line numbers", "theme"

        let (_, spans) = app.which_key_popup(800.0, 580.0).unwrap();
        let joined: String = spans.iter().map(|(s, _, _)| s.as_str()).collect();
        // "line numbers" sorts before "theme" -- alphabetical by label.
        assert!(joined.find("line numbers").unwrap() < joined.find("theme").unwrap());
        assert!(joined.contains('n')); // the `n` key column, for line numbers
        assert!(!joined.contains("more")); // both entries fit, nothing truncated
    }

    #[test]
    fn which_key_popup_rect_never_extends_past_the_window_or_under_the_modeline() {
        let mut app = App::with_file(None);
        app.leader_matcher.feed(KeyPress::char(' ')); // root: 8 top-level groups

        // A window far too short for 8 rows -- this is the bug this popup
        // system fixes: the panel used to have no notion of "not enough
        // room" and could draw its background/text straight through the
        // modeline bar.
        let modeline_top = 40.0;
        let (rect, _) = app.which_key_popup(300.0, modeline_top).unwrap();

        assert!(rect.x >= 0.0);
        assert!(rect.y >= 0.0);
        assert!(rect.x + rect.w <= 300.0 + 0.01);
        assert!(rect.y <= modeline_top);
    }

    #[test]
    fn which_key_popup_truncates_and_reports_how_many_more_when_content_overflows() {
        let mut app = App::with_file(None);
        app.leader_matcher.feed(KeyPress::char(' ')); // root: 8 top-level groups, more than fit below

        let (_, spans) = app.which_key_popup(300.0, 40.0).unwrap();
        let joined: String = spans.iter().map(|(s, _, _)| s.as_str()).collect();
        assert!(joined.contains("more"));
    }

    #[test]
    fn explorer_open_selected_on_a_directory_navigates_into_it() {
        let dir = TempDir::new("open_dir");
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let mut app = App::with_file(None);
        app.explorer = Some(ExplorerState::open(dir.path()).unwrap());
        app.main_view = MainView::Explorer;

        app.explorer_open_selected(); // selected = "sub" (only entry)
        assert_eq!(app.explorer.as_ref().unwrap().cwd, dir.path().join("sub"));
        assert_eq!(app.main_view, MainView::Explorer); // still in the explorer, just deeper
    }

    #[test]
    fn explorer_open_selected_on_a_file_in_full_buffer_mode_returns_to_editor() {
        let dir = TempDir::new("open_file_full");
        dir.touch("a.txt");
        let mut app = App::with_file(None);
        app.explorer = Some(ExplorerState::open(dir.path()).unwrap());
        app.main_view = MainView::Explorer;

        app.explorer_open_selected();
        assert_eq!(app.main_view, MainView::Editor);
        assert!(app.explorer.is_none());
        assert_eq!(app.open().buffer.text(), "hello\n");
    }

    #[test]
    fn explorer_open_selected_on_a_file_in_sidebar_mode_keeps_the_sidebar() {
        let dir = TempDir::new("open_file_sidebar");
        dir.touch("a.txt");
        let mut app = App::with_file(None);
        app.sidebar = Some(ExplorerState::open(dir.path()).unwrap());
        app.sidebar_open = true;
        app.sidebar_focused = true;

        app.explorer_open_selected();
        assert!(app.sidebar_open); // sidebar persists
        assert!(!app.sidebar_focused); // focus returns to the editor
        assert!(app.sidebar.is_some());
        assert_eq!(app.open().buffer.text(), "hello\n");
    }

    #[test]
    fn explorer_handle_action_down_and_up_move_selection() {
        let dir = TempDir::new("action_down_up");
        dir.touch("a");
        dir.touch("b");
        let mut app = App::with_file(None);
        app.explorer = Some(ExplorerState::open(dir.path()).unwrap());
        app.main_view = MainView::Explorer;

        app.explorer_handle_action(ExplorerAction::Down);
        assert_eq!(app.explorer.as_ref().unwrap().selected, 1);
        app.explorer_handle_action(ExplorerAction::Up);
        assert_eq!(app.explorer.as_ref().unwrap().selected, 0);
    }

    #[test]
    fn rename_prompt_flow_types_a_name_and_submits_on_enter() {
        let dir = TempDir::new("rename_prompt_flow");
        dir.touch("old.txt");
        let mut app = App::with_file(None);
        app.explorer = Some(ExplorerState::open(dir.path()).unwrap());
        app.main_view = MainView::Explorer;

        app.explorer_handle_action(ExplorerAction::BeginRename);
        assert!(app.explorer_prompt.is_some());
        assert_eq!(app.explorer_prompt.as_ref().unwrap().input, "old.txt"); // pre-filled with the current name
        for _ in 0.."old.txt".len() {
            app.explorer_prompt_key(KeyPress::named(FenixNamedKey::Backspace));
        }
        for c in "new.txt".chars() {
            app.explorer_prompt_key(KeyPress::char(c));
        }
        app.explorer_prompt_key(KeyPress::named(FenixNamedKey::Enter));

        assert!(app.explorer_prompt.is_none());
        assert!(dir.path().join("new.txt").exists());
        assert!(!dir.path().join("old.txt").exists());
    }

    #[test]
    fn delete_prompt_confirms_on_y_and_cancels_on_anything_else() {
        let dir = TempDir::new("delete_prompt_cancel");
        dir.touch("keep.txt");
        let mut app = App::with_file(None);
        app.explorer = Some(ExplorerState::open(dir.path()).unwrap());
        app.main_view = MainView::Explorer;

        app.explorer_handle_action(ExplorerAction::BeginDelete);
        app.explorer_prompt_key(KeyPress::named(FenixNamedKey::Escape)); // any non-'y' key cancels
        assert!(app.explorer_prompt.is_none());
        assert!(dir.path().join("keep.txt").exists());
    }

    #[test]
    fn delete_prompt_deletes_on_y() {
        let dir = TempDir::new("delete_prompt_confirm");
        dir.touch("gone.txt");
        let mut app = App::with_file(None);
        app.explorer = Some(ExplorerState::open(dir.path()).unwrap());
        app.main_view = MainView::Explorer;

        app.explorer_handle_action(ExplorerAction::BeginDelete);
        app.explorer_prompt_key(KeyPress::char('y'));
        assert!(app.explorer_prompt.is_none());
        assert!(!dir.path().join("gone.txt").exists());
    }

    #[test]
    fn find_file_candidates_labels_are_relative_to_root() {
        let dir = TempDir::new("find_file_candidates");
        dir.touch("a.txt");
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        dir.write("sub/b.txt", "hello\n");

        let mut labels: Vec<String> =
            App::find_file_candidates(dir.path()).into_iter().map(|c| c.label).collect();
        labels.sort();
        assert_eq!(labels, vec!["a.txt".to_string(), "sub/b.txt".to_string()]);
    }

    #[test]
    fn enter_picker_and_picker_cancel_leave_the_buffer_untouched() {
        let dir = TempDir::new("picker_enter_cancel");
        let file = dir.touch("a.txt");
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        app.test_insert_str(" extra");
        let dirty_text = app.open().buffer.text();
        let focused_before = app.focused_buffer_id();

        app.enter_picker(ActivePicker::FindFile(fenix_picker::PickerState::new(Vec::new())));
        assert_eq!(app.main_view, MainView::Picker);
        assert!(app.active_picker.is_some());
        assert_eq!(app.focused_buffer_id(), focused_before);

        app.picker_cancel();
        assert_eq!(app.main_view, MainView::Editor);
        assert!(app.active_picker.is_none());
        assert_eq!(app.open().buffer.text(), dirty_text);
    }

    #[test]
    fn picker_confirm_on_find_file_opens_the_selected_path_and_returns_to_editor() {
        let dir = TempDir::new("picker_confirm_find_file");
        let target = dir.write("target.txt", "picked contents\n");
        let mut app = App::with_file(None);

        let candidates = vec![fenix_picker::Candidate::new("target.txt", target.clone())];
        app.enter_picker(ActivePicker::FindFile(fenix_picker::PickerState::new(candidates)));
        app.picker_confirm();

        assert_eq!(app.main_view, MainView::Editor);
        assert!(app.active_picker.is_none());
        assert_eq!(app.open().buffer.path(), Some(target.as_path()));
        assert_eq!(app.open().buffer.text(), "picked contents\n");
    }

    #[test]
    fn picker_confirm_with_no_matches_is_a_no_op() {
        let mut app = App::with_file(None);
        let candidates: Vec<fenix_picker::Candidate<PathBuf>> =
            vec![fenix_picker::Candidate::new("only.txt", PathBuf::from("only.txt"))];
        let mut state = fenix_picker::PickerState::new(candidates);
        state.push_char('z'); // no match against "only.txt"
        assert!(state.selected().is_none());
        app.enter_picker(ActivePicker::FindFile(state));

        app.picker_confirm();

        // Nothing to confirm -- still open, nothing stashed/opened changed.
        assert_eq!(app.main_view, MainView::Picker);
        assert!(app.active_picker.is_some());
    }

    #[test]
    fn run_grep_and_jump_to_grep_match_moves_the_cursor_to_the_match() {
        let dir = TempDir::new("run_grep");
        dir.write("a.txt", "line one\nneedle here\nline three\n");
        let mut app = App::with_file(None);
        app.project_root = Some(dir.path().to_path_buf());

        app.run_grep("needle");

        match &app.active_picker {
            Some(ActivePicker::Grep(state)) => assert_eq!(state.len(), 1),
            other => panic!("expected an open Grep picker with one match, got is_some={}", other.is_some()),
        }
        app.picker_confirm();

        assert_eq!(app.main_view, MainView::Editor);
        let (line, _) = { let ob = app.open(); ob.buffer.line_col(&ob.cursor) };
        assert_eq!(line, 1); // "needle here" is the second line (index 1)
    }

    #[test]
    fn run_grep_with_no_matches_still_opens_an_empty_picker() {
        // `grep_project` treats "no matches" as `Ok(empty)`, not an error
        // -- `run_grep` still opens the (now-empty) Grep picker rather
        // than silently doing nothing, so the user sees "no results"
        // instead of a keypress with no visible effect.
        let dir = TempDir::new("run_grep_no_matches");
        dir.write("a.txt", "nothing interesting\n");
        let mut app = App::with_file(None);
        app.project_root = Some(dir.path().to_path_buf());

        app.run_grep("needle");
        assert_eq!(app.main_view, MainView::Picker);
        match &app.active_picker {
            Some(ActivePicker::Grep(state)) => assert!(state.is_empty()),
            _ => panic!("expected an open (empty) Grep picker"),
        }
    }

    #[test]
    fn grep_query_key_routes_chars_backspace_and_enter() {
        let dir = TempDir::new("grep_query_key");
        dir.write("a.txt", "target line\n");
        let mut app = App::with_file(None);
        app.project_root = Some(dir.path().to_path_buf());
        app.pending_grep_query = Some(String::new());

        for c in "targetx".chars() {
            app.grep_query_key(KeyPress::char(c));
        }
        app.grep_query_key(KeyPress::named(FenixNamedKey::Backspace)); // drop the trailing 'x'
        assert_eq!(app.pending_grep_query.as_deref(), Some("target"));

        app.grep_query_key(KeyPress::named(FenixNamedKey::Enter));
        assert!(app.pending_grep_query.is_none()); // submitted, no longer pending
        assert!(app.active_picker.is_some()); // run_grep found a match and opened a picker
    }

    #[test]
    fn grep_query_key_escape_cancels_without_searching() {
        let mut app = App::with_file(None);
        app.pending_grep_query = Some("some query".to_string());
        app.grep_query_key(KeyPress::named(FenixNamedKey::Escape));
        assert!(app.pending_grep_query.is_none());
        assert!(app.active_picker.is_none());
    }

    #[test]
    fn switch_to_project_registers_the_root_and_chains_into_find_file() {
        let known_dir = TempDir::new("switch_known");
        let mut app = App::with_file(None);
        app.known_projects = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));

        let project_dir = TempDir::new("switch_target");
        project_dir.touch("main.rs");

        // Already mid-picker (as `picker_confirm` would leave it before
        // dispatching to `switch_to_project`) -- switch shouldn't touch
        // `main_view` or the focused buffer, just swap the active picker.
        let focused_before = app.focused_buffer_id();
        app.enter_picker(ActivePicker::SwitchProject(fenix_picker::PickerState::new(Vec::new())));

        app.switch_to_project(project_dir.path().to_path_buf());

        assert_eq!(app.project_root, Some(project_dir.path().to_path_buf()));
        assert_eq!(app.known_projects.roots(), &[project_dir.path().to_path_buf()]);
        assert_eq!(app.main_view, MainView::Picker);
        assert_eq!(app.focused_buffer_id(), focused_before);
        match &app.active_picker {
            Some(ActivePicker::FindFile(state)) => {
                assert_eq!(state.selected().map(|c| c.label.as_str()), Some("main.rs"))
            }
            _ => panic!("expected switch_to_project to chain into a FindFile picker"),
        }
    }

    #[test]
    fn picker_switch_buffer_lists_open_buffers_mru_first() {
        let dir = TempDir::new("switch_buffer_candidates");
        let a = dir.write("a.txt", "a");
        let b = dir.write("b.txt", "b");
        let mut app = App::with_file(None);
        app.test_open_path(&a);
        app.test_open_path(&b); // b opened (and touched) last -> MRU-first

        app.picker_switch_buffer();

        match &app.active_picker {
            Some(ActivePicker::SwitchBuffer(state)) => {
                let labels: Vec<&str> = state.visible_rows(0, 10).map(|(_, c)| c.label.as_str()).collect();
                assert_eq!(labels[0], b.display().to_string());
                assert!(labels.contains(&a.display().to_string().as_str()));
            }
            _ => panic!("expected picker_switch_buffer to open a SwitchBuffer picker"),
        }
    }

    #[test]
    fn picker_switch_buffer_marks_dirty_buffers() {
        let mut app = App::with_file(None); // one scratch buffer, untouched
        app.picker_switch_buffer();
        match &app.active_picker {
            Some(ActivePicker::SwitchBuffer(state)) => {
                assert_eq!(state.visible_rows(0, 10).next().unwrap().1.label, "*scratch*");
            }
            _ => panic!("expected a SwitchBuffer picker"),
        }

        app.test_insert('x'); // now dirty
        app.picker_switch_buffer();
        match &app.active_picker {
            Some(ActivePicker::SwitchBuffer(state)) => {
                assert_eq!(state.visible_rows(0, 10).next().unwrap().1.label, "+ *scratch*");
            }
            _ => panic!("expected a SwitchBuffer picker"),
        }
    }

    #[test]
    fn picker_confirm_on_switch_buffer_points_the_focused_pane_at_it() {
        let dir = TempDir::new("switch_buffer_confirm");
        let a = dir.write("a.txt", "a");
        let mut app = App::with_file(None);
        let scratch_id = app.focused_buffer_id();
        app.test_open_path(&a); // now focused on a.txt

        let candidates = vec![fenix_picker::Candidate::new("*scratch*", scratch_id)];
        app.enter_picker(ActivePicker::SwitchBuffer(fenix_picker::PickerState::new(candidates)));
        app.picker_confirm();

        assert_eq!(app.focused_buffer_id(), scratch_id);
        assert_eq!(app.main_view, MainView::Editor);
        assert!(app.active_picker.is_none());
    }

    #[test]
    fn next_and_prev_buffer_cycle_through_every_open_buffer() {
        let dir = TempDir::new("cycle_buffer");
        let a = dir.write("a.txt", "a");
        let b = dir.write("b.txt", "b");
        let mut app = App::with_file(None); // scratch, then a, then b
        app.test_open_path(&a);
        app.test_open_path(&b);

        let ids = app.buffers.ids_sorted_by_path(); // scratch, a, b (scratch sorts first)
        assert_eq!(app.focused_buffer_id(), ids[2]); // currently on b

        app.next_buffer(); // wraps to scratch
        assert_eq!(app.focused_buffer_id(), ids[0]);

        app.prev_buffer(); // back to b
        assert_eq!(app.focused_buffer_id(), ids[2]);

        app.prev_buffer(); // to a
        assert_eq!(app.focused_buffer_id(), ids[1]);
    }

    #[test]
    fn kill_buffer_falls_back_to_the_mru_next_buffer() {
        let dir = TempDir::new("kill_buffer");
        let a = dir.write("a.txt", "a");
        let mut app = App::with_file(None);
        let scratch_id = app.focused_buffer_id();
        app.test_open_path(&a); // focused on a, scratch is MRU-next

        app.kill_buffer();

        assert_eq!(app.focused_buffer_id(), scratch_id);
        assert!(app.buffers.get(scratch_id).is_some());
        assert_eq!(app.buffers.len(), 1);
    }

    #[test]
    fn kill_buffer_on_the_last_buffer_falls_back_to_a_fresh_scratch() {
        let mut app = App::with_file(None); // single scratch buffer
        let original_id = app.focused_buffer_id();

        app.kill_buffer();

        assert_ne!(app.focused_buffer_id(), original_id); // a *new* scratch, not the old one
        assert_eq!(app.buffers.len(), 1);
        assert_eq!(app.open().buffer.text(), "");
    }

    #[test]
    fn kill_buffer_retargets_every_other_pane_showing_it() {
        let dir = TempDir::new("kill_buffer_multi_pane");
        let a = dir.write("a.txt", "a");
        let mut app = App::with_file(None);
        let scratch_id = app.focused_buffer_id();
        app.test_open_path(&a); // focused pane now on a
        app.split_vertical(); // new pane, also showing a
        assert_eq!(app.windows().window_count(), 2);

        app.kill_buffer(); // closes a, focused in the new pane

        for pane in app.windows().windows() {
            assert_eq!(app.windows().content(pane), Some(&scratch_id));
        }
    }

    #[test]
    fn new_scratch_buffer_opens_a_fresh_empty_buffer_in_the_focused_pane() {
        let dir = TempDir::new("new_scratch");
        let a = dir.write("a.txt", "hello");
        let mut app = App::with_file(None);
        app.test_open_path(&a);
        let a_id = app.focused_buffer_id();

        app.new_scratch_buffer();

        assert_ne!(app.focused_buffer_id(), a_id);
        assert_eq!(app.open().buffer.text(), "");
        assert_eq!(app.open().buffer.path(), None);
    }

    #[test]
    fn picker_key_routes_typing_navigation_and_escape() {
        let candidates = vec![
            fenix_picker::Candidate::new("apple", PathBuf::from("apple")),
            fenix_picker::Candidate::new("banana", PathBuf::from("banana")),
        ];
        let mut app = App::with_file(None);
        app.enter_picker(ActivePicker::FindFile(fenix_picker::PickerState::new(candidates)));

        app.picker_key(KeyPress::char('b'));
        match &app.active_picker {
            Some(ActivePicker::FindFile(state)) => assert_eq!(state.query(), "b"),
            _ => panic!("expected an active FindFile picker"),
        }

        app.picker_key(KeyPress::named(FenixNamedKey::Backspace));
        match &app.active_picker {
            Some(ActivePicker::FindFile(state)) => assert_eq!(state.query(), ""),
            _ => panic!("expected an active FindFile picker"),
        }

        app.picker_key(KeyPress::named(FenixNamedKey::Down));
        match &app.active_picker {
            Some(ActivePicker::FindFile(state)) => assert_eq!(state.selected_row(), 1),
            _ => panic!("expected an active FindFile picker"),
        }

        app.picker_key(KeyPress::named(FenixNamedKey::Escape));
        assert!(app.active_picker.is_none());
        assert_eq!(app.main_view, MainView::Editor);
    }

    #[test]
    fn picker_key_enter_confirms_the_selection() {
        let dir = TempDir::new("picker_key_enter");
        let target = dir.write("only.txt", "confirmed\n");
        let mut app = App::with_file(None);
        let candidates = vec![fenix_picker::Candidate::new("only.txt", target.clone())];
        app.enter_picker(ActivePicker::FindFile(fenix_picker::PickerState::new(candidates)));

        app.picker_key(KeyPress::named(FenixNamedKey::Enter));

        assert_eq!(app.main_view, MainView::Editor);
        assert_eq!(app.open().buffer.path(), Some(target.as_path()));
    }

    #[test]
    fn picker_row_spans_shows_the_query_prompt_and_flags_the_selected_row() {
        let candidates =
            vec![fenix_picker::Candidate::new("a.txt", PathBuf::from("a.txt")), fenix_picker::Candidate::new("b.txt", PathBuf::from("b.txt"))];
        let app = App::with_file(None);
        let picker = ActivePicker::FindFile(fenix_picker::PickerState::new(candidates));

        let (spans, selected_row) = app.picker_row_spans(&picker, 5);
        assert_eq!(selected_row, Some(1)); // row 0 is the prompt, row 1 the first candidate
        let joined: String = spans.iter().map(|(s, _, _)| s.as_str()).collect();
        assert!(joined.starts_with("> "));
        assert!(joined.contains("a.txt"));
        assert!(joined.contains("b.txt"));
    }
}
