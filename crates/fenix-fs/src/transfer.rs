//! Copying and moving, with the question that has to be asked first.
//!
//! The explorer this replaces copied straight over whatever was already
//! at the destination. Its own doc comment said the GUI was expected to
//! check for that beforehand, and the GUI did not -- so a copy into a
//! folder that happened to contain a file of the same name destroyed it
//! silently, with no prompt and nothing in the Recycle Bin.
//!
//! Here the check comes first (`conflicts_in`), the caller decides once
//! for the whole batch, and every path's fate is reported back.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::recycle::Outcome;

/// What to do about a source whose name is already taken at the
/// destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnConflict {
    /// Replace what is there. A directory is merged into rather than
    /// swapped wholesale, which is what every file manager means by
    /// copying a folder onto a folder.
    Overwrite,
    /// Leave both alone and report the source as skipped, so a batch
    /// that mostly succeeds still tells you which ones did not.
    Skip,
    /// Keep both, giving the incoming one a numbered name --
    /// `notes.txt` beside `notes (2).txt`, the convention Windows uses.
    KeepBoth,
}

/// Which of `sources` would land on something that already exists in
/// `dest`.
///
/// Checked before anything is written, so the user is asked once about
/// the whole batch rather than finding out half way through that files
/// were replaced.
pub fn conflicts_in(sources: &[PathBuf], dest: &Path) -> Vec<PathBuf> {
    sources
        .iter()
        .filter_map(|src| {
            let candidate = dest.join(src.file_name()?);
            candidate.exists().then_some(candidate)
        })
        .collect()
}

/// How far along a long transfer is.
///
/// Counted in both files and bytes because neither alone is honest:
/// "3 of 4000 files" says nothing about a copy dominated by one huge
/// file, and a byte count says nothing about a copy of forty thousand
/// tiny ones.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    pub files_done: usize,
    pub files_total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// What is being copied at this moment.
    pub current: PathBuf,
}

/// What a transfer counts before it starts, so progress has a
/// denominator.
///
/// Walking first costs a pass over the tree, which on a share is not
/// free -- but a progress indicator with no total is barely better than
/// none, and the walk is metadata where the copy is data. Cancellable,
/// because on a big enough tree the *measuring* is long enough to want
/// out of.
pub fn measure(sources: &[PathBuf], cancel: &impl Fn() -> bool) -> (usize, u64) {
    let mut files = 0usize;
    let mut bytes = 0u64;
    for source in sources {
        measure_one(source, cancel, &mut files, &mut bytes);
    }
    (files, bytes)
}

fn measure_one(path: &Path, cancel: &impl Fn() -> bool, files: &mut usize, bytes: &mut u64) {
    if cancel() {
        return;
    }
    let Ok(metadata) = fs::symlink_metadata(path) else { return };
    if metadata.file_type().is_symlink() {
        // A link is one entry to recreate, not the tree behind it.
        *files += 1;
        return;
    }
    if metadata.is_dir() {
        let Ok(entries) = fs::read_dir(path) else { return };
        for entry in entries.flatten() {
            measure_one(&entry.path(), cancel, files, bytes);
        }
        return;
    }
    *files += 1;
    *bytes += metadata.len();
}

/// Copies every source into `dest`, keeping each one's own filename.
pub fn copy_into(sources: &[PathBuf], dest: &Path, on_conflict: OnConflict) -> Vec<Outcome> {
    copy_into_reporting(sources, dest, on_conflict, &|| false, &mut |_| {})
}

/// `copy_into` that says how it is going and can be told to stop.
///
/// `cancel` is checked before each file. A file already in flight has to
/// finish: there is no way to abandon a `fs::copy` part way through
/// without leaving a truncated file behind, so a single very large file
/// is the one thing cancelling cannot interrupt. Worth stating rather
/// than implying otherwise with a button that appears to do nothing.
///
/// What has already been copied stays. That is what actually happened,
/// and deleting it to make the operation look atomic would destroy work
/// the user may well want.
pub fn copy_into_reporting(
    sources: &[PathBuf],
    dest: &Path,
    on_conflict: OnConflict,
    cancel: &impl Fn() -> bool,
    report: &mut impl FnMut(&Progress),
) -> Vec<Outcome> {
    let (files_total, bytes_total) = measure(sources, cancel);
    let mut progress = Progress { files_total, bytes_total, ..Progress::default() };
    run_reporting(sources, dest, on_conflict, cancel, |src, target| {
        copy_recursive_reporting(src, target, cancel, &mut progress, report)
    })
}

