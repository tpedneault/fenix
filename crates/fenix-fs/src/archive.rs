//! Zipping and unzipping, through the archiver Windows already ships.
//!
//! Windows 10 and later include `bsdtar` as `System32\tar.exe`, and it
//! makes and reads real zip files. That is the whole implementation --
//! no archive crate, matching how the rest of this workspace reaches
//! for `git`, `docker` and `net` rather than linking their jobs in.
//!
//! **The absolute path matters.** `tar` on `PATH` is very often GNU tar
//! (Git for Windows puts it there), and GNU tar does not make zips: ask
//! it for `out.zip` and it writes an uncompressed *tar* under that name,
//! which Explorer then refuses to open. Confirmed on this machine
//! before this was written. So the system copy is named outright, and
//! anything else is treated as unavailable rather than tried and
//! trusted.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the archiver lives, when there is one.
///
/// `None` means archiving is simply not available here, which callers
/// should say plainly rather than failing at the moment of use -- the
/// same posture the project-file listing takes when `fd` is missing.
pub fn archiver() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let path = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
            .join("System32")
            .join("tar.exe");
        path.exists().then_some(path)
    }
    #[cfg(not(windows))]
    {
        // Elsewhere `bsdtar` is the one that speaks zip; GNU tar does
        // not, and quietly producing a tar named `.zip` is worse than
        // saying no.
        let candidate = PathBuf::from("/usr/bin/bsdtar");
        candidate.exists().then_some(candidate)
    }
}

/// Packs `sources` into `archive`.
///
/// Every source must live in the same directory, which is the only case
/// this needs to serve (the marked set of one listing) and the only one
/// with an obvious answer for what the paths inside the archive should
/// be: their own names, relative to that directory. Anything else would
/// be inventing a layout nobody asked for.
///
/// The format comes from `archive`'s extension -- `.zip`, `.tar.gz`,
/// `.tgz` and the rest -- because that is what the user typed and what
/// whatever opens it will expect.
pub fn create(sources: &[PathBuf], archive: &Path) -> io::Result<()> {
    let Some(tar) = archiver() else {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "no archiver available on this machine"));
    };
    if sources.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "nothing to archive"));
    }
    let base = sources[0].parent().map(Path::to_path_buf).unwrap_or_default();
    let mut names = Vec::with_capacity(sources.len());
    for source in sources {
        if source.parent().map(Path::to_path_buf).unwrap_or_default() != base {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "everything in one archive has to come from one directory"));
        }
        let Some(name) = source.file_name() else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "nothing to name inside the archive"));
        };
        names.push(name.to_owned());
    }

    // `-a` picks the format from the name, which is why the extension is
    // the whole of the format decision.
    let mut command = no_window(&tar);
    command.arg("-a").arg("-c").arg("-f").arg(archive).arg("-C").arg(&base);
    for name in &names {
        command.arg(name);
    }
    run(command)
}

/// Unpacks `archive` into `dest`, which is created if it is not there.
///
/// Extracts into a directory of its own rather than over whatever is
/// around it -- an archive with forty files at its root, emptied into a
/// folder that already had things in it, is a mess nobody can undo.
/// Choosing that directory is the caller's job; this only makes it.
pub fn extract(archive: &Path, dest: &Path) -> io::Result<()> {
    let Some(tar) = archiver() else {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "no archiver available on this machine"));
    };
    std::fs::create_dir_all(dest)?;
    let mut command = no_window(&tar);
    command.arg("-x").arg("-f").arg(archive).arg("-C").arg(dest);
    run(command)
}

/// Whether `path` looks like something `extract` could open, by name.
///
/// By name and not by content: reading a header would mean opening
/// every file in a listing to decide whether to offer one key on it,
/// which on a share is a listing that never finishes.
pub fn looks_like_archive(path: &Path) -> bool {
    const SUFFIXES: &[&str] = &[".zip", ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tbz", ".tar.xz", ".txz", ".tar.zst", ".7z"];
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
}

/// A sensible name for the archive of a set of things: the single item's
/// own name with `.zip` on it, or the containing folder's name when
/// there are several.
pub fn suggested_name(sources: &[PathBuf]) -> String {
    match sources {
        [only] => format!("{}.zip", only.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "archive".into())),
        _ => {
            let folder = sources
                .first()
                .and_then(|s| s.parent())
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "archive".into());
            format!("{folder}.zip")
        }
    }
}

/// A sensible directory to unpack into: the archive's own name without
/// its extension, beside it.
pub fn suggested_destination(archive: &Path) -> PathBuf {
    let name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "extracted".into());
    let lower = name.to_lowercase();
    let stem = ["tar.gz", "tar.bz2", "tar.xz", "tar.zst"]
        .iter()
        .find_map(|multi| lower.strip_suffix(&format!(".{multi}")).map(|s| s.to_string()))
        .unwrap_or_else(|| name.rsplit_once('.').map(|(stem, _)| stem.to_string()).unwrap_or_else(|| name.clone()));
    archive.parent().unwrap_or(Path::new(".")).join(stem)
}

