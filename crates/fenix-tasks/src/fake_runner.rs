//! `fake-test-runner`: a made-up test runner for the test explorer's
//! tests (plan `docs/plans/test-explorer.md` §14), as `fenix-lsp`'s
//! `fake-language-server` is a language server's. Built only with the
//! `fake-runner` feature: the binary `fake-test-runner`, and the app's
//! tests, which run it from their own harness (a dependency's binary
//! isn't built by `cargo test -p fenix-gui`).
//!
//! It runs a script, a directive a line, from:
//!
//! - each argument naming a file under its folder: that file's lines
//!   that start `#! ` (the provider guest's tests are in such files, so
//!   one file holds both its tests and what its runner does);
//! - an argument `file::name`: `file`'s, once;
//! - an argument `--mode`: every line of `fake-<mode>.txt` in its folder.
//!
//! Directives: `out TEXT` and `err TEXT` (a line on stdout, stderr),
//! `sleep MS`, `exit CODE`, `child` (starts a process that holds its
//! pipes and lives two minutes -- the cargo case -- and says `CHILD
//! <pid>`), `long BYTES TEXT` (one line: `TEXT` and `é`s to `BYTES`),
//! `lines N TEXT` (`N` lines, `TEXT <i>`), `invalid` (a line that isn't
//! UTF-8), `crash` (ends by `abort`: a signal on Unix, an NTSTATUS on
//! Windows), `wait` (two minutes: what a stop ends). It exits 0 at the
//! script's end.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Set in the environment of the process `child` starts.
pub const CHILD_ENV: &str = "FENIX_FAKE_RUNNER_CHILD";

/// How long `child`'s process and `wait` last: past any test's bound,
/// short enough that one left behind by a failed test goes on its own.
const LONG: Duration = Duration::from_secs(120);

/// The script `args` name, from the folder it runs in.
pub fn script(args: &[String]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut read: Vec<String> = Vec::new();
    let mut from_file = |file: &str, lines: &mut Vec<String>| {
        if read.iter().any(|r| r == file) || !Path::new(file).is_file() {
            return;
        }
        read.push(file.to_string());
        let text = std::fs::read_to_string(file).unwrap_or_default();
        lines.extend(text.lines().filter_map(|l| l.trim_start().strip_prefix("#! ")).map(str::to_string));
    };
    for arg in args {
        if let Some(mode) = arg.strip_prefix("--") {
            let text = std::fs::read_to_string(format!("fake-{mode}.txt")).unwrap_or_default();
            lines.extend(text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string));
        } else if let Some((file, _)) = arg.split_once("::") {
            from_file(file, &mut lines);
        } else {
            from_file(arg, &mut lines);
        }
    }
    lines
}

/// Runs as the fake runner, with its own arguments `args`; `again`
/// starts this same program again, for `child`. Never returns.
pub fn main(args: &[String], again: &dyn Fn() -> Command) -> ! {
    if std::env::var_os(CHILD_ENV).is_some() {
        std::thread::sleep(LONG);
        std::process::exit(0);
    }
    let mut out = std::io::stdout().lock();
    for directive in script(args) {
        let (word, rest) = directive.split_once(' ').unwrap_or((directive.as_str(), ""));
        match word {
            "out" => {
                let _ = writeln!(out, "{rest}");
            }
            "err" => {
                let _ = out.flush();
                eprintln!("{rest}");
            }
            "sleep" => {
                let _ = out.flush();
                std::thread::sleep(Duration::from_millis(rest.trim().parse().unwrap_or(0)));
            }
            "exit" => {
                let _ = out.flush();
                std::process::exit(rest.trim().parse().unwrap_or(0));
            }
            "child" => {
                let child = again().env(CHILD_ENV, "1").stdin(Stdio::null()).stdout(Stdio::inherit()).stderr(Stdio::inherit()).spawn();
                match child {
                    Ok(child) => {
                        let _ = writeln!(out, "CHILD {}", child.id());
                        // Not waited for: it holds the pipes after we go.
                        std::mem::forget(child);
                    }
                    Err(e) => {
                        let _ = writeln!(out, "no child: {e}");
                    }
                }
            }
            "long" => {
                let (bytes, text) = rest.split_once(' ').unwrap_or((rest, ""));
                let bytes: usize = bytes.parse().unwrap_or(0);
                let mut line = text.to_string();
                while line.len() < bytes {
                    line.push('é');
                }
                let _ = writeln!(out, "{line}");
            }
            "lines" => {
                let (n, text) = rest.split_once(' ').unwrap_or((rest, ""));
                let mut buffered = std::io::BufWriter::new(&mut out);
                for i in 0..n.parse::<u64>().unwrap_or(0) {
                    let _ = writeln!(buffered, "{text} {i}");
                }
                let _ = buffered.flush();
            }
            "invalid" => {
                let _ = out.write_all(b"OUT not \xff UTF-8\n");
            }
            "crash" => {
                let _ = out.flush();
                std::process::abort();
            }
            "wait" => {
                let _ = out.flush();
                std::thread::sleep(LONG);
            }
            _ => {
                let _ = writeln!(out, "fake-test-runner: no directive {word}");
            }
        }
        let _ = out.flush();
    }
    let _ = out.flush();
    std::process::exit(0)
}

/// Whether process `pid` is still running: a zombie no one has reaped
/// yet counts as gone.
pub fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: signal 0 only asks whether the process exists; nothing
        // is sent and no memory is touched.
        if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
            return false;
        }
        // In a container whose first process doesn't reap, a killed
        // grandchild stays a zombie: it's gone all the same.
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => stat.rsplit_once(')').is_none_or(|(_, rest)| !rest.trim_start().starts_with('Z')),
            Err(_) => !Path::new("/proc/self").exists(),
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
        // SAFETY: the handle is checked before use and closed once; a
        // process that's gone can't be opened, and is gone.
        unsafe {
            let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if handle.is_null() {
                return false;
            }
            let running = WaitForSingleObject(handle, 0) == WAIT_TIMEOUT;
            CloseHandle(handle);
            running
        }
    }
}

/// Waits up to `bound` for process `pid` to be gone: whether it went.
pub fn gone_within(pid: u32, bound: Duration) -> bool {
    let deadline = Instant::now() + bound;
    while alive(pid) {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}
