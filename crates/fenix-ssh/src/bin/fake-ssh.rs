//! An `ssh` for the tests: whatever host it's given, the "remote
//! machine" is this one, and the helper runs in this very process.
//!
//! It plays the two ends a real connection has: `ssh` and what the
//! remote shell does with the command Fenix sends. `-o FakeState=<folder>`
//! names a folder that stands for the remote machine's state:
//!
//! - `refuse` in it: no connection, with that file's text as the reason.
//! - `no-helper` in it: the machine has no helper yet. The run says so,
//!   takes one on stdin as the real script does, and removes the file.
//! - `runs` in it counts the runs.

use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let state = std::env::args().find_map(|arg| arg.strip_prefix("FakeState=").map(PathBuf::from));
    let file = |name: &str| state.as_ref().map(|state| state.join(name));
    if let Some(runs) = file("runs") {
        let so_far: u32 = std::fs::read_to_string(&runs).ok().and_then(|text| text.trim().parse().ok()).unwrap_or(0);
        let _ = std::fs::write(&runs, (so_far + 1).to_string());
    }
    if let Some(why) = file("refuse").and_then(|refuse| std::fs::read_to_string(refuse).ok()) {
        eprintln!("{}", why.trim());
        return ExitCode::from(255);
    }
    let mut out = io::stdout();
    // A login shell with something to say.
    let _ = writeln!(out, "Welcome to the fake machine.");
    if let Some(missing) = file("no-helper").filter(|missing| missing.exists()) {
        let _ = writeln!(out, "{}Linux x86_64", fenix_remote::MISSING).and_then(|()| out.flush());
        let mut stdin = io::stdin().lock();
        let mut want = String::new();
        let mut program = Vec::new();
        if stdin.read_line(&mut want).is_err() || stdin.read_to_end(&mut program).is_err() || program.is_empty() {
            return ExitCode::FAILURE;
        }
        // The real script counts what arrived; the test's helper is a
        // stand-in file, so its count is taken on trust and said back.
        let _ = writeln!(out, "FENIX-SUM {}", want.trim());
        let _ = std::fs::write(file("installed").expect("state"), &program);
        let _ = std::fs::remove_file(missing);
        return ExitCode::SUCCESS;
    }
    let hello = serde_json::to_string(&fenix_remote::server::hello()).expect("hello serializes");
    if writeln!(out, "\n{}{hello}", fenix_remote::BANNER).and_then(|()| out.flush()).is_err() {
        return ExitCode::FAILURE;
    }
    match fenix_remote::server::serve(io::stdin().lock(), out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}