/// `move_into` that says how it is going and can be told to stop.
///
/// Cancelling leaves the files already moved at the destination and the
/// rest where they were -- again, what actually happened. A move is not
/// a transaction and pretending it is would mean copying everything
/// back.
pub fn move_into_reporting(
    sources: &[PathBuf],
    dest: &Path,
    on_conflict: OnConflict,
    cancel: &impl Fn() -> bool,
    report: &mut impl FnMut(&Progress),
) -> Vec<Outcome> {
    let (files_total, bytes_total) = measure(sources, cancel);
    let mut progress = Progress { files_total, bytes_total, ..Progress::default() };
    run_reporting(sources, dest, on_conflict, cancel, |src, target| {
        if fs::rename(src, target).is_ok() {
            // A rename moves the whole tree in one step, so there is
            // nothing to report per file -- count it as arrived.
            progress.files_done = progress.files_total;
            progress.bytes_done = progress.bytes_total;
            src.clone_into(&mut progress.current);
            report(&progress);
            return Ok(());
        }
        copy_recursive_reporting(src, target, cancel, &mut progress, report)?;
        let is_dir = fs::symlink_metadata(src).map(|m| m.is_dir()).unwrap_or(false);
        if is_dir {
            fs::remove_dir_all(src)
        } else {
            fs::remove_file(src)
        }
    })
}

fn copy_recursive_reporting(
    src: &Path,
    dest: &Path,
    cancel: &impl Fn() -> bool,
    progress: &mut Progress,
    report: &mut impl FnMut(&Progress),
) -> io::Result<()> {
    if cancel() {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
    }
    let metadata = fs::symlink_metadata(src)?;
    if metadata.file_type().is_symlink() {
        copy_link(src, dest)?;
        progress.files_done += 1;
        src.clone_into(&mut progress.current);
        report(progress);
        return Ok(());
    }
    if metadata.is_dir() {
        fs::create_dir_all(dest)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            copy_recursive_reporting(&entry.path(), &dest.join(entry.file_name()), cancel, progress, report)?;
        }
        return Ok(());
    }
    fs::copy(src, dest)?;
    progress.files_done += 1;
    progress.bytes_done += metadata.len();
    src.clone_into(&mut progress.current);
    report(progress);
    Ok(())
}

/// Moves every source into `dest`.
///
/// Tries a plain rename first -- fast, atomic, and the only option that
/// does not briefly double the disk usage. Falls back to
/// copy-then-remove when that fails, which it always does across
/// filesystems (`EXDEV`), and which is the normal case for anything
/// going to or from a network share.
pub fn move_into(sources: &[PathBuf], dest: &Path, on_conflict: OnConflict) -> Vec<Outcome> {
    move_into_reporting(sources, dest, on_conflict, &|| false, &mut |_| {})
}

