//! What a breakpoint can be beyond a line (the debugger overhaul's fourth
//! phase): a condition, a hit count, a message logged in place of a stop,
//! and off without being taken away. And beside the breakpoints: the
//! exceptions an adapter can stop on, the values pinned in the code, and
//! the launch each project last used. The breakpoints, pins, exception
//! choices and launches are kept in the session file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::debug_drawer::{lens_expression, LinePurpose};
use super::*;

/// What a breakpoint is beyond its line. The default is a plain one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Breakpoint {
    /// It stops only when this is true (`x > 3`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    /// It stops only once hit this often, as the adapter reads it
    /// (`>= 3`, `% 2`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hits: Option<String>,
    /// A logpoint: this is written to the console, `{x}` its value, and
    /// it doesn't stop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
    /// Off: kept, not sent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub off: bool,
}

/// How a breakpoint looks in the gutter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BreakpointLook {
    Plain,
    /// A condition or a hit count: a square with a hole.
    Conditional,
    /// A logpoint: two short bars, lines of a message.
    Log,
    /// Off: dim.
    Off,
}

impl Breakpoint {
    pub fn look(&self) -> BreakpointLook {
        if self.off {
            BreakpointLook::Off
        } else if self.log.is_some() {
            BreakpointLook::Log
        } else if self.condition.is_some() || self.hits.is_some() {
            BreakpointLook::Conditional
        } else {
            BreakpointLook::Plain
        }
    }

    /// What it is past its line, for the drawer: `if x > 3  hits >= 2`.
    pub fn describe(&self) -> String {
        let mut out = Vec::new();
        if let Some(c) = &self.condition {
            out.push(format!("if {c}"));
        }
        if let Some(h) = &self.hits {
            out.push(format!("hits {h}"));
        }
        if let Some(l) = &self.log {
            out.push(format!("log \"{l}\""));
        }
        if self.off {
            out.push("off".to_string());
        }
        out.join("  ")
    }

    /// Its glyph in the drawer.
    pub fn glyph(&self) -> &'static str {
        match self.look() {
            BreakpointLook::Plain => "●",
            BreakpointLook::Conditional => "◉",
            BreakpointLook::Log => "◆",
            BreakpointLook::Off => "○",
        }
    }
}

/// The `setBreakpoints` list for a file's breakpoints: those on, with
/// what they are; `run_to` a plain one besides, unless there's one there.
pub(super) fn source_breakpoints(breakpoints: &BTreeMap<usize, Breakpoint>, run_to: Option<usize>) -> Vec<fenix_dap::types::SourceBreakpoint> {
    let mut on: BTreeMap<usize, Breakpoint> = breakpoints.iter().filter(|(_, b)| !b.off).map(|(l, b)| (*l, b.clone())).collect();
    if let Some(line) = run_to {
        on.entry(line).or_default();
    }
    on.into_iter()
        .map(|(line, b)| fenix_dap::types::SourceBreakpoint::builder().line(line as i32).condition(b.condition).hit_condition(b.hits).log_message(b.log).build())
        .collect()
}

/// A value pinned in the code: an expression, written at the end of its
/// line whenever the program stops.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct Pin {
    pub path: PathBuf,
    /// 1-based.
    pub line: usize,
    pub expression: String,
}

/// An exception an adapter can stop on (`exceptionBreakpointFilters`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExceptionFilter {
    pub filter: String,
    pub label: String,
    /// On unless you've said otherwise.
    pub default: bool,
}

/// The debugger's part of the session file.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct SavedDebugging {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub breakpoints: Vec<SavedBreakpoint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pins: Vec<Pin>,
    /// Each project's last launch, by the launch's name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub launches: Vec<(PathBuf, String)>,
    /// The exceptions you've turned on or off, by filter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exceptions: Vec<(String, bool)>,
}

impl SavedDebugging {
    pub fn is_empty(&self) -> bool {
        *self == SavedDebugging::default()
    }

    /// What's wrong with it, for a session file that can't be trusted.
    pub fn validate(&self) -> Result<(), String> {
        if self.breakpoints.len() > 100_000 || self.pins.len() > 10_000 {
            return Err("too many breakpoints or pins".into());
        }
        let paths = self.breakpoints.iter().map(|b| (&b.path, b.line)).chain(self.pins.iter().map(|p| (&p.path, p.line)));
        for (path, line) in paths {
            if !path.is_absolute() || line == 0 {
                return Err("a breakpoint or pin must be at a line of an absolute path".into());
            }
        }
        Ok(())
    }
}

/// A breakpoint as the session file has it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct SavedBreakpoint {
    pub path: PathBuf,
    pub line: usize,
    #[serde(flatten)]
    pub breakpoint: Breakpoint,
}

