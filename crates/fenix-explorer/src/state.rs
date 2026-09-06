use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::entry::{Entry, GitStatus};
use crate::git;

/// What a listing is ordered by. Directories are grouped ahead of files
/// independently of this (`ExplorerState::dirs_first`), because that is
/// a separate question from which key you are sorting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Modified,
    /// Groups by file type, with the name as the tie-break -- the point
    /// is having every `.rs` together, and within them a stable order.
    Extension,
}

/// How a listing should be ordered, in full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    pub descending: bool,
    /// Directories ahead of files regardless of `key`. On by default:
    /// it is what every file manager does, and it is what makes a
    /// listing scannable rather than a mixed pile.
    pub dirs_first: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Self { key: SortKey::Name, descending: false, dirs_first: true }
    }
}

impl Sort {
    /// Cycles the direction on the same key, or switches to `key`
    /// ascending -- the near-universal "click the column header"
    /// behaviour, which is also what pressing a sort key twice should
    /// do.
    pub fn toggled_to(self, key: SortKey) -> Self {
        if self.key == key {
            Self { descending: !self.descending, ..self }
        } else {
            Self { key, descending: false, ..self }
        }
    }

    /// A short label for a status line: `name`, `size v`, ...
    pub fn label(self) -> String {
        let key = match self.key {
            SortKey::Name => "name",
            SortKey::Size => "size",
            SortKey::Modified => "date",
            SortKey::Extension => "type",
        };
        if self.descending {
            format!("{key} v")
        } else {
            format!("{key} ^")
        }
    }
}

/// One directory's contents, read from disk and not yet shown.
///
/// Exists so that reading (slow, and on a network share arbitrarily
/// slow) is a plain function a caller can run on whatever thread it
/// likes, while applying the result to what is on screen stays a fast,
/// synchronous, single-threaded step. `ExplorerState` never does the
/// read itself, which is what lets the host keep the UI responsive over
/// a share that may not answer for a minute.
#[derive(Debug, Clone)]
pub struct Listing {
    pub cwd: PathBuf,
    pub entries: Vec<Entry>,
}

/// Reads `path`'s immediate children. Safe to call from any thread, and
/// meant to be: this is the call that blocks.
///
/// Hidden entries are filtered here rather than at render time so that
/// everything downstream -- selection, marks, `targets()` -- operates on
/// exactly the rows the user can see. Nothing gets acted on that is not
/// on screen.
pub fn read_listing(path: &Path, show_hidden: bool) -> io::Result<Listing> {
    let mut entries: Vec<Entry> =
        fenix_fs::list_dir(path)?.into_iter().map(|info| Entry::from_fs(info, 0)).filter(|e| show_hidden || !e.is_hidden()).collect();
    sort_entries(&mut entries, Sort::default());
    Ok(Listing { cwd: path.to_path_buf(), entries })
}

/// State for one directory listing: what's shown, what's selected, and
/// what's marked. Host-agnostic -- no rendering or windowing-system
/// dependency, mirroring `fenix_vim::VimState`'s role for text editing.
///
/// Reading directories is deliberately *not* this type's job (see
/// `read_listing`); it applies listings it is handed. The operations
/// that change the filesystem do live here, but none of them re-lists
/// afterwards -- the host decides when to re-read, because on a slow
/// path that read has to happen off-thread.
pub struct ExplorerState {
    pub cwd: PathBuf,
    /// The rows currently on screen. Everything else -- selection,
    /// marks, `targets()` -- works against this and only this, so
    /// nothing can ever be acted on that is not visible.
    pub entries: Vec<Entry>,
    /// Everything that was read, before filtering. Kept so narrowing
    /// and widening the filter are instant and never touch the disk,
    /// which matters when the disk is a share.
    all_entries: Vec<Entry>,
    /// Narrows the listing to names containing this, case-insensitively.
    /// Empty means everything.
    pub filter: String,
    pub selected: usize,
    pub marks: HashSet<PathBuf>,
    pub show_hidden: bool,
    pub sort: Sort,
    /// Cached from one `git status` call per listing, applied as a
    /// second pass so badges never hold up the listing itself (see
    /// `apply_git_statuses`). `git status` is already recursive, so one
    /// scan covers every path that could appear under `cwd`, including
    /// ones not listed yet because their parent isn't expanded.
    git_statuses: HashMap<PathBuf, GitStatus>,
}