fn run_reporting(
    sources: &[PathBuf],
    dest: &Path,
    on_conflict: OnConflict,
    cancel: &impl Fn() -> bool,
    mut each: impl FnMut(&Path, &Path) -> io::Result<()>,
) -> Vec<Outcome> {
    let mut outcomes = Vec::with_capacity(sources.len());
    for src in sources {
        if cancel() {
            outcomes.push(Outcome { path: src.clone(), error: Some("cancelled".to_string()) });
            continue;
        }
        let Some(name) = src.file_name() else {
            outcomes.push(Outcome { path: src.clone(), error: Some("no filename to copy under".to_string()) });
            continue;
        };
        let mut target = dest.join(name);

        // Copying something into the directory it is already in would
        // otherwise be a no-op that reports success while doing nothing,
        // or -- for a move -- a rename onto itself.
        if target == *src {
            match on_conflict {
                OnConflict::KeepBoth => target = free_name(&target),
                _ => {
                    outcomes.push(Outcome { path: src.clone(), error: Some("source and destination are the same".to_string()) });
                    continue;
                }
            }
        } else if target.exists() {
            match on_conflict {
                OnConflict::Skip => {
                    outcomes.push(Outcome { path: src.clone(), error: Some("skipped -- already exists".to_string()) });
                    continue;
                }
                OnConflict::KeepBoth => target = free_name(&target),
                OnConflict::Overwrite => {
                    // A file has to go before it can be replaced; a
                    // directory is merged into, so it stays.
                    if fs::symlink_metadata(&target).map(|m| !m.is_dir()).unwrap_or(false) {
                        if let Err(err) = fs::remove_file(&target) {
                            outcomes.push(Outcome { path: src.clone(), error: Some(err.to_string()) });
                            continue;
                        }
                    }
                }
            }
        }

        match each(src, &target) {
            Ok(()) => outcomes.push(Outcome { path: src.clone(), error: None }),
            Err(err) => outcomes.push(Outcome { path: src.clone(), error: Some(err.to_string()) }),
        }
    }
    outcomes
}

