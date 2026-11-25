//! The two things fenix should hand back to the operating system.
//!
//! A file manager that can do everything eventually meets a `.xlsx`, and
//! the right answer there is Excel, not a hex dump. And however good the
//! listing gets, "show me this in Explorer" is the escape hatch that
//! makes trusting it easy -- being unable to leave is not the same as
//! not needing to.
//!
//! On macOS both go through `open` (`open -R` selects in Finder); on
//! Linux, `xdg-open`. On Windows both go through `explorer.exe`, which is the shell itself: opening a
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
    crate::remote::local_only(path, "Opening it with its own program")?;
    if !path.exists() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("{} is not there", path.display())));
    }
    #[cfg(windows)]
    {
        // Deliberately not waited on: this hands the file to another
        // application, which may sit open for hours. `spawn` and forget.
        no_window("explorer.exe").arg(path).spawn().map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        // `./`: a relative name that starts with `-` isn't an option.
        launch(Command::new("/usr/bin/open").arg(not_an_option(path)))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
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
    crate::remote::local_only(path, "Showing it in the system's file manager")?;
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
    #[cfg(target_os = "macos")]
    {
        launch(Command::new("/usr/bin/open").arg("-R").arg(not_an_option(path)))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        // No universal equivalent, so open the containing directory --
        // less than asked for, but more useful than an error.
        let target = path.parent().unwrap_or(path);
        Command::new("xdg-open").arg(target).spawn().map(|_| ())
    }
}

/// Opens a web or mail link in the system's browser or mail program.
///
/// Only `http:`, `https:` and `mailto:` -- a link comes from a document
/// that may not be trusted, and handing the shell a `file:` path or an
/// unknown scheme could run something.
pub fn open_url(url: &str) -> io::Result<()> {
    let lower = url.trim().to_ascii_lowercase();
    if !["http://", "https://", "mailto:"].iter().any(|scheme| lower.starts_with(scheme)) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{url} isn't a web or mail link")));
    }
    #[cfg(windows)]
    {
        no_window("explorer.exe").arg(url.trim()).spawn().map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        launch(Command::new("/usr/bin/open").arg(url.trim()))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Command::new("xdg-open").arg(url.trim()).spawn().map(|_| ())
    }
}

/// `path` as an argument `open` can't read as one of its options.
#[cfg(target_os = "macos")]
fn not_an_option(path: &Path) -> std::path::PathBuf {
    if path.is_absolute() { path.to_path_buf() } else { Path::new(".").join(path) }
}

/// Starts `command`, which hands over to another application and ends at
/// once, and waits for it on a thread of its own so it isn't left a zombie.
#[cfg(target_os = "macos")]
fn launch(command: &mut Command) -> io::Result<()> {
    let mut child = command.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
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
    fn only_web_and_mail_links_are_opened() {
        for url in ["file:///C:/Windows/System32/calc.exe", "C:/tools/x.exe", "javascript:alert(1)", "ms-settings:"] {
            assert_eq!(open_url(url).unwrap_err().kind(), io::ErrorKind::InvalidInput, "{url}");
        }
    }

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
