use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::entry::{Entry, GitStatus};
use crate::git;

/// State for one directory listing: what's shown, what's selected, and
/// what's marked. Host-agnostic -- real filesystem I/O, no rendering or
/// windowing-system dependency, mirroring `fenix-vim::VimState`'s role
/// for text editing.
pub struct ExplorerState {
    pub cwd: PathBuf,
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub marks: HashSet<PathBuf>,
    pub show_hidden: bool,
    /// Cached from one `git status` call per `open`/`refresh` (not one
    /// per subtree expansion -- `git status` is already recursive, so
    /// this single scan covers every path that could ever appear under
    /// `cwd`, including ones not listed yet because their parent isn't
    /// expanded).
    git_statuses: HashMap<PathBuf, GitStatus>,
}

impl ExplorerState {
    /// Lists `path` (which must be a directory -- callers wanting
    /// "jump to this *file's* directory" pass `file.parent()`, not the
    /// file itself). Dotfiles hidden by default.
    pub fn open(path: &Path) -> io::Result<Self> {
        let cwd = path.to_path_buf();
        let git_statuses = git::status_for_dir(&cwd);
        let mut entries = list_dir(&cwd, 0, false)?;
        git::annotate_entries(&mut entries, &git_statuses);
        Ok(Self { cwd, entries, selected: 0, marks: HashSet::new(), show_hidden: false, git_statuses })
    }

    /// Re-lists `cwd` at the top level, keeping the selection on the same
    /// path if it still exists (else clamped) and dropping marks for
    /// paths that no longer exist. Collapses any subtree expansion --
    /// refreshing re-derives the listing from scratch, it doesn't try to
    /// preserve which directories were expanded. Also re-runs `git
    /// status`, so badges stay current after edits/commits made outside
    /// this listing.
    pub fn refresh(&mut self) -> io::Result<()> {
        let selected_path = self.entries.get(self.selected).map(|e| e.path.clone());
        self.git_statuses = git::status_for_dir(&self.cwd);
        self.entries = list_dir(&self.cwd, 0, self.show_hidden)?;
        git::annotate_entries(&mut self.entries, &self.git_statuses);
        self.marks.retain(|p| self.entries.iter().any(|e| &e.path == p));
        self.selected = selected_path
            .and_then(|p| self.entries.iter().position(|e| e.path == p))
            .unwrap_or(0)
            .min(self.entries.len().saturating_sub(1));
        Ok(())
    }

