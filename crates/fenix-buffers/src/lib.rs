use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use fenix_core::{Buffer, Cursor};

/// Identity of one open buffer, stable for as long as it stays open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BufferId(u32);

/// What a buffer's content actually is, beyond "it wraps a `Buffer`" --
/// every `OpenBuffer` still always has a real `buffer`/`cursor` (see
/// `OpenBuffer`'s own doc comment for why this is a tag, not a wrapping
/// enum), but the host uses this to tell an ordinary text buffer apart
/// from one whose content is generated/informational rather than
/// something the user is editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferKind {
    Text,
    /// The startup dashboard (`fenix-gui`'s `SPC d d`) -- a real buffer
    /// like any other, just tagged so the host can special-case what
    /// `Enter` does on it and how it's colored.
    Dashboard,
    /// A dired-style directory listing (`SPC f j`) -- a real buffer like
    /// any other (splittable, closable via `SPC b k`, listed in `SPC b
    /// b`), tagged so the host can special-case navigation/actions and
    /// coloring the same way it already does for `Dashboard`.
    Explorer,
    /// A Lazydocker-style container/image dashboard (`SPC d d`) -- same
    /// "real buffer, just tagged" shape as `Dashboard`/`Explorer`.
    Docker,
    /// A Lazygit-style repo status/files/branches/commits/stash panel
    /// (`SPC g g`) -- same "real buffer, just tagged" shape as `Docker`.
    Git,
    /// The Jira dashboard's projects/users/issues/detail panel
    /// (`SPC j j`) -- same "real buffer, just tagged" shape as `Docker`/
    /// `Git`.
    Jira,
    /// A tab-separated `Text` buffer toggled into elastic-column table
    /// view (`SPC f t`) -- unlike `Dashboard`/`Explorer`/`Docker`/`Git`,
    /// this isn't a distinct generated buffer: it's the *same* buffer,
    /// same path, same undo history, just retagged in place (see
    /// `fenix-gui`'s `App::toggle_table_view`), because the whole point
    /// is that the file's real content stays genuinely tab-separated at
    /// all times -- the alignment is purely a rendering effect
    /// (`fenix-gui`'s `tabstops` module), never baked into the text.
    Table,
    /// The project-wide search-and-replace review list (`SPC s p`) --
    /// same "real buffer, just tagged" shape as `Dashboard`/`Docker`/
    /// `Git`: a real, Vim-navigable, generated listing of every file a
    /// pending replace would touch, one row each, toggle-able before
    /// anything is actually written to disk.
    SearchReplace,
    /// A live VNC console view (`SPC v v`) -- same "real buffer, just
    /// tagged" shape as `Docker`/`Git`/`Jira` in that it's a real pane
    /// slot in the window tree, but unlike those there is nothing
    /// meaningful to put in the buffer's own text at all: the actual
    /// pixel content is a separate GPU-side texture layer keyed by this
    /// buffer's pane, never stored in `buffer` itself (see
    /// `BufferList::open_vnc`).
    Vnc,
    /// An open PDF document, rendered one page at a time (`SPC r ...`) --
    /// same "real pane slot, no meaningful text content" shape as `Vnc`:
    /// the rasterized page bitmap is a separate GPU-side texture layer
    /// keyed by this buffer's pane, never stored in `buffer` itself (see
    /// `BufferList::open_pdf`). Deliberately never carries the PDF's real
    /// file path on the `Buffer` itself either -- see `open_pdf`'s own
    /// doc comment for why.
    Pdf,
    /// A PDF's flattened bookmark tree (`SPC r o`), shown as an indented
    /// listing in its own companion pane next to the `Pdf` pane it was
    /// opened from -- unlike `Pdf` itself, this *is* real Vim-navigable
    /// text (same "real buffer, just tagged" shape as `Dashboard`), since
    /// an outline is naturally just a list of lines, not pixel content.
    PdfOutline,
    /// A PDF text search's flat match list (`SPC r /`), shown as one row
    /// per hit in its own companion pane next to the `Pdf` pane it was
    /// searched from -- same "real buffer, just tagged" shape as
    /// `PdfOutline`, just a list of matches instead of bookmarks.
    PdfSearchResults,
    /// The build/task runner's live output panel (`SPC t t`) -- same
    /// "real buffer, just tagged, host appends to it as output streams
    /// in" shape as `Docker`'s own Logs pane, just a single pane rather
    /// than a whole six-pane session (see `fenix-gui`'s `TaskSession`).
    TaskOutput,
    /// The DAP debug session's Call Stack/Variables/Watches/Breakpoints
    /// panel (`SPC u u`) -- same "real buffer, just tagged" shape as
    /// `Docker`, all four panes sharing this one kind exactly the way
    /// Docker's six panes all share `BufferKind::Docker` (see `fenix-
    /// gui`'s `DebugSession`/`DebugPaneRole`).
    Debug,
    /// A rendered diff (`fenix-gui`'s `diff_view`) -- the working
    /// tree's changes, a commit, a ref-to-ref comparison, or a merge
    /// request's changes. Its own kind rather than another `Git` buffer
    /// because the *same* rendered diff is hosted by several different
    /// panels, and each row carries a much richer payload than a style
    /// tag (the file/hunk/line it came from, see `diff_view::
    /// DiffAnchor`) -- so it gets its own per-line metadata map and
    /// highlight pass, exactly as `Docker`/`Git`/`Jira` each already do.
    Diff,
    /// The commit graph (`SPC g l`) -- one row per commit with its rail
    /// art, hash, ref decorations and subject as separately-colored
    /// spans, which is why it has its own kind and per-line metadata
    /// rather than being another `Git` buffer (see `fenix-gui`'s
    /// `graph_view::GraphLine`).
    Graph,
    /// The tool status listing (`SPC l m`) -- which LSP servers/DAP
    /// adapters are configured, found on `PATH`, and running, per
    /// language, with an install hint for anything missing (see
    /// `fenix-gui`'s `tool_status` module). Same single-pane "real
    /// buffer, just tagged, host regenerates it on demand" shape as
    /// `TaskOutput`, minus the live-appending: this one's whole content
    /// is just recomputed and swapped in fresh each time it's opened or
    /// refreshed, since a `PATH` scan is fast enough to redo outright
    /// rather than being worth incrementally patching.
    ToolStatus,
    /// A conflicted file shown as two aligned columns, ours beside
    /// theirs (`SPC g x`). Its own kind rather than another `Diff`
    /// buffer because the per-line metadata is different in kind: a
    /// diff row names a file/hunk/line, a merge row names a *conflict*
    /// and which column the text came from (see `fenix-gui`'s
    /// `merge_view::MergeViewLine`), and the two columns are colored
    /// independently within a single row.
    Merge,
}