impl ExplorerState {
    /// An empty listing for `path` -- what a pane shows while its real
    /// contents are still being read.
    pub fn pending_at(path: &Path) -> Self {
        Self {
            cwd: path.to_path_buf(),
            entries: Vec::new(),
            all_entries: Vec::new(),
            filter: String::new(),
            selected: 0,
            marks: HashSet::new(),
            show_hidden: false,
            sort: Sort::default(),
            git_statuses: HashMap::new(),
        }
    }

    /// Reads `path` and applies it in one step. The synchronous path:
    /// used by tests, and by a host with no event loop to report back
    /// through -- the same "synchronous fallback when `event_proxy` is
    /// `None`" shape the rest of this workspace already uses.
    pub fn opened(path: &Path) -> io::Result<Self> {
        let mut state = Self::pending_at(path);
        let listing = read_listing(path, state.show_hidden)?;
        state.apply_listing(listing);
        state.apply_git_statuses(git::status_for_dir(path));
        Ok(state)
    }

    /// Replaces the visible entries with a freshly-read listing, keeping
    /// the selection on the same path if it survived (else clamped) and
    /// dropping marks for paths that are no longer there.
    ///
    /// Collapses any subtree expansion: the listing that was read is the
    /// top level, and re-expanding it would mean more blocking reads
    /// that the caller did not ask for.
    pub fn apply_listing(&mut self, listing: Listing) {
        let selected_path = self.entries.get(self.selected).map(|e| e.path.clone());
        self.cwd = listing.cwd;
        self.all_entries = listing.entries;
        self.apply_filter();
        self.sort_in_place();
        git::annotate_entries(&mut self.entries, &self.git_statuses);
        self.marks.retain(|p| self.entries.iter().any(|e| &e.path == p));
        self.selected =
            selected_path.and_then(|p| self.entries.iter().position(|e| e.path == p)).unwrap_or(0).min(self.entries.len().saturating_sub(1));
    }

    /// Applies a `git status` scan on top of whatever is currently
    /// listed -- the second pass, arriving after the listing it decorates
    /// rather than delaying it.
    pub fn apply_git_statuses(&mut self, statuses: HashMap<PathBuf, GitStatus>) {
        self.git_statuses = statuses;
        git::annotate_entries(&mut self.entries, &self.git_statuses);
    }

    /// Re-reads and re-applies `cwd`, synchronously. Same caveat as
    /// `opened`: for a path that might be slow, the host should read off
    /// the main thread and call `apply_listing` itself.
    pub fn refresh(&mut self) -> io::Result<()> {
        let listing = read_listing(&self.cwd, self.show_hidden)?;
        self.apply_listing(listing);
        self.apply_git_statuses(git::status_for_dir(&self.cwd));
        Ok(())
    }

    /// Narrows the listing to names containing `filter`.
    ///
    /// Purely a view over what was already read: no I/O, so typing in
    /// the filter stays instant even on a share. Clears any subtree
    /// expansion, because a child shown without the parent it belongs
    /// to is worse than not shown -- and the point of filtering is to
    /// get a flat list of what you were looking for.
    pub fn set_filter(&mut self, filter: &str) {
        let selected_path = self.entries.get(self.selected).map(|e| e.path.clone());
        filter.clone_into(&mut self.filter);
        self.apply_filter();
        self.sort_in_place();
        self.selected =
            selected_path.and_then(|p| self.entries.iter().position(|e| e.path == p)).unwrap_or(0).min(self.entries.len().saturating_sub(1));
    }