    pub fn toggle_hidden(&mut self) -> io::Result<()> {
        self.show_hidden = !self.show_hidden;
        self.refresh()
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let len = self.entries.len() as isize;
        let new = (self.selected as isize + delta).clamp(0, len - 1);
        self.selected = new as usize;
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

    /// Expands the directory at point in place (inserting its children
    /// right after it, indented one level deeper) if collapsed, or
    /// collapses it (removing every row below it whose depth is greater --
    /// its children and, transitively, anything expanded under them) if
    /// already expanded. A no-op on a plain file. Keeps the flat
    /// `entries` list as the single source of truth for both rendering
    /// and selection -- no separate tree structure to keep in sync.
    pub fn toggle_expand(&mut self) -> io::Result<()> {
        let Some(entry) = self.entries.get(self.selected) else { return Ok(()) };
        if !entry.is_dir {
            return Ok(());
        }
        let depth = entry.depth;
        let path = entry.path.clone();

        let is_expanded = self.entries.get(self.selected + 1).is_some_and(|next| next.depth > depth);
        if is_expanded {
            let start = self.selected + 1;
            let mut end = start;
            while end < self.entries.len() && self.entries[end].depth > depth {
                end += 1;
            }
            self.entries.drain(start..end);
        } else {
            let children = list_dir(&path, depth + 1, self.show_hidden)?;
            let insert_at = self.selected + 1;
            for (i, child) in children.into_iter().enumerate() {
                self.entries.insert(insert_at + i, child);
            }
            // Reuses the cached scan from the last open/refresh -- `git
            // status` is already recursive, so no new subprocess call is
            // needed just because a subtree got expanded.
            git::annotate_entries(&mut self.entries, &self.git_statuses);
        }
        Ok(())
    }

    /// Creates an empty file named `name` in `cwd`. Fails with
    /// `ErrorKind::AlreadyExists` rather than silently truncating an
    /// existing file -- unlike `fs::File::create`, which would.
    pub fn create_file(&mut self, name: &str) -> io::Result<()> {
        fs::OpenOptions::new().write(true).create_new(true).open(self.cwd.join(name))?;
        self.refresh()
    }

    pub fn create_dir(&mut self, name: &str) -> io::Result<()> {
        fs::create_dir(self.cwd.join(name))?;
        self.refresh()
    }

    /// Deletes every target (marks if any, else the entry at point).
    /// Best-effort across the whole set -- one failure doesn't stop the
    /// rest from being attempted -- then always refreshes so the listing
    /// reflects whatever actually happened, returning the first error (if
    /// any) afterward. Paths that were actually removed lose their marks
    /// via the normal `refresh` cleanup; paths that failed to delete keep
    /// theirs, since they still exist.
    pub fn delete_targets(&mut self) -> io::Result<()> {
        let targets: Vec<(PathBuf, bool)> = self.targets().iter().map(|e| (e.path.clone(), e.is_dir)).collect();
        let mut first_err = None;
        for (path, is_dir) in targets {
            let result = if is_dir { fs::remove_dir_all(&path) } else { fs::remove_file(&path) };
            if let Err(e) = result {
                first_err.get_or_insert(e);
            }
        }
        self.refresh()?;
        first_err.map_or(Ok(()), Err)
    }

    /// Renames the entry at point (not the marked set -- dired's
    /// interactive rename is single-target; bulk rename is a different,
    /// out-of-scope feature). Refuses to overwrite an existing path.
    pub fn rename_selected(&mut self, new_name: &str) -> io::Result<()> {
        let Some(entry) = self.selected_entry() else {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no entry selected"));
        };
        let old_path = entry.path.clone();
        let new_path = self.cwd.join(new_name);
        if new_path.exists() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "a file with that name already exists"));
        }
        fs::rename(&old_path, &new_path)?;
        self.refresh()
    }

    /// Copies every target into `dest` (a directory), preserving each
    /// entry's own filename. Overwrites an existing file at the
    /// destination if one exists -- callers wanting a confirmation
    /// prompt for that (the GUI layer does) check for it themselves
    /// before calling this; this method just performs the copy.
    pub fn copy_targets_to(&mut self, dest: &Path) -> io::Result<()> {
        let targets: Vec<(PathBuf, String)> = self.targets().iter().map(|e| (e.path.clone(), e.name.clone())).collect();
        let mut first_err = None;
        for (src, name) in targets {
            if let Err(e) = copy_recursive(&src, &dest.join(&name)) {
                first_err.get_or_insert(e);
            }
        }
        self.refresh()?;
        first_err.map_or(Ok(()), Err)
    }

    /// Moves every target into `dest`. Tries a plain rename first (fast,
    /// atomic, same-filesystem); falls back to copy-then-remove-source
    /// when that fails (e.g. moving across filesystems, where `rename`
    /// always fails with `EXDEV`).
    pub fn move_targets_to(&mut self, dest: &Path) -> io::Result<()> {
        let targets: Vec<(PathBuf, String, bool)> =
            self.targets().iter().map(|e| (e.path.clone(), e.name.clone(), e.is_dir)).collect();
        let mut first_err = None;
        for (src, name, is_dir) in targets {
            let dest_path = dest.join(&name);
            let result = fs::rename(&src, &dest_path).or_else(|_| {
                copy_recursive(&src, &dest_path)?;
                if is_dir { fs::remove_dir_all(&src) } else { fs::remove_file(&src) }
            });
            if let Err(e) = result {
                first_err.get_or_insert(e);
            }
        }
        self.refresh()?;
        first_err.map_or(Ok(()), Err)
    }
}

fn copy_recursive(src: &Path, dest: &Path) -> io::Result<()> {
    if src.is_dir() {
        fs::create_dir_all(dest)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &dest.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(src, dest).map(|_| ())
    }
}

/// Lists one directory's *immediate* children (not recursive), sorted
/// directories-first-then-alphabetical, at the given indentation `depth`.
pub(crate) fn list_dir(path: &Path, depth: usize, show_hidden: bool) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for dirent in fs::read_dir(path)? {
        let dirent = dirent?;
        let name = dirent.file_name().to_string_lossy().into_owned();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let metadata = dirent.metadata()?;
        entries.push(Entry {
            name,
            path: dirent.path(),
            is_dir: metadata.is_dir(),
            size: metadata.len(),
            modified: metadata.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
            depth,
            git_status: None,
        });
    }
    sort_entries(&mut entries);
    Ok(entries)
}

