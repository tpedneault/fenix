//! Turns one raw line of a running task's output into what the Task
//! Output panel should actually display, plus (when a source location
//! can be recovered from it) an entry for the quickfix list -- the same
//! generalized list `SPC p n`/`SPC p N` already step through for grep
//! matches and LSP references, so a broken build's errors are navigable
//! exactly the same way.

use serde::Deserialize;
use std::path::PathBuf;

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
        None => ParsedLine { display: line.to_string(), location: parse_generic_line(line) },
    }
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
/// failure's own location. No `regex` dependency: scans for the first
/// `:<digits>:<digits>:` (or `:<digits>:`) run, treating everything
/// before it as the path and everything after the final `:` as the
/// message. A leading Windows drive letter (`C:\...`) is walked past
/// first so its own colon is never mistaken for the line-number
/// separator. Doesn't handle MSVC's `path(line,col): message`
/// parenthesized convention -- a disclosed gap, not silently wrong: a
/// line in that shape just displays with no recovered location, the
/// same as any other line this can't make sense of.
fn parse_generic_line(line: &str) -> Option<TaskLocation> {
    let search_start = if line.as_bytes().get(1) == Some(&b':') && line.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) { 2 } else { 0 };
    let mut i = search_start;
    while let Some(rel) = line[i..].find(':') {
        let colon1 = i + rel;
        let after_colon1 = &line[colon1 + 1..];
        let Some((line_no, rest)) = take_leading_digits(after_colon1) else {
            i = colon1 + 1;
            continue;
        };
        if let Some(after_second_colon) = rest.strip_prefix(':') {
            if let Some((col_no, rest2)) = take_leading_digits(after_second_colon) {
                if let Some(message) = rest2.strip_prefix(':') {
                    return Some(TaskLocation { path: PathBuf::from(&line[..colon1]), line: line_no, col: col_no, message: message.trim().to_string() });
                }
            }
            // `path:line: message` -- no column.
            return Some(TaskLocation { path: PathBuf::from(&line[..colon1]), line: line_no, col: 1, message: after_second_colon.trim().to_string() });
        }
        i = colon1 + 1;
    }
    None
}

fn take_leading_digits(s: &str) -> Option<(usize, &str)> {
    let digit_count = s.chars().take_while(char::is_ascii_digit).count();
    if digit_count == 0 {
        return None;
    }
    let (digits, rest) = s.split_at(digit_count);
    digits.parse().ok().map(|n| (n, rest))
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
    fn parse_line_handles_a_compiler_message_with_no_spans() {
        let json = r#"{"reason":"compiler-message","message":{"message":"crate-level lint","rendered":null,"spans":[]}}"#;
        let parsed = parse_line(json);
        assert_eq!(parsed.display, "crate-level lint"); // falls back to `message` when `rendered` is absent
        assert_eq!(parsed.location, None);
    }
}
