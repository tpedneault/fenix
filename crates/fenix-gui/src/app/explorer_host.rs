//! The explorer's host half: listings read off the UI thread and
//! applied to the pane, the sidebar or the directory picker
//! (`ExplorerTarget`) -- all three drawn by the explorer page
//! (`explorer_page`) -- the bulk rename as text, the path bar and the
//! places it offers, and file operations and their prompts. The
//! listing's own state is `fenix-explorer`'s.

use super::*;
use crate::page::Role as PageRole;

/// What the directory picker is for -- ordinary
/// directory browsing (`SPC f j`, the sidebar) behaves exactly as it
/// always has; `PickProjectDir` is the same listing and the same
/// navigation, just with `ExplorerAction::SelectCwd` (`S`) wired to
/// register `cwd` as a known project instead of being ignored, and
/// `Open`/`Enter`/`l` on a *file* doing nothing instead of opening it
/// into the editor (there's nothing sensible to do with a file when
/// what's being picked is a directory). See `picker_add_project_prompt`.
/// `FindFrom` is for `SPC f e`: browsing starts at the home directory
/// (not the project root -- the whole point is reaching files *outside*
/// any project without typing their absolute path) and opening a file
/// works exactly as in `Browse`, but `S` additionally fires off a
/// recursive fuzzy `FindFile` picker rooted at whatever directory is
/// currently being browsed -- lets you either navigate straight to a
/// known file or narrow the rest of the way by name once you've gotten
/// close. See `start_explore_from_home`/`start_find_from_here`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExplorerPurpose {
    Browse,
    PickProjectDir,
    FindFrom,
    /// The new-project wizard's "In" folder: `S` hands it back.
    PickWizardParent,
    /// A path for the settings page (`b`, `Ctrl-O`): `Enter` on a file
    /// hands it back (unless a folder's wanted), `S` the folder.
    PickSettingPath { folder: bool },
}

/// Which explorer-mode prompt (if any) is capturing the next keystrokes --
/// same "next key(s) mean something special" pattern `fenix-vim`'s
/// `Mode::Command`/`pending_replace` already use, applied to file-manager
/// text input instead of Vim's own command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PromptKind {
    /// Type where you want to be. The one thing the explorer had no way
    /// to do at all: reaching `\\nas\media\projects\2026` meant walking
    /// there one `Enter` at a time from wherever you happened to start.
    GoToPath,
    /// Where to write an archive of the marked set.
    ArchiveTo,
    /// Narrows the listing as you type. Unlike every other prompt here
    /// it acts on each keystroke rather than on Enter -- a filter you
    /// cannot see the effect of while typing is just a slower search.
    Filter,
    ConfirmDelete,
    CopyTo,
    MoveTo,
}

impl PromptKind {
    /// How to describe what just happened, for the status line.
    fn past_tense(self) -> &'static str {
        match self {
            PromptKind::GoToPath => "opened",
            PromptKind::ArchiveTo => "archived",
            PromptKind::Filter => "filtered",
            PromptKind::ConfirmDelete => "deleted",
            PromptKind::CopyTo => "copied",
            PromptKind::MoveTo => "moved",
        }
    }
}

pub(super) struct ExplorerPrompt {
    pub(super) kind: PromptKind,
    pub(super) input: String,
}

/// How long a directory may take to answer before the explorer says
/// what it is waiting on.
///
/// A local directory answers in single-digit milliseconds. Anything past
/// this is a network share, a disk spinning up, or a path that is never
/// going to answer -- and in every one of those cases a pane that simply
/// sits there is indistinguishable from a hung editor.
pub(super) const SLOW_LISTING_AFTER: Duration = Duration::from_millis(300);

/// `sidebar_rows`'s answer: the rows' spans, the selected row, the
/// tinted rows, and the underlines as (row, first cell, end cell).
pub(super) type SidebarRows = (RowSpans, Option<usize>, Vec<usize>, Vec<(usize, usize, usize)>);

/// What the preview shows as a picture, by extension.
const PICTURES: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp", "ico", "tif", "tiff"];

/// How much of a file the preview reads, and how many lines it shows.
const PREVIEW_BYTES: usize = 256 * 1024;
const PREVIEW_LINES: usize = 2000;

/// Where each project's explorer is kept, in the state folder.
pub(super) const SESSIONS_FILE: &str = "explorer.json";

/// A project's explorer as it was left: whether the sidebar was open,
/// how wide, which folders were open (relative to the project), and
/// its filter.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct ExplorerSession {
    #[serde(default)]
    pub(super) open: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) width: Option<f32>,
    #[serde(default)]
    pub(super) folders: Vec<PathBuf>,
    #[serde(default)]
    pub(super) filter: String,
}

/// The most a filter's search below a folder shows.
const SEARCH_LIMIT: usize = 2000;

/// Two clicks on a row this close together open it.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// How far either side of the sidebar's edge a drag can start, in
/// pixels.
const SIDEBAR_EDGE: f32 = 4.0;

/// Which of the three listings a directory read belongs to.
///
/// There are three because they answer different questions -- a pane
/// you can split, a strip that stays open beside the editor, and a
/// modal "pick a folder" for `SPC p a`/`SPC m a`/`SPC f e`. They share
/// one `ExplorerState`, one key table and one set of operations; what
/// they do not share is *where* the state lives, which is all this
/// distinguishes.
///
/// Keyed by this rather than by `BufferId` so all three read
/// directories the same way. Only the pane-hosted one did before, which
/// meant the sidebar could still freeze the editor on a share -- the
/// exact problem the async work existed to remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ExplorerTarget {
    /// A listing in a pane (`SPC f j`/`SPC e e`), identified by its
    /// buffer.
    Buffer(BufferId),
    /// The persistent strip (`SPC e t`).
    Sidebar,
    /// The modal directory picker (`SPC p a`, `SPC m a`, `SPC f e`).
    Overlay,
}

/// One directory read, in flight or just finished, for one explorer
/// listing.
///
/// The `id` is what makes abandoning possible. A blocking `read_dir` on
/// a share that has gone away **cannot be cancelled** -- the thread sits
/// in the kernel until SMB gives up, minutes later, and nothing can
/// shorten that. So nothing tries: pressing Escape bumps the id, which
/// makes whatever that thread eventually returns arrive stale and get
/// dropped, and the thread is left to finish and exit on its own. The
/// editor moves on immediately; the corpse cleans itself up.
pub(super) struct ExplorerRequest {
    pub(super) id: u64,
    /// Where this read is aimed -- named in the "still reading" line, so
    /// the user can see it is the share and not fenix.
    pub(super) path: PathBuf,
    pub(super) started: Instant,
    /// `true` once the listing has been applied. The record outlives it
    /// so the git-status second pass, which arrives later, can still
    /// check it is talking about the same read.
    pub(super) done: bool,
}

/// A file operation big enough to be worth watching.
///
/// Copying a few files is instant and needs none of this. Copying a
/// directory to a share is minutes, and doing it on the main thread
/// would freeze the editor for all of them -- the same reason listings
/// moved off it, applied to the operation that actually moves the
/// bytes.
pub(super) struct ExplorerJob {
    /// What to call it while it runs ("copying") and once it is over
    /// ("copied"). Two words rather than one chopped about: deriving
    /// the participle from the past tense gave "copi", which is what
    /// the progress line actually said until someone read it.
    pub(super) running: &'static str,
    pub(super) done: &'static str,
    /// Set to stop. The worker checks it before each file; a file
    /// already being written has to finish (see `fenix_fs::
    /// copy_into_reporting`), so this is "stop soon", not "stop now".
    pub(super) cancel: Arc<AtomicBool>,
    /// The most recent progress the worker reported.
    pub(super) progress: fenix_fs::Progress,
}

/// A copy or move that has been asked for, found to collide with
/// something already at the destination, and is waiting for the user to
/// say what to do about it.
///
/// The collision is found before anything is written (`fenix_fs::
/// conflicts_in`), so nothing has happened yet and every answer --
/// including cancelling outright -- is still available. The explorer
/// this replaces simply overwrote, with no prompt and nothing in the
/// Recycle Bin afterwards.
pub(super) struct ExplorerConflict {
    /// Which operation was asked for: the answer applies to it, not to
    /// some remembered general preference.
    pub(super) kind: PromptKind,
    /// Everything the operation was going to act on, not only the
    /// colliding ones -- the non-colliding files still have to be
    /// copied once the question is answered.
    pub(super) targets: Vec<PathBuf>,
    pub(super) dest: PathBuf,
    /// What is already there, for naming it in the prompt. Showing the
    /// first one and a count beats "some files already exist".
    pub(super) conflicts: Vec<PathBuf>,
}

/// Whether properties go on to count what's inside: a real folder only.
/// A link is measured as itself, so counting one would only replace
/// where it points with "0B in 1 file".
pub(super) fn counts_contents(props: &fenix_fs::Properties) -> bool {
    props.kind.is_dir_like() && !props.kind.is_link()
}

/// One line of everything a listing column cannot hold.
pub(super) fn describe_properties(props: &fenix_fs::Properties) -> String {
    let name = props.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| readable_path(&props.path));
    let mut out = name;
    if !props.kind.is_dir_like() {
        out.push_str(&format!("  --  {}", format_size(props.size)));
    }
    if let Some(modified) = props.modified {
        out.push_str(&format!("  --  modified {} ago", format_age(Some(modified))));
    }
    if let Some(created) = props.created {
        out.push_str(&format!(", created {} ago", format_age(Some(created))));
    }
    let mut flags = Vec::new();
    if props.attributes.readonly {
        flags.push("read-only");
    }
    if props.attributes.hidden {
        flags.push("hidden");
    }
    if props.attributes.system {
        flags.push("system");
    }
    if !flags.is_empty() {
        out.push_str(&format!("  --  {}", flags.join(", ")));
    }
    if let Some(target) = &props.link_target {
        out.push_str(&format!("  --  links to {}", readable_path(target)));
    }
    if counts_contents(props) {
        out.push_str("  --  counting...");
    }
    out
}

/// Whether `path` is a server with no share named -- `\\\\nas`, which is
/// not a directory anybody can list, but is a question a server can
/// answer.
pub(super) fn is_bare_server(path: &Path) -> bool {
    let text = path.as_os_str().to_string_lossy().replace('/', "\\");
    let Some(rest) = text.strip_prefix("\\\\") else { return false };
    let rest = rest.trim_end_matches('\\');
    !rest.is_empty() && !rest.contains('\\')
}

/// Whether `path` names a network location by its own spelling --
/// `\\\\server\\share\\...`.
///
/// Only catches paths written that way: a share mapped to a drive
/// letter looks exactly like a local disk from here, and telling those
/// apart needs a real drive-type lookup. Used to skip work that is
/// merely slow over a network, never to decide anything that would be
/// *wrong* if the guess missed.
pub(super) fn is_unc(path: &Path) -> bool {
    let s = path.as_os_str().to_string_lossy();
    s.starts_with("\\\\") || s.starts_with("//")
}

/// The branch `root`'s repository has checked out -- or the commit, when
/// detached -- from `.git/HEAD`. `None` outside a repository.
pub(super) fn head_branch(root: &Path) -> Option<String> {
    let dot_git = root.join(".git");
    // A worktree or submodule: `.git` is a file naming the real one.
    let git_dir = if dot_git.is_file() {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        let dir = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
        if dir.is_absolute() { dir } else { root.join(dir) }
    } else {
        dot_git
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref:") {
        Some(reference) => Some(reference.trim().trim_start_matches("refs/heads/").to_string()),
        None => Some(head.chars().take(7).collect()),
    }
}