impl BufferKind {
    /// Whether a dirty flag on a buffer of this kind reflects real,
    /// savable user work. `Text` and `Table` are the same underlying,
    /// path-backed `fenix_core::Buffer` a real file lives in (`Table`
    /// is just `Text` retagged in place, see its own doc comment
    /// above). `Dashboard`/`Explorer`/`Docker`/`Git`/`Jira`/
    /// `SearchReplace`/`Vnc` are always pathless, host-regenerated (or,
    /// for `Vnc`, never-written-to-at-all) views that can never be
    /// saved -- for the text-generating ones, content gets rewritten by
    /// the host on every refresh, which trips `dirty` with nothing the
    /// user could ever do to clear it again, so treating that as
    /// "unsaved work" is wrong.
    pub fn tracks_unsaved_changes(self) -> bool {
        matches!(self, BufferKind::Text | BufferKind::Table)
    }
}

/// One open buffer's full state. `cursor` here is the buffer's
/// *remembered* position (mirrors Vim's own `'"` "last position" mark) --
/// used only to seed a pane's live cursor the first time that buffer is
/// shown in it (opening a file fresh, or splitting to show an already-
/// open buffer in a second pane for the first time). It is not updated
/// while a pane is actively editing the buffer -- live cursor/scroll
/// position during editing is per-*window*, owned by the host
/// (`fenix-gui`'s `App`, keyed by `WindowId`), not here: if the same
/// buffer is shown in two panes at once, each pane scrolls/positions its
/// cursor independently, the way real Emacs windows work. This registry
/// has no notion of windows/panes at all (it doesn't depend on
/// `fenix-window`), so it can't own that state itself.
///
/// `buffer: Buffer` is unconditional, not wrapped in a content enum --
/// dozens of call sites across `fenix-vim`/`fenix-gui` (every motion,
/// every render, save, undo/redo) already assume `buffer`/`cursor` exist
/// directly. `kind` is an additive tag instead: every `OpenBuffer` is
/// still a real, navigable, undoable rope buffer; `kind` only changes
/// what the host layer does with it beyond that (see `BufferKind`).
pub struct OpenBuffer {
    pub buffer: Buffer,
    pub cursor: Cursor,
    pub syntax: Option<fenix_syntax::SyntaxState>,
    pub kind: BufferKind,
}

