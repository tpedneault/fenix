//! What `ssh` has to ask, asked by Fenix.
//!
//! `ssh` is started with no terminal. With `SSH_ASKPASS` set and
//! `SSH_ASKPASS_REQUIRE=force`, it runs that program for every question
//! (a key's passphrase, a password, a one-time code, a yes to a host it
//! hasn't seen) with the question as its argument, and reads the answer
//! from its output. The program is Fenix itself: [`answer_if_asked`],
//! first thing in `main`, sees the hook's variables and passes the
//! question to the Fenix that started the `ssh`.

use std::io::{self, BufRead, BufReader, Write};
use std::process::{Command, ExitCode};

use serde::{Deserialize, Serialize};

use crate::local::{self, Door};

const ADDR: &str = "FENIX_ASKPASS_ADDR";
const TOKEN: &str = "FENIX_ASKPASS_TOKEN";

#[derive(Serialize, Deserialize)]
struct Question {
    token: String,
    prompt: String,
}

#[derive(Serialize, Deserialize)]
struct Answer {
    /// `None`: the user gave none, and `ssh` gives up on this way in.
    answer: Option<String>,
}

/// Answers `ssh`'s questions with `ask` for as long as the process
/// lives.
pub struct Askpass {
    door: Door,
}

impl Askpass {
    /// `ask` gets each question as `ssh` words it, on a thread of its
    /// own, and gives the answer, or `None` to decline.
    pub fn start(ask: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> io::Result<Askpass> {
        let door = local::listen("fenix-askpass", move |stream, token| {
            let mut line = String::new();
            let Ok(mut reader) = stream.try_clone().map(BufReader::new) else { return };
            if reader.read_line(&mut line).is_err() {
                return;
            }
            let answer = match serde_json::from_str::<Question>(&line) {
                Ok(question) if question.token == token => ask(&question.prompt),
                _ => None,
            };
            let mut stream = stream;
            let _ = writeln!(stream, "{}", serde_json::to_string(&Answer { answer }).expect("an answer serializes"));
        })?;
        Ok(Askpass { door })
    }

    /// Has `ssh` ask through this hook, and never on a terminal.
    pub fn hook(&self, ssh: &mut Command) -> io::Result<()> {
        ssh.env("SSH_ASKPASS", std::env::current_exe()?)
            .env("SSH_ASKPASS_REQUIRE", "force")
            // Older OpenSSH only looks at SSH_ASKPASS when DISPLAY is set.
            .env("DISPLAY", std::env::var("DISPLAY").unwrap_or_else(|_| "fenix:0".into()))
            .env(ADDR, self.door.addr.to_string())
            .env(TOKEN, &self.door.token);
        Ok(())
    }
}

/// When this process is `ssh`'s askpass program, asks Fenix the question
/// and gives the exit code to end with. `None` for any other run.
pub fn answer_if_asked() -> Option<ExitCode> {
    let (addr, token) = (std::env::var(ADDR).ok()?, std::env::var(TOKEN).ok()?);
    let prompt = std::env::args().nth(1).unwrap_or_default();
    let asked = (|| -> io::Result<Option<String>> {
        let mut stream = local::knock(&addr)?;
        writeln!(stream, "{}", serde_json::to_string(&Question { token, prompt }).expect("a question serializes"))?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        Ok(serde_json::from_str::<Answer>(&line).map_err(io::Error::other)?.answer)
    })();
    Some(match asked {
        Ok(Some(answer)) => {
            println!("{answer}");
            ExitCode::SUCCESS
        }
        _ => ExitCode::FAILURE,
    })
}