impl App {
    /// The breakpoint on the cursor's line in the file you're in, made
    /// when there's none: its file and line. None for a buffer with no
    /// file.
    fn breakpoint_here(&mut self) -> Option<(PathBuf, usize)> {
        let Some(path) = self.open().buffer.path().map(Path::to_path_buf) else {
            self.set_error("buffer has no file to set a breakpoint in".to_string());
            return None;
        };
        let (line, _) = self.open().buffer.line_col(&self.cursor());
        let canonical = fenix_lsp::normalize(std::fs::canonicalize(&path).unwrap_or(path));
        Some((canonical, line + 1))
    }

    /// `SPC u B`, `SPC u h`, `SPC u l`: the drawer's line, to give the
    /// cursor's line's breakpoint a condition, a hit count or a message.
    pub(crate) fn debug_edit_breakpoint(&mut self, what: BreakpointPart) {
        let Some((path, line)) = self.breakpoint_here() else { return };
        self.debug_edit_breakpoint_at(path, line, what);
    }

    /// The drawer's line, to give the breakpoint at `line` of `path` a
    /// condition, a hit count or a message: what it has now, to change.
    pub(super) fn debug_edit_breakpoint_at(&mut self, path: PathBuf, line: usize, what: BreakpointPart) {
        let now = self.breakpoints.get(&path).and_then(|l| l.get(&line)).cloned().unwrap_or_default();
        let text = match what {
            BreakpointPart::Condition => now.condition,
            BreakpointPart::Hits => now.hits,
            BreakpointPart::Log => now.log,
        };
        self.debug_dock_open = true;
        self.debug_drawer.focused = true;
        self.debug_drawer.line = Some(debug_drawer::EvalLine { text: text.unwrap_or_default(), purpose: LinePurpose::Breakpoint(path, line, what) });
    }

    /// What the drawer's line gave a breakpoint: set, or taken away when
    /// it's empty. The breakpoint's made if it wasn't there.
    pub(super) fn debug_set_breakpoint_part(&mut self, path: PathBuf, line: usize, what: BreakpointPart, text: String) {
        let text = Some(text.trim().to_string()).filter(|t| !t.is_empty());
        let breakpoint = self.breakpoints.entry(path.clone()).or_default().entry(line).or_default();
        match what {
            BreakpointPart::Condition => breakpoint.condition = text,
            BreakpointPart::Hits => breakpoint.hits = text,
            BreakpointPart::Log => breakpoint.log = text,
        }
        self.send_set_breakpoints_for(&path);
        self.refresh_debug_panes();
    }

    /// `SPC u t`: the cursor's line's breakpoint off, or on again --
    /// kept either way. One's made, on, where there's none.
    pub(crate) fn debug_toggle_breakpoint_on(&mut self) {
        let Some((path, line)) = self.breakpoint_here() else { return };
        self.debug_breakpoint_on_off(path, line);
    }

    /// The breakpoint at `line` of `path` off, or on again.
    pub(super) fn debug_breakpoint_on_off(&mut self, path: PathBuf, line: usize) {
        let lines = self.breakpoints.entry(path.clone()).or_default();
        match lines.get_mut(&line) {
            Some(b) => b.off = !b.off,
            None => {
                lines.insert(line, Breakpoint::default());
            }
        }
        self.send_set_breakpoints_for(&path);
        self.refresh_debug_panes();
    }

    /// Whether the adapter's exception `filter` is on: as you said, or as
    /// the adapter has it.
    pub(super) fn debug_exception_on(&self, filter: &ExceptionFilter) -> bool {
        self.debug_exceptions.get(&filter.filter).copied().unwrap_or(filter.default)
    }

    /// An exception filter on or off, and the adapter told.
    pub(super) fn debug_toggle_exception(&mut self, filter: String) {
        let Some(known) = self.debug_session.as_ref().and_then(|s| s.exception_filters.iter().find(|f| f.filter == filter).cloned()) else { return };
        let on = !self.debug_exception_on(&known);
        self.debug_exceptions.insert(filter, on);
        let ready: Vec<u64> = self.debug_session.as_ref().map(|s| std::iter::once((s.generation, s.initialized)).chain(s.children.iter().map(|c| (c.generation, c.initialized))).filter(|(_, r)| *r).map(|(on, _)| on).collect()).unwrap_or_default();
        for on in ready {
            self.send_exception_breakpoints_to(on);
        }
    }