/// A registry of open buffers, keyed by `BufferId`. Depends only on
/// `fenix-core` + `fenix-syntax` (no GPU/winit) -- host-agnostic, unit-
/// testable directly, mirroring `fenix-explorer`/`fenix-project`'s role.
pub struct BufferList {
    buffers: BTreeMap<BufferId, OpenBuffer>,
    path_index: HashMap<PathBuf, BufferId>,
    /// Most-recently-touched first -- drives buffer-switcher ordering and
    /// "what to fall back to when the current buffer closes."
    mru: Vec<BufferId>,
    next_id: u32,
}

impl Default for BufferList {
    fn default() -> Self {
        Self::new()
    }
}

impl BufferList {
    pub fn new() -> Self {
        Self { buffers: BTreeMap::new(), path_index: HashMap::new(), mru: Vec::new(), next_id: 0 }
    }

    fn insert(&mut self, buffer: Buffer, syntax: Option<fenix_syntax::SyntaxState>, kind: BufferKind) -> BufferId {
        let id = BufferId(self.next_id);
        self.next_id += 1;
        if let Some(path) = buffer.path() {
            self.path_index.insert(path.to_path_buf(), id);
        }
        self.buffers.insert(id, OpenBuffer { buffer, cursor: Cursor::at_start(), syntax, kind });
        self.touch(id);
        id
    }

    /// An empty, unnamed buffer -- `SPC b X`.
    pub fn open_scratch(&mut self) -> BufferId {
        self.insert(Buffer::empty(), None, BufferKind::Text)
    }

    /// A real buffer seeded with `text` up front (no undo history, via
    /// `Buffer::from_text`) and tagged `Dashboard` -- the startup
    /// dashboard, `SPC d d`. A real buffer like any other: splittable,
    /// closable via `SPC b k`, listed in the buffer switcher. The host
    /// uses `kind` to special-case what Enter does and how it's colored;
    /// nothing about storage/navigation/undo differs from an ordinary
    /// buffer.
    pub fn open_dashboard(&mut self, text: &str) -> BufferId {
        self.insert(Buffer::from_text(text), None, BufferKind::Dashboard)
    }

    /// A real buffer seeded with `text` (a rendered directory listing)
    /// and tagged `Explorer` -- `SPC f j`. Same "real buffer, just
    /// tagged" shape as `open_dashboard`; the host owns re-rendering
    /// `text` (via `Buffer::replace_range`) whenever the underlying
    /// listing changes (navigating into a directory, a refresh, ...).
    pub fn open_explorer(&mut self, text: &str) -> BufferId {
        self.insert(Buffer::from_text(text), None, BufferKind::Explorer)
    }

    /// A real buffer seeded with `text` (a rendered container/image
    /// listing) and tagged `Docker` -- `SPC d d`. Same "real buffer, just
    /// tagged" shape as `open_dashboard`/`open_explorer`; the host
    /// re-renders `text` via `Buffer::replace_range` on refresh.
    pub fn open_docker(&mut self, text: &str) -> BufferId {
        self.insert(Buffer::from_text(text), None, BufferKind::Docker)
    }

    /// A real buffer seeded with `text` and tagged `TaskOutput` -- `SPC
    /// t t`. Same "real buffer, just tagged" shape as `open_docker`; the
    /// host appends each new line as it streams in rather than
    /// rewriting the whole buffer on every update the way `open_docker`/
    /// `open_git`/`open_jira`'s panels do.
    pub fn open_task_output(&mut self, text: &str) -> BufferId {
        self.insert(Buffer::from_text(text), None, BufferKind::TaskOutput)
    }

