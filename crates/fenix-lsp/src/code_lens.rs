//! Code lens (`textDocument/codeLens`): what a server's lenses say, in
//! the terms Fenix draws and runs them by. Pure: no session, no drawing.
//!
//! A line's lenses become one row of titles joined by ` | `
//! ([`rows`]); the lenses move by line with each change sent
//! ([`shift_lines`]); and what running one means is worked out from its
//! command ([`action`]): a command the server runs itself, or one of
//! the few that clients are expected to run, which Fenix maps to its
//! own actions.

use std::collections::BTreeMap;
use std::ops::Range;

/// Whether the server offers lenses, and whether it resolves them
/// (`codeLens/resolve`) once they're asked for.
pub fn provider(capabilities: &lsp_types::ServerCapabilities) -> Option<Provider> {
    let options = capabilities.code_lens_provider.as_ref()?;
    Some(Provider { resolve: options.resolve_provider.unwrap_or(false) })
}

/// What a server offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provider {
    pub resolve: bool,
}

/// Whether the server runs `command` itself (`workspace/executeCommand`).
pub fn executes(capabilities: &lsp_types::ServerCapabilities, command: &str) -> bool {
    capabilities.execute_command_provider.as_ref().is_some_and(|p| p.commands.iter().any(|c| c == command))
}

/// What Fenix advertises: lenses asked for, no dynamic registration.
pub fn client_capabilities() -> lsp_types::CodeLensClientCapabilities {
    lsp_types::CodeLensClientCapabilities { dynamic_registration: Some(false) }
}

/// What Fenix advertises for the workspace: it asks again when told to.
pub fn workspace_capabilities() -> lsp_types::CodeLensWorkspaceClientCapabilities {
    lsp_types::CodeLensWorkspaceClientCapabilities { refresh_support: Some(true) }
}

/// The title drawn for a lens not resolved yet.
pub const UNRESOLVED: &str = "…";

/// Between two lenses on one row.
pub const SEPARATOR: &str = " | ";

/// One line's lenses as a row: its text, and each lens's columns in it
/// (chars) with what it is (`T`, the caller's: which server's, which
/// lens).
#[derive(Debug, Clone, PartialEq)]
pub struct LensRow<T> {
    pub text: String,
    pub spans: Vec<(Range<usize>, T)>,
}

impl<T: Clone> LensRow<T> {
    /// The row cut to `width` chars: whole titles while they fit, then
    /// `…` when some don't. A title cut off isn't clickable.
    pub fn fit(&self, width: usize) -> LensRow<T> {
        if self.text.chars().count() <= width {
            return self.clone();
        }
        // Room after a kept title for a space and the ellipsis.
        let tail = 1 + UNRESOLVED.chars().count();
        let mut spans = Vec::new();
        for (cols, tag) in &self.spans {
            if cols.end + tail > width {
                break;
            }
            spans.push((cols.clone(), tag.clone()));
        }
        let kept = spans.last().map_or(0, |(c, _)| c.end);
        let mut text: String = self.text.chars().take(kept).collect();
        if width > kept {
            if kept > 0 {
                text.push(' ');
            }
            text.push_str(UNRESOLVED);
        }
        LensRow { text: text.chars().take(width).collect(), spans }
    }

    /// The lens under column `col`, if any.
    pub fn at(&self, col: usize) -> Option<&T> {
        self.spans.iter().find(|(c, _)| c.contains(&col)).map(|(_, t)| t)
    }
}

