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

/// One directory's contents, read from disk and not yet shown -- with
/// the folders that were open under it, read again alongside it.
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
    /// Open folders under `cwd` and what is in them, so a refresh keeps
    /// the tree as it was rather than folding it all away.
    pub folders: Vec<(PathBuf, Vec<Entry>)>,
}

/// Reads `path`'s immediate children. Safe to call from any thread, and
/// meant to be: this is the call that blocks.
///
/// Hidden entries are filtered here rather than at render time so that
/// everything downstream -- selection, marks, `targets()` -- operates on
/// exactly the rows the user can see. Nothing gets acted on that is not
/// on screen.
pub fn read_listing(path: &Path, show_hidden: bool) -> io::Result<Listing> {
    Ok(Listing { cwd: path.to_path_buf(), entries: read_children(path, show_hidden)?, folders: Vec::new() })
}

/// `read_listing`, plus each of the `open` folders under it. A folder
/// that can no longer be read (deleted, renamed) is left out, and so
/// closes; only `path` itself failing is an error.
pub fn read_tree(path: &Path, open: &[PathBuf], show_hidden: bool) -> io::Result<Listing> {
    let mut listing = read_listing(path, show_hidden)?;
    listing.folders = open
        .iter()
        .filter(|dir| dir.starts_with(path) && dir.as_path() != path)
        .filter_map(|dir| read_children(dir, show_hidden).ok().map(|entries| (dir.clone(), entries)))
        .collect();
    Ok(listing)
}

/// One folder's entries, hidden ones left out unless asked for.
pub fn read_children(path: &Path, show_hidden: bool) -> io::Result<Vec<Entry>> {
    let mut entries: Vec<Entry> =
        fenix_fs::list_dir(path)?.into_iter().map(|info| Entry::from_fs(info, 0)).filter(|e| show_hidden || !e.is_hidden()).collect();
    sort_entries(&mut entries, Sort::default());
    Ok(entries)
}

/// How a visible row sits in the tree, beside its `Entry`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Row {
    /// The name shown: the entry's own, or -- with compact folders -- a
    /// run of folders that each hold only the next, `src/main/java`,
    /// whose row is the last of them.
    pub label: String,
    /// Whether its children are shown under it.
    pub expanded: bool,
    /// For each level above it, whether a guide runs down through this
    /// row there: that ancestor still has siblings to come.
    pub guides: Vec<bool>,
    /// Whether it is the last of its siblings.
    pub last: bool,
    /// Whether it matched the filter itself, rather than being shown as
    /// the way down to something that did.
    pub matched: bool,
    /// Where in `label` the filter matched, in chars.
    pub hit: Option<std::ops::Range<usize>>,
}

/// State for one directory listing, shown as a tree: what's read,
/// which folders are open, what's selected and what's marked.
/// Host-agnostic -- no rendering or windowing-system dependency,
/// mirroring `fenix_vim::VimState`'s role for text editing.
///
/// Reading directories is deliberately *not* this type's job (see
/// `read_listing`); it applies listings it is handed. The operations
/// that change the filesystem do live here, but none of them re-lists
/// afterwards -- the host decides when to re-read, because on a slow
/// path that read has to happen off-thread.
pub struct ExplorerState {
    pub cwd: PathBuf,
    /// The rows currently on screen, the tree flattened. Everything
    /// else -- selection, marks, `targets()` -- works against this and
    /// only this, so nothing can ever be acted on that is not visible.
    pub entries: Vec<Entry>,
    /// Parallel to `entries`: where each row sits in the tree.
    pub rows: Vec<Row>,
    /// Every folder read so far, by path: `cwd` and the folders opened
    /// under it. Kept so folding, filtering and sorting are instant and
    /// never touch the disk, which matters when the disk is a share.
    children: HashMap<PathBuf, Vec<Entry>>,
    /// The folders that are open. Kept across a refresh.
    expanded: HashSet<PathBuf>,
    /// Narrows the tree to names containing this, case-insensitively,
    /// keeping the folders on the way down to each. Empty means
    /// everything.
    pub filter: String,
    /// What a search below `cwd` found for `filter` (`set_found`).
    found: Option<Vec<Entry>>,
    pub selected: usize,
    pub marks: HashSet<PathBuf>,
    pub show_hidden: bool,
    pub sort: Sort,
    /// A folder holding only one folder shares its row with it
    /// (`src/main/java`). On by default.
    pub compact: bool,
    /// Names not listed, as globs (`*.pyc`, `node_modules`).
    hide: Vec<String>,
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
            rows: Vec::new(),
            children: HashMap::new(),
            expanded: HashSet::new(),
            filter: String::new(),
            found: None,
            selected: 0,
            marks: HashSet::new(),
            show_hidden: false,
            sort: Sort::default(),
            compact: true,
            hide: Vec::new(),
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