    /// A real buffer seeded with `text` and tagged `Debug` -- `SPC u u`.
    /// Same "real buffer, just tagged" shape as `open_docker`.
    pub fn open_debug(&mut self, text: &str) -> BufferId {
        self.insert(Buffer::from_text(text), None, BufferKind::Debug)
    }

    /// A real buffer seeded with `text` (a rendered diff) and tagged
    /// `Diff` -- see `BufferKind::Diff`. Same "real buffer, just tagged"
    /// shape as `open_git`.
    pub fn open_diff(&mut self, text: &str) -> BufferId {
        self.insert(Buffer::from_text(text), None, BufferKind::Diff)
    }

    /// A real buffer seeded with `text` (a rendered commit graph) and
    /// tagged `Graph` -- `SPC g l`. Same "real buffer, just tagged"
    /// shape as `open_git`.
    pub fn open_graph(&mut self, text: &str) -> BufferId {
        self.insert(Buffer::from_text(text), None, BufferKind::Graph)
    }

    /// A real buffer seeded with `text` (a conflicted file rendered as
    /// two columns) and tagged `Merge` -- `SPC g x`. Same "real buffer,
    /// just tagged" shape as `open_diff`.
    pub fn open_merge(&mut self, text: &str) -> BufferId {
        self.insert(Buffer::from_text(text), None, BufferKind::Merge)
    }

    /// A real buffer seeded with `text` (a rendered tool status listing)
    /// and tagged `ToolStatus` -- `SPC l m`. Same "real buffer, just
    /// tagged" shape as `open_docker`/`open_debug`.
    pub fn open_tool_status(&mut self, text: &str) -> BufferId {
        self.insert(Buffer::from_text(text), None, BufferKind::ToolStatus)
    }

    /// A real buffer seeded with `text` (a rendered status/files/branches/
    /// commits/stash listing) and tagged `Git` -- `SPC g g`. Same "real
    /// buffer, just tagged" shape as `open_docker`; the host re-renders
    /// `text` via `Buffer::replace_range` on refresh.
    pub fn open_git(&mut self, text: &str) -> BufferId {
        self.insert(Buffer::from_text(text), None, BufferKind::Git)
    }

    /// A real buffer seeded with `text` (a rendered projects/users/
    /// issues/detail listing) and tagged `Jira` -- `SPC j j`. Same "real
    /// buffer, just tagged" shape as `open_docker`/`open_git`; the host
    /// re-renders `text` via `Buffer::replace_range` on refresh.
    pub fn open_jira(&mut self, text: &str) -> BufferId {
        self.insert(Buffer::from_text(text), None, BufferKind::Jira)
    }

    /// An empty, pathless buffer tagged `Vnc` -- `SPC v v`. Deliberately
    /// `Buffer::empty()`, not `Buffer::from_text(...)` like `open_docker`/
    /// `open_jira` seed a rendered text panel: a VNC pane's content is a
    /// live pixel framebuffer, not text, so there is nothing to seed --
    /// this buffer only exists to give the pane a `BufferId` slot in the
    /// window tree; its own (always-empty) text is never shown.
    pub fn open_vnc(&mut self) -> BufferId {
        self.insert(Buffer::empty(), None, BufferKind::Vnc)
    }

    /// An empty, pathless buffer tagged `Pdf` -- `SPC r o`. Same reasoning
    /// as `open_vnc`: the rendered page bitmap is a GPU-side texture, not
    /// text, so there is nothing to seed. Deliberately `Buffer::empty()`
    /// with no path attached even though a PDF *does* have a real path on
    /// disk (unlike a VNC session) -- `Buffer::save_as` unconditionally
    /// overwrites whatever path it's given with the buffer's own (always
    /// empty) rope content, so attaching the real PDF path here would
    /// make a stray `:w` silently truncate the user's actual PDF file.
    /// The host (`fenix-gui`) keeps the real path in its own side table
    /// instead.
    pub fn open_pdf(&mut self) -> BufferId {
        self.insert(Buffer::empty(), None, BufferKind::Pdf)
    }

