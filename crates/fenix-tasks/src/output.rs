//! Turns one raw line of a running task's output into what the Task
//! Output panel should actually display, plus (when a source location
//! can be recovered from it) an entry for the quickfix list -- the same
//! generalized list `SPC p n`/`SPC p N` already step through for grep
//! matches and LSP references, so a broken build's errors are navigable
//! exactly the same way.

use serde::Deserialize;
use std::path::PathBuf;

use crate::location;

/// A source location recovered from one output line, in the same
/// 1-indexed `(line, col)` convention `fenix_project::grep::GrepMatch`
/// already uses (this app's `App::jump_to_grep_match` converts either
/// straight to a 0-indexed char offset the same way) -- `message` is
/// this location's own diagnostic text, standing in for `GrepMatch`'s
/// `text` field when a caller wraps this into that exact struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLocation {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    pub message: String,
}

/// What to do with one raw line of output -- `display` is what the Task
/// Output buffer actually appends (verbatim for an ordinary line;
/// `cargo --message-format=json`'s own human-readable `rendered` block,
/// carets and all, for a decoded compiler message -- multi-line, so a
/// caller splits it on `\n` before inserting; empty for a recognized-
/// but-uninteresting cargo JSON line, which a caller skips inserting
/// entirely rather than leaving a blank line behind for every one),
/// and `location`, when present, is what the quickfix list gets fed.
pub struct ParsedLine {
    pub display: String,
    pub location: Option<TaskLocation>,
}

/// Parses one line of task output. Tries `cargo --message-format=json`
/// decoding first (every `cargo build`/`test`/`clippy` task this crate
/// defines asks for it specifically -- see `defs::default_tasks`'s own
/// doc comment for why cargo's *default* human output can't be parsed
/// line-by-line at all: a diagnostic's `--> file:line:col` sits on the
/// line *after* its message, not the same one). A line that decodes as
/// *some* recognized cargo JSON message but isn't a `compiler-message`
/// (`compiler-artifact` -- one per crate compiled, `build-finished`,
/// `build-script-executed`, ...) is suppressed outright (`display`
/// empty) rather than shown as raw JSON -- a real multi-crate build
/// would otherwise flood the panel with one unreadable JSON line per
/// dependency. Only a line that isn't cargo JSON *at all* (a task that
/// was never asked for it, or cargo's own plain-text progress lines --
/// `Compiling ...`/`Finished ...` -- which stay plain text even under
/// `--message-format=json`) falls through to `parse_generic_line`, so a
/// plain `gcc`/`clang`/`ctest`/`pytest` line's own `file:line:col:
/// message` convention is still recovered without needing its own
/// task-level flag to opt in.
pub fn parse_line(line: &str) -> ParsedLine {
    match parse_cargo_json_line(line) {
        Some(CargoOutcome::Diagnostic { display, location }) => ParsedLine { display, location },
        Some(CargoOutcome::Suppressed) => ParsedLine { display: String::new(), location: None },
        None => ParsedLine { display: line.to_string(), location: parse_panic_line(line).or_else(|| parse_arrow_line(line)).or_else(|| parse_generic_line(line)) },
    }
}

/// A location anywhere in `line`, for `Enter` on a line of a task's
/// output: what `parse_line` finds, else the first word that is a
/// `path:line[:col]` by itself (`at src/lib.rs:4:2`, `(tests/a.py:12)`).
pub fn find_location(line: &str) -> Option<TaskLocation> {
    if let Some(found) = parse_line(line).location {
        return Some(found);
    }
    line.split_whitespace().find_map(|word| {
        let word = word.trim_matches(|c: char| matches!(c, '(' | ')' | '[' | ']' | '<' | '>' | '\'' | '"' | '`' | ',' | ';'));
        let word = word.strip_suffix(':').unwrap_or(word);
        // A file's name, not a ratio or a time: it has an extension or a
        // directory.
        let found = location::split(word, |s| s.rest.is_empty() && s.path.contains(['.', '/', '\\']))?;
        Some(TaskLocation { path: PathBuf::from(found.path), line: found.line, col: found.col.unwrap_or(1), message: line.trim().to_string() })
    })
}

