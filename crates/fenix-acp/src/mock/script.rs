//! What a prompt makes the mock do. The prompt's first text block is cut at
//! `;` into commands, each one a step of the turn, so a test can write
//! `read a.rs; edit a.rs foo=>bar; run cargo test` and get that turn:
//!
//! - `read FILE`: a Read tool call;
//! - `edit FILE OLD=>NEW`: an Edit replacing the first OLD with NEW;
//! - `write FILE CONTENT`: a Write creating (or replacing) FILE;
//! - `run CMD`: a Bash tool call running CMD in the session's cwd;
//!
//! and, anywhere in a command, the words:
//!
//! - `hello`: a thought, then an answer streamed in several chunks;
//! - `todo`: a plan update;
//! - `plan`: ExitPlanMode ("Approve Plan"), which switches the mode;
//! - `slow`: many chunks, slowly (for cancelling);
//! - `fail`: a tool call that fails;
//! - `refuse`: ends the turn with `refusal`;
//! - `long`: ends the turn with `max_tokens`.
//!
//! Anything else is answered "You said: <text>". In OLD, NEW and CONTENT,
//! `\n` stands for a line break.

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Step {
    Hello,
    Echo(String),
    Read(String),
    Edit {
        path: String,
        old: String,
        new: String,
    },
    Write {
        path: String,
        content: String,
    },
    Run(String),
    Todo,
    Plan,
    Slow,
    Fail,
    Refuse,
    Long,
}

pub(super) fn parse(text: &str) -> Vec<Step> {
    let steps: Vec<Step> = text
        .split(';')
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(command)
        .collect();
    if steps.is_empty() {
        return vec![Step::Echo(text.trim().to_string())];
    }
    steps
}

fn command(text: &str) -> Step {
    let (verb, rest) = match text.split_once(char::is_whitespace) {
        Some((verb, rest)) => (verb, rest.trim()),
        None => (text, ""),
    };
    let (first, after) = match rest.split_once(char::is_whitespace) {
        Some((first, after)) => (first, after.trim_start()),
        None => (rest, ""),
    };
    match verb {
        "read" if !rest.is_empty() => return Step::Read(rest.to_string()),
        "run" if !rest.is_empty() => return Step::Run(rest.to_string()),
        "write" if !first.is_empty() => {
            return Step::Write {
                path: first.to_string(),
                content: unescape(after),
            };
        }
        "edit" => {
            if let Some((old, new)) = after.split_once("=>") {
                return Step::Edit {
                    path: first.to_string(),
                    old: unescape(old.trim()),
                    new: unescape(new.trim()),
                };
            }
        }
        _ => {}
    }
    let words: Vec<String> = text
        .split_whitespace()
        .map(|word| {
            word.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .collect();
    let has = |keyword: &str| words.iter().any(|word| word == keyword);
    if has("hello") || has("hi") {
        Step::Hello
    } else if has("todo") {
        Step::Todo
    } else if has("plan") {
        Step::Plan
    } else if has("slow") {
        Step::Slow
    } else if has("fail") {
        Step::Fail
    } else if has("refuse") {
        Step::Refuse
    } else if has("long") {
        Step::Long
    } else {
        Step::Echo(text.to_string())
    }
}

fn unescape(text: &str) -> String {
    text.replace("\\n", "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_compose() {
        assert_eq!(
            parse("read a.rs; edit a.rs foo => bar baz; run cargo test -q; say hello!"),
            vec![
                Step::Read("a.rs".into()),
                Step::Edit {
                    path: "a.rs".into(),
                    old: "foo".into(),
                    new: "bar baz".into()
                },
                Step::Run("cargo test -q".into()),
                Step::Hello,
            ]
        );
        assert_eq!(
            parse("write n.txt a\\nb"),
            vec![Step::Write {
                path: "n.txt".into(),
                content: "a\nb".into()
            }]
        );
        assert_eq!(
            parse("what is this?"),
            vec![Step::Echo("what is this?".into())]
        );
        assert_eq!(parse(""), vec![Step::Echo(String::new())]);
        assert_eq!(parse("explain the plan."), vec![Step::Plan]);
        assert_eq!(parse("explain"), vec![Step::Echo("explain".into())]);
    }
}
