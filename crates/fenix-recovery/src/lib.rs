//! Unsaved work, written somewhere it survives a crash.
//!
//! Fenix keeps edits in memory until you save, so a crash, a power cut
//! or a killed process loses everything since the last `:w`. This is the
//! net under that: every dirty buffer is periodically written to a
//! snapshot, and the snapshot is deleted the moment the buffer is saved
//! or closed cleanly -- so a normal session leaves nothing behind, and
//! only an abnormal exit leaves anything to find.
//!
//! **Not Vim's swap files.** Those live next to the file being edited,
//! which means `.gitignore` entries, `ls` noise, and build tools tripping
//! over them; and they carry a locking protocol for "another instance
//! has this file open" that Fenix has no use for. Their famous failure
//! mode -- a stale `.swp` prompting about a file nobody is editing --
//! is worse than the loss they prevent. Snapshots here live in one
//! directory under Fenix's own config location, keyed by a hash of the
//! file's path, and never touch the working tree. Emacs' `#auto-save#`
//! shape, minus the litter.
//!
//! One snapshot is one file: a small text header naming the original
//! path and when it was written, then the buffer's contents. The header
//! is length-prefixed rather than delimited, so a path containing a
//! newline (legal on Unix) can't corrupt the format.

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The first line of every snapshot. Bumped only if the format changes
/// in a way older Fenix can't read -- a snapshot with an unknown
/// version is skipped rather than guessed at, since the whole point is
/// to hand back exactly what was typed.
const MAGIC: &str = "fenix-recovery 1";

/// One recovered file: where it came from, when it was written, and
/// what was in the buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The file the buffer was editing.
    pub original: PathBuf,
    pub saved_at: SystemTime,
    pub contents: String,
    /// The snapshot's own file, so a caller can delete exactly this one.
    pub snapshot: PathBuf,
}

impl Snapshot {
    /// How long ago this was written, for a human-readable listing.
    /// `None` if the clock has moved backwards since (a reboot with a
    /// bad RTC, a VM restored from a saved state).
    pub fn age(&self) -> Option<std::time::Duration> {
        SystemTime::now().duration_since(self.saved_at).ok()
    }
}

/// Where snapshots live: `<config>/fenix/recovery`.
///
/// Alongside `config.ini` rather than in a temp directory, because a
/// temp directory is exactly what an operating system is entitled to
/// empty out from under you -- and this is the copy that exists for
/// when things went wrong.
pub fn default_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("fenix").join("recovery"))
}

/// The snapshot file for `original`, inside `dir`.
///
/// Named by a hash of the path rather than by the path itself: a real
/// path contains separators, drive letters and colons, none of which
/// survive being flattened into a filename, and two files with the same
/// basename in different directories must not collide. The original
/// path is recorded inside the file, which is what `list` reads back.
pub fn snapshot_path(dir: &Path, original: &Path) -> PathBuf {
    dir.join(format!("{}.fenixsave", path_key(original)))
}

/// A stable, filename-safe key for a path.
///
/// FNV-1a, written out rather than pulled in as a dependency: this needs
/// to be stable across runs (so the same file always maps to the same
/// snapshot) and nothing more. It is not a security boundary -- a
/// collision costs one snapshot, and the path stored inside the file is
/// what's actually trusted when reading one back.
fn path_key(original: &Path) -> String {
    let text = original.to_string_lossy();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Writes `contents` as the snapshot for `original`.
///
/// Written to a temporary name and renamed into place, so a crash
/// *during* the snapshot leaves either the previous complete snapshot or
/// none -- never a half-written one, which is the one thing worse than
/// having no snapshot at all.
pub fn write(dir: &Path, original: &Path, contents: &str) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let target = snapshot_path(dir, original);
    let temp = target.with_extension("fenixsave.part");
    let seconds = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let path_text = original.to_string_lossy();

    let mut body = String::with_capacity(contents.len() + 128);
    body.push_str(MAGIC);
    body.push('\n');
    body.push_str(&format!("saved {seconds}\n"));
    // Length-prefixed, not delimited: a path may legally contain a
    // newline, and a delimiter would let one truncate the header and
    // swallow the file's own contents.
    body.push_str(&format!("path {}\n", path_text.len()));
    body.push_str(&path_text);
    body.push('\n');
    body.push_str(contents);

    std::fs::write(&temp, body)?;
    std::fs::rename(&temp, &target)?;
    Ok(target)
}

