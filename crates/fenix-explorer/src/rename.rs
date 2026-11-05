//! Renaming by editing the listing.
//!
//! The one thing an editor can do that a file manager cannot: the
//! listing is already text, so making it editable turns `:%s/`, visual
//! block, macros and counts into bulk-rename tools that nobody had to
//! build. Emacs calls this wdired; this is the same bargain.
//!
//! What that costs is a way to say what an edited line *means*. The
//! answer here, as in wdired, is position: line N is entry N, always.
//! It is the only rule that survives arbitrary editing -- a name is not
//! an identity when renaming names is the entire point -- and it is why
//! adding or removing a line is refused rather than guessed at. A
//! deleted line is not a deleted file; it is an edit nobody can
//! interpret.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::entry::Entry;

/// Why an edited listing could not be turned into renames.
///
/// Every one of these is refused *before* anything is renamed, so a
/// rejected edit leaves the directory exactly as it was and the text
/// still on screen to fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameError {
    /// A line was added or removed. Position is identity here, so there
    /// is no honest way to read this: deleting a line might mean
    /// "delete that file", or might mean the line above should take its
    /// place, and picking one would eventually pick wrong.
    LineCountChanged { expected: usize, found: usize },
    /// A name was edited away to nothing.
    EmptyName { line: usize },
    /// Two entries were given the same name. One of them would silently
    /// destroy the other.
    DuplicateName { name: String },
    /// A new name is already taken by something that is not itself being
    /// renamed out of the way.
    AlreadyExists { path: PathBuf },
}

impl std::fmt::Display for RenameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenameError::LineCountChanged { expected, found } => write!(
                f,
                "the listing has {found} lines but {expected} entries -- lines can be edited, not added or removed (a deleted line is not a deleted file)"
            ),
            RenameError::EmptyName { line } => write!(f, "line {} has no name left", line + 1),
            RenameError::DuplicateName { name } => write!(f, "two entries would both be named {name}"),
            RenameError::AlreadyExists { path } => write!(f, "{} already exists", path.display()),
        }
    }
}

/// One file's move, from where it is to where the edited listing says
/// it should be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rename {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// Works out what an edited listing is asking for.
///
/// `edited` is the buffer's lines, one per entry, in the order they
/// were rendered. Names may contain separators -- `2026/notes.md` moves
/// a file into a subdirectory, which is what makes this useful for
/// reorganising and not only for renaming.
///
/// Returns only the entries that actually changed, so applying a plan
/// touches nothing that was left alone.
pub fn plan_renames(entries: &[Entry], edited: &[&str]) -> Result<Vec<Rename>, RenameError> {
    if edited.len() != entries.len() {
        return Err(RenameError::LineCountChanged { expected: entries.len(), found: edited.len() });
    }

    let mut targets: Vec<PathBuf> = Vec::with_capacity(entries.len());
    for (i, (entry, line)) in entries.iter().zip(edited).enumerate() {
        let name = line.trim();
        // The rendering marks directories with a trailing slash; typing
        // over the name should not require deleting it, and leaving it
        // on should not rename `src` to `src/`.
        let name = name.trim_end_matches(['/', '\\']);
        if name.is_empty() {
            return Err(RenameError::EmptyName { line: i });
        }
        let parent = entry.path.parent().unwrap_or(Path::new(""));
        targets.push(parent.join(name));
    }

    // Two entries pointed at one name is the case that would silently
    // destroy work, so it is caught here rather than discovered half way
    // through applying.
    let mut seen: HashMap<&Path, usize> = HashMap::new();
    for target in &targets {
        if seen.insert(target, 0).is_some() {
            let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| target.display().to_string());
            return Err(RenameError::DuplicateName { name });
        }
    }

    // A target may collide with something already on disk, unless that
    // something is one of these entries and is itself moving away --
    // which is what makes swapping two names work.
    let sources: Vec<&Path> = entries.iter().map(|e| e.path.as_path()).collect();
    for (target, source) in targets.iter().zip(&sources) {
        if target == *source {
            continue;
        }
        let freed_by_this_edit = sources.contains(&target.as_path());
        if !freed_by_this_edit && target.exists() {
            return Err(RenameError::AlreadyExists { path: target.clone() });
        }
    }

    Ok(entries
        .iter()
        .zip(targets)
        .filter(|(entry, target)| &entry.path != target)
        .map(|(entry, target)| Rename { from: entry.path.clone(), to: target })
        .collect())
}