/// A Rust panic: `thread 'tests::amounts' panicked at src/lib.rs:89:9:`
/// (Rust 1.73 on, the message on the next line; from 1.88 with the
/// thread's id, `thread 'main' (16816) panicked at ...`), or the older
/// `thread 'main' panicked at 'boom', src/main.rs:2:5`. The generic
/// reading would take everything before `:89:9` for the file's name.
fn parse_panic_line(line: &str) -> Option<TaskLocation> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("thread '") {
        return None;
    }
    let (head, after) = trimmed.split_once(" panicked at ")?;
    let message = format!("{head} panicked");
    // The old form: the message, quoted, then `, path:line:col`.
    let place = if after.starts_with('\'') {
        let (said, place) = after.rsplit_once("', ")?;
        let found = location::split(place.trim_end(), |s| s.rest.is_empty())?;
        return Some(TaskLocation { path: PathBuf::from(found.path), line: found.line, col: found.col.unwrap_or(1), message: format!("{message}: {}", &said[1..]) });
    } else {
        after.trim_end()
    };
    let found = location::split(place, |s| s.rest.is_empty() || s.rest.starts_with(':'))?;
    let said = found.rest.strip_prefix(':').map(str::trim).filter(|r| !r.is_empty());
    Some(TaskLocation {
        path: PathBuf::from(found.path),
        line: found.line,
        col: found.col.unwrap_or(1),
        message: said.map(|said| format!("{message}: {said}")).unwrap_or(message),
    })
}

/// rustc's own (not JSON) diagnostics, as `cargo test` prints a build
/// that fails: `  --> src/main.rs:3:5` under the message.
fn parse_arrow_line(line: &str) -> Option<TaskLocation> {
    let place = line.trim_start().strip_prefix("--> ")?.trim_end();
    let found = location::split(place, |s| s.rest.is_empty())?;
    Some(TaskLocation { path: PathBuf::from(found.path), line: found.line, col: found.col.unwrap_or(1), message: String::new() })
}

enum CargoOutcome {
    Diagnostic { display: String, location: Option<TaskLocation> },
    Suppressed,
}

#[derive(Deserialize)]
struct CargoMessage {
    reason: String,
    message: Option<CargoDiagnostic>,
}

#[derive(Deserialize)]
struct CargoDiagnostic {
    message: String,
    rendered: Option<String>,
    spans: Vec<CargoSpan>,
}

#[derive(Deserialize)]
struct CargoSpan {
    file_name: String,
    line_start: usize,
    column_start: usize,
    is_primary: bool,
}

/// Decodes one `cargo --message-format=json` line -- `None` if it isn't
/// valid JSON, or doesn't have the `reason` field every real cargo
/// message carries (not this crate's JSON at all); `Some(Suppressed)`
/// for any *other* recognized reason than `compiler-message`; `Some(
/// Diagnostic { .. })` only for that one. `display` is the message's
/// own `rendered` field when present (the exact human-readable block
/// cargo would have printed without `--message-format=json`, ANSI codes
/// included) -- falls back to the plain `message` field on the rare
/// server/cargo version that omits `rendered`. `location` comes from
/// whichever span is `is_primary` (falling back to the first span if
/// none is marked, which shouldn't happen per cargo's own schema but
/// costs nothing to tolerate); `None` if there are no spans at all (a
/// message with no specific location, e.g. a crate-level lint).
fn parse_cargo_json_line(line: &str) -> Option<CargoOutcome> {
    let msg: CargoMessage = serde_json::from_str(line).ok()?;
    if msg.reason != "compiler-message" {
        return Some(CargoOutcome::Suppressed);
    }
    let diagnostic = msg.message?;
    let display = diagnostic.rendered.clone().unwrap_or_else(|| diagnostic.message.clone());
    let span = diagnostic.spans.iter().find(|s| s.is_primary).or_else(|| diagnostic.spans.first());
    let location = span.map(|s| TaskLocation {
        path: PathBuf::from(&s.file_name),
        line: s.line_start,
        col: s.column_start,
        message: diagnostic.message.clone(),
    });
    Some(CargoOutcome::Diagnostic { display, location })
}