    /// `setExceptionBreakpoints` on connection `on`, with the filters on:
    /// nothing for an adapter that offers none.
    pub(super) fn send_exception_breakpoints_to(&mut self, on: u64) {
        let Some(session) = self.debug_session.as_ref() else { return };
        if session.exception_filters.is_empty() {
            return;
        }
        let filters: Vec<String> = session.exception_filters.iter().filter(|f| self.debug_exception_on(f)).map(|f| f.filter.clone()).collect();
        let args = fenix_dap::requests::SetExceptionBreakpointsRequestArguments::builder().filters(filters).build();
        if let Some(client) = session.client_of(on) {
            let _ = client.request(fenix_dap::requests::Request::SetExceptionBreakpoints(args));
        }
    }

    /// `SPC u x`: the drawer, at the breakpoints and the exceptions to
    /// stop on.
    pub(crate) fn debug_exception_breakpoints(&mut self) {
        if self.debug_session.is_none() {
            self.set_error("no debug session -- the exceptions to stop on are the adapter's, once one starts".to_string());
            return;
        }
        self.debug_dock_open = true;
        self.debug_drawer.focused = true;
        self.debug_drawer.column = debug_drawer::BREAKPOINTS_COLUMN;
        self.debug_drawer.selected[debug_drawer::BREAKPOINTS_COLUMN] = 0;
    }

    /// `SPC u P`: the name under the cursor pinned to its line -- its value
    /// written there at every stop -- or unpinned.
    pub(crate) fn debug_toggle_pin(&mut self) {
        let Some(path) = self.open().buffer.path().map(Path::to_path_buf) else {
            self.set_error("buffer has no file to pin a value in".to_string());
            return;
        };
        let (line, col) = self.open().buffer.line_col(&self.cursor());
        let text = self.open().buffer.line(line).to_string();
        let Some(expression) = lens_expression(&text, col) else {
            self.set_error("no name under the cursor to pin".to_string());
            return;
        };
        let path = fenix_lsp::normalize(std::fs::canonicalize(&path).unwrap_or(path));
        let pin = Pin { path, line: line + 1, expression };
        if let Some(at) = self.debug_pins.iter().position(|p| *p == pin) {
            self.debug_pins.remove(at);
        } else {
            self.debug_pins.push(pin.clone());
            self.debug_ask_pin(&pin);
        }
        self.refresh_debug_panes();
    }

    /// A pin's value asked for in the selected frame, while stopped.
    pub(super) fn debug_ask_pin(&mut self, pin: &Pin) {
        let Some(session) = self.debug_session.as_mut().filter(|s| s.stopped_thread.is_some()) else { return };
        let Some(frame) = session.current_frame_id else { return };
        let on = session.stopped_on;
        let args = fenix_dap::requests::EvaluateRequestArguments::builder().expression(pin.expression.clone()).frame_id(Some(frame)).context(Some(fenix_dap::requests::EvaluateRequestContext::Watch)).build();
        session.send(on, fenix_dap::requests::Request::Evaluate(args), PendingDapRequest::Pin { pin: pin.clone() });
    }

    /// A pin's value come: kept, fresh. One it couldn't give keeps the
    /// last, gone stale.
    pub(super) fn debug_pin_answered(&mut self, pin: Pin, value: Option<String>) {
        let Some(session) = self.debug_session.as_mut() else { return };
        match value {
            Some(v) => {
                session.pin_values.insert(pin, (v, true));
            }
            None => {
                if let Some(old) = session.pin_values.get_mut(&pin) {
                    old.1 = false;
                }
            }
        }
        self.refresh_debug_panes();
    }

    /// The pins' decorations for the buffer on `path`: each pin of its,
    /// `◆ expr = value` at the end of its line, dim once stale.
    pub(super) fn debug_pin_decorations(&self, path: &Path, buffer: &fenix_core::Buffer) -> Vec<crate::decorations::Decoration> {
        let Some(session) = self.debug_session.as_ref() else { return Vec::new() };
        self.debug_pins
            .iter()
            .filter(|p| debug_host::same_file(&p.path, path) && p.line >= 1 && p.line <= buffer.line_count())
            .map(|p| {
                let (value, fresh) = session.pin_values.get(p).map(|(v, f)| (v.as_str(), *f)).unwrap_or(("…", false));
                let value: String = value.chars().map(|c| if c.is_control() { ' ' } else { c }).take(80).collect();
                let at = buffer.line_start_char(p.line - 1) + buffer.line_len(p.line - 1);
                let role = if fresh { crate::page::Role::Accent } else { crate::page::Role::Muted };
                crate::decorations::Decoration { start: at, end: at, look: crate::decorations::Look::Text { text: format!("    ◆ {} = {value}", p.expression), role, after: true }, id: None }
            })
            .collect()
    }