    /// Replaces what was read with a fresh listing, keeping the
    /// selection on the same path if it survived (else near where it
    /// was) and dropping marks for paths that are no longer there.
    ///
    /// The same directory again keeps its open folders -- those that
    /// the listing read again; another directory starts folded, at the
    /// top.
    pub fn apply_listing(&mut self, listing: Listing) {
        let moved = listing.cwd != self.cwd;
        self.cwd = listing.cwd;
        self.children.clear();
        self.children.insert(self.cwd.clone(), listing.entries);
        if moved {
            self.expanded.clear();
            self.found = None;
        }
        // The folders read with it are the open ones -- kept open, or
        // opened again (a session's).
        self.expanded.extend(listing.folders.iter().map(|(dir, _)| dir.clone()));
        self.children.extend(listing.folders);
        let children = &self.children;
        self.expanded.retain(|dir| children.contains_key(dir));
        self.rebuild();
        if moved {
            self.selected = 0;
        }
        self.marks.retain(|p| self.entries.iter().any(|e| &e.path == p));
    }

    /// Applies a `git status` scan on top of whatever is currently
    /// listed -- the second pass, arriving after the listing it decorates
    /// rather than delaying it.
    pub fn apply_git_statuses(&mut self, statuses: HashMap<PathBuf, GitStatus>) {
        self.git_statuses = statuses;
        self.rebuild();
    }

    /// Re-reads and re-applies `cwd` and its open folders,
    /// synchronously. Same caveat as `opened`: for a path that might be
    /// slow, the host should read off the main thread and call
    /// `apply_listing` itself.
    pub fn refresh(&mut self) -> io::Result<()> {
        let listing = read_tree(&self.cwd, &self.open_folders(), self.show_hidden)?;
        self.apply_listing(listing);
        self.apply_git_statuses(git::status_for_dir(&self.cwd));
        Ok(())
    }

    /// The folders that are open, for reading again with `read_tree`.
    pub fn open_folders(&self) -> Vec<PathBuf> {
        let mut open: Vec<PathBuf> = self.expanded.iter().cloned().collect();
        open.sort();
        open
    }

    /// How many things `cwd` itself holds, whatever is open or filtered.
    pub fn item_count(&self) -> usize {
        self.children.get(&self.cwd).map_or(0, Vec::len)
    }

    /// How many paths under `cwd` git says changed (ignored ones aside).
    pub fn changed_count(&self) -> usize {
        self.git_statuses.values().filter(|s| **s != GitStatus::Ignored).count()
    }

    /// Narrows the tree to names containing `filter`, with the folders
    /// on the way down to each opened to show it.
    ///
    /// Purely a view over what was already read: no I/O, so typing in
    /// the filter stays instant even on a share. Clearing it brings the
    /// tree back folded as it was.
    pub fn set_filter(&mut self, filter: &str) {
        if filter.trim().is_empty() {
            self.found = None;
        }
        filter.clone_into(&mut self.filter);
        self.rebuild();
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
        self.sort = sort;
        self.rebuild();
    }

    /// Names not to list, as globs: `*` any run of characters, `?` any
    /// one, ignoring case.
    pub fn set_hide(&mut self, globs: &[String]) {
        self.hide = globs.iter().map(|g| g.trim().to_string()).filter(|g| !g.is_empty()).collect();
        self.rebuild();
    }

    /// Turns compact folders on or off.
    pub fn set_compact(&mut self, compact: bool) {
        self.compact = compact;
        self.rebuild();
    }