/// `lenses` (each with its tag, in the order they're to be shown: the
/// primary server's first) as rows, by the line each is on. A lens with
/// no command yet shows as `…`; one whose title is empty isn't shown.
pub fn rows<'a, T: Clone + 'a>(lenses: impl IntoIterator<Item = (T, &'a lsp_types::CodeLens)>) -> BTreeMap<usize, LensRow<T>> {
    let mut by_line: BTreeMap<usize, Vec<(T, String)>> = BTreeMap::new();
    for (tag, lens) in lenses {
        let title = match &lens.command {
            Some(command) => command.title.trim().to_string(),
            None => UNRESOLVED.to_string(),
        };
        if title.is_empty() {
            continue;
        }
        by_line.entry(lens.range.start.line as usize).or_default().push((tag, title));
    }
    by_line
        .into_iter()
        .map(|(line, lenses)| {
            let mut text = String::new();
            let mut spans = Vec::new();
            for (tag, title) in lenses {
                if !text.is_empty() {
                    text.push_str(SEPARATOR);
                }
                let start = text.chars().count();
                text.push_str(&title);
                spans.push((start..text.chars().count(), tag));
            }
            (line, LensRow { text, spans })
        })
        .collect()
}

/// The lines one change touched, from `changed_span`, so it's the change
/// that was sent: `(start, after_old, after_new)` -- lines before
/// `start` are untouched, `old`'s lines from `after_old` on are `new`'s
/// from `after_new` on, and those between were replaced. A line the
/// change ends right at the start of is after it, not in it: text typed
/// at a line's start moves that line down.
pub fn changed_lines(old: &str, new: &str) -> (usize, usize, usize) {
    let (start, old_end, new_end) = crate::changed_span(old, new);
    let line_of = |text: &str, char: usize| text.chars().take(char).filter(|&c| c == '\n').count();
    // The first line wholly after `char`: its own when it's at a line's
    // start, else the next.
    let after = |text: &str, char: usize| {
        let at_start = char == 0 || text.chars().nth(char - 1) == Some('\n');
        line_of(text, char) + usize::from(!at_start)
    };
    (line_of(old, start), after(old, old_end).max(line_of(old, start)), after(new, new_end).max(line_of(new, start)))
}

/// `lenses` moved by line through a change (`changed_lines`): those
/// above stay, those after move by the difference, those on a replaced
/// line stay on it (or on the change's last line if it shrank past
/// them) until the server answers again.
pub fn shift_lines(lenses: &mut [lsp_types::CodeLens], (start, after_old, after_new): (usize, usize, usize)) {
    for lens in lenses {
        let line = lens.range.start.line as usize;
        let moved = if line < start {
            line
        } else if line >= after_old {
            line - after_old + after_new
        } else {
            line.min(after_new.saturating_sub(1).max(start))
        };
        let span = lens.range.end.line.saturating_sub(lens.range.start.line);
        lens.range.start.line = moved as u32;
        lens.range.end.line = moved as u32 + span;
    }
}

/// What running a lens does.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Sent back to the server: `workspace/executeCommand`.
    Server { command: String, arguments: Vec<serde_json::Value> },
    /// The references list, as `gr` shows it.
    ShowReferences { locations: Vec<lsp_types::Location> },
    /// A cargo runnable (rust-analyzer's `runSingle`): run in `cwd`.
    CargoRun(CargoRunnable),
    /// The same, under the debugger (`debugSingle`).
    CargoDebug(CargoRunnable),
    /// One of Fenix's own lenses: run the test at this line (1-based) of
    /// this file.
    RunTest { path: String, line: usize },
    /// Not one Fenix can run.
    Unknown { title: String, command: String },
    /// Not resolved yet: nothing to run.
    Unresolved,
}

/// What a cargo runnable runs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CargoRunnable {
    pub label: String,
    pub cargo_args: Vec<String>,
    pub executable_args: Vec<String>,
    pub cwd: Option<String>,
}

/// Fenix's own command: running the test at a line.
pub const RUN_TEST: &str = "fenix.runTest";

/// What running `lens` does, for a server that runs `executes` itself.
pub fn action(lens: &lsp_types::CodeLens, executes: impl Fn(&str) -> bool) -> Action {
    let Some(command) = &lens.command else { return Action::Unresolved };
    let arguments = command.arguments.clone().unwrap_or_default();
    match command.command.as_str() {
        RUN_TEST => {
            let path = arguments.first().and_then(|v| v.as_str()).map(str::to_string);
            let line = arguments.get(1).and_then(|v| v.as_u64()).map(|l| l as usize);
            match (path, line) {
                (Some(path), Some(line)) => Action::RunTest { path, line },
                _ => unknown(command),
            }
        }
        // The server runs it: whatever it is, it knows best.
        c if executes(c) => Action::Server { command: c.to_string(), arguments },
        "editor.action.showReferences" | "rust-analyzer.showReferences" => {
            // (uri, position, locations)
            match arguments.get(2).cloned().map(serde_json::from_value::<Vec<lsp_types::Location>>) {
                Some(Ok(locations)) => Action::ShowReferences { locations },
                _ => unknown(command),
            }
        }
        "rust-analyzer.runSingle" | "rust-analyzer.debugSingle" => match arguments.first().and_then(cargo_runnable) {
            Some(runnable) if command.command.ends_with("debugSingle") => Action::CargoDebug(runnable),
            Some(runnable) => Action::CargoRun(runnable),
            None => unknown(command),
        },
        _ => unknown(command),
    }
}