    /// A real buffer seeded with `text` (an indented, flattened rendering
    /// of a PDF's bookmark tree) and tagged `PdfOutline` -- `SPC r o`.
    /// Same "real buffer, just tagged" shape as `open_dashboard`/
    /// `open_docker`, unlike `open_pdf`/`open_vnc`'s empty-buffer shape:
    /// an outline's content genuinely is text worth Vim-navigating
    /// (`j`/`k`/`/`/`n`), not a stand-in pane slot for pixel content.
    pub fn open_pdf_outline(&mut self, text: &str) -> BufferId {
        self.insert(Buffer::from_text(text), None, BufferKind::PdfOutline)
    }

    /// A real buffer seeded with `text` (one formatted row per text-
    /// search match) and tagged `PdfSearchResults` -- `SPC r /`. Same
    /// "real buffer, just tagged" shape as `open_pdf_outline`.
    pub fn open_pdf_search_results(&mut self, text: &str) -> BufferId {
        self.insert(Buffer::from_text(text), None, BufferKind::PdfSearchResults)
    }

    /// A real, ordinary `Text`-kind buffer seeded with `text` up front --
    /// e.g. `docker logs` output shown by the Docker panel's `l` action.
    /// Unlike `open_dashboard`/`open_explorer`/`open_docker`, this is
    /// tagged plain `Text`, not a bespoke kind: viewing generated,
    /// read-only-ish content needs nothing beyond what an ordinary
    /// buffer already gives for free (Vim motions, `/` search, closable
    /// via `SPC b k`) -- no special key handling, no per-buffer
    /// line-metadata side table.
    pub fn open_text_view(&mut self, text: &str) -> BufferId {
        self.insert(Buffer::from_text(text), None, BufferKind::Text)
    }

    /// Opens `path`, reusing an already-open buffer for that exact path
    /// instead of creating a duplicate -- matches Emacs: visiting a file
    /// that's already open shows the same buffer (same unsaved edits),
    /// not a second independent copy. Language detection mirrors what
    /// `fenix-gui`'s own startup path used to do directly, moved here now
    /// that opening a path is this crate's job. A read failure degrades to
    /// an empty buffer (still usable, just starts blank) rather than
    /// failing outright -- same posture as every other "can't read this,
    /// don't crash" fallback already established in this project.
    pub fn open_path(&mut self, path: &Path) -> BufferId {
        if let Some(&id) = self.path_index.get(path) {
            self.touch(id);
            return id;
        }
        let buffer = Buffer::from_path(path).unwrap_or_else(|err| {
            eprintln!("fenix: couldn't open {} ({err}), starting empty buffer", path.display());
            Buffer::empty()
        });
        let syntax = buffer
            .path()
            .and_then(fenix_syntax::detect_language_from_path)
            .map(|lang| fenix_syntax::SyntaxState::new(lang, &buffer.text()));
        self.insert(buffer, syntax, BufferKind::Text)
    }

    /// The buffer already open for `path`, if any -- lets a caller that's
    /// about to write to `path` on disk (project-wide search-and-replace
    /// apply, `fenix-gui`'s `SPC s p`) check first and route the edit
    /// through the real in-memory `Buffer` instead, so it never clobbers
    /// an open, possibly-dirty buffer's content out from under it.
    pub fn id_for_path(&self, path: &Path) -> Option<BufferId> {
        self.path_index.get(path).copied()
    }

    /// Keeps `path_index` in sync after `id`'s underlying file was
    /// renamed out from under it (`fenix-gui`'s `SPC f R`) -- `Buffer::
    /// save_as` already updates the buffer's own `path()`, this just
    /// makes the registry's own index (what `open_path`/`id_for_path`
    /// use to detect an already-open buffer for a path) agree with it:
    /// removes `old_path`'s entry and re-inserts under the buffer's
    /// current path. A no-op for either half if there's nothing to do
    /// (e.g. `old_path` was `None` -- the buffer was never saved
    /// before), matching this registry's existing graceful-by-default
    /// posture elsewhere.
    pub fn path_changed(&mut self, id: BufferId, old_path: Option<&Path>) {
        if let Some(old) = old_path {
            self.path_index.remove(old);
        }
        if let Some(new_path) = self.buffers.get(&id).and_then(|ob| ob.buffer.path()) {
            self.path_index.insert(new_path.to_path_buf(), id);
        }
    }