/// Whether applying `renames` needs to go through temporary names.
///
/// It does when any destination is also a source: `a -> b` beside
/// `b -> a` cannot be done one at a time in any order without one of
/// them landing on a file that has not moved yet. Renaming everything
/// aside first and then into place makes the order irrelevant.
///
/// Checked rather than always done, because the safe path costs two
/// renames per file, and over a network share that is two round trips
/// where one would do.
pub fn needs_two_phases(renames: &[Rename]) -> bool {
    let destinations: Vec<&Path> = renames.iter().map(|r| r.to.as_path()).collect();
    renames.iter().any(|r| destinations.contains(&r.from.as_path()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;
    use crate::ExplorerState;

    fn entries_of(dir: &TempDir) -> Vec<Entry> {
        ExplorerState::opened(dir.path()).unwrap().entries
    }

    #[test]
    fn an_unedited_listing_asks_for_nothing() {
        let dir = TempDir::new("rename_noop");
        dir.touch("a.txt");
        dir.touch("b.txt");
        let entries = entries_of(&dir);

        let plan = plan_renames(&entries, &["a.txt", "b.txt"]).unwrap();

        assert!(plan.is_empty(), "nothing changed, so nothing to do");
    }

    #[test]
    fn only_the_lines_that_changed_become_renames() {
        let dir = TempDir::new("rename_partial");
        dir.touch("a.txt");
        dir.touch("b.txt");
        let entries = entries_of(&dir);

        let plan = plan_renames(&entries, &["renamed.txt", "b.txt"]).unwrap();

        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].from, dir.path().join("a.txt"));
        assert_eq!(plan[0].to, dir.path().join("renamed.txt"));
    }

    #[test]
    fn a_name_with_a_separator_moves_the_file() {
        // What makes this useful for reorganising and not just renaming.
        let dir = TempDir::new("rename_into_subdir");
        dir.touch("a.txt");
        let entries = entries_of(&dir);

        let plan = plan_renames(&entries, &["2026/a.txt"]).unwrap();

        assert_eq!(plan[0].to, dir.path().join("2026").join("a.txt"));
    }

    #[test]
    fn a_directorys_trailing_slash_is_not_part_of_its_name() {
        // The listing marks directories with one; typing over the name
        // should not require deleting it first.
        let dir = TempDir::new("rename_dir_slash");
        dir.mkdir("src");
        let entries = entries_of(&dir);

        assert!(plan_renames(&entries, &["src/"]).unwrap().is_empty(), "unchanged");
        let plan = plan_renames(&entries, &["source/"]).unwrap();
        assert_eq!(plan[0].to, dir.path().join("source"));
    }

    #[test]
    fn surrounding_whitespace_is_not_part_of_a_name() {
        let dir = TempDir::new("rename_trim");
        dir.touch("a.txt");
        let entries = entries_of(&dir);
        assert!(plan_renames(&entries, &["  a.txt  "]).unwrap().is_empty());
    }

    #[test]
    fn adding_or_removing_a_line_is_refused_rather_than_guessed_at() {
        // Position is identity, so a missing line has no honest reading:
        // it might mean "delete that file", or might mean everything
        // below shifts up. Either guess eventually guesses wrong.
        let dir = TempDir::new("rename_line_count");
        dir.touch("a.txt");
        dir.touch("b.txt");
        let entries = entries_of(&dir);

        let err = plan_renames(&entries, &["a.txt"]).unwrap_err();
        assert_eq!(err, RenameError::LineCountChanged { expected: 2, found: 1 });
        assert!(err.to_string().contains("not a deleted file"), "the message explains itself: {err}");

        assert!(plan_renames(&entries, &["a.txt", "b.txt", "c.txt"]).is_err());
    }

    #[test]
    fn a_name_edited_away_to_nothing_is_refused() {
        let dir = TempDir::new("rename_empty");
        dir.touch("a.txt");
        let entries = entries_of(&dir);
        assert_eq!(plan_renames(&entries, &["   "]).unwrap_err(), RenameError::EmptyName { line: 0 });
    }

    #[test]
    fn two_entries_given_the_same_name_are_refused() {
        // One would silently destroy the other.
        let dir = TempDir::new("rename_duplicate");
        dir.touch("a.txt");
        dir.touch("b.txt");
        let entries = entries_of(&dir);

        let err = plan_renames(&entries, &["same.txt", "same.txt"]).unwrap_err();

        assert_eq!(err, RenameError::DuplicateName { name: "same.txt".to_string() });
    }

    #[test]
    fn renaming_onto_an_untouched_file_is_refused() {
        let dir = TempDir::new("rename_collision");
        dir.touch("a.txt");
        dir.touch("keep.txt");
        let mut entries = entries_of(&dir);
        entries.retain(|e| e.name == "a.txt");

        let err = plan_renames(&entries, &["keep.txt"]).unwrap_err();

        assert_eq!(err, RenameError::AlreadyExists { path: dir.path().join("keep.txt") });
    }

    #[test]
    fn swapping_two_names_is_allowed_because_both_are_moving() {
        // The case that makes this worth getting right, and the one a
        // naive collision check would reject.
        let dir = TempDir::new("rename_swap");
        dir.touch("a.txt");
        dir.touch("b.txt");
        let entries = entries_of(&dir);

        let plan = plan_renames(&entries, &["b.txt", "a.txt"]).unwrap();

        assert_eq!(plan.len(), 2);
        assert!(needs_two_phases(&plan), "a swap cannot be done one rename at a time");
    }

    #[test]
    fn a_rotation_of_three_names_also_needs_the_safe_path() {
        let dir = TempDir::new("rename_rotate");
        dir.touch("a.txt");
        dir.touch("b.txt");
        dir.touch("c.txt");
        let entries = entries_of(&dir);

        let plan = plan_renames(&entries, &["b.txt", "c.txt", "a.txt"]).unwrap();

        assert!(needs_two_phases(&plan));
    }

    #[test]
    fn independent_renames_do_not_need_the_safe_path() {
        // Two round trips per file instead of one is worth avoiding when
        // the files are on a share.
        let dir = TempDir::new("rename_independent");
        dir.touch("a.txt");
        dir.touch("b.txt");
        let entries = entries_of(&dir);

        let plan = plan_renames(&entries, &["x.txt", "y.txt"]).unwrap();

        assert!(!needs_two_phases(&plan));
    }
}
