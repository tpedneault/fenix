//! What `ssh` has to ask, asked by Fenix.
//!
//! `ssh` is started with no terminal. With `SSH_ASKPASS` set and
//! `SSH_ASKPASS_REQUIRE=force`, it runs that program for every question
//! (a key's passphrase, a password, a one-time code, a yes to a host it
//! hasn't seen) with the question as its argument, and reads the answer
//! from its output. The program is Fenix itself: [`answer_if_asked`],
//! first thing in `main`, sees the hook's variables and passes the
//! question to the Fenix that started the `ssh`.
//!
//! `ssh` asks again on every run, and a connection is several runs over
//! its life: one to install the helper, one to start it, one for each
//! reconnect. An [`Askpass`] remembers what was answered, in memory
//! only, so the person is asked once.

use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, BufReader, Write};
use std::process::Command;
use std::sync::{Arc, Mutex};

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

#[derive(Default)]
struct Memory {
    /// Answers by question, as `ssh` worded it.
    answers: HashMap<String, String>,
    /// The questions answered from memory in the `ssh` run going on. One
    /// asked again in the same run was answered wrong.
    served: HashSet<String>,
}

/// Answers `ssh`'s questions for as long as it lives.
pub struct Askpass {
    door: Door,
    memory: Arc<Mutex<Memory>>,
}

impl Askpass {
    /// `ask` gets each question as `ssh` words it, on a thread of its
    /// own, and gives the answer, or `None` to decline. An answer is
    /// remembered and given again to the same question in a later run.
    pub fn start(ask: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> io::Result<Askpass> {
        let memory: Arc<Mutex<Memory>> = Arc::default();
        let remembering = memory.clone();
        let door = local::listen("fenix-askpass", move |stream, token| {
            let Ok((line, _)) = local::first_line(&stream) else { return };
            let answer = match serde_json::from_str::<Question>(&line) {
                Ok(question) if local::same_token(&question.token, token) => answer(&remembering, &question.prompt, &ask),
                // Not `ssh`'s hook: nothing is asked of anyone, and what's
                // remembered stays so.
                _ => None,
            };
            let mut stream = stream;
            let _ = writeln!(stream, "{}", serde_json::to_string(&Answer { answer }).expect("an answer serializes"));
        })?;
        Ok(Askpass { door, memory })
    }

    /// Has `ssh` ask through this hook, and never on a terminal. Called
    /// once for each run of `ssh`.
    pub fn hook(&self, ssh: &mut Command) -> io::Result<()> {
        self.memory.lock().unwrap().served.clear();
        ssh.env("SSH_ASKPASS", std::env::current_exe()?)
            .env("SSH_ASKPASS_REQUIRE", "force")
            // Older OpenSSH only looks at SSH_ASKPASS when DISPLAY is set.
            .env("DISPLAY", std::env::var("DISPLAY").unwrap_or_else(|_| "fenix:0".into()))
            .env(ADDR, self.door.addr.to_string())
            .env(TOKEN, &self.door.token);
        Ok(())
    }

    /// Forgets every answer: the next question is asked.
    pub fn forget(&self) {
        let mut memory = self.memory.lock().unwrap();
        for answer in memory.answers.values_mut() {
            scrub(answer);
        }
        *memory = Memory::default();
    }
}

impl Drop for Askpass {
    fn drop(&mut self) {
        self.forget();
    }
}

/// Overwrites a secret before its memory is given back. Best effort: the
/// copies `ssh` and the pipe made are theirs.
fn scrub(secret: &mut String) {
    let blank = "\0".repeat(secret.len());
    secret.clear();
    secret.push_str(&blank);
}

fn answer(memory: &Mutex<Memory>, prompt: &str, ask: &impl Fn(&str) -> Option<String>) -> Option<String> {
    let key = prompt.trim().to_string();
    // A yes to an unknown host is asked of a person every time: once
    // given, `ssh` writes the host down and doesn't ask again.
    let remember = !confirms_a_host(&key);
    if remember {
        let mut memory = memory.lock().unwrap();
        if memory.served.contains(&key) {
            // Asked twice in one run: what's remembered was refused.
            memory.answers.remove(&key);
        } else if let Some(known) = memory.answers.get(&key).cloned() {
            memory.served.insert(key);
            return Some(known);
        }
    }
    let given = ask(prompt)?;
    if remember {
        let mut memory = memory.lock().unwrap();
        memory.served.insert(key.clone());
        memory.answers.insert(key, given.clone());
    }
    Some(given)
}

fn confirms_a_host(prompt: &str) -> bool {
    prompt.contains("(yes/no")
}

/// When this process is `ssh`'s askpass program, asks Fenix the question
/// and gives the exit code to end with. `None` for any other run.
pub fn answer_if_asked() -> Option<i32> {
    let (addr, token) = (std::env::var(ADDR).ok()?, std::env::var(TOKEN).ok()?);
    // `ssh` gives the question and nothing else. Anything other is a
    // Fenix that only inherited the variables.
    let mut args = std::env::args().skip(1);
    let (Some(prompt), None) = (args.next(), args.next()) else { return None };
    if prompt.starts_with("--") {
        return None;
    }
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
            0
        }
        _ => 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn an_answer_is_remembered_until_it_is_refused() {
        let memory = Mutex::new(Memory::default());
        let asked = AtomicUsize::new(0);
        let ask = |_: &str| Some(format!("answer {}", asked.fetch_add(1, Ordering::SeqCst)));
        let password = "tom@devbox's password: ";

        assert_eq!(answer(&memory, password, &ask).as_deref(), Some("answer 0"));
        // The next run of ssh asks the same: answered from memory.
        memory.lock().unwrap().served.clear();
        assert_eq!(answer(&memory, password, &ask).as_deref(), Some("answer 0"));
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        // Asked again in the same run, it was wrong: the person is asked.
        assert_eq!(answer(&memory, password, &ask).as_deref(), Some("answer 1"));
        memory.lock().unwrap().served.clear();
        assert_eq!(answer(&memory, password, &ask).as_deref(), Some("answer 1"));
        assert_eq!(asked.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_new_host_is_always_put_to_a_person_and_no_answer_is_no_answer() {
        let memory = Mutex::new(Memory::default());
        let asked = AtomicUsize::new(0);
        let ask = |_: &str| {
            asked.fetch_add(1, Ordering::SeqCst);
            Some("yes".to_string())
        };
        let host = "Are you sure you want to continue connecting (yes/no/[fingerprint])?";
        answer(&memory, host, &ask);
        memory.lock().unwrap().served.clear();
        answer(&memory, host, &ask);
        assert_eq!(asked.load(Ordering::SeqCst), 2);
        assert_eq!(answer(&memory, "Passphrase:", &|_| None), None);
        assert!(memory.lock().unwrap().answers.is_empty());
    }
}