    /// Flattens the tree into `entries` and `rows` again, keeping the
    /// selection on the same path when it is still shown, else near
    /// where it was.
    fn rebuild(&mut self) {
        let selected_path = self.entries.get(self.selected).map(|e| e.path.clone());
        let needle = self.filter.to_lowercase();
        // While filtering, what a search under `cwd` found joins what has
        // been read, with the folders on the way down to each.
        let merged;
        let tree = match self.found.as_ref().filter(|_| !needle.is_empty()) {
            Some(found) => {
                merged = self.with_found(found);
                &merged
            }
            None => &self.children,
        };
        let mut entries = Vec::new();
        let mut rows = Vec::new();
        let mut guides = Vec::new();
        self.walk(tree, &self.cwd, 0, &mut guides, &needle, &mut entries, &mut rows);
        git::annotate_entries(&mut entries, &self.git_statuses);
        // Nothing under an ignored folder is listed by git, and all of
        // it is just as ignored.
        let mut ignored_at: Vec<bool> = Vec::new();
        for entry in entries.iter_mut() {
            ignored_at.truncate(entry.depth);
            if entry.git_status.is_none() && ignored_at.iter().any(|&i| i) {
                entry.git_status = Some(GitStatus::Ignored);
            }
            ignored_at.push(entry.git_status == Some(GitStatus::Ignored));
        }
        self.entries = entries;
        self.rows = rows;
        self.selected = selected_path
            .and_then(|p| self.entries.iter().position(|e| e.path == p))
            .unwrap_or(self.selected)
            .min(self.entries.len().saturating_sub(1));
    }

    /// What has been read, with `found` added: each found entry in its
    /// folder, and each folder between it and `cwd` in the one above.
    fn with_found(&self, found: &[Entry]) -> HashMap<PathBuf, Vec<Entry>> {
        let mut tree = self.children.clone();
        for entry in found {
            let mut child = entry.path.clone();
            while let Some(parent) = child.parent().map(Path::to_path_buf) {
                if !parent.starts_with(&self.cwd) {
                    break;
                }
                let list = tree.entry(parent.clone()).or_default();
                if !list.iter().any(|e| e.path == child) {
                    list.push(if child == entry.path { entry.clone() } else { Entry::folder(&child) });
                }
                if parent == self.cwd {
                    break;
                }
                child = parent;
            }
        }
        tree
    }

    #[allow(clippy::too_many_arguments)]
    fn walk(&self, tree: &HashMap<PathBuf, Vec<Entry>>, dir: &Path, depth: usize, guides: &mut Vec<bool>, needle: &str, entries: &mut Vec<Entry>, rows: &mut Vec<Row>) {
        let Some(children) = tree.get(dir) else { return };
        let mut children: Vec<&Entry> = children
            .iter()
            .filter(|c| !self.hide.iter().any(|g| glob_matches(g, &c.name)))
            .filter(|c| needle.is_empty() || shows_under_filter(tree, c, needle))
            .collect();
        sort_refs(&mut children, self.sort);
        let count = children.len();
        for (i, child) in children.into_iter().enumerate() {
            let last = i + 1 == count;
            let hit = (!needle.is_empty()).then(|| find_ignoring_case(&child.name, needle)).flatten();
            let mut entry = child.clone();
            let mut label = child.name.clone();
            if self.compact && needle.is_empty() {
                while let Some(only) = self.only_folder_in(&entry.path) {
                    label = format!("{label}/{}", only.name);
                    entry = only.clone();
                }
            }
            entry.depth = depth;
            let expanded = entry.is_dir()
                && tree.contains_key(&entry.path)
                && if needle.is_empty() { self.expanded.contains(&entry.path) } else { has_match_under(tree, &entry.path, needle) };
            let path = entry.path.clone();
            entries.push(entry);
            rows.push(Row { label, expanded, guides: guides.clone(), last, matched: hit.is_some(), hit });
            if expanded {
                guides.push(!last);
                self.walk(tree, &path, depth + 1, guides, needle, entries, rows);
                guides.pop();
            }
        }
    }

    /// With compact folders: the one folder an open `dir` holds, when it
    /// holds nothing else.
    fn only_folder_in(&self, dir: &Path) -> Option<&Entry> {
        if !self.expanded.contains(dir) {
            return None;
        }
        match self.children.get(dir)?.as_slice() {
            [only] if only.is_dir() => Some(only),
            _ => None,
        }
    }