pub(crate) fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn open_lists_entries_sorted_directories_first_then_alphabetical() {
        let dir = TempDir::new("open_sorted");
        dir.touch("zebra.txt");
        dir.touch("apple.txt");
        dir.mkdir("zoo");
        dir.mkdir("bear");

        let state = ExplorerState::open(dir.path()).unwrap();
        let names: Vec<&str> = state.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["bear", "zoo", "apple.txt", "zebra.txt"]);
        assert!(state.entries[0].is_dir);
        assert!(state.entries[1].is_dir);
        assert!(!state.entries[2].is_dir);
    }

    #[test]
    fn open_hides_dotfiles_by_default() {
        let dir = TempDir::new("open_hides_dotfiles");
        dir.touch("visible.txt");
        dir.touch(".hidden");

        let state = ExplorerState::open(dir.path()).unwrap();
        assert_eq!(state.entries.len(), 1);
        assert_eq!(state.entries[0].name, "visible.txt");
    }

    #[test]
    fn toggle_hidden_shows_and_hides_dotfiles() {
        let dir = TempDir::new("toggle_hidden");
        dir.touch("visible.txt");
        dir.touch(".hidden");

        let mut state = ExplorerState::open(dir.path()).unwrap();
        state.toggle_hidden().unwrap();
        assert_eq!(state.entries.len(), 2);
        state.toggle_hidden().unwrap();
        assert_eq!(state.entries.len(), 1);
    }

    #[test]
    fn move_selection_clamps_at_both_ends() {
        let dir = TempDir::new("move_selection_clamps");
        dir.touch("a");
        dir.touch("b");
        dir.touch("c");
        let mut state = ExplorerState::open(dir.path()).unwrap();

        state.move_selection(-5);
        assert_eq!(state.selected, 0);
        state.move_selection(1);
        assert_eq!(state.selected, 1);
        state.move_selection(100);
        assert_eq!(state.selected, 2);
    }

    #[test]
    fn move_selection_on_empty_directory_is_a_no_op() {
        let dir = TempDir::new("move_selection_empty");
        let mut state = ExplorerState::open(dir.path()).unwrap();
        assert!(state.entries.is_empty());
        state.move_selection(1); // must not panic
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn refresh_keeps_selection_on_the_same_path_when_it_still_exists() {
        let dir = TempDir::new("refresh_keeps_selection");
        dir.touch("a");
        dir.touch("b");
        dir.touch("c");
        let mut state = ExplorerState::open(dir.path()).unwrap();
        state.move_selection(1); // now on "b"
        let selected_name = state.selected_entry().unwrap().name.clone();
        assert_eq!(selected_name, "b");

        dir.touch("aa"); // sorts before "b", shifting its index
        state.refresh().unwrap();
        assert_eq!(state.selected_entry().unwrap().name, "b");
    }

    #[test]
    fn refresh_clamps_selection_when_the_selected_entry_is_gone() {
        let dir = TempDir::new("refresh_clamps_gone_entry");
        let path_b = dir.touch("b");
        dir.touch("c");
        let mut state = ExplorerState::open(dir.path()).unwrap();
        state.move_selection(1); // "b"
        std::fs::remove_file(&path_b).unwrap();
        state.refresh().unwrap();
        assert!(state.selected < state.entries.len());
    }

    #[test]
    fn toggle_mark_marks_then_unmarks_the_entry_at_point() {
        let dir = TempDir::new("toggle_mark");
        let path_a = dir.touch("a");
        let mut state = ExplorerState::open(dir.path()).unwrap();

        state.toggle_mark();
        assert!(state.marks.contains(&path_a));
        state.toggle_mark();
        assert!(!state.marks.contains(&path_a));
    }

    #[test]
    fn mark_all_marks_every_entry() {
        let dir = TempDir::new("mark_all");
        dir.touch("a");
        dir.touch("b");
        let mut state = ExplorerState::open(dir.path()).unwrap();
        state.mark_all();
        assert_eq!(state.marks.len(), 2);
    }

    #[test]
    fn unmark_all_clears_marks() {
        let dir = TempDir::new("unmark_all");
        dir.touch("a");
        let mut state = ExplorerState::open(dir.path()).unwrap();
        state.mark_all();
        state.unmark_all();
        assert!(state.marks.is_empty());
    }

    #[test]
    fn toggle_all_marks_flips_each_entry_independently() {
        let dir = TempDir::new("toggle_all_marks");
        let path_a = dir.touch("a");
        let path_b = dir.touch("b");
        let mut state = ExplorerState::open(dir.path()).unwrap();
        state.marks.insert(path_a.clone()); // pre-mark just "a"

        state.toggle_all_marks();
        assert!(!state.marks.contains(&path_a)); // was marked -> unmarked
        assert!(state.marks.contains(&path_b)); // was unmarked -> marked
    }

    #[test]
    fn targets_is_the_entry_at_point_when_nothing_is_marked() {
        let dir = TempDir::new("targets_at_point");
        dir.touch("a");
        dir.touch("b");
        let mut state = ExplorerState::open(dir.path()).unwrap();
        state.move_selection(1); // "b"
        let targets: Vec<&str> = state.targets().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(targets, vec!["b"]);
    }

    #[test]
    fn targets_is_the_marked_set_when_marks_exist_ignoring_point() {
        let dir = TempDir::new("targets_marked");
        dir.touch("a");
        dir.touch("b");
        dir.touch("c");
        let mut state = ExplorerState::open(dir.path()).unwrap();
        // mark "a" and "c", leave selection on "b" (index 1)
        state.marks.insert(dir.path().join("a"));
        state.marks.insert(dir.path().join("c"));
        state.move_selection(1);

        let mut targets: Vec<&str> = state.targets().iter().map(|e| e.name.as_str()).collect();
        targets.sort_unstable();
        assert_eq!(targets, vec!["a", "c"]);
    }

    #[test]
    fn toggle_expand_on_a_file_is_a_no_op() {
        let dir = TempDir::new("toggle_expand_file");
        dir.touch("a");
        let mut state = ExplorerState::open(dir.path()).unwrap();
        let before = state.entries.clone();
        state.toggle_expand().unwrap();
        assert_eq!(state.entries, before);
    }

    #[test]
    fn toggle_expand_inserts_indented_children_then_removes_them() {
        let dir = TempDir::new("toggle_expand_dir");
        let sub = dir.mkdir("sub");
        std::fs::write(sub.join("inner.txt"), b"x").unwrap();
        dir.touch("zzz_after"); // sorts after "sub", used to check it isn't disturbed

        let mut state = ExplorerState::open(dir.path()).unwrap();
        assert_eq!(state.entries.len(), 2); // "sub", "zzz_after"
        assert_eq!(state.entries[0].name, "sub");

        state.toggle_expand().unwrap(); // selected = "sub" (index 0)
        assert_eq!(state.entries.len(), 3);
        assert_eq!(state.entries[1].name, "inner.txt");
        assert_eq!(state.entries[1].depth, 1);
        assert_eq!(state.entries[2].name, "zzz_after");
        assert_eq!(state.entries[2].depth, 0);

        state.toggle_expand().unwrap(); // collapse
        assert_eq!(state.entries.len(), 2);
        assert_eq!(state.entries[1].name, "zzz_after");
    }

    #[test]
    fn collapsing_removes_transitively_expanded_grandchildren_too() {
        let dir = TempDir::new("toggle_expand_nested");
        let sub = dir.mkdir("sub");
        std::fs::create_dir(sub.join("subsub")).unwrap();
        std::fs::write(sub.join("subsub").join("deep.txt"), b"x").unwrap();

        let mut state = ExplorerState::open(dir.path()).unwrap();
        state.toggle_expand().unwrap(); // expand "sub" -> reveals "subsub" at index 1
        assert_eq!(state.entries[1].name, "subsub");
        state.selected = 1;
        state.toggle_expand().unwrap(); // expand "subsub" -> reveals "deep.txt" at index 2
        assert_eq!(state.entries.len(), 3);
        assert_eq!(state.entries[2].name, "deep.txt");
        assert_eq!(state.entries[2].depth, 2);

        state.selected = 0;
        state.toggle_expand().unwrap(); // collapse "sub" -- should remove both descendants
        assert_eq!(state.entries.len(), 1);
        assert_eq!(state.entries[0].name, "sub");
    }

    #[test]
    fn create_file_makes_an_empty_file_and_refreshes() {
        let dir = TempDir::new("create_file");
        let mut state = ExplorerState::open(dir.path()).unwrap();
        state.create_file("new.txt").unwrap();
        assert!(dir.path().join("new.txt").is_file());
        assert!(state.entries.iter().any(|e| e.name == "new.txt"));
    }

    #[test]
    fn create_file_refuses_to_overwrite_an_existing_file() {
        let dir = TempDir::new("create_file_refuses_overwrite");
        dir.write("existing.txt", "keep me");
        let mut state = ExplorerState::open(dir.path()).unwrap();
        let err = state.create_file("existing.txt").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(dir.path().join("existing.txt")).unwrap(), "keep me");
    }

    #[test]
    fn create_dir_makes_a_directory_and_refreshes() {
        let dir = TempDir::new("create_dir");
        let mut state = ExplorerState::open(dir.path()).unwrap();
        state.create_dir("sub").unwrap();
        assert!(dir.path().join("sub").is_dir());
        assert!(state.entries.iter().any(|e| e.name == "sub" && e.is_dir));
    }

    #[test]
    fn delete_targets_removes_the_entry_at_point_when_nothing_marked() {
        let dir = TempDir::new("delete_at_point");
        dir.touch("a");
        dir.touch("b");
        let mut state = ExplorerState::open(dir.path()).unwrap(); // selected = "a"
        state.delete_targets().unwrap();
        assert!(!dir.path().join("a").exists());
        assert!(dir.path().join("b").exists());
    }

    #[test]
    fn delete_targets_removes_the_marked_set() {
        let dir = TempDir::new("delete_marked");
        dir.touch("a");
        dir.touch("b");
        dir.touch("c");
        let mut state = ExplorerState::open(dir.path()).unwrap();
        state.marks.insert(dir.path().join("a"));
        state.marks.insert(dir.path().join("c"));
        state.delete_targets().unwrap();
        assert!(!dir.path().join("a").exists());
        assert!(dir.path().join("b").exists());
        assert!(!dir.path().join("c").exists());
    }

    #[test]
    fn delete_targets_removes_a_directory_recursively() {
        let dir = TempDir::new("delete_dir_recursive");
        dir.mkdir("sub");
        std::fs::write(dir.path().join("sub").join("inner.txt"), b"x").unwrap();
        let mut state = ExplorerState::open(dir.path()).unwrap();
        state.delete_targets().unwrap();
        assert!(!dir.path().join("sub").exists());
    }

    #[test]
    fn rename_selected_renames_and_refuses_to_overwrite() {
        let dir = TempDir::new("rename_selected");
        dir.write("old.txt", "hello");
        dir.touch("taken.txt");
        let mut state = ExplorerState::open(dir.path()).unwrap(); // selected = "old.txt"
        state.rename_selected("new.txt").unwrap();
        assert!(!dir.path().join("old.txt").exists());
        assert_eq!(std::fs::read_to_string(dir.path().join("new.txt")).unwrap(), "hello");

        let err = state.rename_selected("taken.txt").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn copy_targets_to_copies_a_file_and_a_directory_recursively() {
        let dir = TempDir::new("copy_targets_src");
        let dest = TempDir::new("copy_targets_dest");
        dir.write("file.txt", "contents");
        dir.mkdir("sub");
        std::fs::write(dir.path().join("sub").join("inner.txt"), b"nested").unwrap();

        let mut state = ExplorerState::open(dir.path()).unwrap();
        state.mark_all();
        state.copy_targets_to(dest.path()).unwrap();

        assert_eq!(std::fs::read_to_string(dest.path().join("file.txt")).unwrap(), "contents");
        assert_eq!(std::fs::read_to_string(dest.path().join("sub").join("inner.txt")).unwrap(), "nested");
        // source is untouched by a copy
        assert!(dir.path().join("file.txt").exists());
        assert!(dir.path().join("sub").exists());
    }

    #[test]
    fn move_targets_to_moves_and_removes_the_source() {
        let dir = TempDir::new("move_targets_src");
        let dest = TempDir::new("move_targets_dest");
        dir.write("file.txt", "contents");
        let mut state = ExplorerState::open(dir.path()).unwrap();
        state.move_targets_to(dest.path()).unwrap();

        assert!(!dir.path().join("file.txt").exists());
        assert_eq!(std::fs::read_to_string(dest.path().join("file.txt")).unwrap(), "contents");
    }

    #[test]
    fn open_annotates_entries_with_git_status() {
        let dir = TempDir::new("open_git_status");
        let status = std::process::Command::new("git").args(["init", "-q"]).current_dir(dir.path()).status();
        if status.is_err() {
            return; // git not on PATH in this environment -- skip rather than fail
        }
        std::process::Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        std::process::Command::new("git").args(["config", "user.name", "Test"]).current_dir(dir.path()).status().unwrap();
        dir.touch("untracked.txt");

        let state = ExplorerState::open(dir.path()).unwrap();
        let entry = state.entries.iter().find(|e| e.name == "untracked.txt").unwrap();
        assert_eq!(entry.git_status, Some(crate::GitStatus::Untracked));
    }

    #[test]
    fn refresh_drops_marks_for_paths_that_no_longer_exist() {
        let dir = TempDir::new("refresh_drops_stale_marks");
        let path_a = dir.touch("a");
        dir.touch("b");
        let mut state = ExplorerState::open(dir.path()).unwrap();
        state.marks.insert(path_a.clone());
        state.marks.insert(dir.path().join("b"));

        std::fs::remove_file(&path_a).unwrap();
        state.refresh().unwrap();
        assert!(!state.marks.contains(&path_a));
        assert!(state.marks.contains(&dir.path().join("b")));
    }
}