fn run(mut command: Command) -> io::Result<()> {
    let output = command.output()?;
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&output.stderr);
    let message = message.lines().find(|l| !l.trim().is_empty()).unwrap_or("the archiver failed").trim().to_string();
    Err(io::Error::other(message))
}

/// No console flash while the editor is drawing -- same treatment
/// `fenix-git`'s process helper applies for the same reason.
fn no_window(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn a_tree_survives_a_zip_round_trip() {
        let Some(_) = archiver() else { return };
        let dir = TempDir::new("archive_round_trip");
        let tree = dir.mkdir("tree");
        std::fs::write(tree.join("a.txt"), "hello").unwrap();
        std::fs::create_dir(tree.join("sub")).unwrap();
        std::fs::write(tree.join("sub").join("b.txt"), "world").unwrap();
        let zip = dir.path().join("out.zip");

        create(&[tree.clone()], &zip).unwrap();
        let back = dir.path().join("back");
        extract(&zip, &back).unwrap();

        assert_eq!(std::fs::read_to_string(back.join("tree").join("a.txt")).unwrap(), "hello");
        assert_eq!(std::fs::read_to_string(back.join("tree").join("sub").join("b.txt")).unwrap(), "world");
    }

    #[test]
    fn what_is_written_is_really_a_zip() {
        // The trap this module exists to avoid: GNU tar answers a
        // request for `out.zip` with an uncompressed tar under that
        // name, which Explorer will not open.
        let Some(_) = archiver() else { return };
        let dir = TempDir::new("archive_is_real_zip");
        let file = dir.write("a.txt", "hello");
        let zip = dir.path().join("out.zip");

        create(&[file], &zip).unwrap();

        let bytes = std::fs::read(&zip).unwrap();
        assert_eq!(&bytes[..2], b"PK", "not a zip -- got {:?}", &bytes[..4.min(bytes.len())]);
    }

    #[test]
    fn several_files_pack_side_by_side_without_their_directory() {
        let Some(_) = archiver() else { return };
        let dir = TempDir::new("archive_several");
        let a = dir.write("a.txt", "A");
        let b = dir.write("b.txt", "B");
        let zip = dir.path().join("out.zip");

        create(&[a, b], &zip).unwrap();
        let back = dir.path().join("back");
        extract(&zip, &back).unwrap();

        assert_eq!(std::fs::read_to_string(back.join("a.txt")).unwrap(), "A");
        assert_eq!(std::fs::read_to_string(back.join("b.txt")).unwrap(), "B");
    }

    #[test]
    fn sources_from_different_directories_are_refused() {
        // There is no obvious answer for what their paths inside the
        // archive should be, and inventing one would surprise whoever
        // opens it.
        let a = TempDir::new("archive_mixed_a");
        let b = TempDir::new("archive_mixed_b");
        let err = create(&[a.write("a.txt", "A"), b.write("b.txt", "B")], &a.path().join("out.zip")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn archiving_nothing_is_refused() {
        let dir = TempDir::new("archive_empty");
        assert!(create(&[], &dir.path().join("out.zip")).is_err());
    }

    #[test]
    fn a_broken_archive_reports_what_the_archiver_said() {
        let Some(_) = archiver() else { return };
        let dir = TempDir::new("archive_broken");
        let fake = dir.write("not-really.zip", "this is not an archive");

        let err = extract(&fake, &dir.path().join("back")).unwrap_err();

        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn archives_are_recognised_by_their_names() {
        // By name, because reading a header would mean opening every
        // file in a listing to decide whether one key applies.
        assert!(looks_like_archive(Path::new("backup.zip")));
        assert!(looks_like_archive(Path::new("Backup.ZIP")));
        assert!(looks_like_archive(Path::new("src.tar.gz")));
        assert!(looks_like_archive(Path::new("src.tgz")));
        assert!(!looks_like_archive(Path::new("notes.txt")));
        assert!(!looks_like_archive(Path::new("zip")));
    }

    #[test]
    fn one_item_is_named_after_itself_and_several_after_their_folder() {
        assert_eq!(suggested_name(&[PathBuf::from(r"C:\work\report.docx")]), "report.docx.zip");
        assert_eq!(suggested_name(&[PathBuf::from(r"C:\work\a.txt"), PathBuf::from(r"C:\work\b.txt")]), "work.zip");
    }

    #[test]
    fn an_archive_unpacks_into_a_directory_named_after_itself() {
        assert_eq!(suggested_destination(Path::new(r"C:\work\backup.zip")), PathBuf::from(r"C:\work\backup"));
        // A two-part extension is one extension.
        assert_eq!(suggested_destination(Path::new(r"C:\work\src.tar.gz")), PathBuf::from(r"C:\work\src"));
    }
}
