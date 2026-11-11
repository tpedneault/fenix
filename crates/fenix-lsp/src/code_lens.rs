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

/// The most chars of a title shown.
pub const MAX_TITLE: usize = 200;

/// A title as it's drawn on one row: Unicode's format characters (the
/// bidi controls, zero widths, the byte order mark) taken out, line
/// breaks, tabs and other control characters as spaces, runs of spaces
/// as one, none at either end, and no longer than `MAX_TITLE` chars (cut
/// with `…`).
pub fn clean_title(title: &str) -> String {
    let mut out = String::new();
    let title: String = title.chars().filter(|&c| !is_format(c)).collect();
    for word in title.split(|c: char| c.is_control() || c.is_whitespace()).filter(|w| !w.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    if out.chars().count() > MAX_TITLE {
        out = out.chars().take(MAX_TITLE - 1).collect::<String>().trim_end().to_string();
        out.push_str(UNRESOLVED);
    }
    out
}

/// Whether `c` is one of Unicode's format characters (general category
/// Cf): drawn as nothing, or turning the text around it.
fn is_format(c: char) -> bool {
    matches!(c as u32,
        0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x890..=0x891 | 0x8E2 | 0x180E | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F
        | 0xFEFF | 0xFFF9..=0xFFFB | 0x110BD | 0x110CD | 0x13430..=0x1343F | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F)
}

/// `lenses` (each with its tag, in the order they're to be shown: the
/// primary server's first) as rows, by the line each is on. A lens with
/// no command yet shows as `…`; one whose title is empty shows nothing,
/// but its line's row is there all the same.
pub fn rows<'a, T: Clone + 'a>(lenses: impl IntoIterator<Item = (T, &'a lsp_types::CodeLens)>) -> BTreeMap<usize, LensRow<T>> {
    let mut by_line: BTreeMap<usize, Vec<(T, String)>> = BTreeMap::new();
    for (tag, lens) in lenses {
        let title = match &lens.command {
            Some(command) => clean_title(&command.title),
            None => UNRESOLVED.to_string(),
        };
        // A lens with no title still has its row, so the text below
        // doesn't move as lenses resolve; the title just isn't there.
        let line = by_line.entry(lens.range.start.line as usize).or_default();
        if !title.is_empty() {
            line.push((tag, title));
        }
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
    // The lines before `start` and through each end, in one pass a text.
    let lines_before = |text: &str, ends: [usize; 2]| {
        let mut counts = [0usize; 2];
        let mut previous = None;
        for (i, c) in text.chars().enumerate().take(ends[1]) {
            if c == '\n' {
                counts[1] += 1;
                if i < ends[0] {
                    counts[0] += 1;
                }
            }
            previous = Some(c);
        }
        (counts, previous)
    };
    let ([start_line, old_end_line], before_old_end) = lines_before(old, [start, old_end]);
    let ([_, new_end_line], _) = lines_before(new, [start, new_end]);
    // The first line of `old` wholly after the change: the one its end
    // is on when that's a line's start, else the next.
    let at_line_start = old_end == 0 || before_old_end == Some('\n');
    let after_old = (old_end_line + usize::from(!at_line_start)).max(start_line);
    // The text after the change is the same in both: so are its lines,
    // moved by the lines the change added or took away.
    (start_line, after_old, after_old + new_end_line - old_end_line)
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
        assert_eq!(got.keys().copied().collect::<Vec<_>>(), [3, 7, 9], "a row for each line with a lens");
        assert_eq!(got[&9], LensRow { text: String::new(), spans: vec![] }, "an empty title shows nothing");
        assert_eq!(got[&3].text, "▶ Run | Debug");
        assert_eq!(got[&3].spans, vec![(0..5, 0), (8..13, 1)]);
        assert_eq!(got[&7].text, UNRESOLVED, "not resolved yet");
        assert_eq!(got[&3].at(9), Some(&1));
        assert_eq!(got[&3].at(6), None, "the separator runs nothing");
    }

    #[test]
    fn a_title_is_one_row_of_plain_text_and_no_longer_than_the_most_shown() {
        assert_eq!(clean_title("  Run\nDebug\r\n\tnow\u{7}x  "), "Run Debug now x");
        assert_eq!(clean_title("\u{feff}Ru\u{200b}n \u{202e}tset\u{2066}"), "Run tset", "format characters taken out");
        assert_eq!(clean_title("\u{200b}\u{202e}"), "", "nothing left");
        let long = clean_title(&"y".repeat(200_000));
        assert_eq!(long.chars().count(), MAX_TITLE);
        assert!(long.ends_with('…'));
        let a = lens(1, Some("Run\nDebug"), "r", None);
        assert_eq!(rows([(0, &a)])[&1].text, "Run Debug");
        let b = lens(2, Some("\n\t"), "r", None);
        assert!(rows([(0, &b)])[&2].text.is_empty(), "nothing left to show");
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
        assert_eq!(changed_lines("fn a() {}\nb\nc\n", "fn a() {\n}\nb\nc\n"), (0, 1, 2), "Enter inside line 0");
        assert_eq!(changed_lines("a\nb\nc\nd\n", "ab\nc\nd\n"), (0, 1, 0), "line 1 joined to line 0");
        assert_eq!(changed_lines("ab\nc\n", "ax\ny\nb\nc\n"), (0, 1, 3), "two lines pasted mid-line");
    }

    #[test]
    fn lenses_after_any_change_move_by_the_lines_it_added_and_before_it_stay() {
        // A small generator, the same every run.
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = |below: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % below as u64) as usize
        };
        let alphabet = ['a', 'b', '\n'];
        for _ in 0..4_000 {
            let old: String = (0..next(14)).map(|_| alphabet[next(3)]).collect();
            let chars: Vec<char> = old.chars().collect();
            let cut = next(chars.len() + 1);
            let cut_end = cut + next(chars.len() - cut + 1);
            let inserted: String = (0..next(5)).map(|_| alphabet[next(3)]).collect();
            let new = format!("{}{inserted}{}", chars[..cut].iter().collect::<String>(), chars[cut_end..].iter().collect::<String>());
            let added = new.matches('\n').count() as isize - old.matches('\n').count() as isize;
            // The change as it's sent (the text both share around it
            // isn't in it), which is what the server's lines follow.
            let (from, to, _) = crate::changed_span(&old, &new);
            // Where each line of `old` starts.
            let mut starts = vec![0];
            starts.extend(chars.iter().enumerate().filter(|(_, c)| **c == '\n').map(|(i, _)| i + 1));
            let mut lenses: Vec<CodeLens> = (0..starts.len() as u32).map(|l| lens(l, Some("x"), "", None)).collect();
            shift_lines(&mut lenses, changed_lines(&old, &new));
            for (line, &first) in starts.iter().enumerate() {
                let got = lenses[line].range.start.line as isize;
                // Its newline and all before it ahead of the change.
                if starts.get(line + 1).is_some_and(|&next_line| next_line <= from) {
                    assert_eq!(got, line as isize, "{old:?} -> {new:?}: line {line}, before the change");
                }
                // Starting where the change ends, or after.
                if first >= to {
                    assert_eq!(got, line as isize + added, "{old:?} -> {new:?}: line {line}, after the change");
                }
            }
        }
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