impl App {
    /// Directory the explorer opens in when asked to "start somewhere
    /// sensible": the focused buffer's own directory if it has a path,
    /// else the process's cwd.
    pub(super) fn explorer_start_dir(&self) -> PathBuf {
        self.open()
            .buffer
            .path()
            .and_then(|p| p.parent())
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    /// `SPC f j` (dired-jump): a listing of the current file's
    /// directory, as a page in the focused pane -- closable (`q`, `SPC b
    /// k`), splittable and in `SPC b b` like any other buffer.
    pub(crate) fn explorer_jump(&mut self) {
        let dir = self.explorer_start_dir();
        self.open_dired_at(&dir);
    }

    /// Opens a listing of `dir` as a page in the focused pane.
    pub(super) fn open_dired_at(&mut self, dir: &Path) {
        // Opens empty and fills in when the read lands. A fresh page has
        // nothing to keep showing in the meantime, unlike a pane that is
        // navigating from somewhere -- so this is the one case where a
        // pending listing really is blank.
        // The new listing has the keyboard, not the sidebar.
        self.sidebar_focused = false;
        let id = self.open_page_with(|id| PageModel::Explorer(ExplorerTarget::Buffer(id), Box::new(explorer_page::View::new(explorer_page::Form::Pane))));
        let mut state = ExplorerState::pending_at(dir);
        self.configure_explorer(&mut state);
        self.dired_states.insert(id, state);
        self.explorer_header_facts(ExplorerTarget::Buffer(id));
        self.explorer_navigate(ExplorerTarget::Buffer(id), dir);
        // Warmed now so `SPC e b` is instant whenever it comes.
        self.ensure_volumes();
        self.wake_caret();
    }

    /// The directory picker (`SPC p a`, `SPC f e`, a settings path, the
    /// wizard's folder): a listing of `start` as a page in the focused
    /// pane, `S` picking the folder it shows for `purpose`.
    pub(super) fn open_folder_picker(&mut self, start: &Path, purpose: ExplorerPurpose) {
        let mut explorer = match ExplorerState::opened(start) {
            Ok(e) => e,
            Err(err) => {
                self.set_error(format!("couldn't list {} ({err})", start.display()));
                return;
            }
        };
        self.configure_explorer(&mut explorer);
        // One picker at a time.
        if let Some(old) = self.folder_picker_page() {
            self.close_page_quietly(old);
        }
        self.explorer = Some(explorer);
        self.explorer_purpose = purpose;
        self.sidebar_focused = false;
        let picks = match purpose {
            ExplorerPurpose::Browse => "",
            ExplorerPurpose::PickProjectDir => "S adds the folder shown as a project",
            ExplorerPurpose::FindFrom => "Enter opens a file · S finds files under the folder shown",
            ExplorerPurpose::PickWizardParent => "S creates the project in the folder shown",
            ExplorerPurpose::PickSettingPath { folder: true } => "S picks the folder shown",
            ExplorerPurpose::PickSettingPath { folder: false } => "Enter picks a file · S the folder shown",
        };
        self.open_page_with(|_| PageModel::Explorer(ExplorerTarget::Overlay, Box::new(explorer_page::View::new(explorer_page::Form::Picker { picks }))));
        self.explorer_header_facts(ExplorerTarget::Overlay);
        self.wake_caret();
    }

    /// The directory picker's page, if it's open.
    pub(super) fn folder_picker_page(&self) -> Option<BufferId> {
        self.find_page(|m| matches!(m, PageModel::Explorer(ExplorerTarget::Overlay, _)))
    }

    /// Whether the directory picker is what has the keyboard.
    pub(super) fn picking_folder(&self) -> bool {
        self.active_explorer_target() == Some(ExplorerTarget::Overlay)
    }

    /// Closes the directory picker, the pane going back to what it
    /// showed before.
    pub(super) fn close_folder_picker(&mut self) {
        if let Some(id) = self.folder_picker_page() {
            self.close_page_quietly(id);
        }
        self.explorer = None;
        self.explorer_purpose = ExplorerPurpose::Browse;
    }

    /// Page `id` is closing: if it's a listing, its listing goes with it
    /// (the picker's, its purpose too), and any read for it is dropped.
    pub(super) fn forget_explorer_page(&mut self, id: BufferId) {
        if self.explorer_preview.is_some_and(|(_, preview)| preview == id) {
            self.explorer_preview = None;
        }
        // The preview goes with the listing it follows.
        if let Some(PageModel::Explorer(target, _)) = self.pages.get(&id).map(|s| &s.model) {
            if self.explorer_preview.is_some_and(|(owner, _)| owner == *target) {
                self.close_explorer_preview();
            }
        }
        let Some(PageModel::Explorer(target, _)) = self.pages.get(&id).map(|s| &s.model) else { return };
        let target = *target;
        match target {
            ExplorerTarget::Buffer(b) => {
                self.dired_states.remove(&b);
            }
            ExplorerTarget::Overlay => {
                self.explorer = None;
                self.explorer_purpose = ExplorerPurpose::Browse;
            }
            ExplorerTarget::Sidebar => {}
        }
        self.explorer_requests.remove(&target);
    }

    /// The page drawing `target`'s listing.
    fn explorer_page_id(&self, target: ExplorerTarget) -> Option<BufferId> {
        match target {
            ExplorerTarget::Buffer(id) => Some(id),
            ExplorerTarget::Overlay => self.folder_picker_page(),
            ExplorerTarget::Sidebar => None,
        }
    }

    /// The header's project and branch, for where `target` is now.
    pub(super) fn explorer_header_facts(&mut self, target: ExplorerTarget) {
        let Some(cwd) = self.explorer_state(target).map(|s| s.cwd.clone()) else { return };
        let project = (!is_unc(&cwd)).then(|| fenix_project::find_project_root(&cwd)).flatten().map(|root| explorer_page::Project {
            name: root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| readable_path(&root)),
            kind: fenix_project::detect_kind(&root),
            root,
        });
        // A local read of `.git/HEAD`, not a `git` run: cheap enough for
        // every move, and never across a share.
        let branch = project.as_ref().and_then(|p| head_branch(&p.root));
        let view = match target {
            ExplorerTarget::Sidebar => Some(&mut self.sidebar_view),
            _ => match self.explorer_page_id(target).and_then(|id| self.pages.get_mut(&id)) {
                Some(PageState { model: PageModel::Explorer(_, view), .. }) => Some(&mut **view),
                _ => None,
            },
        };
        if let Some(view) = view {
            view.project = project;
            view.branch = branch;
        }
    }

    /// `SPC e d`: two listings side by side.
    ///
    /// The commander layout, arranged out of the window system fenix
    /// already has rather than as a fixed panel of its own. Both halves
    /// are ordinary explorer buffers, so every split, workspace and
    /// `SPC b b` command keeps working on them, and closing one leaves
    /// the other exactly as it was.
    ///
    /// What makes it the commander *gesture* rather than just two panes
    /// is `explorer_other_cwd`: copy and move seed their destination
    /// with the directory the other half is showing, which is the whole
    /// reason anyone arranges two listings in the first place.
    pub(crate) fn explorer_dual_pane(&mut self) {
        // Whatever is focused becomes the left half -- opening one here
        // first means `SPC e d` works from anywhere, not only from an
        // explorer.
        let left = match self.focused_dired_buffer() {
            Some(id) => self.dired_states.get(&id).map(|s| s.cwd.clone()).unwrap_or_else(|| self.explorer_start_dir()),
            None => {
                let dir = self.explorer_start_dir();
                self.open_dired_at(&dir);
                dir
            }
        };
        self.split_vertical();
        // The new pane starts on the same directory. Two views of one
        // place is a useful thing to have (copy something next to
        // itself), and it is one keystroke from being two places.
        self.open_dired_at(&left);
        self.wake_caret();
    }

    /// The directory some *other* visible listing is showing, if there
    /// is exactly one obvious candidate.
    ///
    /// `None` when there is no second listing, or when there are several
    /// and no reason to prefer one -- guessing wrong here would seed a
    /// copy with the wrong destination, which is worse than seeding it
    /// with nothing.
    pub(super) fn explorer_other_cwd(&self) -> Option<PathBuf> {
        let focused = self.focused_pane_id();
        let here = self.active_explorer().map(|e| e.cwd.clone());
        let mut candidates: Vec<PathBuf> = self
            .windows()
            .windows()
            .into_iter()
            .filter(|&pane| pane != focused)
            .filter_map(|pane| self.windows().content(pane).copied())
            .filter_map(|buffer| self.dired_states.get(&buffer))
            .map(|state| state.cwd.clone())
            .filter(|cwd| Some(cwd) != here.as_ref())
            .collect();
        candidates.dedup();
        (candidates.len() == 1).then(|| candidates.remove(0))
    }

    // -- Renaming by editing the listing ---------------------------------

    /// `SPC e w`: the listing's names as text, to edit.
    ///
    /// One name per line, in a text buffer in place of the listing, so
    /// every Vim tool -- `:%s/`, visual block, macros, counts -- becomes
    /// a bulk-rename tool without anyone building one. Emacs calls it
    /// wdired. Line N is row N; a name in an open folder stays in that
    /// folder.
    pub(crate) fn start_rename_mode(&mut self) {
        if self.rename_mode.contains_key(&self.focused_buffer_id()) {
            self.set_message("already editing -- SPC e W applies, Esc cancels");
            return;
        }
        let Some(listing) = self.focused_dired_buffer() else {
            self.set_error("rename mode needs a file listing (SPC e e)");
            return;
        };
        let Some(entries) = self.dired_states.get(&listing).map(|s| s.entries.clone()) else { return };
        if entries.is_empty() {
            self.set_error("nothing here to rename");
            return;
        }
        // A read landing mid-edit would change what the lines stand for,
        // so stop waiting for one first.
        self.explorer_abandon_listing(ExplorerTarget::Buffer(listing));
        let text: String = entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>().join("\n");
        let id = self.buffers.open_explorer(&text);
        self.rename_mode.insert(id, (listing, entries));
        self.open_buffer_in_focused_pane(id);
        self.set_message("editing names -- SPC e W applies, Esc cancels");
        self.wake_caret();
    }

    /// Ends an edit of names: the text goes, and the listing comes back
    /// in its pane.
    fn end_rename_mode(&mut self, buffer: BufferId) -> Option<BufferId> {
        let (listing, _) = self.rename_mode.remove(&buffer)?;
        self.buffers.close(buffer);
        if self.pages.contains_key(&listing) {
            self.repoint_panes_showing(buffer, listing);
        } else {
            let fallback = self.buffers.mru().first().copied().unwrap_or_else(|| self.buffers.open_scratch());
            self.repoint_panes_showing(buffer, fallback);
        }
        Some(listing)
    }

    /// `Esc` in rename mode: throw the edits away and put the listing
    /// back.
    pub(super) fn cancel_rename_mode(&mut self, buffer: BufferId) {
        if self.end_rename_mode(buffer).is_none() {
            return;
        }
        self.set_message("edits discarded");
        self.wake_caret();
    }

    /// `SPC e W`: read the edited names, work out what they ask for, and
    /// ask before doing it.
    ///
    /// A rejected edit changes nothing and leaves the text on screen to
    /// fix, which is the whole reason the checking happens up front (see
    /// `fenix_explorer::plan_renames`).
    pub(crate) fn apply_rename_mode(&mut self) {
        let buffer = self.focused_buffer_id();
        let Some((_, entries)) = self.rename_mode.get(&buffer).cloned() else {
            self.set_error("not editing names -- SPC e w starts");
            return;
        };
        let text = self.buffers.get(buffer).map(|ob| ob.buffer.text()).unwrap_or_default();
        let edited: Vec<&str> = text.lines().collect();
        let plan = match fenix_explorer::plan_renames(&entries, &edited) {
            Ok(plan) => plan,
            Err(err) => {
                self.set_error(err.to_string());
                return;
            }
        };
        if plan.is_empty() {
            self.end_rename_mode(buffer);
            self.set_message("no names changed");
            return;
        }
        self.rename_confirm = Some(plan);
        self.wake_caret();
    }