fn unknown(command: &lsp_types::Command) -> Action {
    Action::Unknown { title: command.title.clone(), command: command.command.clone() }
}

/// A rust-analyzer runnable, if it's cargo's: `{ label, kind: "cargo",
/// args: { cargoArgs, executableArgs, cwd | workspaceRoot } }`.
fn cargo_runnable(value: &serde_json::Value) -> Option<CargoRunnable> {
    if value.get("kind").and_then(|k| k.as_str()) != Some("cargo") {
        return None;
    }
    let args = value.get("args")?;
    let strings = |key: &str| -> Vec<String> { args.get(key).and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default() };
    let cargo_args = strings("cargoArgs");
    if cargo_args.is_empty() {
        return None;
    }
    Some(CargoRunnable {
        label: value.get("label").and_then(|l| l.as_str()).unwrap_or_default().to_string(),
        cargo_args,
        executable_args: strings("executableArgs"),
        cwd: args.get("cwd").or_else(|| args.get("workspaceRoot")).and_then(|c| c.as_str()).map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{CodeLens, Command, Position, Range as LspRange};

    fn lens(line: u32, title: Option<&str>, command: &str, arguments: Option<Vec<serde_json::Value>>) -> CodeLens {
        CodeLens {
            range: LspRange { start: Position { line, character: 0 }, end: Position { line, character: 3 } },
            command: title.map(|t| Command { title: t.into(), command: command.into(), arguments }),
            data: None,
        }
    }

    fn caps(json: serde_json::Value) -> lsp_types::ServerCapabilities {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn a_provider_and_what_the_server_executes_are_read() {
        let c = caps(serde_json::json!({"codeLensProvider": {"resolveProvider": true}, "executeCommandProvider": {"commands": ["x.run"]}}));
        assert_eq!(provider(&c), Some(Provider { resolve: true }));
        assert!(executes(&c, "x.run") && !executes(&c, "y"));
        assert_eq!(provider(&caps(serde_json::json!({}))), None);
        assert_eq!(provider(&caps(serde_json::json!({"codeLensProvider": {}}))), Some(Provider { resolve: false }));
    }

    #[test]
    fn a_line_s_lenses_are_one_row_of_titles_in_order() {
        let a = lens(3, Some("▶ Run"), "r", None);
        let b = lens(3, Some("Debug"), "d", None);
        let c = lens(7, None, "", None);
        let d = lens(9, Some("  "), "e", None);
        let got = rows([(0, &a), (1, &b), (2, &c), (3, &d)]);
        assert_eq!(got.keys().copied().collect::<Vec<_>>(), [3, 7], "an empty title isn't shown");
        assert_eq!(got[&3].text, "▶ Run | Debug");
        assert_eq!(got[&3].spans, vec![(0..5, 0), (8..13, 1)]);
        assert_eq!(got[&7].text, UNRESOLVED, "not resolved yet");
        assert_eq!(got[&3].at(9), Some(&1));
        assert_eq!(got[&3].at(6), None, "the separator runs nothing");
    }

    #[test]
    fn a_row_too_wide_keeps_whole_titles_then_an_ellipsis() {
        let a = lens(0, Some("12 references"), "r", None);
        let b = lens(0, Some("3 implementations"), "i", None);
        let row = &rows([(0, &a), (1, &b)])[&0];
        let fit = row.fit(20);
        assert_eq!(fit.text, "12 references …");
        assert_eq!(fit.spans, vec![(0..13, 0)]);
        assert_eq!(row.fit(40), *row, "room for all of it");
        assert!(row.fit(5).spans.is_empty());
        assert!(row.fit(5).text.chars().count() <= 5);
    }

    #[test]
    fn changed_lines_are_the_change_sent() {
        assert_eq!(changed_lines("a\nb\nc\n", "a\nB\nc\n"), (1, 2, 2), "line 1 replaced");
        assert_eq!(changed_lines("a\nb\nc\n", "a\nb\nx\ny\nc\n"), (2, 2, 4), "two lines inserted before line 2");
        assert_eq!(changed_lines("a\nb\nc\nd\n", "a\nd\n"), (1, 3, 1), "lines 1 and 2 taken away");
        assert_eq!(changed_lines("fn a\n", "// x\nfn a\n"), (0, 0, 1), "a line typed above line 0 moves it down");
    }

    fn lines_of(lenses: &[CodeLens]) -> Vec<u32> {
        lenses.iter().map(|l| l.range.start.line).collect()
    }

    #[test]
    fn lenses_move_with_lines_added_and_taken_away_above_them() {
        let mut lenses = vec![lens(0, Some("a"), "", None), lens(5, Some("b"), "", None), lens(9, Some("c"), "", None)];
        shift_lines(&mut lenses, changed_lines("x\n", "// new\nx\n"));
        assert_eq!(lines_of(&lenses), [1, 6, 10], "a line above all of them");
        shift_lines(&mut lenses, (2, 2, 4)); // two lines inserted before line 2
        assert_eq!(lines_of(&lenses), [1, 8, 12]);
        shift_lines(&mut lenses, (7, 10, 7)); // lines 7..=9 taken away: the lens on 8 stays on line 7
        assert_eq!(lines_of(&lenses), [1, 7, 9]);
        assert_eq!(lenses[2].range.end.line, 9);
        // Typing inside a lens's line keeps it there.
        shift_lines(&mut lenses, changed_lines("a\nfn b\n", "a\nfn bc\n"));
        assert_eq!(lines_of(&lenses), [1, 7, 9]);
    }

    #[test]
    fn a_command_the_server_runs_goes_back_to_it() {
        let l = lens(0, Some("Run"), "x.run", Some(vec![serde_json::json!(1)]));
        assert_eq!(action(&l, |c| c == "x.run"), Action::Server { command: "x.run".into(), arguments: vec![serde_json::json!(1)] });
        assert_eq!(action(&l, |_| false), Action::Unknown { title: "Run".into(), command: "x.run".into() });
        assert_eq!(action(&lens(0, None, "", None), |_| true), Action::Unresolved);
    }

    #[test]
    fn show_references_opens_the_list() {
        let location = serde_json::json!({"uri": "file:///a.rs", "range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 3}}});
        let args = vec![serde_json::json!("file:///a.rs"), serde_json::json!({"line": 0, "character": 0}), serde_json::json!([location])];
        match action(&lens(0, Some("1 reference"), "rust-analyzer.showReferences", Some(args)), |_| false) {
            Action::ShowReferences { locations } => assert_eq!(locations.len(), 1),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rust_analyzer_s_runnables_run_and_debug_with_cargo() {
        let runnable = serde_json::json!({"label": "test it", "kind": "cargo", "args": {"cargoArgs": ["test", "--package", "a", "--lib"], "executableArgs": ["it", "--exact"], "workspaceRoot": "/w"}});
        let want = CargoRunnable { label: "test it".into(), cargo_args: vec!["test".into(), "--package".into(), "a".into(), "--lib".into()], executable_args: vec!["it".into(), "--exact".into()], cwd: Some("/w".into()) };
        assert_eq!(action(&lens(0, Some("▶ Run"), "rust-analyzer.runSingle", Some(vec![runnable.clone()])), |_| false), Action::CargoRun(want.clone()));
        assert_eq!(action(&lens(0, Some("Debug"), "rust-analyzer.debugSingle", Some(vec![runnable])), |_| false), Action::CargoDebug(want));
        let shell = serde_json::json!({"label": "x", "kind": "shell", "args": {}});
        assert!(matches!(action(&lens(0, Some("x"), "rust-analyzer.runSingle", Some(vec![shell])), |_| false), Action::Unknown { .. }));
    }

    #[test]
    fn fenix_s_own_lens_runs_a_test_at_its_line() {
        let l = lens(4, Some("▶ Run test"), RUN_TEST, Some(vec![serde_json::json!("/p/a.rs"), serde_json::json!(5)]));
        assert_eq!(action(&l, |_| false), Action::RunTest { path: "/p/a.rs".into(), line: 5 });
    }
}