/// `notes.txt` -> `notes (2).txt`, counting up until nothing is in the
/// way. Keeps the extension where it belongs rather than appending past
/// it, so the result is still recognisably the same kind of file.
fn free_name(taken: &Path) -> PathBuf {
    let parent = taken.parent().unwrap_or(Path::new(""));
    let stem = taken.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = taken.extension().map(|e| e.to_string_lossy().into_owned());
    for n in 2..10_000 {
        let name = match &ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    taken.to_path_buf()
}

#[cfg(windows)]
fn copy_link(src: &Path, dest: &Path) -> io::Result<()> {
    let target = fs::read_link(src)?;
    if fs::metadata(src).map(|m| m.is_dir()).unwrap_or(false) {
        std::os::windows::fs::symlink_dir(&target, dest)
    } else {
        std::os::windows::fs::symlink_file(&target, dest)
    }
}

#[cfg(not(windows))]
fn copy_link(src: &Path, dest: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(fs::read_link(src)?, dest)
}

/// Applies a whole set of renames, in an order that cannot lose a file.
///
/// `via_temporaries` renames everything aside to a unique name first and
/// then into place. That makes the order irrelevant, which is what lets
/// `a -> b` and `b -> a` both happen: done one at a time, whichever went
/// second would land on a file that had not moved yet. The caller
/// decides whether it is needed (`fenix_explorer::needs_two_phases`),
/// because the safe path costs two renames per file and over a share
/// that is two round trips where one would do.
///
/// Missing parent directories are created, so a rename can also move a
/// file somewhere new.
///
/// **The two-phase path is all-or-nothing.** If anything fails, every
/// file already moved is put back and nothing is applied. That is
/// stricter than the rest of this module, which is deliberately
/// best-effort -- but a bulk rename is one edit the user made, and
/// applying half of it would leave a directory in a state they never
/// asked for and cannot easily reconstruct. The one-at-a-time path
/// stays best-effort, because there each rename really is independent.
pub fn rename_all(renames: &[(PathBuf, PathBuf)], via_temporaries: bool) -> Vec<Outcome> {
    if !via_temporaries {
        return renames
            .iter()
            .map(|(from, to)| match rename_one(from, to) {
                Ok(()) => Outcome { path: from.clone(), error: None },
                Err(err) => Outcome { path: from.clone(), error: Some(err.to_string()) },
            })
            .collect();
    }

    // Phase one: everything out of the way, so the order of phase two
    // cannot matter.
    let mut staged: Vec<(PathBuf, PathBuf, PathBuf)> = Vec::with_capacity(renames.len());
    for (from, to) in renames {
        let temp = temporary_beside(from);
        match fs::rename(from, &temp) {
            Ok(()) => staged.push((from.clone(), temp, to.clone())),
            Err(err) => {
                put_back(&staged, &[]);
                return abandoned(renames, from, &err.to_string());
            }
        }
    }

    // Phase two: into place.
    let mut done: Vec<(PathBuf, PathBuf)> = Vec::with_capacity(staged.len());
    for (index, (original, temp, target)) in staged.iter().enumerate() {
        if let Err(err) = rename_one(temp, target) {
            put_back(&staged[index..], &done);
            return abandoned(renames, original, &err.to_string());
        }
        done.push((original.clone(), target.clone()));
    }

    renames.iter().map(|(from, _)| Outcome { path: from.clone(), error: None }).collect()
}

/// Undoes a partly-applied batch: whatever is still wearing a temporary
/// name goes back to its original, and whatever already reached its
/// target is moved back from there.
fn put_back(staged: &[(PathBuf, PathBuf, PathBuf)], done: &[(PathBuf, PathBuf)]) {
    for (original, temp, _) in staged {
        let _ = fs::rename(temp, original);
    }
    for (original, target) in done {
        let _ = fs::rename(target, original);
    }
}

/// Every path reported as not applied, with the real reason on the one
/// that caused it -- so the message names the file that actually went
/// wrong rather than blaming the first in the list.
fn abandoned(renames: &[(PathBuf, PathBuf)], culprit: &Path, reason: &str) -> Vec<Outcome> {
    renames
        .iter()
        .map(|(from, _)| {
            let error =
                if from == culprit { reason.to_string() } else { format!("not applied -- {} could not be renamed", culprit.display()) };
            Outcome { path: from.clone(), error: Some(error) }
        })
        .collect()
}

fn rename_one(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(from, to)
}

/// A name nothing else can be using, in the same directory -- so the
/// staging rename stays on the same filesystem and cannot fail for
/// being a cross-volume move.
fn temporary_beside(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().unwrap_or(Path::new(""));
    loop {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".fenix-rename-{}-{n}", std::process::id()));
        if !candidate.exists() {
            return candidate;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn renames_apply_one_at_a_time_when_nothing_collides() {
        let dir = TempDir::new("rename_all_simple");
        let a = dir.write("a.txt", "A");
        let b = dir.write("b.txt", "B");

        let outcomes = rename_all(&[(a, dir.path().join("x.txt")), (b, dir.path().join("y.txt"))], false);

        assert!(outcomes.iter().all(|o| o.succeeded()), "{outcomes:?}");
        assert_eq!(std::fs::read_to_string(dir.path().join("x.txt")).unwrap(), "A");
        assert_eq!(std::fs::read_to_string(dir.path().join("y.txt")).unwrap(), "B");
    }

    #[test]
    fn two_names_can_be_swapped() {
        // The case the safe path exists for: done one at a time, the
        // second rename would land on a file that had not moved yet.
        let dir = TempDir::new("rename_all_swap");
        let a = dir.write("a.txt", "A");
        let b = dir.write("b.txt", "B");

        let outcomes = rename_all(&[(a.clone(), b.clone()), (b.clone(), a.clone())], true);

        assert!(outcomes.iter().all(|o| o.succeeded()), "{outcomes:?}");
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "B");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "A");
    }

    #[test]
    fn three_names_can_be_rotated() {
        let dir = TempDir::new("rename_all_rotate");
        let a = dir.write("a.txt", "A");
        let b = dir.write("b.txt", "B");
        let c = dir.write("c.txt", "C");

        let outcomes = rename_all(&[(a.clone(), b.clone()), (b.clone(), c.clone()), (c.clone(), a.clone())], true);

        assert!(outcomes.iter().all(|o| o.succeeded()), "{outcomes:?}");
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "C");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "A");
        assert_eq!(std::fs::read_to_string(&c).unwrap(), "B");
    }

    #[test]
    fn a_rename_can_move_a_file_into_a_directory_that_does_not_exist_yet() {
        let dir = TempDir::new("rename_all_mkdir");
        let a = dir.write("a.txt", "A");

        let outcomes = rename_all(&[(a, dir.path().join("2026").join("a.txt"))], false);

        assert!(outcomes[0].succeeded(), "{:?}", outcomes[0].error);
        assert_eq!(std::fs::read_to_string(dir.path().join("2026").join("a.txt")).unwrap(), "A");
    }

    #[test]
    fn a_failure_part_way_through_puts_everything_back() {
        // A half-applied rename that left files under temporary names
        // nobody chose would be worse than either outcome.
        let dir = TempDir::new("rename_all_rollback");
        let a = dir.write("a.txt", "A");
        let b = dir.write("b.txt", "B");
        // The second rename cannot succeed: a directory is in the way
        // and is not empty, so it cannot be replaced.
        let blocked = dir.path().join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        std::fs::write(blocked.join("inside.txt"), "x").unwrap();

        let outcomes = rename_all(&[(a.clone(), dir.path().join("x.txt")), (b.clone(), blocked.clone())], true);

        assert!(outcomes.iter().all(|o| !o.succeeded()), "a bulk rename is one edit: none of it applied");
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "A", "put back, even though its own rename worked");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "B", "and so is this one");
        assert!(!dir.path().join("x.txt").exists(), "and nothing was left half-renamed");
        // The message blames the file that actually went wrong, rather
        // than the first one in the list.
        assert!(outcomes[0].error.as_ref().unwrap().contains("b.txt"), "{outcomes:?}");
    }

    #[test]
    fn no_temporary_names_are_left_behind() {
        let dir = TempDir::new("rename_all_no_litter");
        let a = dir.write("a.txt", "A");
        let b = dir.write("b.txt", "B");

        rename_all(&[(a.clone(), b.clone()), (b, a)], true);

        let names: Vec<String> =
            std::fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(!names.iter().any(|n| n.contains("fenix-rename")), "got: {names:?}");
    }

    #[test]
    fn a_conflict_is_reported_before_anything_is_written() {
        let src = TempDir::new("conflict_src");
        let dest = TempDir::new("conflict_dest");
        let a = src.write("a.txt", "new");
        let b = src.write("b.txt", "new");
        dest.write("a.txt", "old");

        let conflicts = conflicts_in(&[a, b], dest.path());

        assert_eq!(conflicts, vec![dest.path().join("a.txt")]);
        assert_eq!(std::fs::read_to_string(dest.path().join("a.txt")).unwrap(), "old", "and nothing was written");
    }

    #[test]
    fn an_empty_destination_conflicts_with_nothing() {
        let src = TempDir::new("conflict_none_src");
        let dest = TempDir::new("conflict_none_dest");
        assert!(conflicts_in(&[src.write("a.txt", "x")], dest.path()).is_empty());
    }

    #[test]
    fn a_transfer_reports_where_it_has_got_to() {
        let src = TempDir::new("progress_src");
        let dest = TempDir::new("progress_dest");
        let tree = src.mkdir("tree");
        std::fs::write(tree.join("a.txt"), "12345").unwrap();
        std::fs::write(tree.join("b.txt"), "123").unwrap();

        let mut seen: Vec<(usize, u64)> = Vec::new();
        let outcomes = copy_into_reporting(&[tree], dest.path(), OnConflict::Overwrite, &|| false, &mut |p| {
            seen.push((p.files_done, p.bytes_done));
            // Both counts, because neither alone is honest.
            assert_eq!(p.files_total, 2);
            assert_eq!(p.bytes_total, 8);
        });

        assert!(outcomes[0].succeeded(), "{:?}", outcomes[0].error);
        assert_eq!(seen.last(), Some(&(2, 8)), "it finishes on the total: {seen:?}");
    }

    #[test]
    fn measuring_counts_a_link_as_one_entry_not_as_the_tree_behind_it() {
        let dir = TempDir::new("progress_measure_link");
        let real = dir.mkdir("real");
        std::fs::write(real.join("big.txt"), vec![b'x'; 4096]).unwrap();
        let link = dir.path().join("link");
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(&real, &link).is_ok();
        #[cfg(not(windows))]
        let made = std::os::unix::fs::symlink(&real, &link).is_ok();
        if !made {
            return;
        }
        assert_eq!(measure(&[link], &|| false), (1, 0));
    }

    #[test]
    fn cancelling_stops_the_batch_and_says_which_ones_did_not_happen() {
        let src = TempDir::new("cancel_src");
        let dest = TempDir::new("cancel_dest");
        let a = src.write("a.txt", "A");
        let b = src.write("b.txt", "B");

        let outcomes = copy_into_reporting(&[a, b], dest.path(), OnConflict::Overwrite, &|| true, &mut |_| {});

        assert!(outcomes.iter().all(|o| o.error.as_deref() == Some("cancelled")), "{outcomes:?}");
        assert!(!dest.path().join("a.txt").exists());
    }

    #[test]
    fn what_was_already_copied_stays_after_a_cancel() {
        // That is what actually happened; deleting it to make the
        // operation look atomic would destroy work the user may want.
        use std::sync::atomic::{AtomicUsize, Ordering};
        let src = TempDir::new("cancel_partial_src");
        let dest = TempDir::new("cancel_partial_dest");
        let a = src.write("a.txt", "A");
        let b = src.write("b.txt", "B");
        let seen = AtomicUsize::new(0);
        // Allow the measuring pass and the first file, then stop.
        let cancel = || seen.fetch_add(1, Ordering::Relaxed) > 4;

        copy_into_reporting(&[a, b], dest.path(), OnConflict::Overwrite, &cancel, &mut |_| {});

        assert!(dest.path().join("a.txt").exists(), "the one that finished is still there");
        assert!(!dest.path().join("b.txt").exists());
    }

    #[test]
    fn copying_brings_files_and_whole_trees() {
        let src = TempDir::new("copy_src");
        let dest = TempDir::new("copy_dest");
        let file = src.write("a.txt", "hello");
        let tree = src.mkdir("tree");
        std::fs::write(tree.join("inner.txt"), "deep").unwrap();

        let outcomes = copy_into(&[file, tree], dest.path(), OnConflict::Overwrite);

        assert!(outcomes.iter().all(|o| o.succeeded()), "{outcomes:?}");
        assert_eq!(std::fs::read_to_string(dest.path().join("a.txt")).unwrap(), "hello");
        assert_eq!(std::fs::read_to_string(dest.path().join("tree").join("inner.txt")).unwrap(), "deep");
    }

    #[test]
    fn skip_leaves_the_existing_file_alone_and_says_it_did() {
        let src = TempDir::new("copy_skip_src");
        let dest = TempDir::new("copy_skip_dest");
        let a = src.write("a.txt", "new");
        dest.write("a.txt", "old");

        let outcomes = copy_into(&[a], dest.path(), OnConflict::Skip);

        assert!(!outcomes[0].succeeded(), "a skip is not a success -- the file did not arrive");
        assert!(outcomes[0].error.as_ref().unwrap().contains("skipped"));
        assert_eq!(std::fs::read_to_string(dest.path().join("a.txt")).unwrap(), "old");
    }

    #[test]
    fn keep_both_numbers_the_newcomer_and_keeps_the_extension() {
        let src = TempDir::new("copy_both_src");
        let dest = TempDir::new("copy_both_dest");
        let a = src.write("notes.txt", "new");
        dest.write("notes.txt", "old");

        let outcomes = copy_into(&[a], dest.path(), OnConflict::KeepBoth);

        assert!(outcomes[0].succeeded(), "{:?}", outcomes[0].error);
        assert_eq!(std::fs::read_to_string(dest.path().join("notes.txt")).unwrap(), "old", "the original is untouched");
        assert_eq!(std::fs::read_to_string(dest.path().join("notes (2).txt")).unwrap(), "new");
    }

    #[test]
    fn keep_both_counts_past_names_that_are_also_taken() {
        let src = TempDir::new("copy_both2_src");
        let dest = TempDir::new("copy_both2_dest");
        let a = src.write("notes.txt", "new");
        dest.write("notes.txt", "old");
        dest.write("notes (2).txt", "older");

        copy_into(&[a], dest.path(), OnConflict::KeepBoth);

        assert!(dest.path().join("notes (3).txt").exists());
    }

    #[test]
    fn overwrite_replaces_a_file() {
        let src = TempDir::new("copy_over_src");
        let dest = TempDir::new("copy_over_dest");
        let a = src.write("a.txt", "new");
        dest.write("a.txt", "old");

        copy_into(&[a], dest.path(), OnConflict::Overwrite);

        assert_eq!(std::fs::read_to_string(dest.path().join("a.txt")).unwrap(), "new");
    }

    #[test]
    fn overwriting_a_directory_merges_into_it_rather_than_emptying_it() {
        // Copying a folder onto a folder of the same name must not
        // delete the files that were only in the destination.
        let src = TempDir::new("copy_merge_src");
        let dest = TempDir::new("copy_merge_dest");
        let tree = src.mkdir("tree");
        std::fs::write(tree.join("new.txt"), "new").unwrap();
        let existing = dest.mkdir("tree");
        std::fs::write(existing.join("kept.txt"), "kept").unwrap();

        copy_into(&[tree], dest.path(), OnConflict::Overwrite);

        assert!(dest.path().join("tree").join("new.txt").exists());
        assert!(dest.path().join("tree").join("kept.txt").exists(), "the destination's own file survived");
    }

    #[test]
    fn moving_removes_the_source() {
        let src = TempDir::new("move_src");
        let dest = TempDir::new("move_dest");
        let a = src.write("a.txt", "hello");

        let outcomes = move_into(&[a.clone()], dest.path(), OnConflict::Overwrite);

        assert!(outcomes[0].succeeded(), "{:?}", outcomes[0].error);
        assert!(!a.exists());
        assert_eq!(std::fs::read_to_string(dest.path().join("a.txt")).unwrap(), "hello");
    }

    #[test]
    fn a_move_that_is_skipped_leaves_the_source_where_it_was() {
        // The dangerous half of a move: deciding not to write must never
        // also mean deleting.
        let src = TempDir::new("move_skip_src");
        let dest = TempDir::new("move_skip_dest");
        let a = src.write("a.txt", "new");
        dest.write("a.txt", "old");

        move_into(&[a.clone()], dest.path(), OnConflict::Skip);

        assert!(a.exists(), "the source is still there");
        assert_eq!(std::fs::read_to_string(dest.path().join("a.txt")).unwrap(), "old");
    }

    #[test]
    fn copying_something_onto_itself_is_refused_rather_than_reported_as_done() {
        let dir = TempDir::new("copy_self");
        let a = dir.write("a.txt", "x");

        let outcomes = copy_into(&[a.clone()], dir.path(), OnConflict::Overwrite);

        assert!(!outcomes[0].succeeded());
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "x", "and it is still there");
    }

    #[test]
    fn copying_into_its_own_directory_with_keep_both_makes_a_numbered_copy() {
        // "Duplicate this file" -- the one case where copying into the
        // same folder is exactly what was meant.
        let dir = TempDir::new("copy_self_both");
        let a = dir.write("a.txt", "x");

        let outcomes = copy_into(&[a], dir.path(), OnConflict::KeepBoth);

        assert!(outcomes[0].succeeded(), "{:?}", outcomes[0].error);
        assert_eq!(std::fs::read_to_string(dir.path().join("a (2).txt")).unwrap(), "x");
    }

    #[test]
    fn one_failure_does_not_stop_the_rest_of_the_batch() {
        let src = TempDir::new("copy_partial_src");
        let dest = TempDir::new("copy_partial_dest");
        let missing = src.path().join("never-existed.txt");
        let real = src.write("real.txt", "x");

        let outcomes = copy_into(&[missing, real], dest.path(), OnConflict::Overwrite);

        assert_eq!(outcomes.len(), 2);
        assert!(!outcomes[0].succeeded());
        assert!(outcomes[1].succeeded(), "the one after the failure still happened");
        assert!(dest.path().join("real.txt").exists());
    }
}