    /// What a search under `cwd` found for the filter: entries anywhere
    /// below it, not only in the folders read. The filter shows them
    /// with the folders on the way down; a new filter or another
    /// directory drops them. Hidden ones are left out unless hidden
    /// entries are shown.
    pub fn set_found(&mut self, found: Vec<Entry>) {
        let cwd = self.cwd.clone();
        let show_hidden = self.show_hidden;
        let visible = |e: &Entry| {
            show_hidden || e.path.strip_prefix(&cwd).is_ok_and(|rel| !rel.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.')))
        };
        self.found = Some(found.into_iter().filter(visible).collect());
        self.rebuild();
    }

    /// Selects the next row the filter matched (`forward`), or the one
    /// before, wrapping. Whether there was one.
    pub fn next_match(&mut self, forward: bool) -> bool {
        let n = self.rows.len();
        let found = (1..=n)
            .map(|step| if forward { (self.selected + step) % n } else { (self.selected + n - step % n) % n })
            .find(|&i| self.rows[i].matched);
        if let Some(i) = found {
            self.selected = i;
        }
        found.is_some()
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

    /// Opens the folder at point (its children shown under it, one
    /// level deeper), or closes it if it is open. A no-op on a plain
    /// file. With compact folders, a folder holding only a folder opens
    /// that one too, down to where the tree branches.
    ///
    /// Reads synchronously, so a host that cares about slow paths reads
    /// off-thread and calls `open_folder` instead.
    pub fn toggle_expand(&mut self) -> io::Result<()> {
        let Some(entry) = self.entries.get(self.selected) else { return Ok(()) };
        if !entry.is_dir() {
            return Ok(());
        }
        let mut dir = entry.path.clone();
        if self.collapse_at(self.selected) {
            return Ok(());
        }
        // Bounded, so a link loop cannot keep it going for ever.
        for _ in 0..64 {
            let children = read_children(&dir, self.show_hidden)?;
            self.open_folder(&dir, children);
            match self.compact_next(&dir) {
                Some(next) => dir = next,
                None => break,
            }
        }
        Ok(())
    }

    /// With compact folders on: the one folder `dir` holds, when it
    /// holds nothing else and is not open yet -- the next to read and
    /// open, so a chain opens to where it branches.
    pub fn compact_next(&self, dir: &Path) -> Option<PathBuf> {
        if !self.compact {
            return None;
        }
        match self.children.get(dir)?.as_slice() {
            [only] if only.is_dir() && !self.expanded.contains(&only.path) => Some(only.path.clone()),
            _ => None,
        }
    }

    /// Whether the row at `index` currently has its children shown.
    pub fn is_expanded(&self, index: usize) -> bool {
        self.rows.get(index).is_some_and(|r| r.expanded)
    }

    /// Closes the folder at `index`, returning whether it was open.
    pub fn collapse_at(&mut self, index: usize) -> bool {
        if !self.is_expanded(index) {
            return false;
        }
        let path = self.entries[index].path.clone();
        self.expanded.remove(&path);
        self.rebuild();
        true
    }

    /// Splices an already-read child listing in under the row at
    /// `index` -- the half of `toggle_expand` that does no I/O, for a
    /// host that read the children off-thread.
    pub fn expand_with(&mut self, index: usize, children: Vec<Entry>) {
        let Some(entry) = self.entries.get(index).filter(|e| e.is_dir()) else { return };
        let path = entry.path.clone();
        self.open_folder(&path, children);
    }

    /// Opens `dir`, with `children` read from it.
    pub fn open_folder(&mut self, dir: &Path, children: Vec<Entry>) {
        self.children.insert(dir.to_path_buf(), children);
        self.expanded.insert(dir.to_path_buf());
        self.rebuild();
    }

    /// The folders between `cwd` and `path` that have to be opened for
    /// `path` to have a row, outermost first -- what to read for
    /// `reveal`. Empty when nothing needs opening, or `path` isn't under
    /// `cwd` at all.
    pub fn folders_to_reveal(&self, path: &Path) -> Vec<PathBuf> {
        let Ok(rel) = path.strip_prefix(&self.cwd) else { return Vec::new() };
        let parts: Vec<_> = rel.components().collect();
        let mut dir = self.cwd.clone();
        let mut out = Vec::new();
        for part in parts.iter().take(parts.len().saturating_sub(1)) {
            dir.push(part);
            if !(self.expanded.contains(&dir) && self.children.contains_key(&dir)) {
                out.push(dir.clone());
            }
        }
        out
    }

    /// Opens `folders` (as `folders_to_reveal` named them, each read)
    /// and puts the selection on `path`. Whether it has a row now.
    pub fn reveal(&mut self, path: &Path, folders: Vec<(PathBuf, Vec<Entry>)>) -> bool {
        for (dir, children) in folders {
            self.children.insert(dir.clone(), children);
            self.expanded.insert(dir);
        }
        self.rebuild();
        self.select_path(path)
    }

    /// Creates an empty file at `name` in `cwd` -- see `create_file_in`.
    pub fn create_file(&self, name: &str) -> io::Result<PathBuf> {
        create_file_in(&self.cwd, name)
    }

    /// Creates a directory at `name` in `cwd` -- see `create_dir_in`.
    pub fn create_dir(&self, name: &str) -> io::Result<PathBuf> {
        create_dir_in(&self.cwd, name)
    }

    /// Renames the entry at point, within the folder it is in. Refuses
    /// to overwrite an existing path. `new_name` may contain separators,
    /// so this doubles as a single-target move.
    pub fn rename_selected(&self, new_name: &str) -> io::Result<PathBuf> {
        let Some(entry) = self.selected_entry() else {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no entry selected"));
        };
        let new_path = entry.path.parent().unwrap_or(&self.cwd).join(new_name);
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

/// Creates an empty file at `name` in `dir`, which may include directory
/// separators (`docs/notes.md` creates `docs/` too) -- typing the path
/// you want is faster than creating each level by hand. Fails with
/// `AlreadyExists` rather than silently truncating an existing file,
/// unlike `fs::File::create`.
pub fn create_file_in(dir: &Path, name: &str) -> io::Result<PathBuf> {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
    Ok(path)
}

/// Creates a directory at `name` in `dir`, including any missing
/// parents. Fails with `AlreadyExists` when it's there already.
pub fn create_dir_in(dir: &Path, name: &str) -> io::Result<PathBuf> {
    let path = dir.join(name);
    if path.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} already exists", path.display())));
    }
    fs::create_dir_all(&path)?;
    Ok(path)
}

/// Whether `entry` shows under the filter `needle`: it matches, or
/// something under it does.
fn shows_under_filter(tree: &HashMap<PathBuf, Vec<Entry>>, entry: &Entry, needle: &str) -> bool {
    entry.name.to_lowercase().contains(needle) || (entry.is_dir() && has_match_under(tree, &entry.path, needle))
}

/// Whether anything in `tree` under `dir` matches.
fn has_match_under(tree: &HashMap<PathBuf, Vec<Entry>>, dir: &Path, needle: &str) -> bool {
    tree.get(dir).is_some_and(|children| children.iter().any(|c| shows_under_filter(tree, c, needle)))
}

/// Whether `name` matches `glob`, ignoring case: `*` is any run of
/// characters, `?` any one.
pub fn glob_matches(glob: &str, name: &str) -> bool {
    let glob: Vec<char> = glob.to_lowercase().chars().collect();
    let name: Vec<char> = name.to_lowercase().chars().collect();
    // The classic two-pointer match, backing up to the last `*`.
    let (mut g, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        if g < glob.len() && (glob[g] == '?' || glob[g] == name[n]) {
            g += 1;
            n += 1;
        } else if g < glob.len() && glob[g] == '*' {
            star = Some((g, n));
            g += 1;
        } else if let Some((sg, sn)) = star {
            g = sg + 1;
            n = sn + 1;
            star = Some((sg, sn + 1));
        } else {
            return false;
        }
    }
    glob[g..].iter().all(|&c| c == '*')
}

/// Where `needle` (lowercase) is in `name`, ignoring case, in chars.
fn find_ignoring_case(name: &str, needle: &str) -> Option<std::ops::Range<usize>> {
    let lower: Vec<char> = name.chars().flat_map(char::to_lowercase).collect();
    let needle: Vec<char> = needle.chars().collect();
    if needle.is_empty() || lower.len() != name.chars().count() {
        // A name whose lowercase changes length: matched, but where is
        // not worth guessing.
        return name.to_lowercase().contains(needle.iter().collect::<String>().as_str()).then_some(0..0);
    }
    let at = lower.windows(needle.len()).position(|w| w == needle.as_slice())?;
    Some(at..at + needle.len())
}

/// `sort_entries` over borrowed rows.
fn sort_refs(entries: &mut [&Entry], sort: Sort) {
    entries.sort_by(|a, b| compare(a, b, sort));
}

/// Orders one run of siblings. Ties always fall back to the name, so
/// the order is stable and predictable even when every file has the same
/// size or timestamp.
pub(crate) fn sort_entries(entries: &mut [Entry], sort: Sort) {
    entries.sort_by(|a, b| compare(a, b, sort));
}

fn compare(a: &Entry, b: &Entry, sort: Sort) -> std::cmp::Ordering {
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

    #[test]
    fn a_refresh_keeps_open_folders_open() {
        let dir = TempDir::new("refresh_keeps_open");
        let sub = dir.mkdir("sub");
        std::fs::write(sub.join("inner.txt"), "").unwrap();
        dir.touch("outer.txt");
        let mut state = ExplorerState::opened(dir.path()).unwrap();
        state.select_path(&sub);
        state.toggle_expand().unwrap();
        std::fs::write(sub.join("new.txt"), "").unwrap();

        state.refresh().unwrap();

        let names: Vec<&str> = state.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["sub", "inner.txt", "new.txt", "outer.txt"]);
        assert!(state.is_expanded(0));
    }

    #[test]
    fn another_directory_starts_folded_at_the_top() {
        let dir = TempDir::new("moved_folds");
        let sub = dir.mkdir("sub");
        std::fs::write(sub.join("inner.txt"), "").unwrap();
        let other = TempDir::new("moved_folds_other");
        other.touch("a.txt");
        other.touch("b.txt");
        let mut state = ExplorerState::opened(dir.path()).unwrap();
        state.toggle_expand().unwrap();

        state.apply_listing(read_listing(other.path(), false).unwrap());

        assert_eq!(state.entries.len(), 2);
        assert_eq!(state.selected, 0);
        assert!(state.open_folders().is_empty());
    }

    #[test]
    fn rows_carry_depth_guides_and_last_sibling() {
        let dir = TempDir::new("rows_guides");
        let a = dir.mkdir("a");
        std::fs::write(a.join("x.txt"), "").unwrap();
        std::fs::write(a.join("y.txt"), "").unwrap();
        dir.touch("z.txt");
        let mut state = ExplorerState::opened(dir.path()).unwrap();
        state.toggle_expand().unwrap();

        let rows: Vec<(&str, usize, Vec<bool>, bool)> =
            state.entries.iter().zip(&state.rows).map(|(e, r)| (r.label.as_str(), e.depth, r.guides.clone(), r.last)).collect();
        assert_eq!(
            rows,
            vec![
                ("a", 0, vec![], false),
                ("x.txt", 1, vec![true], false),
                ("y.txt", 1, vec![true], true),
                ("z.txt", 0, vec![], true),
            ],
            "the guide under `a` runs on down to `z.txt`"
        );
    }

    #[test]
    fn a_folder_holding_only_a_folder_shares_its_row() {
        let dir = TempDir::new("compact");
        let deep = dir.path().join("src").join("main").join("java");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("App.java"), "").unwrap();
        std::fs::write(deep.join("Util.java"), "").unwrap();
        let mut state = ExplorerState::opened(dir.path()).unwrap();

        state.toggle_expand().unwrap();

        let labels: Vec<&str> = state.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, vec!["src/main/java", "App.java", "Util.java"], "opened down to where it branches");
        assert_eq!(state.entries[0].path, deep, "the row is the last folder of the run");

