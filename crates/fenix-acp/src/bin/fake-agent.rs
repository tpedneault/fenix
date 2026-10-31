//! A mock ACP agent for tests, playing Claude Code's adapter
//! (claude-agent-acp 0.99.1) on stdio. Not part of a release.
//!
//! `fake-agent [--profile claude] [--store DIR] [--crash-on METHOD]
//! [--crash-after N] [--logged-out] [--slow MS] [--no-replay]
//! [--forget-sessions] [--unannounced-write] [--bypass] [--fs] [--terminal]
//! [--agent-auth]`: see `fenix_acp::mock` for what each does, and what a
//! prompt makes it do.

use std::io::BufReader;

use fenix_acp::mock::{run, MockOptions};

fn main() {
    let options = match MockOptions::from_args(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(why) => {
            eprintln!("fake-agent: {why}");
            std::process::exit(2);
        }
    };
    if let Err(why) = run(BufReader::new(std::io::stdin()), std::io::stdout(), options) {
        // A crash cue, or a broken pipe: exit at once, as a crash would.
        eprintln!("fake-agent: {why}");
        std::process::exit(3);
    }
}