    fn apply_filter(&mut self) {
        if self.filter.is_empty() {
            self.entries = self.all_entries.clone();
            return;
        }
        let needle = self.filter.to_lowercase();
        self.entries = self
            .all_entries
            .iter()
            .filter(|entry| entry.depth == 0 && entry.name.to_lowercase().contains(&needle))
            .cloned()
            .collect();
    }

    /// Flips hidden-entry visibility. The caller re-lists afterwards --
    /// which entries exist depends on this, so it cannot be applied to
    /// the rows already in hand.
    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
    }

    /// Changes the sort and reorders what is already listed. No I/O:
    /// reordering rows is not a reason to read the directory again,
    /// which matters when reading it takes a minute.
    pub fn set_sort(&mut self, sort: Sort) {
        let selected_path = self.entries.get(self.selected).map(|e| e.path.clone());
        self.sort = sort;
        self.sort_in_place();
        if let Some(idx) = selected_path.and_then(|p| self.entries.iter().position(|e| e.path == p)) {
            self.selected = idx;
        }
    }

    fn sort_in_place(&mut self) {
        sort_tree(&mut self.entries, self.sort);
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let len = self.entries.len() as isize;
        let new = (self.selected as isize + delta).clamp(0, len - 1);
        self.selected = new as usize;
    }

    /// Puts the selection on whichever row is showing `path`, if any --
    /// how a host with its own cursor (a buffer's, rather than this
    /// type's `selected`) tells the state what the user is pointing at
    /// before an operation runs.
    pub fn select_path(&mut self, path: &Path) -> bool {
        match self.entries.iter().position(|e| e.path == path) {
            Some(idx) => {
                self.selected = idx;
                true
            }
            None => false,
        }
    }

    pub fn select_index(&mut self, index: usize) {
        if index < self.entries.len() {
            self.selected = index;
        }
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }

    /// Toggles the mark on the entry at point.
    pub fn toggle_mark(&mut self) {
        let Some(entry) = self.entries.get(self.selected) else { return };
        if !self.marks.remove(&entry.path) {
            self.marks.insert(entry.path.clone());
        }
    }

    pub fn mark_all(&mut self) {
        self.marks = self.entries.iter().map(|e| e.path.clone()).collect();
    }

    pub fn unmark_all(&mut self) {
        self.marks.clear();
    }

    /// Marks and unmarks every visible entry, flipping each independently
    /// -- dired's `t` ("toggle all marks"), distinct from `mark_all`
    /// (which only ever adds).
    pub fn toggle_all_marks(&mut self) {
        for entry in &self.entries {
            if !self.marks.remove(&entry.path) {
                self.marks.insert(entry.path.clone());
            }
        }
    }

    /// The entries an operation (delete/rename/copy/move) should act on:
    /// every marked entry if there are any, else just the entry at point
    /// -- the standard dired convention ("act on the marked set, or on
    /// what's under the cursor if nothing's marked").
    pub fn targets(&self) -> Vec<&Entry> {
        if self.marks.is_empty() {
            self.selected_entry().into_iter().collect()
        } else {
            self.entries.iter().filter(|e| self.marks.contains(&e.path)).collect()
        }
    }

    /// The paths `targets()` names -- the form every operation actually
    /// wants, and one that does not borrow `self`.
    pub fn target_paths(&self) -> Vec<PathBuf> {
        self.targets().iter().map(|e| e.path.clone()).collect()
    }

    /// Expands the directory at point in place (inserting its children
    /// right after it, indented one level deeper) if collapsed, or
    /// collapses it (removing every row below it whose depth is greater --
    /// its children and, transitively, anything expanded under them) if
    /// already expanded. A no-op on a plain file. Keeps the flat
    /// `entries` list as the single source of truth for both rendering
    /// and selection -- no separate tree structure to keep in sync.
    ///
    /// Reads the child directory synchronously, so a host that cares
    /// about slow paths expands via `expand_with` instead.
    pub fn toggle_expand(&mut self) -> io::Result<()> {
        let Some(entry) = self.entries.get(self.selected) else { return Ok(()) };
        if !entry.is_dir() {
            return Ok(());
        }
        let path = entry.path.clone();
        if self.collapse_at(self.selected) {
            return Ok(());
        }
        let children = read_listing(&path, self.show_hidden)?.entries;
        self.expand_with(self.selected, children);
        Ok(())
    }

    /// Whether the row at `index` currently has its children shown.
    pub fn is_expanded(&self, index: usize) -> bool {
        let Some(entry) = self.entries.get(index) else { return false };
        self.entries.get(index + 1).is_some_and(|next| next.depth > entry.depth)
    }

    /// Collapses the row at `index`, returning whether there was
    /// anything to collapse.
    pub fn collapse_at(&mut self, index: usize) -> bool {
        if !self.is_expanded(index) {
            return false;
        }
        let depth = self.entries[index].depth;
        let start = index + 1;
        let mut end = start;
        while end < self.entries.len() && self.entries[end].depth > depth {
            end += 1;
        }
        self.entries.drain(start..end);
        self.all_entries = self.entries.clone();
        true
    }

    /// Splices an already-read child listing in under the row at
    /// `index` -- the half of `toggle_expand` that does no I/O, for a
    /// host that read the children off-thread.
    pub fn expand_with(&mut self, index: usize, children: Vec<Entry>) {
        // Expansion is a view of the unfiltered listing; a filter
        // flattens it away (see `set_filter`), so the two never apply at
        // once and this only has to keep `all_entries` in step.
        if !self.filter.is_empty() {
            return;
        }
        let Some(parent) = self.entries.get(index) else { return };
        let depth = parent.depth;
        let mut children: Vec<Entry> = children
            .into_iter()
            .map(|mut child| {
                child.depth = depth + 1;
                child
            })
            .collect();
        sort_entries(&mut children, self.sort);
        let insert_at = index + 1;
        for (i, child) in children.into_iter().enumerate() {
            self.entries.insert(insert_at + i, child);
        }
        git::annotate_entries(&mut self.entries, &self.git_statuses);
        self.all_entries = self.entries.clone();
    }

    /// Creates an empty file at `name`, which may include directory
    /// separators (`docs/notes.md` creates `docs/` too) -- typing the
    /// path you want is faster than creating each level by hand.
    /// Fails with `AlreadyExists` rather than silently truncating an
    /// existing file, unlike `fs::File::create`.
    pub fn create_file(&self, name: &str) -> io::Result<PathBuf> {
        let path = self.cwd.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
        Ok(path)
    }

    /// Creates a directory at `name`, including any missing parents.
    pub fn create_dir(&self, name: &str) -> io::Result<PathBuf> {
        let path = self.cwd.join(name);
        fs::create_dir_all(&path)?;
        Ok(path)
    }

    /// Renames the entry at point. Refuses to overwrite an existing
    /// path. `new_name` may contain separators, so this doubles as a
    /// single-target move.
    pub fn rename_selected(&self, new_name: &str) -> io::Result<PathBuf> {
        let Some(entry) = self.selected_entry() else {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no entry selected"));
        };
        let new_path = self.cwd.join(new_name);
        if new_path.exists() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} already exists", new_path.display())));
        }
        if let Some(parent) = new_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&entry.path, &new_path)?;
        Ok(new_path)
    }
}

