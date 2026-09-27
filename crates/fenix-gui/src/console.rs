//! `fenix --console`: somewhere to read what Fenix prints.
//!
//! On Windows Fenix is a GUI-subsystem program (see `main.rs`), so
//! starting it opens no console window, and whatever it writes to
//! stdout/stderr -- a language server's startup error, a ctags problem,
//! a panic -- goes nowhere. `--console` brings that back when it's
//! wanted: it attaches to the console of the terminal Fenix was started
//! from, or opens a console window of its own when there isn't one
//! (a shortcut, Explorer), and points stdout/stderr at it.
//!
//! Output a launch has already redirected (`fenix --console 2> log.txt`,
//! or `cargo run`, which hands its own handles down) is left alone.
//!
//! Elsewhere, a program started from a terminal already writes to it,
//! so the flag does nothing.

/// The flag, as typed.
pub const CONSOLE_FLAG: &str = "--console";

/// Whether this launch asked for a console.
pub fn requested(args: &[String]) -> bool {
    args.iter().any(|arg| arg == CONSOLE_FLAG)
}

/// Attaches or opens a console and sends stdout/stderr to it. Best
/// effort: with no console to be had, Fenix runs exactly as it would
/// without the flag.
#[cfg(windows)]
pub fn open() {
    use std::os::windows::io::IntoRawHandle;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        AllocConsole, AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
        STD_OUTPUT_HANDLE,
    };

    // SAFETY: plain Win32 calls with no pointer arguments.
    let attached = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) != 0 || AllocConsole() != 0 };
    if !attached {
        return;
    }
    // A GUI-subsystem process starts with no standard handles, and
    // attaching a console doesn't give it any: open the console's
    // output buffer ourselves. `std` looks the handle up on every
    // write, so `println!`/`eprintln!` use it from here on.
    let Ok(conout) = std::fs::OpenOptions::new().read(true).write(true).open("CONOUT$") else {
        return;
    };
    // Deliberately never closed: it's stdout and stderr for the rest of
    // the process's life.
    let conout = conout.into_raw_handle();
    for which in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: `conout` is a valid handle owned by nobody else, and
        // `which` is one of the standard handle ids.
        unsafe {
            let current = GetStdHandle(which);
            if current.is_null() || current == INVALID_HANDLE_VALUE {
                SetStdHandle(which, conout);
            }
        }
    }
}

#[cfg(not(windows))]
pub fn open() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_flag_asks_for_a_console() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(requested(&args(&["--console"])));
        assert!(requested(&args(&["notes.md", "--console"])));
        assert!(!requested(&args(&["notes.md"])));
        assert!(!requested(&args(&["--consoles", "console"])));
    }
}