/// Removes the snapshot for `original`, if there is one.
///
/// Called the moment a buffer is saved or deliberately closed -- a
/// normal session must leave nothing behind, or the next start would
/// offer to recover work that isn't lost.
pub fn discard(dir: &Path, original: &Path) -> io::Result<()> {
    match std::fs::remove_file(snapshot_path(dir, original)) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

/// Every readable snapshot in `dir`, newest first.
///
/// A snapshot that can't be parsed is skipped rather than erroring:
/// one corrupt file must not hide every other recoverable buffer, which
/// is precisely the moment they matter.
pub fn list(dir: &Path) -> Vec<Snapshot> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Snapshot> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "fenixsave"))
        .filter_map(|path| {
            let raw = std::fs::read_to_string(&path).ok()?;
            parse(&raw, path)
        })
        .collect();
    out.sort_by(|a, b| b.saved_at.cmp(&a.saved_at));
    out
}

/// Deletes snapshots older than `max_age`.
///
/// Without this the directory only ever grows: a file recovered by hand
/// outside Fenix, or one whose original was deleted, leaves a snapshot
/// nothing will ever claim. Returns how many were removed.
pub fn prune(dir: &Path, max_age: std::time::Duration) -> usize {
    list(dir)
        .into_iter()
        .filter(|snapshot| snapshot.age().is_some_and(|age| age > max_age))
        .filter(|snapshot| std::fs::remove_file(&snapshot.snapshot).is_ok())
        .count()
}