/// Recovers a `path:line:col: message` (or `path:line: message`, column
/// omitted) location from one plain-text line -- the convention gcc/
/// clang/most Unix build tools, `pytest`, and `ctest` all use for a
/// failure's own location. No `regex` dependency: `location::split`
/// finds the first `:<digits>:<digits>` (or `:<digits>`) run that a `:`
/// follows, walking past a leading Windows drive letter so its colon is
/// never taken for the line's, and everything after that `:` is the
/// message. Doesn't handle MSVC's `path(line,col): message`
/// parenthesized convention -- a disclosed gap, not silently wrong: a
/// line in that shape just displays with no recovered location, the
/// same as any other line this can't make sense of.
fn parse_generic_line(line: &str) -> Option<TaskLocation> {
    // `path:12:7)` isn't `path:12` with a message `7)`: a line number
    // with a column after it is read with the column or not at all.
    let found = location::split(line, |s| s.rest.starts_with(':') && (s.col.is_some() || !s.rest[1..].starts_with(|c: char| c.is_ascii_digit())))?;
    Some(TaskLocation { path: PathBuf::from(found.path), line: found.line, col: found.col.unwrap_or(1), message: found.rest[1..].trim().to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_unix_style_path_line_col_message() {
        let loc = parse_generic_line("src/main.rs:5:12: mismatched types").unwrap();
        assert_eq!(loc, TaskLocation { path: PathBuf::from("src/main.rs"), line: 5, col: 12, message: "mismatched types".to_string() });
    }

    #[test]
    fn parses_a_line_with_no_column() {
        let loc = parse_generic_line("test_foo.py:42: AssertionError").unwrap();
        assert_eq!(loc, TaskLocation { path: PathBuf::from("test_foo.py"), line: 42, col: 1, message: "AssertionError".to_string() });
    }

    #[test]
    fn a_windows_drive_letter_colon_is_not_mistaken_for_the_separator() {
        let loc = parse_generic_line("C:\\src\\main.c:10:3: error: expected ';'").unwrap();
        assert_eq!(loc.path, PathBuf::from("C:\\src\\main.c"));
        assert_eq!(loc.line, 10);
        assert_eq!(loc.col, 3);
        assert_eq!(loc.message, "error: expected ';'");
    }

    #[test]
    fn a_line_with_no_recognizable_location_returns_none() {
        assert_eq!(parse_generic_line("Compiling fenix-tasks v0.0.1"), None);
        assert_eq!(parse_generic_line(""), None);
    }

    #[test]
    fn a_number_that_looks_like_a_location_but_has_no_trailing_colon_returns_none() {
        assert_eq!(parse_generic_line("ratio is 3:2 today"), None);
    }

    #[test]
    fn parse_line_leaves_cargos_own_plain_text_progress_lines_untouched() {
        // `Compiling .../Finished ...` are cargo's own plain-text
        // progress output -- printed as-is even under `--message-
        // format=json` (only compiler diagnostics/artifacts go through
        // the JSON channel), so these are neither valid JSON nor a
        // `path:line:col:` location and pass straight through.
        let parsed = parse_line("   Compiling fenix-tasks v0.0.1");
        assert_eq!(parsed.display, "   Compiling fenix-tasks v0.0.1");
        assert_eq!(parsed.location, None);
    }

    #[test]
    fn parse_line_falls_back_to_the_generic_parser_for_plain_text() {
        let parsed = parse_line("src/main.rs:5:12: mismatched types");
        assert_eq!(parsed.display, "src/main.rs:5:12: mismatched types");
        assert_eq!(parsed.location.unwrap().line, 5);
    }

    #[test]
    fn parse_line_decodes_a_cargo_compiler_message_and_uses_its_rendered_text() {
        let json = r#"{"reason":"compiler-message","message":{"message":"unused variable: `x`","rendered":"warning: unused variable\n --> src/main.rs:6:9\n","spans":[{"file_name":"src/main.rs","line_start":6,"column_start":9,"is_primary":true}]}}"#;
        let parsed = parse_line(json);
        assert!(parsed.display.starts_with("warning: unused variable"));
        let loc = parsed.location.unwrap();
        assert_eq!(loc.path, PathBuf::from("src/main.rs"));
        assert_eq!(loc.line, 6);
        assert_eq!(loc.col, 9);
        assert_eq!(loc.message, "unused variable: `x`");
    }

    #[test]
    fn parse_line_suppresses_a_non_compiler_message_cargo_json_reason() {
        // `compiler-artifact`/`build-finished`/... are real cargo JSON,
        // just not a diagnostic -- suppressed (empty display) rather
        // than shown as raw, unreadable JSON. A caller skips appending
        // an empty display entirely (see `App::append_task_output_line`).
        let json = r#"{"reason":"build-finished","success":true}"#;
        let parsed = parse_line(json);
        assert_eq!(parsed.display, "");
        assert_eq!(parsed.location, None);
    }

    #[test]
    fn a_rust_panic_is_read_for_its_file_not_the_thread_s_name() {
        let windows = parse_line("thread 'tests::amounts' (16816) panicked at src\\lib.rs:89:9:").location.unwrap();
        assert_eq!((windows.path, windows.line, windows.col), (PathBuf::from("src\\lib.rs"), 89, 9));
        assert_eq!(windows.message, "thread 'tests::amounts' (16816) panicked");
        let unix = parse_line("thread 'main' panicked at src/main.rs:2:5:").location.unwrap();
        assert_eq!((unix.path, unix.line, unix.col), (PathBuf::from("src/main.rs"), 2, 5));
        let said = parse_line("thread 'main' panicked at src/main.rs:2:5: attempt to divide by zero").location.unwrap();
        assert_eq!(said.message, "thread 'main' panicked: attempt to divide by zero");
        let old = parse_line("thread 'main' panicked at 'boom: here', src/main.rs:7:3").location.unwrap();
        assert_eq!((old.path, old.line, old.col), (PathBuf::from("src/main.rs"), 7, 3));
        assert_eq!(old.message, "thread 'main' panicked: boom: here");
        let drive = parse_line("thread 'main' panicked at C:\\ledger\\src\\lib.rs:3:1:").location.unwrap();
        assert_eq!(drive.path, PathBuf::from("C:\\ledger\\src\\lib.rs"));
    }

    #[test]
    fn rustc_s_arrow_line_is_a_location() {
        let found = parse_line("  --> src/main.rs:3:5").location.unwrap();
        assert_eq!((found.path, found.line, found.col), (PathBuf::from("src/main.rs"), 3, 5));
    }

    #[test]
    fn a_location_is_found_anywhere_on_a_line() {
        let found = find_location("    at Object.<anonymous> (tests/sum.test.js:12:7)").unwrap();
        assert_eq!((found.path, found.line, found.col), (PathBuf::from("tests/sum.test.js"), 12, 7));
        let found = find_location("FAILED tests/test_x.py::test_y - see tests/test_x.py:42").unwrap();
        assert_eq!((found.path, found.line), (PathBuf::from("tests/test_x.py"), 42));
        assert_eq!(find_location("test result: FAILED. 1 passed; 1 failed"), None);
        assert_eq!(find_location("ratio is 3:2 today"), None);
    }

    #[test]
    fn parse_line_handles_a_compiler_message_with_no_spans() {
        let json = r#"{"reason":"compiler-message","message":{"message":"crate-level lint","rendered":null,"spans":[]}}"#;
        let parsed = parse_line(json);
        assert_eq!(parsed.display, "crate-level lint"); // falls back to `message` when `rendered` is absent
        assert_eq!(parsed.location, None);
    }
}