    /// `SPC u U`: the launch picked again, rather than the project's last.
    pub(crate) fn debug_pick_launch(&mut self) {
        self.debug_pick_launch = true;
        self.debug_start_or_continue();
        self.debug_pick_launch = false;
    }

    /// The debugger's part of the session file.
    pub(super) fn capture_debugging(&self) -> SavedDebugging {
        let mut breakpoints: Vec<SavedBreakpoint> = self.breakpoints.iter().flat_map(|(path, lines)| lines.iter().map(move |(line, b)| SavedBreakpoint { path: path.clone(), line: *line, breakpoint: b.clone() })).collect();
        breakpoints.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
        let mut launches: Vec<(PathBuf, String)> = self.debug_last_launch.iter().map(|(r, n)| (r.clone(), n.clone())).collect();
        launches.sort();
        let mut exceptions: Vec<(String, bool)> = self.debug_exceptions.iter().map(|(f, on)| (f.clone(), *on)).collect();
        exceptions.sort();
        SavedDebugging { breakpoints, pins: self.debug_pins.clone(), launches, exceptions }
    }

    /// The debugger's part of a session file, put back.
    pub(super) fn restore_debugging(&mut self, saved: SavedDebugging) {
        self.breakpoints.clear();
        for b in saved.breakpoints {
            self.breakpoints.entry(b.path).or_default().insert(b.line, b.breakpoint);
        }
        self.debug_pins = saved.pins;
        self.debug_last_launch = saved.launches.into_iter().collect();
        self.debug_exceptions = saved.exceptions.into_iter().collect();
    }
}

/// What the drawer's line gives a breakpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BreakpointPart {
    Condition,
    Hits,
    Log,
}

impl BreakpointPart {
    /// The drawer's line's prompt for it.
    pub fn prompt(self) -> &'static str {
        match self {
            BreakpointPart::Condition => "condition › ",
            BreakpointPart::Hits => "hit count › ",
            BreakpointPart::Log => "log message › ",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_breakpoint_says_what_it_is_and_off_ones_arent_sent() {
        let mut lines = BTreeMap::new();
        lines.insert(3, Breakpoint { condition: Some("x > 3".into()), hits: Some(">= 2".into()), ..Default::default() });
        lines.insert(5, Breakpoint { log: Some("x is {x}".into()), ..Default::default() });
        lines.insert(7, Breakpoint { off: true, ..Default::default() });
        assert_eq!(lines[&3].describe(), "if x > 3  hits >= 2");
        assert_eq!((lines[&3].look(), lines[&5].look(), lines[&7].look()), (BreakpointLook::Conditional, BreakpointLook::Log, BreakpointLook::Off));
        let sent = source_breakpoints(&lines, Some(9));
        assert_eq!(sent.iter().map(|b| b.line).collect::<Vec<_>>(), [3, 5, 9], "7 is off; 9 is run to cursor's");
        assert_eq!((sent[0].condition.as_deref(), sent[0].hit_condition.as_deref(), sent[1].log_message.as_deref()), (Some("x > 3"), Some(">= 2"), Some("x is {x}")));
        assert_eq!(source_breakpoints(&lines, Some(3))[0].condition.as_deref(), Some("x > 3"), "run to cursor on one leaves it as it is");
    }

    #[test]
    fn the_session_files_debugging_reads_back_and_a_plain_breakpoint_is_only_its_line() {
        let saved = SavedDebugging {
            breakpoints: vec![SavedBreakpoint { path: PathBuf::from("/p/a.py"), line: 3, breakpoint: Breakpoint::default() }, SavedBreakpoint { path: PathBuf::from("/p/a.py"), line: 4, breakpoint: Breakpoint { condition: Some("i == 2".into()), off: true, ..Default::default() } }],
            pins: vec![Pin { path: PathBuf::from("/p/a.py"), line: 3, expression: "self.rows".into() }],
            launches: vec![(PathBuf::from("/p"), "Python: module".into())],
            exceptions: vec![("raised".into(), true)],
        };
        let json = serde_json::to_value(&saved).unwrap();
        assert_eq!(json["breakpoints"][0], serde_json::json!({ "path": "/p/a.py", "line": 3 }));
        assert_eq!(json["breakpoints"][1]["off"], true);
        let back: SavedDebugging = serde_json::from_value(json).unwrap();
        assert_eq!(back, saved);
        if cfg!(not(windows)) {
            assert!(back.validate().is_ok());
        }
        let bad = SavedDebugging { pins: vec![Pin { path: PathBuf::from("rel.py"), line: 1, expression: "x".into() }], ..Default::default() };
        assert!(bad.validate().is_err());
        assert!(SavedDebugging::default().is_empty());
    }
}