        state.set_compact(false);
        let labels: Vec<&str> = state.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, vec!["src", "main", "java", "App.java", "Util.java"]);
    }

    #[test]
    fn reveal_names_the_folders_to_open_and_selects_the_file() {
        let dir = TempDir::new("reveal");
        let deep = dir.path().join("a").join("b");
        std::fs::create_dir_all(&deep).unwrap();
        let file = deep.join("f.txt");
        std::fs::write(&file, "").unwrap();
        std::fs::write(deep.join("g.txt"), "").unwrap();
        dir.touch("top.txt");
        let mut state = ExplorerState::opened(dir.path()).unwrap();

        let folders = state.folders_to_reveal(&file);
        assert_eq!(folders, vec![dir.path().join("a"), deep.clone()]);
        let read = folders.into_iter().map(|d| {
            let children = read_children(&d, false).unwrap();
            (d, children)
        });

        assert!(state.reveal(&file, read.collect()));
        assert_eq!(state.selected_entry().unwrap().path, file);
        assert!(state.folders_to_reveal(&file).is_empty(), "nothing left to open");
        assert!(state.folders_to_reveal(Path::new("/elsewhere/x")).is_empty());
    }

    #[test]
    fn a_filter_reaches_into_open_folders_and_keeps_the_way_down() {
        let dir = TempDir::new("filter_tree");
        let sub = dir.mkdir("sub");
        std::fs::write(sub.join("needle.txt"), "").unwrap();
        std::fs::write(sub.join("hay.txt"), "").unwrap();
        dir.touch("other.txt");
        let mut state = ExplorerState::opened(dir.path()).unwrap();
        state.toggle_expand().unwrap();
        state.collapse_at(0);

        state.set_filter("needle");

        let names: Vec<&str> = state.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["sub", "needle.txt"], "opened on the way down, though it was folded");
        assert!(!state.rows[0].matched && state.rows[1].matched);

        state.set_filter("");
        assert_eq!(state.entries.len(), 2, "folded again once the filter goes");
    }

    #[test]
    fn rename_stays_in_the_folder_of_the_row() {
        let dir = TempDir::new("rename_nested");
        let sub = dir.mkdir("sub");
        std::fs::write(sub.join("a.txt"), "").unwrap();
        std::fs::write(sub.join("b.txt"), "").unwrap();
        let mut state = ExplorerState::opened(dir.path()).unwrap();
        state.toggle_expand().unwrap();
        state.select_path(&sub.join("a.txt"));

        let renamed = state.rename_selected("c.txt").unwrap();

        assert_eq!(renamed, sub.join("c.txt"));
    }

    #[test]
    fn a_filter_shows_what_a_search_found_below_with_the_way_down() {
        let dir = TempDir::new("filter_found");
        let deep = dir.path().join("a").join("b");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("needle.txt"), "").unwrap();
        std::fs::write(deep.join("hay.txt"), "").unwrap();
        dir.touch("top.txt");
        let mut state = ExplorerState::opened(dir.path()).unwrap();

        state.set_filter("needle");
        assert!(state.entries.is_empty(), "nothing read matches yet");
        state.set_found(crate::search([deep.join("needle.txt"), deep.join("hay.txt")].iter().map(PathBuf::as_path), dir.path(), "needle", 10));

        let rows: Vec<(String, usize)> = state.entries.iter().map(|e| (e.name.clone(), e.depth)).collect();
        assert_eq!(rows, [("a".to_string(), 0), ("b".to_string(), 1), ("needle.txt".to_string(), 2)]);
        assert_eq!(state.rows[2].hit, Some(0..6));
        assert!(!state.rows[0].matched);

        state.set_filter("");
        assert_eq!(state.entries.len(), 2, "the tree as it was");
        state.set_filter("needle");
        assert!(state.entries.is_empty(), "a cleared filter drops what was found");
    }

    #[test]
    fn n_steps_through_the_matches_and_wraps() {
        let dir = TempDir::new("next_match");
        dir.touch("ab.txt");
        dir.touch("b.txt");
        dir.touch("cab.txt");
        let mut state = ExplorerState::opened(dir.path()).unwrap();
        state.set_filter("ab");
        assert_eq!(state.selected_entry().unwrap().name, "ab.txt");
        assert!(state.next_match(true));
        assert_eq!(state.selected_entry().unwrap().name, "cab.txt");
        assert!(state.next_match(true));
        assert_eq!(state.selected_entry().unwrap().name, "ab.txt", "wraps");
        assert!(state.next_match(false));
        assert_eq!(state.selected_entry().unwrap().name, "cab.txt");
    }

    #[test]
    fn globs_hide_names_wherever_they_are() {
        assert!(glob_matches("*.pyc", "Mod.PYC"));
        assert!(glob_matches("node_modules", "node_modules"));
        assert!(glob_matches("a?c*", "abcdef"));
        assert!(!glob_matches("*.pyc", "mod.py"));
        assert!(!glob_matches("node_modules", "node_modules2"));

        let dir = TempDir::new("hide_globs");
        let sub = dir.mkdir("sub");
        std::fs::write(sub.join("x.pyc"), "").unwrap();
        std::fs::write(sub.join("x.py"), "").unwrap();
        dir.mkdir("node_modules");
        let mut state = ExplorerState::opened(dir.path()).unwrap();
        state.select_path(&sub);
        state.toggle_expand().unwrap();
        state.set_hide(&["*.pyc".to_string(), "node_modules".to_string()]);
        let names: Vec<&str> = state.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["sub", "x.py"]);
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