/// Sorts a flat listing that may contain expanded subtrees, keeping each
/// group of siblings together with the descendants below it.
///
/// A plain sort over the whole vector would scatter children away from
/// their parents and turn the tree into nonsense. This walks the flat
/// list, finds each run of siblings at the same depth, sorts the run,
/// and moves each sibling's whole block of descendants with it.
pub fn sort_tree(entries: &mut Vec<Entry>, sort: Sort) {
    if entries.is_empty() {
        return;
    }
    let depth = entries[0].depth;
    let taken = std::mem::take(entries);

    // Split into (sibling, everything nested under it) blocks.
    let mut blocks: Vec<(Entry, Vec<Entry>)> = Vec::new();
    for entry in taken {
        if entry.depth == depth {
            blocks.push((entry, Vec::new()));
        } else if let Some(last) = blocks.last_mut() {
            last.1.push(entry);
        }
    }

    let mut heads: Vec<Entry> = blocks.iter().map(|(head, _)| head.clone()).collect();
    sort_entries(&mut heads, sort);

    for head in heads {
        let Some(pos) = blocks.iter().position(|(h, _)| h.path == head.path) else { continue };
        let (head, mut nested) = blocks.remove(pos);
        entries.push(head);
        sort_tree(&mut nested, sort);
        entries.append(&mut nested);
    }
}