    /// Answers the "rename N files?" confirmation.
    ///
    /// Arm-then-confirm, like every other destructive action here. A
    /// bulk rename is easy to get wrong in a way that is tedious to
    /// undo, and the preview is the last chance to notice that `:%s/`
    /// matched more than intended.
    pub(super) fn rename_confirm_key(&mut self, key: KeyPress) {
        let Some(plan) = self.rename_confirm.take() else { return };
        if !matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
            self.set_message("cancelled");
            self.wake_caret();
            return;
        }
        let two_phase = fenix_explorer::needs_two_phases(&plan);
        let pairs: Vec<(PathBuf, PathBuf)> = plan.iter().map(|r| (r.from.clone(), r.to.clone())).collect();
        let outcomes: Vec<Result<(), String>> =
            fenix_fs::rename_all(&pairs, two_phase).into_iter().map(|o| o.error.map_or(Ok(()), Err)).collect();
        self.report_explorer_outcomes("renamed", outcomes);
        let buffer = self.focused_buffer_id();
        if self.end_rename_mode(buffer).is_some() {
            self.explorer_relist();
        }
        self.wake_caret();
    }

    /// What the confirmation says: how many, and a real example, because
    /// "rename 40 files?" is not something anyone can answer.
    pub(super) fn rename_confirm_text(&self) -> Option<String> {
        let plan = self.rename_confirm.as_ref()?;
        let first = plan.first()?;
        let from = first.from.file_name().unwrap_or(first.from.as_os_str()).to_string_lossy();
        let to = first.to.strip_prefix(first.from.parent().unwrap_or(Path::new(""))).unwrap_or(&first.to).to_string_lossy();
        Some(if plan.len() == 1 {
            format!("Rename {from} -> {to}? (y/n)")
        } else {
            format!("Rename {} files? e.g. {from} -> {to} (y/n)", plan.len())
        })
    }

    // -- Reading a directory without freezing the editor -----------------

    /// Points a listing at `path`, reading it off the main thread.
    ///
    /// The pane keeps showing where it already is until the new listing
    /// arrives. That is deliberate on two counts: it is what a file
    /// manager should do anyway (you can still read the directory you
    /// are leaving), and it means a path that never answers costs you
    /// nothing at all -- Escape abandons the wait and you are still
    /// where you were, rather than sitting in an empty pane.
    ///
    /// Git status follows as a separate event on the same thread, so
    /// badges never delay the listing. It is skipped outright for UNC
    /// paths: running a `git status` subprocess across a share is slow
    /// and almost never what anyone wants. (A network drive mapped to a
    /// letter is not detected here -- telling those apart needs a real
    /// drive-type lookup, which `SPC e b`'s volume list has and this
    /// does not want to wait for.)
    pub(super) fn explorer_navigate(&mut self, target: ExplorerTarget, path: &Path) {
        let show_hidden = self.explorer_state(target).is_some_and(|s| s.show_hidden);
        // The same directory again keeps its open folders, so they are
        // read again with it; the sidebar moving to a project opens the
        // ones it had open there.
        let open = match self.explorer_state(target).filter(|s| s.cwd == path) {
            Some(state) => state.open_folders(),
            None => self.saved_open_folders(target, path),
        };
        self.next_explorer_request += 1;
        let request = self.next_explorer_request;
        let path = path.to_path_buf();
        self.explorer_requests.insert(target, ExplorerRequest { id: request, path: path.clone(), started: Instant::now(), done: false });

        match self.event_proxy.clone() {
            Some(proxy) => {
                std::thread::spawn(move || {
                    let result = fenix_explorer::read_tree(&path, &open, show_hidden).map_err(|err| err.to_string());
                    let worth_annotating = result.is_ok() && !is_unc(&path);
                    if proxy.send_event(FenixUserEvent::ExplorerListed { target, request, result }).is_err() {
                        return;
                    }
                    if worth_annotating {
                        let statuses = fenix_explorer::status_for_dir(&path);
                        let _ = proxy.send_event(FenixUserEvent::ExplorerGitStatus { target, request, statuses });
                    }
                });
            }
            None => {
                // No event loop to report back through (every test) --
                // read and apply inline, same posture as every other
                // background call in this file.
                let result = fenix_explorer::read_tree(&path, &open, show_hidden).map_err(|err| err.to_string());
                let worth_annotating = result.is_ok() && !is_unc(&path);
                self.apply_explorer_listed(target, request, result);
                if worth_annotating {
                    let statuses = fenix_explorer::status_for_dir(&path);
                    self.apply_explorer_git_status(target, request, statuses);
                }
            }
        }
        self.render_explorer(target, false);
    }

    /// Wherever `target`'s listing lives.
    pub(super) fn explorer_state(&self, target: ExplorerTarget) -> Option<&ExplorerState> {
        match target {
            ExplorerTarget::Buffer(id) => self.dired_states.get(&id),
            ExplorerTarget::Sidebar => self.sidebar.as_ref(),
            ExplorerTarget::Overlay => self.explorer.as_ref(),
        }
    }

    pub(super) fn explorer_state_mut(&mut self, target: ExplorerTarget) -> Option<&mut ExplorerState> {
        match target {
            ExplorerTarget::Buffer(id) => self.dired_states.get_mut(&id),
            ExplorerTarget::Sidebar => self.sidebar.as_mut(),
            ExplorerTarget::Overlay => self.explorer.as_mut(),
        }
    }

    /// Which listing the user is currently working in -- the one every
    /// action, prompt and navigation applies to: the sidebar while it
    /// has the keyboard, else the explorer page in the focused pane.
    pub(super) fn active_explorer_target(&self) -> Option<ExplorerTarget> {
        if self.sidebar_focused {
            return self.sidebar.as_ref().map(|_| ExplorerTarget::Sidebar);
        }
        match self.pages.get(self.windows().content(self.focused_pane_id())?).map(|s| &s.model) {
            Some(PageModel::Explorer(target, _)) => self.explorer_state(*target).map(|_| *target),
            _ => None,
        }
    }

    /// `target`'s listing changed: its page is laid out again. `moved`
    /// says it is showing another directory, so the header's project
    /// and branch are looked up again too. (The sidebar is laid out
    /// every frame it's drawn.)
    pub(super) fn render_explorer(&mut self, target: ExplorerTarget, moved: bool) {
        if moved {
            self.explorer_header_facts(target);
        }
        self.preview_follow(target);
        if let Some(id) = self.explorer_page_id(target) {
            self.restale_page(id);
        }
    }

    /// `FenixUserEvent::ExplorerListed`: swaps in a listing, unless it
    /// has been superseded by a newer request or abandoned outright, or
    /// the listing it was for has since gone away.
    pub(super) fn apply_explorer_listed(&mut self, target: ExplorerTarget, request: u64, result: Result<fenix_explorer::Listing, String>) {
        if !self.explorer_request_is_current(target, request) {
            return;
        }
        match result {
            Ok(listing) => {
                let path = listing.cwd.clone();
                if let Some(state) = self.explorer_state_mut(target) {
                    state.apply_listing(listing);
                }
                self.mark_explorer_request_done(target, request);
                // A path waiting for this listing: the sidebar's file in
                // its new root, a name just made.
                if let Some(path) = self.explorer_reveal_pending.remove(&target) {
                    self.explorer_reveal_path(target, &path);
                }
                // A filter kept across the move searches the new place.
                if self.explorer_state(target).is_some_and(|s| !s.filter.trim().is_empty()) {
                    self.explorer_search(target);
                }
                self.render_explorer(target, true);
                self.remember_visited_directory(&path);
            }
            Err(err) => {
                // The listing stays where it was, which is the useful
                // place to be left after a directory refuses to open.
                self.mark_explorer_request_done(target, request);
                self.set_error(err);
                self.render_explorer(target, false);
            }
        }
        self.wake_caret();
    }

    /// `FenixUserEvent::ExplorerGitStatus`: the second pass, applied on
    /// top of a listing that is already on screen.
    pub(super) fn apply_explorer_git_status(&mut self, target: ExplorerTarget, request: u64, statuses: HashMap<PathBuf, fenix_explorer::GitStatus>) {
        // Checked against the request the *listing* came from: badges
        // computed for one directory must never decorate another.
        if self.explorer_requests.get(&target).map(|r| r.id) != Some(request) {
            return;
        }
        if let Some(state) = self.explorer_state_mut(target) {
            state.apply_git_statuses(statuses);
        }
        self.render_explorer(target, false);
        self.wake_caret();
    }

    /// Whether `request` is still the read `target` is waiting for.
    pub(super) fn explorer_request_is_current(&self, target: ExplorerTarget, request: u64) -> bool {
        let alive = match target {
            ExplorerTarget::Buffer(id) => self.buffers.get(id).is_some(),
            ExplorerTarget::Sidebar => self.sidebar.is_some(),
            ExplorerTarget::Overlay => self.explorer.is_some(),
        };
        alive && self.explorer_requests.get(&target).is_some_and(|r| r.id == request && !r.done)
    }

    pub(super) fn mark_explorer_request_done(&mut self, target: ExplorerTarget, request: u64) {
        if let Some(r) = self.explorer_requests.get_mut(&target) {
            if r.id == request {
                r.done = true;
            }
        }
    }

    /// Which directory `target` is currently waiting on, if the wait has
    /// gone on long enough to be worth mentioning. Below
    /// `SLOW_LISTING_AFTER` this stays `None`, so an ordinary local
    /// directory never flashes a "reading..." line on its way in.
    pub(super) fn explorer_waiting_on(&self, target: ExplorerTarget) -> Option<&Path> {
        let request = self.explorer_requests.get(&target)?;
        (!request.done && request.started.elapsed() >= SLOW_LISTING_AFTER).then_some(request.path.as_path())
    }

    /// Escape on a listing with a read outstanding: stop waiting for it.
    ///
    /// Bumping the id is the whole mechanism -- the thread cannot be
    /// killed and is not asked to be (see `ExplorerRequest`); its result
    /// simply arrives stale and is dropped. Returns whether there was
    /// anything to abandon, so the key falls through when there was not.
    pub(super) fn explorer_abandon_listing(&mut self, target: ExplorerTarget) -> bool {
        let Some(request) = self.explorer_requests.get(&target) else { return false };
        if request.done {
            return false;
        }
        let path = request.path.clone();
        self.explorer_requests.remove(&target);
        self.set_message(format!("stopped waiting for {}", readable_path(&path)));
        self.render_explorer(target, false);
        self.wake_caret();
        true
    }

    /// Records that a directory was actually opened, for `SPC e R`.
    ///
    /// Called where the listing *lands*, not where it is asked for, so
    /// a path that turned out not to exist -- or a share that never
    /// answered -- never joins the list of places you have been.
    pub(super) fn remember_visited_directory(&mut self, path: &Path) {
        self.recent_dirs.add(path.to_path_buf());
        if let Err(err) = self.recent_dirs.save() {
            eprintln!("fenix: couldn't save recent directories ({err})");
        }
    }

    /// `SPC e t`: toggles the sidebar open/closed. It opens on the
    /// file being edited's project, showing the file, and focused (you
    /// just asked to browse); closing always returns focus to the
    /// editor.
    pub(crate) fn toggle_sidebar(&mut self) {
        if self.sidebar_open {
            self.sidebar_open = false;
            self.remember_sidebar();
            self.close_sidebar();
        } else {
            self.open_sidebar();
            self.sidebar_focused = self.sidebar_open;
        }
        self.wake_caret();
    }

    /// Closes the sidebar (and a preview following it).
    fn close_sidebar(&mut self) {
        if self.explorer_preview.is_some_and(|(owner, _)| owner == ExplorerTarget::Sidebar) {
            self.close_explorer_preview();
        }
        self.sidebar_open = false;
        self.sidebar_focused = false;
        self.sidebar = None;
    }

    /// Opens the sidebar on the file being edited's project, showing
    /// the file.
    fn open_sidebar(&mut self) {
        let file = self.followed_file();
        let dir = self.sidebar_root_for(file.as_deref());
        // As it was left in this project: its open folders, its filter,
        // its width.
        let saved = self.explorer_sessions.get(&dir).cloned().unwrap_or_default();
        let folders: Vec<PathBuf> = saved.folders.iter().map(|f| dir.join(f)).collect();
        match fenix_explorer::read_tree(&dir, &folders, false) {
            Ok(listing) => {
                let mut explorer = ExplorerState::pending_at(&dir);
                self.configure_explorer(&mut explorer);
                explorer.apply_listing(listing);
                if !is_unc(&dir) {
                    explorer.apply_git_statuses(fenix_explorer::status_for_dir(&dir));
                }
                explorer.set_filter(&saved.filter);
                if let Some(width) = saved.width {
                    self.sidebar_width = width;
                }
                self.sidebar = Some(explorer);
                self.sidebar_open = true;
                self.explorer_search(ExplorerTarget::Sidebar);
                self.sidebar_view = explorer_page::View::new(explorer_page::Form::Sidebar);
                self.explorer_header_facts(ExplorerTarget::Sidebar);
                self.sidebar_followed = file.clone();
                if let Some(file) = file {
                    self.sidebar_view.current = Some(file.clone());
                    self.explorer_reveal_path(ExplorerTarget::Sidebar, &file);
                }
            }
            Err(err) => self.set_error(format!("couldn't list {} ({err})", dir.display())),
        }
    }

    /// The file the sidebar follows: the focused buffer's, when it's a
    /// file (a page, Home or a scratch buffer leaves it where it is).
    fn followed_file(&self) -> Option<PathBuf> {
        self.open().buffer.path().map(Path::to_path_buf)
    }

    /// Where the sidebar is rooted for `file`: its project, else the
    /// workspace's, else where the explorer starts.
    fn sidebar_root_for(&self, file: Option<&Path>) -> PathBuf {
        file.filter(|f| !is_unc(f))
            .and_then(fenix_project::find_project_root)
            .or_else(|| self.workspaces.active_workspace().project.clone().filter(|root| file.is_none_or(|f| f.starts_with(root))))
            .unwrap_or_else(|| self.explorer_start_dir())
    }

    /// Once a frame: when the file being edited has changed, the
    /// sidebar follows it -- to its project if it's outside the one
    /// shown, then opening the folders down to it and tinting it -- and
    /// leaves the keyboard where it is. Pinned (`explorer.follow` off),
    /// it stays put.
    pub(super) fn sidebar_follow(&mut self) {
        if !self.sidebar_open || self.config.explorer_follow == Some(false) || self.sidebar.is_none() {
            return;
        }
        let Some(file) = self.followed_file() else { return };
        if self.sidebar_followed.as_ref() == Some(&file) {
            return;
        }
        self.sidebar_followed = Some(file.clone());
        self.sidebar_show_file(&file);
    }

    /// The sidebar shows `file`: rooted at its project when it's
    /// outside the folder shown, its folders opened, it selected and
    /// tinted.
    fn sidebar_show_file(&mut self, file: &Path) {
        self.sidebar_view.current = Some(file.to_path_buf());
        let cwd = self.sidebar.as_ref().map(|s| s.cwd.clone()).unwrap_or_default();
        if file.starts_with(&cwd) {
            self.explorer_reveal_path(ExplorerTarget::Sidebar, file);
        } else {
            let root = self.sidebar_root_for(Some(file));
            self.sidebar_move_to(&root);
            self.explorer_reveal_pending.insert(ExplorerTarget::Sidebar, file.to_path_buf());
            self.explorer_navigate(ExplorerTarget::Sidebar, &root);
        }
    }

    /// The sidebar is about to move to `root`: how it was here is kept,
    /// and how it was left there -- its filter, its width -- comes back
    /// (its open folders are read with it, `explorer_navigate`).
    fn sidebar_move_to(&mut self, root: &Path) {
        self.remember_sidebar();
        let saved = self.explorer_sessions.get(root).cloned().unwrap_or_default();
        if let Some(width) = saved.width {
            self.sidebar_width = width;
        }
        if let Some(state) = self.sidebar.as_mut() {
            state.set_filter(&saved.filter);
        }
    }

    // -- Each project's explorer, as it was left --------------------------

    /// The sidebar as it is now, kept for the folder it's rooted at.
    pub(super) fn remember_sidebar(&mut self) {
        let Some(state) = &self.sidebar else { return };
        let root = state.cwd.clone();
        let session = ExplorerSession {
            open: self.sidebar_open,
            width: Some(self.sidebar_width),
            folders: state.open_folders().iter().filter_map(|f| f.strip_prefix(&root).ok().map(Path::to_path_buf)).collect(),
            filter: state.filter.clone(),
        };
        if self.explorer_sessions.get(&root) != Some(&session) {
            self.explorer_sessions.insert(root, session);
            self.explorer_sessions_saved = false;
        }
    }

    /// Writes each project's explorer to the disk, if it changed.
    pub(super) fn save_explorer_sessions(&mut self) {
        self.remember_sidebar();
        if self.explorer_sessions_saved {
            return;
        }
        let path = self.config.state_dir().join(SESSIONS_FILE);
        match fenix_storage::state::write(&path, "projects", &self.explorer_sessions) {
            Ok(()) => self.explorer_sessions_saved = true,
            Err(err) => eprintln!("fenix: couldn't save the explorer's state ({err})"),
        }
    }

    /// The folders `target` opens when it moves to `path`: the sidebar's
    /// as they were left in that project.
    fn saved_open_folders(&self, target: ExplorerTarget, path: &Path) -> Vec<PathBuf> {
        if target != ExplorerTarget::Sidebar {
            return Vec::new();
        }
        self.explorer_sessions.get(path).map(|s| s.folders.iter().map(|f| path.join(f)).collect()).unwrap_or_default()
    }

    /// The focused project changed to `root`: the sidebar is open or
    /// closed as it was left in that project.
    pub(super) fn explorer_project_switched(&mut self, root: &Path) {
        let Some(saved) = self.explorer_sessions.get(root).cloned() else { return };
        if saved.open && !self.sidebar_open {
            self.open_sidebar();
        } else if !saved.open && self.sidebar_open && self.sidebar.as_ref().is_some_and(|s| s.cwd != root) {
            self.remember_sidebar();
            self.close_sidebar();
        }
    }

    // -- Settings ---------------------------------------------------------

    /// `state` as the settings say: names hidden, compact folders,
    /// hidden files listed or not.
    pub(super) fn configure_explorer(&self, state: &mut ExplorerState) {
        state.set_hide(&self.config.explorer_hide);
        state.set_compact(self.config.explorer_compact != Some(false));
        state.show_hidden = self.config.explorer_show_hidden == Some(true);
    }

    /// `explorer.hide`, `explorer.compact` or `explorer.show_hidden`
    /// changed: every listing takes it -- one that lists hidden files
    /// now, or no longer, is read again.
    pub(super) fn reconfigure_explorers(&mut self) {
        let (hide, compact) = (self.config.explorer_hide.clone(), self.config.explorer_compact != Some(false));
        let show_hidden = self.config.explorer_show_hidden == Some(true);
        let mut targets: Vec<ExplorerTarget> = self.dired_states.keys().map(|&id| ExplorerTarget::Buffer(id)).collect();
        targets.extend([ExplorerTarget::Sidebar, ExplorerTarget::Overlay]);
        let mut reread = Vec::new();
        for target in targets.clone() {
            let Some(state) = self.explorer_state_mut(target) else { continue };
            state.set_hide(&hide);
            state.set_compact(compact);
            if state.show_hidden != show_hidden {
                state.show_hidden = show_hidden;
                reread.push((target, state.cwd.clone()));
            }
        }
        for target in targets {
            self.render_explorer(target, false);
        }
        for (target, cwd) in reread {
            self.explorer_navigate(target, &cwd);
        }
    }

    /// `.` in the explorer, `SPC e h`: hidden files listed, or not --
    /// in every explorer, and remembered (`explorer.show_hidden`).
    pub(crate) fn explorer_toggle_hidden(&mut self) {
        let shown = self.config.explorer_show_hidden == Some(true);
        if let Err(err) = self.set_setting("explorer.show_hidden", Some(fenix_config::Value::Bool(!shown))) {
            self.set_error(format!("couldn't keep the setting: {err}"));
            return;
        }
        self.set_message(if shown { "hidden files hidden" } else { "hidden files shown" });
    }

    /// `SPC e H`: a listing of your home folder, as a page in the
    /// focused pane.
    pub(crate) fn explorer_home(&mut self) {
        match dirs::home_dir() {
            Some(home) => self.open_dired_at(&home),
            None => self.set_error("no home folder (USERPROFILE / HOME isn't set)"),
        }
    }

    /// The sidebar's edge was let go: its width is the setting now, and
    /// this project's.
    pub(super) fn sidebar_width_dragged(&mut self) {
        let width = self.sidebar_width.round() as i64;
        if let Err(err) = self.set_setting("explorer.width", Some(fenix_config::Value::Int(width))) {
            self.set_error(err);
        }
        self.remember_sidebar();
    }

    /// `SPC e r`: the file being edited, in the sidebar -- opened if it
    /// isn't, followed there even when pinned -- and the keyboard to it.
    pub(crate) fn sidebar_reveal_current(&mut self) {
        let file = self.followed_file();
        if !self.sidebar_open {
            self.open_sidebar();
        } else if let Some(file) = &file {
            self.sidebar_followed = Some(file.clone());
            self.sidebar_show_file(file);
        }
        if self.sidebar_open {
            self.sidebar_focused = true;
            self.terminal_focused = false;
        }
        if file.is_none() {
            self.set_message("no file to show -- the sidebar shows the project");
        }
        self.wake_caret();
    }

    /// Opens the folders down to `path` in `target`'s listing and
    /// selects it. The folders are read here: they're local and few, and
    /// a path on a share is left alone rather than read on the UI
    /// thread. Whether it has a row now.
    pub(super) fn explorer_reveal_path(&mut self, target: ExplorerTarget, path: &Path) -> bool {
        if is_unc(path) {
            return false;
        }
        let Some(state) = self.explorer_state(target) else { return false };
        let show_hidden = state.show_hidden;
        let folders: Vec<(PathBuf, Vec<fenix_explorer::Entry>)> = state
            .folders_to_reveal(path)
            .into_iter()
            .map_while(|dir| fenix_explorer::read_children(&dir, show_hidden).ok().map(|children| (dir, children)))
            .collect();
        let shown = self.explorer_state_mut(target).is_some_and(|state| state.reveal(path, folders));
        self.render_explorer(target, false);
        shown
    }

    // -- The preview -------------------------------------------------------

    /// `p`: a preview of the selected row in a pane beside the listing,
    /// following the selection; again to close it. The keyboard stays
    /// with the listing.
    pub(super) fn toggle_explorer_preview(&mut self) {
        if self.explorer_preview.is_some() {
            self.close_explorer_preview();
            return;
        }
        let Some(target) = self.active_explorer_target() else { return };
        let back = self.focused_pane_id();
        let sidebar = self.sidebar_focused;
        self.split_vertical();
        let id = self.open_page_with(|_| PageModel::Preview(Box::default()));
        self.windows_mut().focus(back);
        self.sidebar_focused = sidebar;
        self.explorer_preview = Some((target, id));
        self.preview_follow(target);
    }

    /// The pane showing `buffer`, if one does.
    pub(super) fn pane_showing(&self, buffer: BufferId) -> Option<fenix_window::WindowId> {
        self.windows().windows().into_iter().find(|&p| self.windows().content(p) == Some(&buffer))
    }

    /// Closes the preview and the pane it was in.
    pub(super) fn close_explorer_preview(&mut self) {
        let Some((_, id)) = self.explorer_preview.take() else { return };
        let back = self.focused_pane_id();
        if let Some(pane) = self.pane_showing(id).filter(|_| self.windows().window_count() > 1) {
            self.windows_mut().focus(pane);
            self.close_window();
            if pane != back {
                self.windows_mut().focus(back);
            }
        }
        self.close_page_quietly(id);
        self.wake_caret();
    }

    /// The preview shows `target`'s selected row, if it's `target`'s
    /// and that has changed.
    pub(super) fn preview_follow(&mut self, target: ExplorerTarget) {
        let Some((owner, id)) = self.explorer_preview else { return };
        if owner != target {
            return;
        }
        let path = self.explorer_state(target).and_then(|s| s.selected_entry()).map(|e| e.path.clone());
        let shown = match self.pages.get(&id).map(|s| &s.model) {
            Some(PageModel::Preview(p)) => p.path.clone(),
            _ => return,
        };
        if path == shown {
            return;
        }
        let page = match &path {
            Some(path) => self.read_preview(path),
            None => crate::preview_page::PreviewPage::default(),
        };
        if let Some(PageModel::Preview(p)) = self.pages.get_mut(&id).map(|s| &mut s.model) {
            **p = page;
        }
        self.restale_page(id);
    }

    /// What the preview shows for `path`, read from the disk: a folder's
    /// facts and contents, a picture, a diagram (when an extension draws
    /// them), or a file's first lines, highlighted. Nothing is read
    /// across a share.
    fn read_preview(&mut self, path: &Path) -> crate::preview_page::PreviewPage {
        use crate::preview_page::{Content, PreviewPage};
        let mut page = PreviewPage { path: Some(path.to_path_buf()), ..Default::default() };
        if is_unc(path) {
            page.content = Content::Remote;
            return page;
        }
        let metadata = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(err) => {
                page.content = Content::Unreadable(err.to_string());
                return page;
            }
        };
        page.modified = metadata.modified().ok();
        if metadata.is_dir() {
            page.content = match fenix_explorer::read_children(path, false) {
                Ok(children) => Content::Folder {
                    folders: children.iter().filter(|c| c.is_dir()).count(),
                    files: children.iter().filter(|c| !c.is_dir()).count(),
                    bytes: children.iter().filter(|c| !c.is_dir()).map(|c| c.size).sum(),
                    names: children.iter().map(|c| (c.name.clone(), c.is_dir())).collect(),
                },
                Err(err) => Content::Unreadable(err.to_string()),
            };
            return page;
        }
        page.size = Some(metadata.len());
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if PICTURES.contains(&ext.as_str()) {
            page.content = Content::Picture;
            return page;
        }
        let bytes = match std::fs::File::open(path).and_then(|f| {
            use std::io::Read;
            let mut head = Vec::new();
            f.take(PREVIEW_BYTES as u64).read_to_end(&mut head).map(|_| head)
        }) {
            Ok(bytes) => bytes,
            Err(err) => {
                page.content = Content::Unreadable(err.to_string());
                return page;
            }
        };
        let text = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            // Cut through a character at the end of what was read.
            Err(err) if err.error_len().is_none() => std::str::from_utf8(&bytes[..err.valid_up_to()]).unwrap_or_default(),
            Err(_) => "\0",
        };
        if text.contains('\0') {
            page.content = Content::Binary;
            return page;
        }
        // A diagram, drawn by the extension that draws them.
        if matches!(ext.as_str(), "mmd" | "mermaid") && self.diagram_server().is_some() {
            let key = self.diagram_key_of(text);
            self.ask_diagrams(vec![text.to_string()]);
            page.content = Content::Diagram(key);
            return page;
        }
        let cut = bytes.len() >= PREVIEW_BYTES || text.lines().count() > PREVIEW_LINES;
        let text: String = text.lines().take(PREVIEW_LINES).collect::<Vec<_>>().join("\n");
        let language = fenix_syntax::detect_language_from_path(path);
        let colours = language.map(|l| super::reading_host::highlight_lines(l, &text)).unwrap_or_default();
        // A tab is one column here, as the colours count it.
        let lines = text.lines().map(|l| l.replace('\t', " ")).collect();
        page.content = Content::Text { lines, colours, language: language.map(|l| format!("{l:?}")), cut };
        page
    }

    /// `P`: the selected file in the editor's preview tab, in the pane
    /// beside the listing -- the preview's, which it replaces, else
    /// another pane, else a new one. The keyboard stays with the
    /// listing.
    pub(super) fn explorer_open_preview_tab(&mut self) {
        let target = self.explorer_preview.map(|(t, _)| t).or_else(|| self.active_explorer_target());
        let Some(target) = target else { return };
        let Some(entry) = self.explorer_state(target).and_then(|s| s.selected_entry()) else { return };
        if entry.is_dir() {
            self.set_message("a folder -- Enter goes into it");
            return;
        }
        let path = entry.path.clone();
        let sidebar = self.sidebar_focused;
        let listing_pane = self.explorer_page_id(target).and_then(|id| self.pane_showing(id));
        let back = listing_pane.unwrap_or_else(|| self.focused_pane_id());
        let preview = self.explorer_preview.take();
        let beside = match preview.and_then(|(_, id)| self.pane_showing(id).map(|pane| (pane, id))) {
            Some((pane, id)) => {
                self.windows_mut().focus(pane);
                self.open_file_as_preview(&path);
                self.close_page_quietly(id);
                pane
            }
            None if target == ExplorerTarget::Sidebar => {
                self.open_file_as_preview(&path);
                back
            }
            None => {
                match self.windows().windows().into_iter().find(|&p| p != back) {
                    Some(other) => {
                        self.windows_mut().focus(other);
                    }
                    None => self.split_vertical(),
                }
                self.open_file_as_preview(&path);
                self.focused_pane_id()
            }
        };
        if beside != back {
            self.windows_mut().focus(back);
        }
        self.sidebar_focused = sidebar;
        self.wake_caret();
    }

    /// `J`/`K`: the preview scrolls.
    fn explorer_scroll_preview(&mut self, lines: isize) {
        let Some((_, id)) = self.explorer_preview else {
            self.set_message("no preview -- p opens one");
            return;
        };
        if let Some(PageModel::Preview(p)) = self.pages.get_mut(&id).map(|s| &mut s.model) {
            p.scroll_by(lines);
        }
        self.restale_page(id);
    }

    // -- The mouse --------------------------------------------------------

    /// A click on page line `line`, column `col` of `target`'s listing:
    /// a breadcrumb segment goes to its folder, a chevron opens or
    /// closes its folder, a row is selected -- and a second click on it
    /// opens it.
    pub(super) fn explorer_click(&mut self, target: ExplorerTarget, line: usize, col: usize) {
        let Some(view) = self.explorer_view_mut(target) else { return };
        if line == 0 {
            if let Some(path) = view.crumb_at(col).map(Path::to_path_buf) {
                self.explorer_navigate(target, &path);
            }
            return;
        }
        let Some(row) = view.row_at(line) else { return };
        let chevron = view.on_chevron(line, col);
        let now = Instant::now();
        let double = self.explorer_last_click.is_some_and(|(at, t, r)| t == target && r == row && now.duration_since(at) < DOUBLE_CLICK);
        self.explorer_last_click = (!double).then_some((now, target, row));
        if let Some(state) = self.explorer_state_mut(target) {
            state.select_index(row);
        }
        if chevron {
            self.explorer_handle_action(ExplorerAction::ToggleExpand);
        } else if double {
            self.explorer_handle_action(ExplorerAction::Open);
        } else {
            self.render_explorer(target, false);
        }
    }

    /// The wheel over `target`'s listing: its rows scroll, the
    /// selection stays.
    pub(super) fn explorer_wheel(&mut self, target: ExplorerTarget, lines: isize) {
        if let Some(view) = self.explorer_view_mut(target) {
            view.scroll_by(-lines);
        }
        self.render_explorer(target, false);
    }

    /// The listing a pane is showing, if it's showing one.
    pub(super) fn explorer_in_pane(&self, pane: fenix_window::WindowId) -> Option<ExplorerTarget> {
        match self.pages.get(self.windows().content(pane)?).map(|s| &s.model) {
            Some(PageModel::Explorer(target, _)) => Some(*target),
            _ => None,
        }
    }

    /// Whether `pos` is on the sidebar's edge, where dragging resizes it.
    pub(super) fn on_sidebar_edge(&self, pos: (f32, f32)) -> bool {
        self.sidebar_open && self.main_view == MainView::Editor && (pos.0 - self.sidebar_width).abs() <= SIDEBAR_EDGE
    }

    /// The sidebar's edge dragged to `x`, in a window `window_width`
    /// wide.
    pub(super) fn drag_sidebar_to(&mut self, x: f32, window_width: f32) {
        let max = (window_width * text::SIDEBAR_MAX_SHARE).max(text::SIDEBAR_MIN_WIDTH);
        self.sidebar_width = x.clamp(text::SIDEBAR_MIN_WIDTH, max).round();
        self.wake_caret();
    }

    /// The sidebar's rows, laid out `cols` wide and `rows` tall by the
    /// explorer page, which of them is selected, which are tinted
    /// (marked, or the file being edited), and what's underlined.
    pub(super) fn sidebar_rows(&mut self, cols: usize, rows: usize) -> Option<SidebarRows> {
        self.sidebar.as_ref()?;
        self.sidebar_view.waiting_on = self.explorer_waiting_on(ExplorerTarget::Sidebar).map(Path::to_path_buf);
        let page = explorer_page::layout(self.sidebar.as_ref(), &mut self.sidebar_view, cols, Some(rows));
        let focus = page.focus.as_ref().map(|(line, _)| *line);
        let tinted = page.panels.iter().map(|(line, _)| *line).collect();
        let underlines = page.underlines.iter().map(|(line, cols, _)| (*line, cols.start, cols.end)).collect();
        Some((pages::page_rich_spans(&page, self.theme), focus, tinted, underlines))
    }

    /// The listing currently receiving input (see
    /// `active_explorer_target`), if any.
    pub(super) fn active_explorer(&self) -> Option<&ExplorerState> {
        self.explorer_state(self.active_explorer_target()?)
    }

    pub(super) fn active_explorer_mut(&mut self) -> Option<&mut ExplorerState> {
        let target = self.active_explorer_target()?;
        self.explorer_state_mut(target)
    }

    /// The listing page (`SPC f j`) the focused pane is showing, if it
    /// is showing one.
    pub(super) fn focused_dired_buffer(&self) -> Option<BufferId> {
        let id = *self.windows().content(self.focused_pane_id())?;
        self.dired_states.contains_key(&id).then_some(id)
    }

    /// Starts typing a name in place in the active listing's row (or
    /// says there's nothing to rename).
    fn explorer_begin_edit(&mut self, edit: Option<explorer_page::RowEdit>) {
        let Some(edit) = edit else {
            self.set_message("nothing here to rename");
            return;
        };
        let Some(target) = self.active_explorer_target() else { return };
        if let Some(view) = self.explorer_view_mut(target) {
            view.edit = Some(edit);
        }
    }

    /// Whether `target`'s listing has a name being typed in it.
    pub(super) fn explorer_editing(&self, target: ExplorerTarget) -> bool {
        match target {
            ExplorerTarget::Sidebar => self.sidebar_view.edit.is_some(),
            _ => matches!(self.explorer_page_id(target).and_then(|id| self.pages.get(&id)).map(|s| &s.model), Some(PageModel::Explorer(_, v)) if v.edit.is_some()),
        }
    }

    /// A key while a name is typed in a row: everything goes to it --
    /// Space and `:` included -- until Enter does what it's for or
    /// Escape gives it up.
    fn explorer_edit_key(&mut self, target: ExplorerTarget, keypress: KeyPress) {
        if keypress == KeyPress::char('v').with_ctrl() {
            if let Some(text) = self.clipboard_text() {
                if let Some(edit) = self.explorer_view_mut(target).and_then(|v| v.edit.as_mut()) {
                    edit.paste(&text);
                }
            }
            self.render_explorer(target, false);
            return;
        }
        let key = match (keypress.code, keypress.mods.ctrl) {
            (KeyCode::Named(FenixNamedKey::Backspace), true) | (KeyCode::Char('w'), true) => PageKey::DeleteWordBack,
            (KeyCode::Named(FenixNamedKey::Left), true) => PageKey::WordLeft,
            (KeyCode::Named(FenixNamedKey::Right), true) => PageKey::WordRight,
            (_, true) => return,
            (KeyCode::Char(c), false) => PageKey::Char(c),
            (KeyCode::Named(FenixNamedKey::Enter), _) => PageKey::Enter,
            (KeyCode::Named(FenixNamedKey::Escape), _) => PageKey::Escape,
            (KeyCode::Named(FenixNamedKey::Backspace), _) => PageKey::Backspace,
            (KeyCode::Named(FenixNamedKey::Delete), _) => PageKey::Delete,
            (KeyCode::Named(FenixNamedKey::Left), _) => PageKey::Left,
            (KeyCode::Named(FenixNamedKey::Right), _) => PageKey::Right,
            (KeyCode::Named(FenixNamedKey::Home), _) => PageKey::Home,
            (KeyCode::Named(FenixNamedKey::End), _) => PageKey::End,
            _ => return,
        };
        let Some(edit) = self.explorer_view_mut(target).and_then(|v| v.edit.as_mut()) else { return };
        match edit.key(key) {
            explorer_page::EditStep::Typing => {}
            explorer_page::EditStep::Cancel => {
                if let Some(view) = self.explorer_view_mut(target) {
                    view.edit = None;
                }
            }
            explorer_page::EditStep::Done(name) => {
                let edit = edit.clone();
                self.explorer_finish_edit(target, &edit, &name);
            }
        }
        self.render_explorer(target, false);
    }

    /// Enter on a name typed in a row: the file or folder made (a `/` in
    /// the name makes the folders on the way), or the row renamed. The
    /// listing catches up with the new name selected. A name that can't
    /// be used is said, and stays to fix.
    fn explorer_finish_edit(&mut self, target: ExplorerTarget, edit: &explorer_page::RowEdit, name: &str) {
        use explorer_page::EditKind;
        if name.is_empty() {
            if let Some(view) = self.explorer_view_mut(target) {
                view.edit = None;
            }
            return;
        }
        let made = match edit.kind {
            EditKind::NewFile => fenix_explorer::create_file_in(&edit.dir, name),
            EditKind::NewFolder => fenix_explorer::create_dir_in(&edit.dir, name),
            EditKind::Rename => match self.explorer_state(target).map(|s| s.rename_selected(name)) {
                Some(result) => result,
                None => return,
            },
        };
        match made {
            Ok(path) => {
                if let Some(view) = self.explorer_view_mut(target) {
                    view.edit = None;
                }
                let verb = if edit.kind == EditKind::Rename { "renamed" } else { "created" };
                self.set_message(format!("{verb} {}", readable_path(&path)));
                self.explorer_reveal_pending.insert(target, path);
                if let Some(cwd) = self.explorer_state(target).map(|s| s.cwd.clone()) {
                    self.explorer_navigate(target, &cwd);
                }
            }
            Err(err) => self.set_error(err.to_string()),
        }
    }

    /// A key for an explorer page or the sidebar: its key table's. The
    /// leader and `:` still reach Vim while no sequence of the table's
    /// is under way, as from any page. Whether the key was taken.
    pub(super) fn explorer_page_key(&mut self, target: ExplorerTarget, keypress: KeyPress) -> bool {
        if self.explorer_editing(target) {
            self.explorer_edit_key(target, keypress);
            self.wake_caret();
            return true;
        }
        // A modifier is the editor's (Ctrl-W and friends).
        if keypress.mods.ctrl || keypress.mods.alt || keypress.mods.super_ {
            return false;
        }
        if !self.explorer_matcher.is_pending() && matches!(keypress.code, KeyCode::Char(' ' | ':')) {
            return false;
        }
        // Escape stops waiting for a directory that isn't answering,
        // then clears a filter, before it means quit.
        if keypress.code == KeyCode::Named(FenixNamedKey::Escape) && !self.explorer_matcher.is_pending() {
            if self.explorer_abandon_listing(target) {
                return true;
            }
            if self.explorer_state(target).is_some_and(|s| !s.filter.is_empty()) {
                self.explorer_set_filter("");
                self.wake_caret();
                return true;
            }
            // A pane's listing is closed with `q`, as a page is; Escape
            // there has nothing left to stop.
            if matches!(target, ExplorerTarget::Buffer(_)) {
                return true;
            }
        }
        if let Step::Matched(&action) = self.explorer_matcher.feed(keypress) {
            self.explorer_handle_action(action);
        }
        self.wake_caret();
        true
    }

    /// Dispatches one explorer command against whichever listing is
    /// active. Re-fetches `active_explorer_mut()` fresh per arm rather
    /// than binding it once, since a few arms (`Open`, `ParentDir`,
    /// `Quit`) need to touch other `self` fields too (replacing the
    /// buffer, restoring the stash) and can't do that while still
    /// holding it borrowed.
    pub(super) fn explorer_handle_action(&mut self, action: ExplorerAction) {
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
            ExplorerAction::First => self.active_explorer_mut().unwrap().select_index(0),
            ExplorerAction::Last => {
                let explorer = self.active_explorer_mut().unwrap();
                explorer.select_index(explorer.entries.len().saturating_sub(1));
            }
            ExplorerAction::Help => {
                let target = self.active_explorer_target();
                if let Some(view) = target.and_then(|t| self.explorer_view_mut(t)) {
                    view.help = !view.help;
                }
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
            ExplorerAction::BeginFilter => {
                let seed = self.active_explorer().map(|e| e.filter.clone()).unwrap_or_default();
                self.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::Filter, input: seed });
            }
            ExplorerAction::BeginFind => self.explorer_find_under_here(),
            ExplorerAction::TogglePreview => self.toggle_explorer_preview(),
            ExplorerAction::OpenPreviewTab => self.explorer_open_preview_tab(),
            ExplorerAction::PreviewDown => self.explorer_scroll_preview(3),
            ExplorerAction::PreviewUp => self.explorer_scroll_preview(-3),
            ExplorerAction::NextMatch | ExplorerAction::PrevMatch => {
                let explorer = self.active_explorer_mut().unwrap();
                if explorer.filter.is_empty() || !explorer.next_match(action == ExplorerAction::NextMatch) {
                    self.set_message("no filter matches -- / filters");
                }
            }
            ExplorerAction::BeginArchive => self.begin_archive(),
            ExplorerAction::ExtractArchive => self.extract_selected_archive(),
            ExplorerAction::ShowProperties => self.show_properties(),
            ExplorerAction::ToggleReadOnly => self.toggle_read_only(),
            ExplorerAction::ToggleHidden => self.explorer_toggle_hidden(),
            ExplorerAction::CycleSort => {
                let next = match self.active_explorer().unwrap().sort.key {
                    fenix_explorer::SortKey::Name => fenix_explorer::SortKey::Size,
                    fenix_explorer::SortKey::Size => fenix_explorer::SortKey::Modified,
                    fenix_explorer::SortKey::Modified => fenix_explorer::SortKey::Extension,
                    fenix_explorer::SortKey::Extension => fenix_explorer::SortKey::Name,
                };
                let sort = fenix_explorer::Sort { key: next, ..self.active_explorer().unwrap().sort };
                self.explorer_set_sort(sort);
            }
            ExplorerAction::ReverseSort => {
                let sort = self.active_explorer().unwrap().sort;
                self.explorer_set_sort(fenix_explorer::Sort { descending: !sort.descending, ..sort });
            }
            ExplorerAction::Refresh => self.explorer_relist(),
            ExplorerAction::ParentDir => {
                let parent = self.active_explorer().unwrap().cwd.parent().map(Path::to_path_buf);
                if let Some(parent) = parent {
                    self.explorer_go_to(&parent);
                }
            }
            ExplorerAction::Open => self.explorer_open_selected(),
            ExplorerAction::BeginDelete => {
                if !self.active_explorer().unwrap().targets().is_empty() {
                    self.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::ConfirmDelete, input: String::new() });
                }
            }
            ExplorerAction::BeginRename => {
                let edit = explorer_page::RowEdit::rename(self.active_explorer().unwrap());
                self.explorer_begin_edit(edit);
            }
            ExplorerAction::BeginCreateFile => {
                let edit = explorer_page::RowEdit::new(explorer_page::EditKind::NewFile, self.active_explorer().unwrap());
                self.explorer_begin_edit(Some(edit));
            }
            ExplorerAction::BeginCreateDir => {
                let edit = explorer_page::RowEdit::new(explorer_page::EditKind::NewFolder, self.active_explorer().unwrap());
                self.explorer_begin_edit(Some(edit));
            }
            ExplorerAction::BeginCopy => {
                if !self.active_explorer().unwrap().targets().is_empty() {
                    let seed = self.explorer_other_cwd().map(|p| p.display().to_string()).unwrap_or_default();
                    self.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::CopyTo, input: seed });
                }
            }
            ExplorerAction::BeginMove => {
                if !self.active_explorer().unwrap().targets().is_empty() {
                    let seed = self.explorer_other_cwd().map(|p| p.display().to_string()).unwrap_or_default();
                    self.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::MoveTo, input: seed });
                }
            }
            ExplorerAction::Quit => self.explorer_quit(),
            ExplorerAction::SelectCwd => {
                let picking = self.picking_folder();
                if picking && self.explorer_purpose == ExplorerPurpose::PickProjectDir {
                    let cwd = self.active_explorer().unwrap().cwd.clone();
                    self.register_project_dir(&cwd);
                } else if picking && self.explorer_purpose == ExplorerPurpose::FindFrom {
                    let cwd = self.active_explorer().unwrap().cwd.clone();
                    self.start_find_from_here(&cwd);
                } else if picking && self.explorer_purpose == ExplorerPurpose::PickWizardParent {
                    let cwd = self.active_explorer().unwrap().cwd.clone();
                    self.wizard_parent_picked(&cwd);
                } else if picking && matches!(self.explorer_purpose, ExplorerPurpose::PickSettingPath { .. }) {
                    // A path setting takes a folder as readily as a file.
                    let cwd = self.active_explorer().unwrap().cwd.clone();
                    self.settings_path_picked(&cwd);
                }
                // No-op during ordinary browsing (`SPC f j`/the sidebar) --
                // `S` only means something while picking a project/MIB dir,
                // or searching from a directory (`SPC f e`).
            }
        }
        // Nearly every action changes what the page shows -- a mark, the
        // selection, a count in the header.
        if let Some(target) = self.active_explorer_target() {
            self.render_explorer(target, false);
        }
        self.wake_caret();
    }

    /// `target`'s page's view: the sidebar's own, or its page's.
    pub(super) fn explorer_view_mut(&mut self, target: ExplorerTarget) -> Option<&mut explorer_page::View> {
        if target == ExplorerTarget::Sidebar {
            return Some(&mut self.sidebar_view);
        }
        match self.explorer_page_id(target).and_then(|id| self.pages.get_mut(&id)).map(|s| &mut s.model) {
            Some(PageModel::Explorer(_, view)) => Some(&mut **view),
            _ => None,
        }
    }

    /// `Enter`/`l` on the entry at point: goes into a folder, or opens a
    /// file -- into the focused pane, from a pane's listing or the
    /// sidebar (which hands the keyboard back to the editor); from the
    /// directory picker, which closes, only when it's finding files.
    /// While picking a folder (`SPC p a`) a file does nothing: there's
    /// nothing sensible to do with one when what's wanted is a folder --
    /// `S` picks the folder shown.
    pub(super) fn explorer_open_selected(&mut self) {
        let Some(target) = self.active_explorer_target() else { return };
        let Some(entry) = self.active_explorer().and_then(|e| e.selected_entry()) else { return };
        let path = entry.path.clone();
        if entry.is_dir() {
            self.explorer_go_to(&path);
            return;
        }
        if target == ExplorerTarget::Overlay {
            match self.explorer_purpose {
                ExplorerPurpose::PickProjectDir | ExplorerPurpose::PickWizardParent | ExplorerPurpose::PickSettingPath { folder: true } => return,
                ExplorerPurpose::PickSettingPath { folder: false } => {
                    self.settings_path_picked(&path);
                    return;
                }
                ExplorerPurpose::Browse | ExplorerPurpose::FindFrom => self.close_folder_picker(),
            }
        }
        if target == ExplorerTarget::Sidebar {
            self.sidebar_focused = false;
        }
        self.open_file_from_picker(&path);
    }

    /// `q`/`Escape`: the directory picker closes, the sidebar hands the
    /// keyboard back to the editor, and a pane's listing closes as any
    /// page does.
    pub(super) fn explorer_quit(&mut self) {
        match self.active_explorer_target() {
            Some(ExplorerTarget::Overlay) => self.close_folder_picker(),
            Some(ExplorerTarget::Sidebar) => self.sidebar_focused = false,
            Some(ExplorerTarget::Buffer(id)) => self.close_page_quietly(id),
            None => {}
        }
    }

    /// Routes one keypress to whichever explorer prompt is active: a
    /// bare y/n for delete confirmation, or free-text input (with
    /// Backspace/Enter/Escape) for rename/create/copy/move.
    pub(super) fn explorer_prompt_key(&mut self, key: KeyPress) {
        // A pending collision owns the keyboard until it is answered:
        // the operation is half-specified until then, and letting other
        // keys through would leave it in that state indefinitely.
        if self.explorer_conflict.is_some() {
            self.explorer_conflict_key(key);
            return;
        }

        let Some(prompt) = &mut self.explorer_prompt else { return };

        if prompt.kind == PromptKind::ConfirmDelete {
            if key.code == KeyCode::Char('y') || key.code == KeyCode::Char('Y') {
                self.explorer_prompt = None;
                self.explorer_delete_targets();
            } else {
                self.explorer_prompt = None;
            }
            self.wake_caret();
            return;
        }

        if key == KeyPress::char('v').with_ctrl() {
            let pasted = self.clipboard_text();
            if let (Some(prompt), Some(text)) = (&mut self.explorer_prompt, pasted) {
                prompt.input.push_str(&text);
            }
            self.wake_caret();
            return;
        }

        // Tab completes a path, and only a path -- everywhere else it
        // has no meaning and is better left doing nothing than doing
        // something surprising.
        if key.code == KeyCode::Named(FenixNamedKey::Tab) && prompt.kind == PromptKind::GoToPath {
            self.complete_path_prompt();
            self.wake_caret();
            return;
        }

        match key.code {
            KeyCode::Named(FenixNamedKey::Escape) => {
                // Cancelling a filter widens the listing back out --
                // leaving it narrowed after Escape would be a filter you
                // cannot remember setting.
                let was_filtering = prompt.kind == PromptKind::Filter;
                self.explorer_prompt = None;
                if was_filtering {
                    self.explorer_set_filter("");
                }
            }
            KeyCode::Named(FenixNamedKey::Enter) => {
                let ExplorerPrompt { kind, input } = self.explorer_prompt.take().unwrap();
                self.explorer_completions.clear();
                self.explorer_prompt_submit(kind, &input);
            }
            KeyCode::Named(FenixNamedKey::Backspace) => {
                prompt.input.pop();
                self.refresh_path_completions();
                self.apply_live_filter();
            }
            KeyCode::Char(c) => {
                prompt.input.push(c);
                self.refresh_path_completions();
                self.apply_live_filter();
            }
            _ => {}
        }
        self.wake_caret();
    }

    /// `SPC e p`: opens the path bar, seeded with where you already are
    /// so a nearby directory is an edit rather than a retype.
    pub(crate) fn start_path_prompt(&mut self) {
        let seed = self
            .active_explorer()
            .map(|e| e.cwd.display().to_string())
            .unwrap_or_else(|| self.explorer_start_dir().display().to_string());
        // With a trailing separator, so the first Tab offers what is
        // inside rather than the siblings of where you are.
        let seed = if seed.ends_with('\\') || seed.ends_with('/') { seed } else { format!("{seed}\\") };
        self.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::GoToPath, input: seed });
        self.refresh_path_completions();
        self.wake_caret();
    }

    /// Narrows the listing to whatever is currently typed, if what is
    /// being typed is a filter.
    pub(super) fn apply_live_filter(&mut self) {
        let Some(prompt) = self.explorer_prompt.as_ref().filter(|p| p.kind == PromptKind::Filter) else { return };
        let text = prompt.input.clone();
        self.explorer_set_filter(&text);
    }

    /// Applies a filter to whichever listing is active and re-renders.
    /// The rows already read narrow at once, with no I/O -- which is
    /// what keeps it instant on a share; the rest of the tree is searched
    /// behind it (`explorer_search`) and joins when that answers.
    pub(super) fn explorer_set_filter(&mut self, filter: &str) {
        let Some(target) = self.active_explorer_target() else { return };
        if let Some(explorer) = self.explorer_state_mut(target) {
            explorer.set_filter(filter);
        }
        self.explorer_search(target);
        self.render_explorer(target, false);
    }

    /// Searches everything below `target`'s folder for its filter, off
    /// the UI thread, through the project file index's listing of that
    /// folder -- listed first if it hasn't been, the search running when
    /// it arrives (`explorer_index_arrived`). Not across a share.
    pub(super) fn explorer_search(&mut self, target: ExplorerTarget) {
        let Some(state) = self.explorer_state(target) else { return };
        let (root, needle) = (state.cwd.clone(), state.filter.clone());
        if needle.trim().is_empty() || is_unc(&root) {
            return;
        }
        let Some(files) = self.listed_files(&root) else {
            self.prewarm_file_index(&root);
            return;
        };
        self.explorer_search += 1;
        let search = self.explorer_search;
        let find = move || fenix_explorer::search(files.iter().map(|c| c.payload.as_path()), &root, &needle, SEARCH_LIMIT);
        match self.event_proxy.clone() {
            Some(proxy) => {
                std::thread::spawn(move || {
                    let _ = proxy.send_event(FenixUserEvent::ExplorerFound { target, search, found: find() });
                });
            }
            None => {
                let found = find();
                self.apply_explorer_found(target, search, found);
            }
        }
    }

    /// `FenixUserEvent::ExplorerFound`: what the latest search found,
    /// shown with the rows read.
    pub(super) fn apply_explorer_found(&mut self, target: ExplorerTarget, search: u64, found: Vec<fenix_explorer::Entry>) {
        if search != self.explorer_search {
            return;
        }
        if let Some(state) = self.explorer_state_mut(target).filter(|s| !s.filter.trim().is_empty()) {
            state.set_found(found);
        }
        self.render_explorer(target, false);
        self.wake_caret();
    }

    /// `root`'s files have been listed: a filter waiting on them searches.
    pub(super) fn explorer_index_arrived(&mut self, root: &Path) {
        let mut targets: Vec<ExplorerTarget> = self.dired_states.keys().map(|&id| ExplorerTarget::Buffer(id)).collect();
        targets.extend([ExplorerTarget::Sidebar, ExplorerTarget::Overlay]);
        for target in targets {
            if self.explorer_state(target).is_some_and(|s| s.cwd == root && !s.filter.trim().is_empty()) {
                self.explorer_search(target);
            }
        }
    }

    /// `F` in a listing: fuzzy-find by name through everything under the
    /// directory being browsed, not just its immediate children.
    ///
    /// Reuses the project file picker's own candidate list, rooted here
    /// instead of at a project -- including files `.gitignore` would
    /// hide, because a file manager has no business pretending a build
    /// artifact does not exist.
    pub(super) fn explorer_find_under_here(&mut self) {
        let Some(root) = self.active_explorer().map(|e| e.cwd.clone()) else { return };
        self.open_find_file_with(&root, true, true);
    }

    /// Recomputes what the current text could become. Cheap enough to
    /// redo on every keystroke -- it lists one directory -- and a cache
    /// would be one more thing that can disagree with what is typed.
    pub(super) fn refresh_path_completions(&mut self) {
        let Some(prompt) = self.explorer_prompt.as_ref().filter(|p| p.kind == PromptKind::GoToPath) else {
            self.explorer_completions.clear();
            return;
        };
        self.explorer_completions = fenix_fs::complete(&prompt.input);
    }

    /// Tab: fills in as much as every candidate agrees on.
    ///
    /// Not "cycle through the matches": with one match this finishes the
    /// name, and with several it advances to the point where they
    /// diverge and leaves you to type the next character -- which is
    /// what makes repeated Tabs feel like progress rather than a
    /// carousel.
    pub(super) fn complete_path_prompt(&mut self) {
        let candidates = self.explorer_completions.clone();
        let Some(prefix) = fenix_fs::common_prefix(&candidates) else {
            self.set_message("no directory here starts with that");
            return;
        };
        let Some(prompt) = self.explorer_prompt.as_mut() else { return };
        if prefix.len() > prompt.input.trim().len() {
            prompt.input = prefix;
            // A single match is a directory you can go into, so leave the
            // separator on and let the next Tab list its contents.
            if candidates.len() == 1 {
                prompt.input.push('\\');
            }
            self.refresh_path_completions();
        } else if candidates.len() > 1 {
            let shown: Vec<&str> =
                candidates.iter().filter_map(|c| c.rsplit(['\\', '/']).next()).take(6).collect();
            let more = candidates.len().saturating_sub(shown.len());
            let suffix = if more > 0 { format!("  (+{more} more)") } else { String::new() };
            self.set_message(format!("{}{suffix}", shown.join("  ")));
        }
    }

    /// Opens `target` in the explorer, whatever kind of thing it turns
    /// out to be.
    ///
    /// A file opens in the editor rather than being refused: typing a
    /// full filename into a path bar and being told it is not a
    /// directory is pedantry, not help. A bare `\\server` with no share
    /// asks the server what it has (see `explorer_browse_shares`).
    /// Anything else is a directory, and the explorer's own error path
    /// handles it not existing.
    pub(crate) fn explorer_open_path(&mut self, target: &Path) {
        if target.is_file() {
            self.open_file_from_picker(target);
            return;
        }
        if is_bare_server(target) {
            self.explorer_browse_shares(target);
            return;
        }
        if self.active_explorer_target().is_some() {
            self.explorer_go_to(target);
        } else {
            self.open_dired_at(target);
        }
    }

    /// `\\server` with no share named: ask it what it is offering.
    ///
    /// Off the main thread, and for the same reason listings are --
    /// `net view` against a host that is not there blocks for tens of
    /// seconds on name resolution alone. The answer arrives as a picker;
    /// a failure arrives as a message, and either way the editor was
    /// usable the whole time.
    pub(super) fn explorer_browse_shares(&mut self, server: &Path) {
        let host = server.to_string_lossy().trim_start_matches(['\\', '/']).to_string();
        self.set_message(format!("asking {host} what it shares..."));
        match self.event_proxy.clone() {
            Some(proxy) => {
                std::thread::spawn(move || {
                    let result = fenix_fs::shares(&host).map_err(|err| err.to_string());
                    let _ = proxy.send_event(FenixUserEvent::ExplorerShares { host, result });
                });
            }
            None => {
                let result = fenix_fs::shares(&host).map_err(|err| err.to_string());
                self.apply_explorer_shares(host, result);
            }
        }
        self.wake_caret();
    }

    /// `FenixUserEvent::ExplorerShares`: what the server said.
    pub(super) fn apply_explorer_shares(&mut self, host: String, result: Result<Vec<String>, String>) {
        match result {
            Ok(shares) if shares.is_empty() => self.set_error(format!("{host} is not sharing anything you can see")),
            Ok(shares) => {
                let candidates = shares
                    .into_iter()
                    .map(|share| {
                        let path = PathBuf::from(format!("\\\\{host}\\{share}"));
                        fenix_picker::Candidate::new(format!("\\\\{host}\\{share}"), path)
                    })
                    .collect();
                self.enter_picker(ActivePicker::Places(fenix_picker::PickerState::new(candidates)));
            }
            Err(err) => self.set_error(err),
        }
        self.wake_caret();
    }

    // -- Places ----------------------------------------------------------

    /// Starts the one-off drive query, if it has not run already.
    ///
    /// In the background, and started early -- the first time an
    /// explorer opens rather than the first time Places is asked for.
    /// Asking Windows costs about half a second, which is nothing spread
    /// across a session and far too much to spend between a keypress and
    /// a list appearing. By the time anyone presses `SPC e b` the answer
    /// has been sitting there for minutes.
    pub(super) fn ensure_volumes(&mut self) {
        if !self.volumes.is_empty() || self.volumes_loading {
            return;
        }
        self.volumes_loading = true;
        match self.event_proxy.clone() {
            Some(proxy) => {
                std::thread::spawn(move || {
                    let _ = proxy.send_event(FenixUserEvent::ExplorerVolumes(fenix_fs::volumes()));
                });
            }
            None => {
                // No event loop to report back through (every test).
                self.volumes = fenix_fs::volumes();
                self.volumes_loading = false;
            }
        }
    }

    /// `SPC e b`: everywhere worth going, in one list.
    ///
    /// Bookmarks first because they were chosen deliberately, then the
    /// machine's drives, then where you have actually been, then the
    /// projects you have registered. Ordered by how likely each is to be
    /// what you meant rather than alphabetically, since the whole point
    /// is that the answer is near the top before you type anything.
    pub(crate) fn picker_places(&mut self) {
        self.ensure_volumes();
        let mut candidates: Vec<fenix_picker::Candidate<PathBuf>> = Vec::new();
        for (name, path) in &self.config.explorer_bookmarks {
            candidates.push(fenix_picker::Candidate::new(format!("* {name}  --  {}", readable_path(path)), path.clone()));
        }
        for volume in &self.volumes {
            let mut label = format!("{} {}", readable_path(&volume.root), volume.kind.label());
            if !volume.name.is_empty() {
                label = format!("{} ({})", label, volume.name);
            }
            // A mapped drive shows what it really is: `Z:` on its own
            // tells you nothing about where you are about to go.
            if let Some(remote) = &volume.remote {
                label.push_str(&format!("  ->  {remote}"));
            }
            if let (Some(free), Some(total)) = (volume.free, volume.total) {
                label.push_str(&format!("  --  {} free of {}", format_size(free), format_size(total)));
            }
            candidates.push(fenix_picker::Candidate::new(label, volume.root.clone()));
        }
        for path in self.recent_dirs.paths() {
            candidates.push(fenix_picker::Candidate::new(readable_path(path), path.clone()));
        }
        for root in self.known_projects.roots() {
            candidates.push(fenix_picker::Candidate::new(format!("project: {}", readable_path(root)), root.clone()));
        }
        if candidates.is_empty() {
            self.set_message("nowhere bookmarked yet -- SPC e m remembers where you are");
            return;
        }
        self.enter_picker(ActivePicker::Places(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC e R`: just the directories you have been, without the drives
    /// and bookmarks around them -- for when you know you were there a
    /// moment ago.
    pub(crate) fn picker_recent_dirs(&mut self) {
        if self.recent_dirs.paths().is_empty() {
            self.set_message("no directories visited yet");
            return;
        }
        let candidates = self.recent_dirs.paths().iter().map(|p| fenix_picker::Candidate::new(readable_path(p), p.clone())).collect();
        self.enter_picker(ActivePicker::Places(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC e m`: bookmarks wherever the explorer currently is, named
    /// after the folder itself.
    ///
    /// No prompt for a name. The folder's own name is right almost
    /// every time, and a bookmark you can add without stopping to think
    /// is a bookmark you will actually add.
    pub(crate) fn bookmark_current_directory(&mut self) {
        let Some(cwd) = self.active_explorer().map(|e| e.cwd.clone()) else {
            self.set_error("no directory open to bookmark");
            return;
        };
        if self.config.explorer_bookmarks.iter().any(|(_, path)| path == &cwd) {
            self.set_message(format!("{} is already bookmarked", cwd.display()));
            return;
        }
        let name = cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            // A drive or share root has no file name of its own, so it
            // is named by the whole path -- which is short anyway.
            .unwrap_or_else(|| cwd.display().to_string());
        self.config.explorer_bookmarks.push((name.clone(), cwd));
        if let Err(err) = self.config.save() {
            self.set_error(format!("couldn't save the bookmark: {err}"));
            return;
        }
        self.set_message(format!("bookmarked as {name}"));
    }

    /// One keystroke's answer to "something is already there".
    ///
    /// Four single keys rather than a typed word: this appears in the
    /// middle of an operation the user already asked for, and the whole
    /// point of asking is that it costs almost nothing.
    pub(super) fn explorer_conflict_key(&mut self, key: KeyPress) {
        let on_conflict = self.explorer_conflict_answer(key);
        self.explorer_conflict_menu_closed();
        let Some(on_conflict) = on_conflict else { return };
        let Some(ExplorerConflict { kind, targets, dest, .. }) = self.explorer_conflict.take() else { return };
        self.explorer_transfer(kind, &targets, &dest, on_conflict);
        self.wake_caret();
    }

    /// The menu beside the selected row asking what to do about what's
    /// in the way -- the same question the modeline asks, where the
    /// listing is.
    fn explorer_conflict_menu(&mut self) {
        let Some(conflict) = &self.explorer_conflict else { return };
        let what = match conflict.conflicts.as_slice() {
            [one] => format!("{} is already there", one.file_name().unwrap_or(one.as_os_str()).to_string_lossy()),
            many => format!("{} are already there", many.len()),
        };
        let row = |key: &str, label: &str| vec![(format!(" {key} "), PageRole::Accent), (format!("{label} "), PageRole::Text)];
        let rows = vec![
            vec![(format!(" {what} "), PageRole::Title)],
            row("o", "overwrite"),
            row("s", "skip those"),
            row("b", "keep both"),
            row("Esc", "cancel"),
        ];
        let Some(target) = self.active_explorer_target() else { return };
        if let Some(view) = self.explorer_view_mut(target) {
            view.menu = Some(rows);
        }
        self.render_explorer(target, false);
    }

    /// The conflict is answered: its menu goes.
    fn explorer_conflict_menu_closed(&mut self) {
        let Some(target) = self.active_explorer_target() else { return };
        if let Some(view) = self.explorer_view_mut(target) {
            view.menu = None;
        }
        self.render_explorer(target, false);
    }

    /// What a key answers to the conflict; `None` cancels it.
    fn explorer_conflict_answer(&mut self, key: KeyPress) -> Option<fenix_fs::OnConflict> {
        let on_conflict = match key.code {
            KeyCode::Char('o') | KeyCode::Char('O') => fenix_fs::OnConflict::Overwrite,
            KeyCode::Char('s') | KeyCode::Char('S') => fenix_fs::OnConflict::Skip,
            KeyCode::Char('b') | KeyCode::Char('B') => fenix_fs::OnConflict::KeepBoth,
            // Anything else -- Escape, `c`, a stray keystroke -- cancels.
            // Nothing has been written yet, so cancelling really does
            // leave both sides exactly as they were, and defaulting an
            // unrecognised key to "do nothing" is the only safe reading
            // of it.
            _ => {
                self.explorer_conflict = None;
                self.set_message("cancelled");
                self.wake_caret();
                return None;
            }
        };
        Some(on_conflict)
    }

    /// What to show in place of the modeline while `explorer_prompt` is
    /// active -- previously nothing rendered it at all, so typing a
    /// rename/create/copy/move was invisible (the state was captured
    /// correctly, there was just no on-screen feedback that a prompt was
    /// even open). Mirrors `:`/`/`'s own "the modeline becomes the
    /// prompt" convention.
    pub(super) fn explorer_prompt_text(&self) -> Option<String> {
        if let Some(conflict) = &self.explorer_conflict {
            // Names the first collision and counts the rest: "some files
            // already exist" is not enough to decide on.
            let first = conflict.conflicts.first().map(|p| p.file_name().unwrap_or(p.as_os_str()).to_string_lossy().into_owned());
            let what = match (first, conflict.conflicts.len()) {
                (Some(name), 1) => format!("{name} already exists"),
                (Some(name), n) => format!("{name} and {} more already exist", n - 1),
                (None, n) => format!("{n} already exist"),
            };
            return Some(format!("{what}: [o]verwrite  [s]kip  keep [b]oth  Esc to cancel"));
        }
        let prompt = self.explorer_prompt.as_ref()?;
        Some(match prompt.kind {
            PromptKind::ConfirmDelete => {
                let n = self.active_explorer().map(|e| e.targets().len()).unwrap_or(0);
                let items = format!("{n} item{}", if n == 1 { "" } else { "s" });
                // The two deletes are not the same act and must never
                // read the same: one is undoable from the Recycle Bin,
                // the other is not.
                if self.explorer_delete_permanently {
                    format!("PERMANENTLY delete {items}? This cannot be undone. (y/n)")
                } else {
                    format!("Move {items} to the Recycle Bin? (y/n)")
                }
            }
            PromptKind::GoToPath => format!("Go to: {}", prompt.input),
            PromptKind::ArchiveTo => format!("Archive as: {}", prompt.input),
            PromptKind::Filter => format!("Filter: {}", prompt.input),
            PromptKind::CopyTo => format!("Copy to: {}", prompt.input),
            PromptKind::MoveTo => format!("Move to: {}", prompt.input),
        })
    }

    pub(super) fn explorer_prompt_submit(&mut self, kind: PromptKind, input: &str) {
        // The path bar is also available from documents and the dashboard,
        // where there is no listing yet to supply an active explorer.
        if kind == PromptKind::GoToPath {
            self.explorer_open_path(&fenix_fs::expand(input));
            return;
        }
        let Some(explorer) = self.active_explorer() else { return };
        let outcomes = match kind {
            PromptKind::GoToPath => unreachable!("handled before requiring a listing"),
            PromptKind::ArchiveTo => {
                let sources = explorer.target_paths();
                let cwd = explorer.cwd.clone();
                // A bare name means "here"; a path means where it says.
                let typed = fenix_fs::expand(input);
                let archive = if typed.is_absolute() { typed } else { cwd.join(input.trim()) };
                vec![fenix_fs::create_archive(&sources, &archive).map_err(|e| e.to_string())]
            }
            // Copy and move ask about collisions *before* touching
            // anything -- the explorer this replaces overwrote silently.
            PromptKind::CopyTo | PromptKind::MoveTo => {
                let targets = explorer.target_paths();
                let dest = PathBuf::from(input);
                let conflicts = fenix_fs::conflicts_in(&targets, &dest);
                if !conflicts.is_empty() {
                    self.explorer_conflict = Some(ExplorerConflict { kind, targets, dest, conflicts });
                    self.explorer_conflict_menu();
                    self.wake_caret();
                    return;
                }
                self.explorer_transfer(kind, &targets, &dest, fenix_fs::OnConflict::KeepBoth);
                return;
            }
            // Already applied on every keystroke -- Enter just closes
            // the prompt and leaves the listing narrowed.
            PromptKind::Filter => return,
            PromptKind::ConfirmDelete => return, // handled in explorer_prompt_key via y/n, not Enter
        };
        self.report_explorer_outcomes(kind.past_tense(), outcomes);
        self.explorer_relist();
    }

    /// Runs a copy or a move now that its collisions have been settled,
    /// reporting each path's fate rather than only the first failure --
    /// a batch where two of ten files failed should say so.
    pub(super) fn explorer_transfer(&mut self, kind: PromptKind, targets: &[PathBuf], dest: &Path, on_conflict: fenix_fs::OnConflict) {
        if self.explorer_job.is_some() {
            self.set_error("something is already being copied -- SPC e k stops it");
            return;
        }
        let (running, done) = if kind == PromptKind::MoveTo { ("moving", "moved") } else { ("copying", "copied") };
        let cancel = Arc::new(AtomicBool::new(false));
        self.explorer_job = Some(ExplorerJob { running, done, cancel: cancel.clone(), progress: fenix_fs::Progress::default() });

        let sources = targets.to_vec();
        let dest = dest.to_path_buf();
        let is_move = kind == PromptKind::MoveTo;
        match self.event_proxy.clone() {
            Some(proxy) => {
                std::thread::spawn(move || {
                    let stop = || cancel.load(Ordering::Relaxed);
                    let reporter = proxy.clone();
                    // One event per file. A lot of them for a big tree,
                    // but they are cheap, and per-*item* progress does
                    // not move at all while a single large directory
                    // copies -- which is exactly when someone is
                    // watching it.
                    let mut report = |p: &fenix_fs::Progress| {
                        let _ = reporter.send_event(FenixUserEvent::ExplorerJobProgress(p.clone()));
                    };
                    let outcomes = if is_move {
                        fenix_fs::move_into_reporting(&sources, &dest, on_conflict, &stop, &mut report)
                    } else {
                        fenix_fs::copy_into_reporting(&sources, &dest, on_conflict, &stop, &mut report)
                    };
                    let _ = proxy.send_event(FenixUserEvent::ExplorerJobDone(outcomes));
                });
            }
            None => {
                // No event loop to report back through (every test) --
                // run it inline and finish immediately.
                let stop = || cancel.load(Ordering::Relaxed);
                let outcomes = if is_move {
                    fenix_fs::move_into_reporting(&sources, &dest, on_conflict, &stop, &mut |_| {})
                } else {
                    fenix_fs::copy_into_reporting(&sources, &dest, on_conflict, &stop, &mut |_| {})
                };
                self.finish_explorer_job(outcomes);
            }
        }
        self.wake_caret();
    }

    /// A long operation is over: say how it went and catch the listing
    /// up with what changed.
    pub(super) fn finish_explorer_job(&mut self, outcomes: Vec<fenix_fs::Outcome>) {
        let verb = self.explorer_job.take().map(|job| job.done).unwrap_or("done");
        let cancelled = outcomes.iter().any(|o| o.error.as_deref() == Some("cancelled"));
        let results: Vec<Result<(), String>> = outcomes.into_iter().map(|o| o.error.map_or(Ok(()), Err)).collect();
        if cancelled {
            let done = results.iter().filter(|r| r.is_ok()).count();
            // Names what did happen rather than only that it stopped:
            // after a cancel the useful question is what is already at
            // the other end.
            self.set_message(format!("stopped -- {done} {verb} before that"));
        } else {
            self.report_explorer_outcomes(verb, results);
        }
        self.explorer_relist();
        self.wake_caret();
    }

    /// `z`: pack the marked set (or the entry at point) into an archive.
    ///
    /// The name is offered rather than demanded -- one item is named
    /// after itself, several after the folder they are in -- so the
    /// common case is one keystroke and Enter.
    pub(super) fn begin_archive(&mut self) {
        if fenix_fs::archiver().is_none() {
            self.set_error("no archiver on this machine (Windows 10+ ships one as System32\\tar.exe)");
            return;
        }
        let Some(explorer) = self.active_explorer() else { return };
        let sources = explorer.target_paths();
        if sources.is_empty() {
            return;
        }
        let suggested = fenix_fs::suggested_name(&sources);
        self.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::ArchiveTo, input: suggested });
        self.wake_caret();
    }

    /// `x`: unpack the archive at point, into a directory named after
    /// it.
    ///
    /// Into its own directory rather than over whatever is around it:
    /// an archive with forty files at its root, emptied into a folder
    /// that already had things in it, is a mess nobody can undo.
    pub(super) fn extract_selected_archive(&mut self) {
        if fenix_fs::archiver().is_none() {
            self.set_error("no archiver on this machine (Windows 10+ ships one as System32\\tar.exe)");
            return;
        }
        let Some(entry) = self.active_explorer().and_then(|e| e.selected_entry()) else { return };
        let path = entry.path.clone();
        if !fenix_fs::looks_like_archive(&path) {
            self.set_error(format!("{} does not look like an archive", entry.name));
            return;
        }
        let dest = fenix_fs::suggested_destination(&path);
        match fenix_fs::extract_archive(&path, &dest) {
            Ok(()) => self.set_message(format!("extracted into {}", dest.file_name().unwrap_or(dest.as_os_str()).to_string_lossy())),
            Err(err) => self.set_error(err.to_string()),
        }
        self.explorer_relist();
        self.wake_caret();
    }

    /// `i`: everything about the entry at point that does not fit in a
    /// column.
    ///
    /// For a directory it also starts counting what is inside, in the
    /// background -- that answer needs a walk of the whole tree, which
    /// is why it is asked for rather than shown in a column.
    pub(super) fn show_properties(&mut self) {
        let Some(entry) = self.active_explorer().and_then(|e| e.selected_entry()) else { return };
        let path = entry.path.clone();
        let props = match fenix_fs::properties(&path) {
            Ok(props) => props,
            Err(err) => {
                self.set_error(err.to_string());
                return;
            }
        };
        self.set_message(describe_properties(&props));
        if counts_contents(&props) {
            match self.event_proxy.clone() {
                Some(proxy) => {
                    std::thread::spawn(move || {
                        let total = fenix_fs::measure_tree(&path, &|| false);
                        let _ = proxy.send_event(FenixUserEvent::ExplorerMeasured { path, total });
                    });
                }
                None => {
                    let total = fenix_fs::measure_tree(&path, &|| false);
                    self.apply_explorer_measured(path, total);
                }
            }
        }
        self.wake_caret();
    }

    /// `w`: flip the read-only attribute on the marked set, or on the
    /// entry at point.
    ///
    /// Flipped rather than set, and decided by the *first* target, so a
    /// mixed selection ends up consistent instead of each item swapping
    /// to the opposite of whatever it happened to be.
    pub(super) fn toggle_read_only(&mut self) {
        let Some(explorer) = self.active_explorer() else { return };
        let targets = explorer.target_paths();
        let Some(first) = targets.first() else { return };
        let make_readonly = !fenix_fs::properties(first).map(|p| p.attributes.readonly).unwrap_or(false);
        let outcomes: Vec<Result<(), String>> =
            targets.iter().map(|path| fenix_fs::set_readonly(path, make_readonly).map_err(|e| e.to_string())).collect();
        self.report_explorer_outcomes(if make_readonly { "made read-only" } else { "made writable" }, outcomes);
        self.explorer_relist();
        self.wake_caret();
    }

    /// A recursive directory count came back.
    pub(super) fn apply_explorer_measured(&mut self, path: PathBuf, total: fenix_fs::Total) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| readable_path(&path));
        // "at least" when the walk did not finish -- passing off a
        // partial count as the answer would be worse than saying so.
        let at_least = if total.partial { "at least " } else { "" };
        self.set_message(format!(
            "{name}: {at_least}{} in {} file{} across {} folder{}",
            format_size(total.bytes),
            total.files,
            if total.files == 1 { "" } else { "s" },
            total.directories,
            if total.directories == 1 { "" } else { "s" },
        ));
        self.wake_caret();
    }

    // -- Handing things back to the rest of the system -------------------

    /// `SPC e o`: open the entry at point with whatever the system
    /// associates with it.
    ///
    /// The answer to a `.xlsx` is Excel, not a hex dump. A file fenix
    /// can edit still opens in fenix on `Enter`; this is the other key,
    /// for when it should not.
    pub(crate) fn explorer_open_externally(&mut self) {
        let Some(path) = self.explorer_selected_path() else { return };
        match fenix_fs::open_with_default(&path) {
            Ok(()) => self.set_message(format!("opened {}", file_label(&path))),
            Err(err) => self.set_error(err.to_string()),
        }
        self.wake_caret();
    }

    /// `SPC e O`: show the entry at point in Explorer, selected.
    ///
    /// The escape hatch. Being unable to leave is not the same as not
    /// needing to, and a file manager you can always step out of is one
    /// it is easy to start trusting.
    pub(crate) fn explorer_reveal(&mut self) {
        let Some(path) = self.explorer_selected_path() else { return };
        match fenix_fs::reveal(&path) {
            Ok(()) => self.set_message(format!("showed {} in Explorer", file_label(&path))),
            Err(err) => self.set_error(err.to_string()),
        }
        self.wake_caret();
    }

    /// `SPC e y`: put the entry's full path on the clipboard.
    ///
    /// The whole path, not the name: a path is what you paste into a
    /// terminal, a chat message or another program's open dialog, and
    /// a bare filename is not useful in any of those.
    pub(crate) fn explorer_yank_path(&mut self) {
        let Some(path) = self.explorer_selected_path() else { return };
        let text = readable_path(&path);
        if let Some(clipboard) = self.clipboard.as_mut() {
            let _ = clipboard.set_text(text.clone());
        }
        self.set_message(format!("copied {text}"));
        self.wake_caret();
    }

    /// `SPC e T`: a shell in the directory being browsed.
    ///
    /// The integration the pane-terminal work made possible: without a
    /// working directory it would be a shell somewhere else, and the
    /// first thing anyone types is a `cd`.
    pub(crate) fn explorer_terminal_here(&mut self) {
        let Some(cwd) = self.active_explorer().map(|e| e.cwd.clone()) else {
            self.set_error("no directory open");
            return;
        };
        self.open_terminal_buffer_in(Some(cwd));
    }

    /// `SPC e g`: search this directory and everything under it.
    pub(crate) fn explorer_grep_here(&mut self) {
        let Some(cwd) = self.active_explorer().map(|e| e.cwd.clone()) else {
            self.set_error("no directory open");
            return;
        };
        self.search_open(super::SearchOpen::Folder(cwd));
    }

    /// `SPC e G`: treat this directory as the project, then open the
    /// Git panel on it.
    ///
    /// Moving the project root rather than teaching the Git panel a
    /// second notion of "where": once the root is here, `SPC g g`,
    /// `SPC p f` and `SPC s p` all follow, which is what somebody
    /// browsing another repository actually wants -- not one panel
    /// pointed elsewhere while everything around it looks at the old
    /// project.
    pub(crate) fn explorer_git_here(&mut self) {
        let Some(cwd) = self.active_explorer().map(|e| e.cwd.clone()) else {
            self.set_error("no directory open");
            return;
        };
        let root = fenix_project::find_project_root(&cwd).unwrap_or(cwd);
        if self.git_session.is_some() {
            // A panel already open is pointed at the old root and would
            // simply refresh against it, which looks like the key did
            // nothing.
            self.git_session_close();
        }
        self.project_root = Some(root.clone());
        self.open_git_panel();
        // Set again afterwards: opening the panel focuses a pathless
        // buffer, and `refresh_project_root` derives the root from
        // whatever the focused buffer's path is -- which for a panel is
        // nothing, so it would clear what was just chosen.
        self.project_root = Some(root.clone());
        self.set_message(format!("project is now {}", readable_path(&root)));
    }

    /// The path an entry-scoped action should act on.
    pub(super) fn explorer_selected_path(&mut self) -> Option<PathBuf> {
        self.active_explorer()?.selected_entry().map(|e| e.path.clone())
    }

    /// `SPC e k`: stop the running operation.
    ///
    /// Stops between files. A file already being written has to finish
    /// -- there is no way to abandon a copy part way through without
    /// leaving a truncated file behind -- so a single very large file is
    /// the one thing this cannot interrupt, and saying so beats a key
    /// that appears to do nothing.
    pub(crate) fn cancel_explorer_job(&mut self) {
        let Some(job) = &self.explorer_job else {
            self.set_message("nothing running");
            return;
        };
        job.cancel.store(true, Ordering::Relaxed);
        self.set_message("stopping after the file in flight...");
        self.wake_caret();
    }

    /// The progress line, for the modeline.
    pub(super) fn explorer_job_text(&self) -> Option<String> {
        let job = self.explorer_job.as_ref()?;
        let p = &job.progress;
        let name = p.current.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let percent = (p.bytes_done * 100).checked_div(p.bytes_total).unwrap_or(0);
        Some(format!(
            "{} {}/{} files, {} of {} ({percent}%)  {name}  --  SPC e k stops",
            job.running,
            p.files_done,
            p.files_total,
            format_size(p.bytes_done),
            format_size(p.bytes_total),
        ))
    }

    /// Moves the marked set (or the entry at point) to the Recycle Bin,
    /// so a mistake is recoverable through Windows' own restore rather
    /// than being final. `permanent` is the explicit `!` variant.
    pub(super) fn explorer_delete_targets(&mut self) {
        let Some(explorer) = self.active_explorer() else { return };
        let targets = explorer.target_paths();
        let permanent = self.explorer_delete_permanently;
        let outcomes = if permanent { fenix_fs::permanently(&targets) } else { fenix_fs::to_recycle_bin(&targets) };
        let verb = if permanent { "deleted" } else { "moved to the Recycle Bin" };
        let outcomes: Vec<Result<(), String>> = outcomes.into_iter().map(|o| o.error.map_or(Ok(()), Err)).collect();
        self.report_explorer_outcomes(verb, outcomes);
        self.explorer_relist();
    }

    /// One status line for a whole batch: how many worked, and what went
    /// wrong with the first that did not. Silent failure is what the
    /// explorer this replaces did (an `eprintln!` nobody sees).
    pub(super) fn report_explorer_outcomes(&mut self, verb: &str, outcomes: Vec<Result<(), String>>) {
        let total = outcomes.len();
        let failures: Vec<String> = outcomes.into_iter().filter_map(|o| o.err()).collect();
        match failures.first() {
            None if total == 1 => self.set_message(format!("1 item {verb}")),
            None => self.set_message(format!("{total} items {verb}")),
            Some(first) if failures.len() == total => self.set_error(first.clone()),
            Some(first) => self.set_error(format!("{} of {total} failed -- {first}", failures.len())),
        }
    }

    /// Re-reads whichever listing is active.
    ///
    /// A buffer-backed listing goes through the off-thread path, because
    /// that is the one that might be pointed at a share. The sidebar and
    /// the directory-picking overlay still read inline: both are
    /// deliberately rooted in local project directories, and giving them
    /// the same treatment is the last step of this milestone rather than
    /// something to half-do here.
    pub(super) fn explorer_relist(&mut self) {
        let Some(target) = self.active_explorer_target() else { return };
        let Some(cwd) = self.explorer_state(target).map(|s| s.cwd.clone()) else { return };
        self.explorer_navigate(target, &cwd);
    }

    /// Points the active listing at `path`, whichever of the three it
    /// is. All of them read off the main thread now: the sidebar could
    /// freeze the editor on a share just as easily as a pane could, and
    /// it was the one still doing so.
    pub(super) fn explorer_go_to(&mut self, path: &Path) {
        let Some(target) = self.active_explorer_target() else { return };
        self.explorer_navigate(target, path);
    }

    /// Reorders the active listing. No re-read: rows are already in
    /// hand, and re-listing to change the order would be a needless
    /// round trip to a disk that might be slow.
    pub(super) fn explorer_set_sort(&mut self, sort: fenix_explorer::Sort) {
        let Some(target) = self.active_explorer_target() else { return };
        let Some(explorer) = self.explorer_state_mut(target) else { return };
        explorer.set_sort(sort);
        let label = explorer.sort.label();
        self.set_message(format!("sorted by {label}"));
        self.render_explorer(target, false);
    }
}
