//! Fenix's helper on a remote machine. Fenix starts it over SSH as
//! `fenix-remote --stdio` and talks to it on its stdin and stdout; it
//! ends when that connection does.

use std::io::{self, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("--stdio") => {
            let hello = serde_json::to_string(&fenix_remote::server::hello()).expect("hello serializes");
            let mut out = io::stdout();
            // A line of its own, whatever the shell printed before it.
            if write!(out, "\n{}{hello}\n", fenix_remote::BANNER).and_then(|()| out.flush()).is_err() {
                return ExitCode::FAILURE;
            }
            match fenix_remote::server::serve(io::stdin().lock(), out) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("fenix-remote: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("--version") => {
            println!("fenix-remote {} (protocol {})", env!("CARGO_PKG_VERSION"), fenix_remote::PROTOCOL);
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("fenix-remote is started by Fenix over SSH; it has nothing to do on its own.");
            ExitCode::from(2)
        }
    }
}