    pub fn get(&self, id: BufferId) -> Option<&OpenBuffer> {
        self.buffers.get(&id)
    }

    pub fn get_mut(&mut self, id: BufferId) -> Option<&mut OpenBuffer> {
        self.buffers.get_mut(&id)
    }

    /// Removes `id` from the registry. No "never truly empty" guarantee
    /// here -- ensuring a fallback/scratch buffer exists, and retargeting
    /// any panes that were showing `id`, is a policy decision that belongs
    /// to the caller (which knows about panes; this registry doesn't).
    pub fn close(&mut self, id: BufferId) {
        if let Some(open) = self.buffers.remove(&id) {
            if let Some(path) = open.buffer.path() {
                self.path_index.remove(path);
            }
        }
        self.mru.retain(|&x| x != id);
    }

    /// Moves `id` to the front of the MRU list (or inserts it there if
    /// new). Every `open_*`/`open_path`-reuse call already does this;
    /// exposed publicly too so callers can bump it on focus changes that
    /// don't go through open (e.g. switching panes onto an already-open
    /// buffer).
    pub fn touch(&mut self, id: BufferId) {
        self.mru.retain(|&x| x != id);
        self.mru.insert(0, id);
    }

    pub fn mru(&self) -> &[BufferId] {
        &self.mru
    }

    /// A stable listing order for a buffer-switcher UI: by path (buffers
    /// with no path -- scratch buffers -- sort first, in creation order
    /// among themselves since `None < Some(_)`).
    pub fn ids_sorted_by_path(&self) -> Vec<BufferId> {
        let mut ids: Vec<BufferId> = self.buffers.keys().copied().collect();
        ids.sort_by(|&a, &b| self.buffers[&a].buffer.path().cmp(&self.buffers[&b].buffer.path()));
        ids
    }

