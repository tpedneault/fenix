//! The two things fenix should hand back to the operating system.
//!
//! A file manager that can do everything eventually meets a `.xlsx`, and
//! the right answer there is Excel, not a hex dump. And however good the
//! listing gets, "show me this in Explorer" is the escape hatch that
//! makes trusting it easy -- being unable to leave is not the same as
//! not needing to.
//!
//! Both go through `explorer.exe`, which is the shell itself: opening a
//! path through it applies whatever the user has associated with that
//! type, including the choices they made in Windows' own "open with"
//! dialog. Shelling out to `cmd /c start` would work too and brings
//! quoting rules that a path with `&` in it fails, for no gain.

use std::io;
use std::path::Path;
use std::process::Command;

/// Opens `path` with whatever the system associates with it.
///
/// A directory opens in Explorer, which is what asking the shell to
/// open a folder means everywhere else too.
pub fn open_with_default(path: &Path) -> io::Result<()> {
    if !path.exists() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("{} is not there", path.display())));
    }
    #[cfg(windows)]
    {
        // Deliberately not waited on: this hands the file to another
        // application, which may sit open for hours. `spawn` and forget.
        no_window("explorer.exe").arg(path).spawn().map(|_| ())
    }
    #[cfg(not(windows))]
    {
        Command::new("xdg-open").arg(path).spawn().map(|_| ())
    }
}

/// Shows `path` in the system file manager, selected.
///
/// Selected, not merely opened: the point of reaching for this is
/// usually to do something Explorer can do and fenix cannot -- drag it
/// somewhere, use a shell extension -- and landing in the right folder
/// with the wrong thing highlighted is half an answer.
pub fn reveal(path: &Path) -> io::Result<()> {
    if !path.exists() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("{} is not there", path.display())));
    }
    #[cfg(windows)]
    {
        // `/select,` takes its argument glued on with no space, which is
        // why this is one argument and not two -- passed separately,
        // Explorer opens the *parent* and selects nothing.
        no_window("explorer.exe").arg(format!("/select,{}", path.display())).spawn().map(|_| ())
    }
    #[cfg(not(windows))]
    {
        // No universal equivalent, so open the containing directory --
        // less than asked for, but more useful than an error.
        let target = path.parent().unwrap_or(path);
        Command::new("xdg-open").arg(target).spawn().map(|_| ())
    }
}

#[cfg(windows)]
fn no_window(program: &str) -> Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    // These deliberately do not open anything. Launching Explorer from
    // a test suite would leave a window on the developer's desktop for
    // every run, and what is worth checking here is the refusal -- the
    // part with a decision in it. The launching itself is one `spawn`
    // call, verified by using it.

    #[test]
    fn opening_something_that_is_not_there_is_refused_rather_than_launched() {
        let dir = TempDir::new("shell_open_missing");
        let err = open_with_default(&dir.path().join("nope.txt")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(err.to_string().contains("nope.txt"), "the message names it: {err}");
    }

    #[test]
    fn revealing_something_that_is_not_there_is_refused_too() {
        let dir = TempDir::new("shell_reveal_missing");
        assert_eq!(reveal(&dir.path().join("nope.txt")).unwrap_err().kind(), io::ErrorKind::NotFound);
    }
}