/// Orders one run of siblings. Ties always fall back to the name, so
/// the order is stable and predictable even when every file has the same
/// size or timestamp.
pub(crate) fn sort_entries(entries: &mut [Entry], sort: Sort) {
    entries.sort_by(|a, b| {
        if sort.dirs_first {
            let by_kind = b.is_dir().cmp(&a.is_dir());
            if by_kind != std::cmp::Ordering::Equal {
                return by_kind;
            }
        }
        let by_key = match sort.key {
            SortKey::Name => std::cmp::Ordering::Equal,
            SortKey::Size => a.size.cmp(&b.size),
            SortKey::Modified => a.modified.cmp(&b.modified),
            SortKey::Extension => a.extension().cmp(&b.extension()),
        };
        let by_key = if sort.descending { by_key.reverse() } else { by_key };
        if by_key != std::cmp::Ordering::Equal {
            return by_key;
        }
        let by_name = a.name.to_lowercase().cmp(&b.name.to_lowercase());
        // The name is the key here, not a tie-break, so it is the one
        // that follows the direction.
        if sort.key == SortKey::Name && sort.descending {
            by_name.reverse()
        } else {
            by_name
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn opened_lists_entries_sorted_directories_first_then_alphabetical() {
        let dir = TempDir::new("open_sorted");
        dir.touch("zebra.txt");
        dir.touch("apple.txt");
        dir.mkdir("zoo");
        dir.mkdir("bear");

        let state = ExplorerState::opened(dir.path()).unwrap();
        let names: Vec<&str> = state.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["bear", "zoo", "apple.txt", "zebra.txt"]);
        assert!(state.entries[0].is_dir());
        assert!(!state.entries[2].is_dir());
    }

    #[test]
    fn a_pending_listing_is_empty_but_knows_where_it_is() {
        // What a pane shows while a slow directory is still being read.
        let state = ExplorerState::pending_at(Path::new("\\\\server\\share"));
        assert!(state.entries.is_empty());
        assert_eq!(state.cwd, Path::new("\\\\server\\share"));
    }

    #[test]
    fn hidden_entries_are_filtered_when_reading_not_when_rendering() {
        // So that selection, marks and `targets()` can never name a row
        // the user cannot see.
        let dir = TempDir::new("read_hidden");
        dir.touch("visible.txt");
        let hidden = dir.touch("hidden.txt");
        set_hidden(&hidden);

        assert_eq!(read_listing(dir.path(), false).unwrap().entries.len(), 1);
        assert_eq!(read_listing(dir.path(), true).unwrap().entries.len(), 2);
    }

    #[test]
    fn toggle_hidden_flips_the_flag_and_leaves_re_reading_to_the_caller() {
        let dir = TempDir::new("toggle_hidden");
        let mut state = ExplorerState::opened(dir.path()).unwrap();
        assert!(!state.show_hidden);
        state.toggle_hidden();
        assert!(state.show_hidden);
    }

    #[test]
    fn move_selection_clamps_at_both_ends() {
        let dir = TempDir::new("move_selection_clamps");
        dir.touch("a");
        dir.touch("b");
        dir.touch("c");
        let mut state = ExplorerState::opened(dir.path()).unwrap();

        state.move_selection(-5);
        assert_eq!(state.selected, 0);
        state.move_selection(1);
        assert_eq!(state.selected, 1);
        state.move_selection(50);
        assert_eq!(state.selected, 2);
    }

    #[test]
    fn select_path_points_the_state_at_a_row_by_path() {
        // How a host whose real cursor lives elsewhere (a buffer's) tells
        // the state what an operation should act on.
        let dir = TempDir::new("select_path");
        dir.touch("a.txt");
        let b = dir.touch("b.txt");
        let mut state = ExplorerState::opened(dir.path()).unwrap();

        assert!(state.select_path(&b));
        assert_eq!(state.selected_entry().unwrap().name, "b.txt");
        assert!(!state.select_path(&dir.path().join("nope.txt")));
    }

    #[test]
    fn targets_are_the_marked_set_or_the_entry_at_point() {
        let dir = TempDir::new("targets");
        dir.touch("a.txt");
        dir.touch("b.txt");
        dir.touch("c.txt");
        let mut state = ExplorerState::opened(dir.path()).unwrap();

        assert_eq!(state.target_paths().len(), 1, "nothing marked -- just what's under the cursor");

        state.toggle_mark();
        state.move_selection(2);
        state.toggle_mark();
        let names: Vec<String> =
            state.targets().iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["a.txt", "c.txt"], "marked wins over the cursor");
    }

    // -- Sorting ---------------------------------------------------------

    fn names_sorted_by(dir: &TempDir, sort: Sort) -> Vec<String> {
        let mut state = ExplorerState::opened(dir.path()).unwrap();
        state.set_sort(sort);
        state.entries.iter().map(|e| e.name.clone()).collect()
    }

    #[test]
    fn sorting_by_size_orders_files_by_size_and_keeps_directories_first() {
        let dir = TempDir::new("sort_size");
        dir.write("big.txt", "0123456789");
        dir.write("small.txt", "x");
        dir.mkdir("adir");

        let names = names_sorted_by(&dir, Sort { key: SortKey::Size, descending: false, dirs_first: true });
        assert_eq!(names, vec!["adir", "small.txt", "big.txt"]);

        let names = names_sorted_by(&dir, Sort { key: SortKey::Size, descending: true, dirs_first: true });
        assert_eq!(names, vec!["adir", "big.txt", "small.txt"]);
    }

    #[test]
    fn sorting_by_name_descending_reverses_the_names() {
        let dir = TempDir::new("sort_name_desc");
        dir.touch("a.txt");
        dir.touch("b.txt");
        let names = names_sorted_by(&dir, Sort { key: SortKey::Name, descending: true, dirs_first: true });
        assert_eq!(names, vec!["b.txt", "a.txt"]);
    }

    #[test]
    fn sorting_by_type_groups_extensions_and_breaks_ties_by_name() {
        let dir = TempDir::new("sort_ext");
        dir.touch("b.rs");
        dir.touch("a.rs");
        dir.touch("c.md");

        let names = names_sorted_by(&dir, Sort { key: SortKey::Extension, descending: false, dirs_first: true });
        assert_eq!(names, vec!["c.md", "a.rs", "b.rs"]);
    }

    #[test]
    fn turning_off_dirs_first_mixes_them_in_by_the_sort_key() {
        let dir = TempDir::new("sort_no_dirs_first");
        dir.touch("a.txt");
        dir.mkdir("m");
        dir.touch("z.txt");

        let names = names_sorted_by(&dir, Sort { key: SortKey::Name, descending: false, dirs_first: false });
        assert_eq!(names, vec!["a.txt", "m", "z.txt"]);
    }

    #[test]
    fn sorting_keeps_the_selection_on_the_same_entry() {
        // Reordering rows must not silently move what an operation would
        // act on.
        let dir = TempDir::new("sort_keeps_selection");
        dir.write("a.txt", "0123456789");
        dir.write("z.txt", "x");
        let mut state = ExplorerState::opened(dir.path()).unwrap();
        state.select_path(&dir.path().join("a.txt"));

        state.set_sort(Sort { key: SortKey::Size, descending: false, dirs_first: true });

        assert_eq!(state.selected_entry().unwrap().name, "a.txt");
        assert_eq!(state.selected, 1, "it really did move rows");
    }

    #[test]
    fn sorting_an_expanded_tree_keeps_children_under_their_own_parent() {
        // The bug a flat sort would introduce: children scattered away
        // from the directory they belong to.
        let dir = TempDir::new("sort_tree");
        let a = dir.mkdir("a");
        std::fs::write(a.join("zzz.txt"), "").unwrap();
        std::fs::write(a.join("aaa.txt"), "").unwrap();
        dir.mkdir("b");
        let mut state = ExplorerState::opened(dir.path()).unwrap();
        state.select_path(&a);
        state.toggle_expand().unwrap();

        state.set_sort(Sort { key: SortKey::Name, descending: true, dirs_first: true });

        let rows: Vec<(String, usize)> = state.entries.iter().map(|e| (e.name.clone(), e.depth)).collect();
        assert_eq!(
            rows,
            vec![
                ("b".to_string(), 0),
                ("a".to_string(), 0),
                ("zzz.txt".to_string(), 1),
                ("aaa.txt".to_string(), 1),
            ],
            "children stay under `a`, and are themselves reversed"
        );
    }

    // -- Filtering -------------------------------------------------------

    #[test]
    fn a_filter_narrows_the_listing_to_matching_names() {
        let dir = TempDir::new("filter_narrow");
        dir.touch("alpha.txt");
        dir.touch("beta.txt");
        dir.touch("alpine.rs");
        let mut state = ExplorerState::opened(dir.path()).unwrap();

        state.set_filter("alp");

        let names: Vec<&str> = state.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["alpha.txt", "alpine.rs"]);
    }

    #[test]
    fn a_filter_ignores_case() {
        let dir = TempDir::new("filter_case");
        dir.touch("README.md");
        let mut state = ExplorerState::opened(dir.path()).unwrap();
        state.set_filter("readme");
        assert_eq!(state.entries.len(), 1);
    }

    #[test]
    fn clearing_the_filter_brings_everything_back_without_re_reading() {
        let dir = TempDir::new("filter_clear");
        dir.touch("a.txt");
        dir.touch("b.txt");
        let mut state = ExplorerState::opened(dir.path()).unwrap();
        state.set_filter("a");
        assert_eq!(state.entries.len(), 1);

        state.set_filter("");

        assert_eq!(state.entries.len(), 2);
    }

    #[test]
    fn nothing_hidden_by_a_filter_can_be_acted_on() {
        // The invariant the whole design rests on: `targets()` names
        // rows, and a row you cannot see is not one you meant.
        let dir = TempDir::new("filter_targets");
        dir.touch("keep.txt");
        dir.touch("other.txt");
        let mut state = ExplorerState::opened(dir.path()).unwrap();
        state.set_filter("keep");

        state.mark_all();

        assert_eq!(state.target_paths(), vec![dir.path().join("keep.txt")]);
    }

    #[test]
    fn a_filter_keeps_the_selection_on_the_same_entry_when_it_survives() {
        let dir = TempDir::new("filter_selection");
        dir.touch("alpha.txt");
        dir.touch("beta.txt");
        let mut state = ExplorerState::opened(dir.path()).unwrap();
        state.select_path(&dir.path().join("beta.txt"));

        state.set_filter("bet");

        assert_eq!(state.selected_entry().unwrap().name, "beta.txt");
    }

    #[test]
    fn a_filter_that_matches_nothing_leaves_an_empty_listing_rather_than_everything() {
        let dir = TempDir::new("filter_empty");
        dir.touch("a.txt");
        let mut state = ExplorerState::opened(dir.path()).unwrap();
        state.set_filter("zzz");
        assert!(state.entries.is_empty());
        assert!(state.selected_entry().is_none());
    }

    #[test]
    fn re_listing_keeps_the_filter_applied() {
        // Otherwise every operation would silently widen the view back
        // out from under you.
        let dir = TempDir::new("filter_survives_refresh");
        dir.touch("alpha.txt");
        dir.touch("beta.txt");
        let mut state = ExplorerState::opened(dir.path()).unwrap();
        state.set_filter("alp");

        state.refresh().unwrap();

        assert_eq!(state.entries.len(), 1);
    }

    // -- Expansion -------------------------------------------------------

    #[test]
    fn expanding_then_collapsing_restores_the_flat_listing() {
        let dir = TempDir::new("expand_collapse");
        let sub = dir.mkdir("sub");
        std::fs::write(sub.join("inner.txt"), "").unwrap();
        dir.touch("outer.txt");
        let mut state = ExplorerState::opened(dir.path()).unwrap();
        state.select_path(&sub);

        state.toggle_expand().unwrap();
        assert_eq!(state.entries.len(), 3);
        assert!(state.is_expanded(state.selected));

        state.toggle_expand().unwrap();
        assert_eq!(state.entries.len(), 2);
        assert!(!state.is_expanded(state.selected));
    }

    #[test]
    fn expanding_a_file_does_nothing() {
        let dir = TempDir::new("expand_file");
        dir.touch("a.txt");
        let mut state = ExplorerState::opened(dir.path()).unwrap();
        state.toggle_expand().unwrap();
        assert_eq!(state.entries.len(), 1);
    }

    // -- Operations ------------------------------------------------------

    #[test]
    fn create_file_makes_missing_parent_directories() {
        // Typing the path you want beats creating each level by hand.
        let dir = TempDir::new("create_nested");
        let state = ExplorerState::opened(dir.path()).unwrap();

        let made = state.create_file("docs/deep/notes.md").unwrap();

        assert!(made.exists());
        assert_eq!(made, dir.path().join("docs").join("deep").join("notes.md"));
    }

    #[test]
    fn create_file_refuses_to_truncate_something_that_exists() {
        let dir = TempDir::new("create_existing");
        dir.write("keep.txt", "precious");
        let state = ExplorerState::opened(dir.path()).unwrap();

        let err = state.create_file("keep.txt").unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(dir.path().join("keep.txt")).unwrap(), "precious");
    }

    #[test]
    fn rename_refuses_to_overwrite_and_says_what_is_in_the_way() {
        let dir = TempDir::new("rename_conflict");
        dir.write("a.txt", "one");
        dir.write("b.txt", "two");
        let mut state = ExplorerState::opened(dir.path()).unwrap();
        state.select_path(&dir.path().join("a.txt"));

        let err = state.rename_selected("b.txt").unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(err.to_string().contains("b.txt"), "got: {err}");
        assert_eq!(std::fs::read_to_string(dir.path().join("b.txt")).unwrap(), "two", "untouched");
    }

    #[test]
    fn rename_can_move_into_a_subdirectory() {
        let dir = TempDir::new("rename_into_subdir");
        dir.write("a.txt", "one");
        let mut state = ExplorerState::opened(dir.path()).unwrap();
        state.select_path(&dir.path().join("a.txt"));

        let moved = state.rename_selected("sub/a.txt").unwrap();

        assert!(moved.exists());
        assert!(!dir.path().join("a.txt").exists());
    }

    /// Marks a path hidden the way the platform means it, so the hidden
    /// test exercises the real rule rather than a filename convention.
    fn set_hidden(path: &Path) {
        #[cfg(windows)]
        {
            std::process::Command::new("attrib").arg("+H").arg(path).output().expect("attrib");
        }
        #[cfg(not(windows))]
        {
            let hidden = path.parent().unwrap().join(format!(".{}", path.file_name().unwrap().to_string_lossy()));
            std::fs::rename(path, hidden).unwrap();
        }
    }
}
