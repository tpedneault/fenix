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
            .and_then(|p| p.extension())
            .and_then(|ext| ext.to_str())
            .and_then(fenix_syntax::detect_language)
            .map(|lang| fenix_syntax::SyntaxState::new(lang, &buffer.text()));
        self.insert(buffer, syntax, BufferKind::Text)
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