fn parse(raw: &str, snapshot: PathBuf) -> Option<Snapshot> {
    let rest = raw.strip_prefix(MAGIC)?.strip_prefix('\n')?;
    let (saved_line, rest) = rest.split_once('\n')?;
    let seconds: u64 = saved_line.strip_prefix("saved ")?.parse().ok()?;
    let (path_line, rest) = rest.split_once('\n')?;
    let path_len: usize = path_line.strip_prefix("path ")?.parse().ok()?;
    // The path is exactly `path_len` bytes, then one newline, then
    // everything else is the buffer -- including any newlines of its
    // own, which is the whole reason for the length prefix.
    if rest.len() < path_len + 1 || !rest.is_char_boundary(path_len) {
        return None;
    }
    let (original, rest) = rest.split_at(path_len);
    let contents = rest.strip_prefix('\n')?;
    Some(Snapshot {
        original: PathBuf::from(original),
        saved_at: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds),
        contents: contents.to_string(),
        snapshot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("fenix-recovery-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_snapshot_round_trips_exactly_what_was_in_the_buffer() {
        let dir = TempDir::new("round_trip");
        let original = Path::new("/home/thomas/project/src/main.rs");

        write(dir.path(), original, "fn main() {}\n").unwrap();
        let found = list(dir.path());

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].original, original);
        assert_eq!(found[0].contents, "fn main() {}\n");
    }

    #[test]
    fn contents_with_newlines_and_a_header_shaped_first_line_survive() {
        // The buffer is arbitrary text, and nothing stops it starting
        // with something that looks like this format's own header.
        let dir = TempDir::new("tricky_contents");
        let contents = "fenix-recovery 1\nsaved 12345\npath 4\nfake\nreal contents\n\nwith a blank line\n";

        write(dir.path(), Path::new("/tmp/a.txt"), contents).unwrap();

        assert_eq!(list(dir.path())[0].contents, contents);
    }

    #[test]
    fn a_path_containing_a_newline_does_not_corrupt_the_format() {
        // Legal on Unix, and the reason the path is length-prefixed
        // rather than newline-delimited.
        let dir = TempDir::new("newline_path");
        let original = PathBuf::from("/tmp/weird\nname.txt");

        write(dir.path(), &original, "body\n").unwrap();

        let found = list(dir.path());
        assert_eq!(found[0].original, original);
        assert_eq!(found[0].contents, "body\n");
    }

    #[test]
    fn an_empty_buffer_round_trips_as_empty_rather_than_failing_to_parse() {
        let dir = TempDir::new("empty");
        write(dir.path(), Path::new("/tmp/empty.txt"), "").unwrap();
        assert_eq!(list(dir.path())[0].contents, "");
    }

    #[test]
    fn writing_twice_replaces_rather_than_accumulating() {
        let dir = TempDir::new("replace");
        let original = Path::new("/tmp/a.txt");

        write(dir.path(), original, "first\n").unwrap();
        write(dir.path(), original, "second\n").unwrap();

        let found = list(dir.path());
        assert_eq!(found.len(), 1, "one file, one snapshot");
        assert_eq!(found[0].contents, "second\n");
    }

    #[test]
    fn two_files_with_the_same_basename_get_separate_snapshots() {
        // The reason snapshots are keyed by a hash of the whole path
        // rather than by the file's name.
        let dir = TempDir::new("same_basename");
        write(dir.path(), Path::new("/a/mod.rs"), "one\n").unwrap();
        write(dir.path(), Path::new("/b/mod.rs"), "two\n").unwrap();

        let found = list(dir.path());
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn discarding_removes_only_that_files_snapshot() {
        let dir = TempDir::new("discard");
        write(dir.path(), Path::new("/a/one.txt"), "one\n").unwrap();
        write(dir.path(), Path::new("/a/two.txt"), "two\n").unwrap();

        discard(dir.path(), Path::new("/a/one.txt")).unwrap();

        let found = list(dir.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].original, PathBuf::from("/a/two.txt"));
    }

    #[test]
    fn discarding_something_that_was_never_snapshotted_is_not_an_error() {
        // Every save calls this, and most saves have nothing to remove.
        let dir = TempDir::new("discard_missing");
        assert!(discard(dir.path(), Path::new("/never/seen.txt")).is_ok());
    }

    #[test]
    fn a_corrupt_snapshot_is_skipped_rather_than_hiding_the_others() {
        // A half-written file shouldn't cost you every other recoverable
        // buffer -- which is exactly the moment they matter.
        let dir = TempDir::new("corrupt");
        write(dir.path(), Path::new("/a/good.txt"), "good\n").unwrap();
        std::fs::write(dir.path().join("garbage.fenixsave"), "not a snapshot at all").unwrap();
        std::fs::write(dir.path().join("truncated.fenixsave"), "fenix-recovery 1\nsaved 1\n").unwrap();

        let found = list(dir.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].contents, "good\n");
    }

    #[test]
    fn a_snapshot_from_a_future_format_version_is_skipped() {
        let dir = TempDir::new("future");
        std::fs::write(dir.path().join("newer.fenixsave"), "fenix-recovery 9\nsaved 1\npath 6\n/a.txt\nbody").unwrap();
        assert!(list(dir.path()).is_empty(), "better to offer nothing than to hand back the wrong bytes");
    }

    #[test]
    fn files_that_are_not_snapshots_are_ignored() {
        let dir = TempDir::new("other_files");
        write(dir.path(), Path::new("/a/real.txt"), "real\n").unwrap();
        std::fs::write(dir.path().join("README"), "unrelated").unwrap();
        std::fs::write(dir.path().join("a.fenixsave.part"), "an interrupted write").unwrap();

        assert_eq!(list(dir.path()).len(), 1);
    }

    #[test]
    fn listing_a_directory_that_does_not_exist_is_empty_not_an_error() {
        // The ordinary first-run case.
        assert!(list(&std::env::temp_dir().join("fenix-recovery-definitely-not-here")).is_empty());
    }

    #[test]
    fn pruning_removes_the_stale_and_keeps_the_fresh() {
        let dir = TempDir::new("prune");
        write(dir.path(), Path::new("/a/fresh.txt"), "fresh\n").unwrap();
        // A snapshot dated well in the past, as one left by a crash
        // months ago would be.
        std::fs::write(
            dir.path().join("old.fenixsave"),
            format!("{MAGIC}\nsaved 1000\npath 11\n/a/old.txt\n\nold contents\n"),
        )
        .unwrap();

        let removed = prune(dir.path(), std::time::Duration::from_secs(60 * 60 * 24));

        assert_eq!(removed, 1);
        let found = list(dir.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].contents, "fresh\n");
    }

    #[test]
    fn the_newest_snapshot_is_listed_first() {
        let dir = TempDir::new("order");
        for (name, seconds) in [("a", 100u64), ("b", 300), ("c", 200)] {
            let path = format!("/x/{name}.txt");
            std::fs::write(
                dir.path().join(format!("{name}.fenixsave")),
                format!("{MAGIC}\nsaved {seconds}\npath {}\n{path}\n{name}\n", path.len()),
            )
            .unwrap();
        }

        let names: Vec<String> = list(dir.path()).into_iter().map(|s| s.contents.trim().to_string()).collect();

        assert_eq!(names, vec!["b", "c", "a"], "newest first, so the listing leads with what you just lost");
    }

    #[test]
    fn an_interrupted_write_leaves_the_previous_snapshot_intact() {
        // Written to a temporary name and renamed, so there is no window
        // where the snapshot is half a file.
        let dir = TempDir::new("atomic");
        let original = Path::new("/a/file.txt");
        write(dir.path(), original, "complete\n").unwrap();
        std::fs::write(snapshot_path(dir.path(), original).with_extension("fenixsave.part"), "half writ").unwrap();

        let found = list(dir.path());
        assert_eq!(found.len(), 1, "the partial file isn't a snapshot");
        assert_eq!(found[0].contents, "complete\n");
    }
}