    pub fn len(&self) -> usize {
        self.buffers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A real, uniquely-named temp directory, removed on drop -- same
    /// reasoning as every other crate's own `TempDir`: `open_path` is real
    /// filesystem I/O, tested against a real filesystem.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("fenix-buffers-test-{name}-{}-{n}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn write(&self, name: &str, contents: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, contents).unwrap();
            path
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn tracks_unsaved_changes_is_true_only_for_text_and_table() {
        assert!(BufferKind::Text.tracks_unsaved_changes());
        assert!(BufferKind::Table.tracks_unsaved_changes());
        assert!(!BufferKind::Dashboard.tracks_unsaved_changes());
        assert!(!BufferKind::Explorer.tracks_unsaved_changes());
        assert!(!BufferKind::Docker.tracks_unsaved_changes());
        assert!(!BufferKind::Git.tracks_unsaved_changes());
        assert!(!BufferKind::Jira.tracks_unsaved_changes());
        assert!(!BufferKind::SearchReplace.tracks_unsaved_changes());
        assert!(!BufferKind::Vnc.tracks_unsaved_changes());
        assert!(!BufferKind::Pdf.tracks_unsaved_changes());
    }

    #[test]
    fn open_scratch_creates_an_empty_unnamed_buffer() {
        let mut list = BufferList::new();
        let id = list.open_scratch();
        let ob = list.get(id).unwrap();
        assert_eq!(ob.buffer.text(), "");
        assert_eq!(ob.buffer.path(), None);
        assert_eq!(ob.kind, BufferKind::Text);
    }

    #[test]
    fn open_dashboard_seeds_the_text_and_tags_the_buffer() {
        let mut list = BufferList::new();
        let id = list.open_dashboard("hello dashboard\n");
        let ob = list.get(id).unwrap();
        assert_eq!(ob.buffer.text(), "hello dashboard\n");
        assert_eq!(ob.buffer.path(), None);
        assert_eq!(ob.kind, BufferKind::Dashboard);
    }

    #[test]
    fn open_explorer_seeds_the_text_and_tags_the_buffer() {
        let mut list = BufferList::new();
        let id = list.open_explorer("a.txt\nb.txt\n");
        let ob = list.get(id).unwrap();
        assert_eq!(ob.buffer.text(), "a.txt\nb.txt\n");
        assert_eq!(ob.buffer.path(), None);
        assert_eq!(ob.kind, BufferKind::Explorer);
    }

    #[test]
    fn open_docker_seeds_the_text_and_tags_the_buffer() {
        let mut list = BufferList::new();
        let id = list.open_docker("containers\nimages\n");
        let ob = list.get(id).unwrap();
        assert_eq!(ob.buffer.text(), "containers\nimages\n");
        assert_eq!(ob.buffer.path(), None);
        assert_eq!(ob.kind, BufferKind::Docker);
    }

    #[test]
    fn open_vnc_creates_an_empty_pathless_buffer_tagged_vnc() {
        let mut list = BufferList::new();
        let id = list.open_vnc();
        let ob = list.get(id).unwrap();
        assert_eq!(ob.buffer.text(), "");
        assert_eq!(ob.buffer.path(), None);
        assert_eq!(ob.kind, BufferKind::Vnc);
    }

    #[test]
    fn open_pdf_creates_an_empty_pathless_buffer_tagged_pdf() {
        let mut list = BufferList::new();
        let id = list.open_pdf();
        let ob = list.get(id).unwrap();
        assert_eq!(ob.buffer.text(), "");
        assert_eq!(ob.buffer.path(), None);
        assert_eq!(ob.kind, BufferKind::Pdf);
    }

    #[test]
    fn open_pdf_search_results_seeds_the_text_and_tags_the_buffer() {
        let mut list = BufferList::new();
        let id = list.open_pdf_search_results("p.  1  the quick brown fox\n");
        let ob = list.get(id).unwrap();
        assert_eq!(ob.buffer.text(), "p.  1  the quick brown fox\n");
        assert_eq!(ob.buffer.path(), None);
        assert_eq!(ob.kind, BufferKind::PdfSearchResults);
    }

    #[test]
    fn open_tool_status_seeds_the_text_and_tags_the_buffer() {
        let mut list = BufferList::new();
        let id = list.open_tool_status("Python  LSP  pyright-langserver  [found]\n");
        let ob = list.get(id).unwrap();
        assert_eq!(ob.buffer.text(), "Python  LSP  pyright-langserver  [found]\n");
        assert_eq!(ob.buffer.path(), None);
        assert_eq!(ob.kind, BufferKind::ToolStatus);
    }

    #[test]
    fn open_git_seeds_the_text_and_tags_the_buffer() {
        let mut list = BufferList::new();
        let id = list.open_git("status\nfiles\n");
        let ob = list.get(id).unwrap();
        assert_eq!(ob.buffer.text(), "status\nfiles\n");
        assert_eq!(ob.buffer.path(), None);
        assert_eq!(ob.kind, BufferKind::Git);
    }

    #[test]
    fn open_text_view_seeds_the_text_and_tags_it_plain_text() {
        let mut list = BufferList::new();
        let id = list.open_text_view("line one\nline two\n");
        let ob = list.get(id).unwrap();
        assert_eq!(ob.buffer.text(), "line one\nline two\n");
        assert_eq!(ob.buffer.path(), None);
        assert_eq!(ob.kind, BufferKind::Text);
    }

    #[test]
    fn open_dashboard_is_a_real_buffer_reachable_through_the_registry_like_any_other() {
        // The whole point of the tag-not-wrap design: no special lookup
        // path, no separate registry -- `get`/`get_mut`/`mru`/`close` all
        // just work.
        let mut list = BufferList::new();
        let id = list.open_dashboard("dashboard\n");
        assert_eq!(list.mru(), &[id]);
        assert_eq!(list.len(), 1);
        list.close(id);
        assert!(list.get(id).is_none());
    }

    #[test]
    fn open_path_reads_real_file_content() {
        let dir = TempDir::new("open_path_reads");
        let path = dir.write("a.txt", "hello\n");
        let mut list = BufferList::new();
        let id = list.open_path(&path);
        assert_eq!(list.get(id).unwrap().buffer.text(), "hello\n");
        assert_eq!(list.get(id).unwrap().kind, BufferKind::Text);
    }

    #[test]
    fn open_path_reuses_an_already_open_buffer_for_the_same_path() {
        let dir = TempDir::new("open_path_reuses");
        let path = dir.write("a.txt", "hello\n");
        let mut list = BufferList::new();
        let first = list.open_path(&path);
        let second = list.open_path(&path);
        assert_eq!(first, second);
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn open_path_on_a_missing_file_degrades_to_an_empty_buffer() {
        let dir = TempDir::new("open_path_missing");
        let mut list = BufferList::new();
        let id = list.open_path(&dir.0.join("does-not-exist.txt"));
        assert_eq!(list.get(id).unwrap().buffer.text(), "");
    }

    #[test]
    fn path_changed_moves_the_registry_entry_from_old_to_new_path() {
        let dir = TempDir::new("path_changed");
        let old_path = dir.write("old.txt", "hi\n");
        let new_path = dir.0.join("new.txt");
        let mut list = BufferList::new();
        let id = list.open_path(&old_path);
        list.get_mut(id).unwrap().buffer.save_as(&new_path).unwrap();

        list.path_changed(id, Some(&old_path));

        assert_eq!(list.id_for_path(&old_path), None);
        assert_eq!(list.id_for_path(&new_path), Some(id));
    }

    #[test]
    fn path_changed_with_no_old_path_just_inserts_the_current_one() {
        let dir = TempDir::new("path_changed_no_old");
        let new_path = dir.0.join("brand_new.txt");
        let mut list = BufferList::new();
        let id = list.open_scratch();
        list.get_mut(id).unwrap().buffer.save_as(&new_path).unwrap();

        list.path_changed(id, None);

        assert_eq!(list.id_for_path(&new_path), Some(id));
    }

    #[test]
    fn open_path_detects_syntax_from_the_extension() {
        let dir = TempDir::new("open_path_syntax");
        let path = dir.write("a.rs", "fn main() {}\n");
        let mut list = BufferList::new();
        let id = list.open_path(&path);
        assert!(list.get(id).unwrap().syntax.is_some());
    }

    #[test]
    fn open_path_on_an_unrecognized_extension_has_no_syntax() {
        let dir = TempDir::new("open_path_no_syntax");
        let path = dir.write("a.xyz", "whatever\n");
        let mut list = BufferList::new();
        let id = list.open_path(&path);
        assert!(list.get(id).unwrap().syntax.is_none());
    }

    #[test]
    fn get_mut_allows_editing_a_buffer_in_place() {
        let mut list = BufferList::new();
        let id = list.open_scratch();
        let ob = list.get_mut(id).unwrap();
        ob.buffer.insert_str(&mut ob.cursor, "typed");
        assert_eq!(list.get(id).unwrap().buffer.text(), "typed");
    }

    #[test]
    fn close_removes_the_buffer_and_its_path_index_entry() {
        let dir = TempDir::new("close_removes");
        let path = dir.write("a.txt", "hello\n");
        let mut list = BufferList::new();
        let id = list.open_path(&path);
        list.close(id);
        assert!(list.get(id).is_none());
        // Reopening the same path after close makes a fresh buffer, not
        // a phantom reuse of the closed one's id.
        let reopened = list.open_path(&path);
        assert_ne!(reopened, id);
    }

    #[test]
    fn touch_moves_a_buffer_to_the_front_of_mru() {
        let mut list = BufferList::new();
        let a = list.open_scratch();
        let b = list.open_scratch();
        assert_eq!(list.mru(), &[b, a]); // b opened last, most recent
        list.touch(a);
        assert_eq!(list.mru(), &[a, b]);
    }

    #[test]
    fn close_removes_the_buffer_from_mru_too() {
        let mut list = BufferList::new();
        let a = list.open_scratch();
        let b = list.open_scratch();
        list.close(b);
        assert_eq!(list.mru(), &[a]);
    }

    #[test]
    fn ids_sorted_by_path_orders_scratch_buffers_before_paths() {
        let dir = TempDir::new("sorted_by_path");
        let path = dir.write("z.txt", "hi\n");
        let mut list = BufferList::new();
        let scratch = list.open_scratch();
        let file = list.open_path(&path);
        assert_eq!(list.ids_sorted_by_path(), vec![scratch, file]);
    }

    #[test]
    fn len_and_is_empty_reflect_open_buffer_count() {
        let mut list = BufferList::new();
        assert!(list.is_empty());
        list.open_scratch();
        assert_eq!(list.len(), 1);
        assert!(!list.is_empty());
    }
}
